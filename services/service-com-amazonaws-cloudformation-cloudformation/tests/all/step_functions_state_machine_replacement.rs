// SPDX-License-Identifier: CC0-1.0

//! A stack update that changes a property of an `AWS::StepFunctions::StateMachine` which
//! cannot be changed on the machine that exists, end to end over the awsQuery wire with
//! AWS Step Functions mounted beside CloudFormation on one engine.
//!
//! The rule under test: **a declaration that changes a property requiring REPLACEMENT is
//! deployed by creating the resource anew — the stack publishes the physical id the owning
//! service just minted, the resource the stack replaced is taken down there, and the stack
//! operation reaches `UPDATE_COMPLETE`.** A change that does NOT require replacement is
//! applied in place and the resource keeps its id. Either way the deployed workflow is the
//! one the template declares, so no stack update rolls back for want of a path to it.
//!
//! `StateMachineType` is the property the rows change: it is fixed at creation
//! ("determines whether a Standard or Express state machine is created"), so
//! `UpdateStateMachine` carries no member for it. The rows differ along what the rule
//! generalises over — the direction of the change (STANDARD to EXPRESS and back), and a
//! revision that changes the DEFINITION instead, which is applied in place — and every
//! expected value is read back out of Step Functions' own API rather than out of the answer
//! being checked.

use serde_json::{json, Value};

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_text, REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

