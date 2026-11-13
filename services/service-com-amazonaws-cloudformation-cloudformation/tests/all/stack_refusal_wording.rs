// SPDX-License-Identifier: CC0-1.0

//! How the STACK-SCOPED refusals of this service are worded, over the awsQuery wire.
//!
//! **The rule every case here holds the service to: a refusal's HTTP status, error code and
//! message belong to the OPERATION that refused. CloudFormation words the "that stack is not
//! there" class with a different fixed template per operation — the name bare, single-quoted,
//! square-bracketed, colon-prefixed, or introduced by "with id" — and the only variable part
//! of any of them is what the request named; a refusal about a stack whose current STATUS
//! does not admit the call names the operation that cannot be called, plus whatever that
//! operation's own template states about the status — the stack's current one where the
//! template interpolates it, the statuses the call CAN be made from where it lists those
//! instead — and never the stack itself; and an operation
//! that addresses a record WITHIN a stack (a change set) resolves the stack FIRST, so a
//! request naming a stack that is not addressable is refused about the stack rather than
//! about the record.**
//!
//! The sentences are spelled out here rather than imported from the crate, because the
//! sentence IS the requirement: a reworded message must fail this suite.
//!
//! The tables span the class rather than one recorded request: the stack-not-found side over
//! every operation this emulator has evidence of a template for, crossed with three ways of
//! naming a stack that is not addressable (a name nothing created, the name of a stack that
//! was created and deleted, and a stack id no stack carries) — and it asserts that the
//! templates really are several, so one shared sentence cannot pass; the resource-not-in-stack
//! side over four logical ids a live stack has not deployed, with the stack addressed both by
//! name and by id; the drift-result side over four ids nothing issued, including the published
//! maximum length; and the status-gated side over three statuses a stack can be in when the
//! call arrives, on both operations that gate on status.
//!
//! Every call drives the real request path (form-encoded body in, XML envelope out) through
//! the one router `stack_lifecycle.rs` builds, so what is asserted is what a client sees.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, deploy, describe_stack, member, router, wait_for_the_transition,
    xml_text, ACCOUNT_ID, REGION,
};

/// The template the stacks here are deployed from: one topic, so the live stack has a
/// resource for the rows that name one and the ones that name another.
const TEMPLATE: &str = r#"{"Resources":{
     "Topic":{"Type":"AWS::SNS::Topic","Properties":{"DisplayName":"refusal-wording"}}}}"#;

/// A stack policy body, for the row that reaches `SetStackPolicy`'s stack resolution.
const POLICY: &str = r#"{"Statement":[{"Effect":"Allow","Action":"Update:*",
     "Principal":"*","Resource":"*"}]}"#;

/// `Stack with id <name> does not exist` — the template of the reads that introduce the name
/// with "with id".
fn with_id(name: &str) -> String {
    format!("Stack with id {name} does not exist")
}

/// `Stack [<name>] does not exist` — the square-bracketed template.
fn bracketed(name: &str) -> String {
    format!("Stack [{name}] does not exist")
}

/// `Stack '<name>' does not exist` — the single-quoted template.
fn quoted(name: &str) -> String {
    format!("Stack '{name}' does not exist")
}

/// `Stack <name> does not exist` — the bare template, with no quoting or bracketing.
fn bare(name: &str) -> String {
    format!("Stack {name} does not exist")
}

/// `Stack: <name> does not exist` — the colon-prefixed template.
fn labelled(name: &str) -> String {
    format!("Stack: {name} does not exist")
}

/// The sentence `ContinueUpdateRollback` answers for a stack whose status the continuation
/// cannot be started from AND whose last update did not fail: the control plane's own status
/// gate, which names the internal operation and no part of the stack. (A stack an update's
/// FAILURE parked is refused by this operation's own precondition instead, which
/// `continue_update_rollback_status_gate.rs` pins.)
const CANNOT_CONTINUE: &str = "RollbackUpdatedStack cannot be called from current stack status";

