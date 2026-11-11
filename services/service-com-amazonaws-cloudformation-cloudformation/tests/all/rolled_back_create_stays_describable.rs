// SPDX-License-Identifier: CC0-1.0

//! A failed CREATE that CloudFormation rolled back RETAINS the stack, end to end over the
//! awsQuery wire.
//!
//! RULE under test: **a create that failed and was rolled back leaves the stack behind in the
//! terminal `ROLLBACK_COMPLETE` status, and that stack stays addressable by its `StackName`:
//! `DescribeStacks` keeps answering HTTP 200 with that status and the reason the create failed,
//! and the event log keeps naming what failed — the rolled-back stack stops resolving by name
//! only once `DeleteStack` has removed it, after which the name is refused `ValidationError`
//! "Stack with id <name> does not exist" (HTTP 400) while the stack ID still resolves.**
//!
//! `StackStatus` distinguishes `ROLLBACK_COMPLETE` from the absence of a stack
//! (`smithy_query(query="shape",shape="StackStatus")`), and `DescribeStacksInput$StackName`
//! is what makes the ID the only spelling a stack answers to once it is gone ("Deleted stacks:
//! You must specify the unique stack ID"). So a rolled-back create is OBSERVABLE: a client can
//! tell it apart from a stack that was never created, read why it failed, and drive the
//! documented clean-up.
//!
//! The rows differ along what the rule generalises over — WHAT failed the create, since the
//! retention is the rollback's and not any one failure's: a custom resource whose provider
//! answered `FAILED` stating its own reason, one whose provider answered `FAILED` stating none,
//! one whose provider never answered at all so the wait for it ran out, and a declaration
//! CloudFormation refused before it asked any provider or owning service — and, with them, how
//! the custom resource's type is spelled (the generic `AWS::CloudFormation::CustomResource` and
//! the author's own `Custom::<Name>` namespace).
//!
//! Every row drives the emulator through its own wire: the stack operations over
//! CloudFormation's awsQuery endpoint and the provider's answer as the plain unsigned `PUT` to
//! the published `ResponseURL` a provider really makes.

use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

use crate::custom_resource_providers::{
    awaited_request, emulator, provider_token, put_response, settled,
    settled_with_a_responsive_provider,
};
use crate::harness::{call, create_stack, member, xml_text};
use crate::stack_lifecycle::describe_stack;
use crate::uncreated_custom_resource_rollback::{age_the_provider_wait, past_the_window};
use crate::update_rollback_recovery::stack_events;

/// The logical id every row that declares a custom resource declares it under.
const WIDGET: &str = "Widget";

/// The `PhysicalResourceId` a provider that answers `FAILED` names in its answer, and so the
/// id the rollback's own `Delete` is sent under.
const FAILED_ID: &str = "widget-failed";