/// One Step Functions call through the same router, signed for `states` the way a client's
/// SDK signs it (`awsJson1_0`, `X-Amz-Target: AWSStepFunctions.<Operation>`): the status
/// and the parsed answer.
///
/// `pub(crate)`: `tests/all.rs` is this binary's crate root, so a sibling suite that reads a
/// stack's state machine back through that service's own wire reads THIS call rather than
/// carrying a second copy of it.
pub(crate) async fn sfn_json(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri(format!("https://states.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSStepFunctions.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/states/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(request.to_string()))
        .expect("a well-formed Step Functions request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let body = response.into_body().to_vec();
    let document: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, document)
}

/// A one-machine template whose state machine declares `type_and_definition`.
///
/// It names no `StateMachineName`, which is the usual form: CloudFormation generates the
/// name, so a REPLACEMENT is free to be created under one of its own.
fn machine_template(definition: &str, sm_type: &str) -> String {
    json!({
        "Resources": {
            "Workflow": {
                "Type": "AWS::StepFunctions::StateMachine",
                "Properties": {
                    "DefinitionString": definition,
                    "RoleArn": "arn:aws:iam::000000000000:role/StepFunctionsRole",
                    "StateMachineType": sm_type,
                },
            },
        },
    })
    .to_string()
}

/// The workflow one row deploys first.
const FIRST_DEFINITION: &str =
    r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","End":true}}}"#;

/// The workflow a row's revision declares instead.
const REVISED_DEFINITION: &str =
    r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","Result":{"revised":true},"End":true}}}"#;

/// The `StackStatus` a stack reports right now.
async fn status_of(router: &MultiServiceRouter, stack: &str) -> String {
    let (code, body) = describe_stack(router, stack).await;
    assert_eq!(code, 200, "DescribeStacks {stack:?}: body {body}");
    xml_text(&body, "StackStatus").unwrap_or_default()
}

/// The physical id the stack publishes for its `Workflow` resource, which for this type is
/// the state machine's ARN.
async fn machine_arn(case: &str, router: &MultiServiceRouter, stack: &str) -> String {
    let (code, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", "Workflow"),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStackResource to answer, got {code} {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// What Step Functions answers `DescribeStateMachine` with for `arn`, or `None` where it
/// holds no such machine.
async fn described(router: &MultiServiceRouter, arn: &str) -> Option<Value> {
    let (status, document) = sfn_json(
        router,
        "DescribeStateMachine",
        &json!({ "stateMachineArn": arn }),
    )
    .await;
    (status == 200).then_some(document)
}

/// One row of the replacement table.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The type the create declares, and the one the revision declares.
    created_type: &'static str,
    revised_type: &'static str,
    /// The definition the revision declares, which a row may change too.
    revised_definition: &'static str,
    /// Whether the change requires the machine to be replaced.
    replaced: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "STANDARD to EXPRESS",
            stack: "sfn-replace-to-express",
            created_type: "STANDARD",
            revised_type: "EXPRESS",
            revised_definition: FIRST_DEFINITION,
            replaced: true,
        },
        Row {
            case: "EXPRESS to STANDARD, with the definition changed in the same revision",
            stack: "sfn-replace-to-standard",
            created_type: "EXPRESS",
            revised_type: "STANDARD",
            revised_definition: REVISED_DEFINITION,
            replaced: true,
        },
        Row {
            case: "the definition alone, which is applied in place",
            stack: "sfn-update-in-place",
            created_type: "STANDARD",
            revised_type: "STANDARD",
            revised_definition: REVISED_DEFINITION,
            replaced: false,
        },
    ]
}

#[tokio::test]
async fn a_state_machine_whose_type_changes_is_replaced_and_the_stack_completes() {
    for row in rows() {
        let Row {
            case,
            stack,
            created_type,
            revised_type,
            revised_definition,
            replaced,
        } = row;
        let router = router();
        let (code, body) = create_stack(
            &router,
            stack,
            &machine_template(FIRST_DEFINITION, created_type),
            &[],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected CreateStack to succeed, got {code} {body}"
        );
        wait_for_the_transition().await;
        let reported = status_of(&router, stack).await;
        assert_eq!(
            reported,
            status::CREATE_COMPLETE,
            "case {case:?}: expected the create to complete, got {reported:?}"
        );
        let before = machine_arn(case, &router, stack).await;
        assert!(
            described(&router, &before).await.is_some(),
            "case {case:?}: expected Step Functions to hold the created machine {before:?}"
        );

        let (code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", stack),
                member(
                    "TemplateBody",
                    &machine_template(revised_definition, revised_type),
                ),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected UpdateStack to succeed, got {code} {body}"
        );
        wait_for_the_transition().await;
        let reported = status_of(&router, stack).await;
        assert_eq!(
            reported,
            status::UPDATE_COMPLETE,
            "case {case:?}: expected the update to complete, got {reported:?} (body {:?})",
            describe_stack(&router, stack).await.1
        );

        // The handle the stack publishes: a new one for a replacement, the same one for a
        // change applied in place.
        let after = machine_arn(case, &router, stack).await;
        assert_eq!(
            after != before,
            replaced,
            "case {case:?}: expected the physical id to change={replaced}, got {before:?} then \
             {after:?}"
        );

        // ...and the machine the stack now publishes really carries the declared
        // configuration, read back through Step Functions' own API.
        let held = described(&router, &after).await.unwrap_or_else(|| {
            panic!("case {case:?}: expected Step Functions to hold the published machine {after:?}")
        });
        assert_eq!(
            held.get("type").and_then(Value::as_str),
            Some(revised_type),
            "case {case:?}: expected the declared type, got {held}"
        );
        assert_eq!(
            held.get("definition").and_then(Value::as_str),
            Some(revised_definition),
            "case {case:?}: expected the declared definition, got {held}"
        );

        // The resource a replacement left behind is taken down in the owning service, so
        // the stack does not leave a workflow nothing manages: Step Functions either holds
        // it no longer or reports it DELETING, which is the status a started deletion puts
        // a state machine in.
        if replaced {
            let left = described(&router, &before).await;
            let status_of_replaced = left
                .as_ref()
                .and_then(|machine| machine.get("status"))
                .and_then(Value::as_str);
            assert_ne!(
                status_of_replaced,
                Some("ACTIVE"),
                "case {case:?}: expected the replaced machine {before:?} to be taken down, got \
                 {left:?}"
            );
        }
    }
}