/// The fixed sentence `RollbackStack` answers for a stack whose status a rollback cannot be
/// started from: it states the statuses the call CAN be made from and nothing of the stack.
const CANNOT_ROLL_BACK: &str =
    "RollbackStack can only be called on any of the [UPDATE_FAILED, CREATE_FAILED] statuses.";

/// One row of the stack-not-found table: the operation, the members it needs besides
/// `StackName`, and the template its refusal must be worded with.
struct NotFoundRow {
    action: &'static str,
    extra: Vec<(String, String)>,
    template: fn(&str) -> String,
}

fn not_found_rows() -> Vec<NotFoundRow> {
    vec![
        // The reads that word the class with "with id": a stack's own record, its
        // resources, its template.
        NotFoundRow {
            action: "DescribeStacks",
            extra: vec![],
            template: with_id,
        },
        NotFoundRow {
            action: "DescribeStackResources",
            extra: vec![],
            template: with_id,
        },
        NotFoundRow {
            action: "ListStackResources",
            extra: vec![],
            template: with_id,
        },
        NotFoundRow {
            action: "GetTemplate",
            extra: vec![],
            template: with_id,
        },
        // The reads of a stack's LOGS — its event stream and its drift results — which word
        // the class with the square brackets instead.
        NotFoundRow {
            action: "DescribeStackEvents",
            extra: vec![],
            template: bracketed,
        },
        NotFoundRow {
            action: "DescribeStackResourceDrifts",
            extra: vec![],
            template: bracketed,
        },
        // The change-set operations: the STACK is resolved first, so each is refused
        // about the stack rather than answered the change-set-not-found of a lookup that
        // never had a stack to look inside — and the creation of an UPDATE change set, which
        // plans against a stack that has to exist, is refused the same way.
        NotFoundRow {
            action: "ListChangeSets",
            extra: vec![],
            template: bracketed,
        },
        NotFoundRow {
            action: "CreateChangeSet",
            extra: vec![
                member("ChangeSetName", "any-set"),
                member("ChangeSetType", "UPDATE"),
                member("TemplateBody", TEMPLATE),
            ],
            template: bracketed,
        },
        NotFoundRow {
            action: "DescribeChangeSetHooks",
            extra: vec![member("ChangeSetName", "any-set")],
            template: bracketed,
        },
        NotFoundRow {
            action: "DescribeChangeSet",
            extra: vec![member("ChangeSetName", "any-set")],
            template: bracketed,
        },
        NotFoundRow {
            action: "ExecuteChangeSet",
            extra: vec![member("ChangeSetName", "any-set")],
            template: bracketed,
        },
        // A change set named beside the stack in an operation whose own template for the
        // class is the "with id" one keeps that template.
        NotFoundRow {
            action: "GetTemplate",
            extra: vec![member("ChangeSetName", "any-set")],
            template: with_id,
        },
        // The square-bracketed template's own operations.
        NotFoundRow {
            action: "DetectStackDrift",
            extra: vec![],
            template: bracketed,
        },
        // A drift check of ONE resource resolves its stack first, and words the class the
        // same way the stack-wide check does.
        NotFoundRow {
            action: "DetectStackResourceDrift",
            extra: vec![member("LogicalResourceId", "Topic")],
            template: bracketed,
        },
        NotFoundRow {
            action: "RollbackStack",
            extra: vec![],
            template: bracketed,
        },
        NotFoundRow {
            action: "UpdateTerminationProtection",
            extra: vec![member("EnableTerminationProtection", "true")],
            template: bracketed,
        },
        // One operation per remaining template.
        NotFoundRow {
            action: "DescribeStackResource",
            extra: vec![member("LogicalResourceId", "Topic")],
            template: quoted,
        },
        NotFoundRow {
            action: "SignalResource",
            extra: vec![
                member("LogicalResourceId", "Topic"),
                member("UniqueId", "signal-1"),
                member("Status", "SUCCESS"),
            ],
            template: bare,
        },
        NotFoundRow {
            action: "SetStackPolicy",
            extra: vec![member("StackPolicyBody", POLICY)],
            template: labelled,
        },
        // The colon-prefixed template's other operations: the stack-policy READ and the
        // resumption of a rollback.
        NotFoundRow {
            action: "GetStackPolicy",
            extra: vec![],
            template: labelled,
        },
        NotFoundRow {
            action: "ContinueUpdateRollback",
            extra: vec![],
            template: labelled,
        },
        // ...and the cancellation of an update, which resolves its stack the same way.
        NotFoundRow {
            action: "CancelUpdateStack",
            extra: vec![],
            template: labelled,
        },
    ]
}