/// What failed one row's create. The rule is about the ROLLBACK, so these are the ways a
/// create arrives at one.
#[derive(Debug, Clone, Copy)]
enum Cause {
    /// The custom resource's provider answered `FAILED`, stating the reason carried here, or
    /// stating none.
    ProviderFailed(Option<&'static str>),
    /// The custom resource's provider never answered, so the wait for its answer ran out.
    ProviderSilent,
    /// CloudFormation refused a declaration itself, before asking any provider or any owning
    /// service — so no custom resource is involved at all.
    RefusedDeclaration,
}

/// One row: the stack a failing create leaves behind, and what failed that create.
struct Row {
    case: &'static str,
    stack: &'static str,
    template: String,
    cause: Cause,
    /// Whether the failure is reported ON a resource of the stack, which is what lets the
    /// event assertion name one. A declaration CloudFormation refused outright fails the
    /// operation rather than a resource it created.
    names_a_failed_resource: bool,
}

/// The custom resource one row declares: the provider topic as its `ServiceToken`, under the
/// row's own spelling of the type.
fn custom_resource_template(resource_type: &str) -> String {
    json!({
        "Resources": {
            WIDGET: {
                "Type": resource_type,
                "Properties": { "ServiceToken": provider_token() },
            },
        },
    })
    .to_string()
}

/// The class the rule covers: any create that failed and was rolled back, however it failed.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a provider that answered FAILED with its own reason, generic type",
            stack: "rolled-back-stated-reason",
            template: custom_resource_template("AWS::CloudFormation::CustomResource"),
            cause: Cause::ProviderFailed(Some("the widget could not be created")),
            names_a_failed_resource: true,
        },
        Row {
            case: "a provider that answered FAILED stating no reason, the author's namespace",
            stack: "rolled-back-silent-reason",
            template: custom_resource_template("Custom::WidgetProvisioner"),
            cause: Cause::ProviderFailed(None),
            names_a_failed_resource: true,
        },
        Row {
            case: "a provider that never answered, so the wait for its answer ran out",
            stack: "rolled-back-unanswered",
            template: custom_resource_template("Custom::SilentProvisioner"),
            cause: Cause::ProviderSilent,
            names_a_failed_resource: true,
        },
        Row {
            case: "a declaration CloudFormation refused before asking anybody",
            stack: "rolled-back-refused-declaration",
            // An `AWS::SSM::Parameter` with no `Value`, which the type requires.
            template: json!({
                "Resources": {
                    "Setting": {
                        "Type": "AWS::SSM::Parameter",
                        "Properties": { "Type": "String" },
                    },
                },
            })
            .to_string(),
            cause: Cause::RefusedDeclaration,
            names_a_failed_resource: false,
        },
    ]
}

/// Drive one row's create to the status it settles in, and answer with that status and the
/// body that reported it.
async fn settle_the_failed_create(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    row: &Row,
) -> (String, String) {
    let case = row.case;
    match row.cause {
        Cause::ProviderFailed(reason) => {
            let (stack_id, request, _) = awaited_request(engine, row.stack, WIDGET)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected the create to be waiting for the provider's \
                         answer about {WIDGET}, got no such wait"
                    )
                });
            let mut answer = json!({ "Status": "FAILED", "PhysicalResourceId": FAILED_ID });
            if let Some(reason) = reason {
                answer["Reason"] = json!(reason);
            }
            let (answered, answer_body) = put_response(
                router,
                engine,
                &stack_id,
                WIDGET,
                &request.request_id,
                &answer,
            )
            .await;
            assert_eq!(
                answered, 200,
                "case {case:?}: expected the provider's FAILED answer to be accepted, got \
                 {answered} {answer_body}"
            );
            // The rollback the failure owes sends the provider the `Delete` of what it was
            // asked to create, and this provider answers it.
            settled_with_a_responsive_provider(router, engine, case, row.stack, FAILED_ID).await
        }
        Cause::ProviderSilent => {
            age_the_provider_wait(engine, row.stack, past_the_window());
            settled(router, case, row.stack).await
        }
        Cause::RefusedDeclaration => settled(router, case, row.stack).await,
    }
}

/// Poll the deletion of one stack to its terminal status, addressing it by its stack ID — the
/// one spelling a stack answers to for the whole of its deletion, including after it.
async fn settle_the_deletion(router: &MultiServiceRouter, case: &str, stack_id: &str) -> String {
    settled(router, case, stack_id).await.0
}