/// The `<Code>` and `<Message>` of a refusal envelope, with the status it arrived under.
async fn refusal(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(String, String)],
) -> (u16, Option<String>, Option<String>, String) {
    let (status, body) = call(router, action, members).await;
    let code = xml_text(&body, "Code");
    let message = xml_text(&body, "Message");
    (status, code, message, body)
}

#[tokio::test]
async fn each_operation_words_the_stack_not_found_class_with_its_own_template() {
    let router = router();
    // A stack that was created and deleted, beside a name nothing ever created and a stack id
    // no stack carries: all three are unaddressable, and each operation words its refusal the
    // same way whichever of them the request named.
    deploy(&router, "wording-gone", TEMPLATE).await;
    let (status, body) =
        call(&router, "DeleteStack", &[member("StackName", "wording-gone")]).await;
    assert_eq!(
        status, 200,
        "expected DeleteStack to be accepted, got {status} {body}"
    );
    wait_for_the_transition().await;

    let ghost_id = format!(
        "arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/wording-ghost/\
         00000000-0000-0000-0000-000000000000"
    );
    for (case, identifier) in [
        ("a name nothing created", "wording-absent"),
        ("the name of a deleted stack", "wording-gone"),
        ("a stack id no stack carries", ghost_id.as_str()),
    ] {
        let mut worded: BTreeSet<String> = BTreeSet::new();
        for row in not_found_rows() {
            let mut members = vec![member("StackName", identifier)];
            members.extend_from_slice(&row.extra);
            let (status, code, message, body) = refusal(&router, row.action, &members).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {}: expected HTTP 400, got {status} {body}",
                row.action
            );
            assert_eq!(
                code.as_deref(),
                Some("ValidationError"),
                "case {case:?} on {}: expected <Code>ValidationError</Code>, got {code:?}",
                row.action
            );
            let expected = (row.template)(identifier);
            assert_eq!(
                message.as_deref(),
                Some(expected.as_str()),
                "case {case:?} on {}: expected {expected:?}, got {message:?}",
                row.action
            );
            worded.insert(expected);
        }
        // The templates really are several: no single sentence satisfies the table, which is
        // what makes this a per-operation requirement rather than a service-wide one.
        assert_eq!(
            worded.len(),
            5,
            "case {case:?}: expected five distinct templates across the operations, got \
             {worded:?}"
        );
    }

    // The refusals are about the stack the request named, not about the operations: the same
    // calls against a live stack are served.
    deploy(&router, "wording-live", TEMPLATE).await;
    for (case, action, extra) in [
        (
            "a resource of a live stack",
            "DescribeStackResource",
            vec![member("LogicalResourceId", "Topic")],
        ),
        ("a drift check of a live stack", "DetectStackDrift", vec![]),
        (
            "a policy on a live stack",
            "SetStackPolicy",
            vec![member("StackPolicyBody", POLICY)],
        ),
        (
            "a signal to a live stack",
            "SignalResource",
            vec![
                member("LogicalResourceId", "Topic"),
                member("UniqueId", "signal-1"),
                member("Status", "SUCCESS"),
            ],
        ),
    ] {
        let mut members = vec![member("StackName", "wording-live")];
        members.extend_from_slice(&extra);
        let (status, body) = call(&router, action, &members).await;
        assert_eq!(
            status, 200,
            "case {case:?} on {action}: expected the call to be served, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn a_drift_check_of_a_resource_the_stack_lacks_names_the_stack_by_id() {
    let router = router();
    deploy(&router, "wording-drift", TEMPLATE).await;
    deploy(
        &router,
        "wording-other",
        r#"{"Resources":{
             "Elsewhere":{"Type":"AWS::SNS::Topic","Properties":{"DisplayName":"elsewhere"}}}}"#,
    )
    .await;
    let (status, described) = describe_stack(&router, "wording-drift").await;
    assert_eq!(
        status, 200,
        "expected the stack to be readable, got {status} {described}"
    );
    let stack_id = xml_text(&described, "StackId").expect("the stack's own id");

    // The logical ids differ along what the rule generalises over: a name nothing declared,
    // the declared name in another case, a declared name with a suffix, and a logical id
    // ANOTHER stack declares. The stack is addressed by name and by id, and the refusal names
    // it by its id either way — the stack as the service addresses it, not as the request
    // spelled it.
    for addressed in [stack_id.as_str(), "wording-drift"] {
        for (case, logical_id) in [
            ("a logical id nothing declared", "Missing"),
            ("the declared id in another case", "topic"),
            ("the declared id with a suffix", "Topic2"),
            ("a logical id another stack declares", "Elsewhere"),
        ] {
            let members = vec![
                member("StackName", addressed),
                member("LogicalResourceId", logical_id),
            ];
            let (status, code, message, body) =
                refusal(&router, "DetectStackResourceDrift", &members).await;
            assert_eq!(
                status, 400,
                "case {case:?} addressed by {addressed:?}: expected HTTP 400, got {status} \
                 {body}"
            );
            assert_eq!(
                code.as_deref(),
                Some("ValidationError"),
                "case {case:?} addressed by {addressed:?}: expected a ValidationError, got \
                 {code:?}"
            );
            let expected = format!("Resource {logical_id} does not exist in stack {stack_id}");
            assert_eq!(
                message.as_deref(),
                Some(expected.as_str()),
                "case {case:?} addressed by {addressed:?}: expected {expected:?}, got \
                 {message:?}"
            );
        }
    }

    // The resource the stack DOES declare is checked rather than refused, so the rows above
    // were about the logical ids they named.
    let (status, body) = call(
        &router,
        "DetectStackResourceDrift",
        &[
            member("StackName", "wording-drift"),
            member("LogicalResourceId", "Topic"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the declared resource\": expected the check to be served, got {status} {body}"
    );
}

#[tokio::test]
async fn a_drift_result_nothing_issued_is_refused_naming_the_id_the_request_sent() {
    let router = router();
    deploy(&router, "wording-detection", TEMPLATE).await;

    // The ids differ along what the rule generalises over: the shortest the model admits, a
    // label, a uuid nothing issued, and one at the published maximum length
    // (`StackDriftDetectionId` is `@length(min: 1, max: 36)`).
    let longest = "z".repeat(36);
    for (case, detection_id) in [
        ("the shortest id the model admits", "z"),
        ("a label nothing issued", "wording-no-such-detection"),
        ("a uuid nothing issued", "00000000-0000-0000-0000-000000000000"),
        ("an id at the published maximum length", longest.as_str()),
    ] {
        let members = vec![member("StackDriftDetectionId", detection_id)];
        let (status, code, message, body) =
            refusal(&router, "DescribeStackDriftDetectionStatus", &members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            code.as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected a ValidationError, got {code:?}"
        );
        let expected = format!("Stack drift detection result [{detection_id}] does not exist");
        assert_eq!(
            message.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // An id this service DID issue is read rather than refused, so the rows above were about
    // the ids they named.
    let (status, body) = call(
        &router,
        "DetectStackDrift",
        &[member("StackName", "wording-detection")],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DetectStackDrift to be accepted, got {status} {body}"
    );
    let issued = xml_text(&body, "StackDriftDetectionId").expect("a detection id");
    let (status, body) = call(
        &router,
        "DescribeStackDriftDetectionStatus",
        &[member("StackDriftDetectionId", &issued)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"an id this service issued\": expected the status to be readable, got {status} \
         {body}"
    );
}

#[tokio::test]
async fn a_status_gated_rollback_refusal_carries_no_part_of_the_stack_it_refused() {
    let router = router();
    // A stack whose create is still in flight, and the same stack once that create has
    // completed and once an update has: three statuses, none of which either call can be made
    // from.
    let (status, created) = create_stack(&router, "wording-status", TEMPLATE, &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStack to succeed, got {status} {created}"
    );
    let stack_id = xml_text(&created, "StackId").expect("the stack's own id");
    // Every refusal the rows below collect, kept to hold the ANSWERS — not this suite's own
    // constants — to the no-per-run-value check at the end.
    let mut answered: Vec<(&str, &str, String)> = Vec::new();

    for (case, settle_first, update_first, reported_status) in [
        (
            "a create still in flight",
            false,
            false,
            status::CREATE_IN_PROGRESS,
        ),
        (
            "a create that completed",
            true,
            false,
            status::CREATE_COMPLETE,
        ),
        (
            "an update that completed",
            true,
            true,
            status::UPDATE_COMPLETE,
        ),
    ] {
        if settle_first {
            wait_for_the_transition().await;
        }
        if update_first {
            let (status, body) = call(
                &router,
                "UpdateStack",
                &[
                    member("StackName", "wording-status"),
                    member(
                        "TemplateBody",
                        r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
                             "Properties":{"DisplayName":"refusal-wording-updated"}}}}"#,
                    ),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateStack to succeed, got {status} {body}"
            );
            wait_for_the_transition().await;
        }
        // The stack is readable throughout, and NEITHER refusal below carries the stack it
        // was made against.
        let (status, described) = describe_stack(&router, "wording-status").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stack to be readable, got {status} {described}"
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(reported_status),
            "case {case:?}: expected the stack to be in {reported_status:?}, got {described}"
        );

        for (action, expected) in [
            ("ContinueUpdateRollback", CANNOT_CONTINUE.to_string()),
            ("RollbackStack", CANNOT_ROLL_BACK.to_string()),
        ] {
            let members = vec![member("StackName", "wording-status")];
            let (status, code, message, body) = refusal(&router, action, &members).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {action}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                code.as_deref(),
                Some("ValidationError"),
                "case {case:?} on {action}: expected a ValidationError, got {code:?}"
            );
            assert_eq!(
                message.as_deref(),
                Some(expected.as_str()),
                "case {case:?} on {action}: expected {expected:?}, got {message:?}"
            );
            answered.push((case, action, message.unwrap_or_default()));
        }
    }

    // NEITHER sentence carries any part of this run's state: neither names the stack — not by
    // id and not by the name the request sent. `RollbackStack`'s states the statuses the call
    // CAN be made from, so it names the stack's current status no more than it names the
    // stack, and the gate `ContinueUpdateRollback` is refused by here carries no status
    // either.
    let (status, described) = describe_stack(&router, "wording-status").await;
    assert_eq!(
        status, 200,
        "expected the stack to be readable, got {status} {described}"
    );
    let reported = xml_text(&described, "StackStatus").unwrap_or_default();
    assert_eq!(
        reported, status::UPDATE_COMPLETE,
        "expected the refused calls to leave the stack where it was, got {reported:?}"
    );
    for (case, action, sentence) in &answered {
        for leaked in [stack_id.as_str(), "wording-status"] {
            assert!(
                !sentence.contains(leaked),
                "case {case:?} on {action}: expected the refusal to name no part of the stack, \
                 got {leaked:?} in {sentence:?}"
            );
        }
        if *action == "RollbackStack" || *action == "ContinueUpdateRollback" {
            assert!(
                !sentence.contains(reported.as_str()),
                "case {case:?} on {action}: expected the refusal to carry no part of this run, \
                 got the stack's own status {reported:?} in {sentence:?}"
            );
        }
    }
}