#[tokio::test]
async fn a_rolled_back_create_leaves_a_stack_that_keeps_answering_by_name() {
    for row in rows() {
        let case = row.case;
        let (router, engine) = emulator(case).await;
        let (code, created) = create_stack(&router, row.stack, &row.template, &[]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {code} {created}"
        );
        let stack_id = xml_text(&created, "StackId").unwrap_or_default();
        assert!(
            !stack_id.is_empty(),
            "case {case:?}: expected CreateStack to answer a stack id, got {created}"
        );

        let (settled_status, settled_body) =
            settle_the_failed_create(&router, &engine, &row).await;
        assert_eq!(
            settled_status,
            status::ROLLBACK_COMPLETE,
            "case {case:?}: expected the failed create to be rolled back, got \
             {settled_status:?} with {settled_body}"
        );

        // The point of the rule: the stack is STILL THERE, under the name the caller created
        // it with. Read repeatedly, because a read may not change what a later caller sees —
        // the rolled-back stack is retained, not consumed by the first reader of it.
        for look in 0..3 {
            let (code, described) = describe_stack(&router, row.stack).await;
            assert_eq!(
                code, 200,
                "case {case:?}: expected DescribeStacks to answer for the name {:?} on look \
                 {look}, got {code} {described}",
                row.stack
            );
            let reported = xml_text(&described, "StackStatus").unwrap_or_default();
            assert_eq!(
                reported,
                status::ROLLBACK_COMPLETE,
                "case {case:?}: expected the retained stack to keep reporting \
                 ROLLBACK_COMPLETE on look {look}, got {reported:?} with {described}"
            );
            // ...carrying WHY the create failed, which is the only channel a client has for
            // the diagnostic.
            let reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
            assert!(
                !reason.trim().is_empty(),
                "case {case:?}: expected the rolled-back stack to report why its create \
                 failed on look {look}, got {described}"
            );
        }

        // The same stack, by the other spelling of its identifier.
        let (code, by_id) = describe_stack(&router, &stack_id).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected DescribeStacks to answer for the stack id, got {code} \
             {by_id}"
        );
        let by_id_status = xml_text(&by_id, "StackStatus").unwrap_or_default();
        assert_eq!(
            by_id_status,
            status::ROLLBACK_COMPLETE,
            "case {case:?}: expected the stack id to report ROLLBACK_COMPLETE, got \
             {by_id_status:?} with {by_id}"
        );

        // ...and the event log of the retained stack is readable too, carrying the stack's
        // own rolled-back event and — where a resource is what failed — that resource's
        // CREATE_FAILED.
        let events = stack_events(case, &router, row.stack).await;
        let rolled_back = (row.stack.to_string(), status::ROLLBACK_COMPLETE.to_string());
        assert!(
            events.contains(&rolled_back),
            "case {case:?}: expected the stack's own {rolled_back:?} event, got {events:?}"
        );
        if row.names_a_failed_resource {
            let failed = (WIDGET.to_string(), resource_status::CREATE_FAILED.to_string());
            assert!(
                events.contains(&failed),
                "case {case:?}: expected the failed custom resource's {failed:?} event, got \
                 {events:?}"
            );
        }

        // Only DELETING it takes the name away. Up to here nothing has, which is what the
        // reads above said.
        let (code, deleted) =
            call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected DeleteStack to be accepted for a rolled-back stack, got \
             {code} {deleted}"
        );
        let after_deletion = settle_the_deletion(&router, case, &stack_id).await;
        assert_eq!(
            after_deletion,
            status::DELETE_COMPLETE,
            "case {case:?}: expected the deletion to complete, got {after_deletion:?}"
        );

        // NOW the name names no stack, in the wording the reads of a stack's own record
        // answer with.
        let (code, refused) = describe_stack(&router, row.stack).await;
        assert_eq!(
            code, 400,
            "case {case:?}: expected the deleted stack's name to be refused, got {code} \
             {refused}"
        );
        assert_eq!(
            xml_text(&refused, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError for the deleted stack's name, got \
             {refused}"
        );
        assert_eq!(
            xml_text(&refused, "Message").as_deref(),
            Some(format!("Stack with id {} does not exist", row.stack).as_str()),
            "case {case:?}: expected the refusal to name the stack it could not find, got \
             {refused}"
        );

        // ...while the stack ID still resolves, which is the one difference between the two
        // spellings once the stack is gone.
        let (code, gone) = describe_stack(&router, &stack_id).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected the deleted stack to stay addressable by its id, got \
             {code} {gone}"
        );
        let gone_status = xml_text(&gone, "StackStatus").unwrap_or_default();
        assert_eq!(
            gone_status,
            status::DELETE_COMPLETE,
            "case {case:?}: expected the stack id to report DELETE_COMPLETE, got \
             {gone_status:?} with {gone}"
        );
    }
}
