// SPDX-License-Identifier: CC0-1.0

//! Which stack statuses take an `UpdateStack` at all, end to end over the awsQuery wire.
//!
//! RULE under test: **an update changes the configuration the stack HOLDS, so it is
//! accepted from the statuses the stack's last operation COMPLETED in, and from the failed
//! statuses an operation that PRESERVED what it had provisioned leaves — where such a stack
//! takes one only from a caller that asks for that state to be preserved again; a stack whose
//! own failure another operation has to resolve first — a deletion or a
//! rollback that could not be carried out, and a create that was rolled back — and one a
//! `CREATE` change set brought into existence refuse the request with a `ValidationError`
//! reading `Stack:<stack-id> is in <status> state and can not be updated.`**
//!
//! `UpdateStackInput$DisableRollback` is "preserve the state of previously provisioned
//! resources when an operation fails" — correcting the template and asking again is what
//! preserving them is for, which is why the two statuses it leaves take an update while the
//! statuses a FAILED delete or rollback leaves do not: those are resolved by retrying the
//! delete, by continuing the rollback, or by deleting the stack. That member defaults to
//! `False`, so an update of a stack whose own operation failed and does not restate it is not
//! asking for the failed state to be kept at all: that stack is mid-change, and the refusal
//! names the two ways on from it rather than saying the status takes no update.
//!
//! The rows are the `StackStatus` values this service puts a stack in (the model's
//! `StackStatus` enum, read with `smithy_query`), each seeded on a stack whose create
//! completed: the five that take an update and the five that refuse it. The statuses of an
//! operation still IN PROGRESS are refused by the same rule and are pinned by
//! `stack_lifecycle.rs`, which drives a second call while one is in flight; `DELETE_COMPLETE`
//! is a stack that no longer exists, which answers that it does not.

use std::time::SystemTime;

use crate::stack_lifecycle::{call, create_stack, describe_stack, member, xml_text};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    stack_cannot_be_updated, stack_is_in_a_non_terminal_state, status,
};

pub(crate) use crate::harness::{router_with_engine, store, wait_for_the_transition, REGION};

/// The template the stack is created with, and — with another value — the update it is
/// asked for: a real change, so an accepted update has something to carry out.
fn template(value: &str) -> String {
    format!(
        r#"{{"Resources":{{"Param":{{"Type":"AWS::SSM::Parameter",
             "Properties":{{"Type":"String","Value":"{value}"}}}}}}}}"#
    )
}

/// One row: the status the stack reports when the update arrives, and whether an update is
/// accepted from it.
struct Row {
    case: &'static str,
    /// Unique per row, so each row drives its own stack.
    stack: &'static str,
    seeded: &'static str,
    /// Whether the update RESTATES `DisableRollback`, which is what an update of a stack
    /// whose own update failed has to ask for.
    preserves: bool,
    accepted: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a create that completed",
        stack: "updatable-created",
        seeded: status::CREATE_COMPLETE,
        preserves: false,
        accepted: true,
    },
    Row {
        case: "an update that completed",
        stack: "updatable-updated",
        seeded: status::UPDATE_COMPLETE,
        preserves: false,
        accepted: true,
    },
    Row {
        case: "an update whose rollback completed",
        stack: "updatable-rolled-back",
        seeded: status::UPDATE_ROLLBACK_COMPLETE,
        preserves: false,
        accepted: true,
    },
    Row {
        case: "a create that failed, asked again to preserve what it provisioned",
        stack: "updatable-create-failed",
        seeded: status::CREATE_FAILED,
        preserves: true,
        accepted: true,
    },
    Row {
        case: "a create that failed, with nothing asked about preserving it",
        stack: "updatable-create-failed-unpreserved",
        seeded: status::CREATE_FAILED,
        preserves: false,
        accepted: false,
    },
    Row {
        case: "an update that failed, asked again to preserve what it provisioned",
        stack: "updatable-update-failed",
        seeded: status::UPDATE_FAILED,
        preserves: true,
        accepted: true,
    },
    Row {
        case: "an update that failed, with nothing asked about preserving it",
        stack: "updatable-update-failed-unpreserved",
        seeded: status::UPDATE_FAILED,
        preserves: false,
        accepted: false,
    },
    Row {
        case: "a deletion an owning service refused",
        stack: "updatable-delete-failed",
        seeded: status::DELETE_FAILED,
        preserves: false,
        accepted: false,
    },
    Row {
        case: "a create that was rolled back",
        stack: "updatable-rollback-complete",
        seeded: status::ROLLBACK_COMPLETE,
        preserves: false,
        accepted: false,
    },
    Row {
        case: "a rollback that could not remove what the create had made",
        stack: "updatable-rollback-failed",
        seeded: status::ROLLBACK_FAILED,
        preserves: false,
        accepted: false,
    },
    Row {
        case: "an update rollback that could not put a resource back",
        stack: "updatable-update-rollback-failed",
        seeded: status::UPDATE_ROLLBACK_FAILED,
        preserves: false,
        accepted: false,
    },
    Row {
        case: "a stack a CREATE change set brought into existence",
        stack: "updatable-in-review",
        seeded: status::REVIEW_IN_PROGRESS,
        preserves: false,
        accepted: false,
    },
];

#[tokio::test]
async fn only_a_stack_that_holds_a_configuration_takes_an_update() {
    for row in ROWS {
        let (router, engine) = router_with_engine();
        let (code, body) = create_stack(&router, row.stack, &template("before"), &[]).await;
        assert_eq!(
            code, 200,
            "case {:?}: CreateStack must succeed, got {code} {body}",
            row.case
        );
        wait_for_the_transition().await;

        // The status the update arrives at, seeded directly: the operations that leave a
        // stack in most of these need an owning service to refuse something, which no wire
        // call of this suite can make happen.
        let stack_id = {
            let state = store(&engine);
            let mut held = state.lock();
            // The create's own clock-owed completion is materialised FIRST, so the status
            // seeded below is not overwritten by the transition the next call settles.
            held.settle(SystemTime::now());
            let stack = held
                .resolve_mut(REGION, row.stack)
                .expect("the stack just created");
            stack.status = row.seeded.to_string();
            stack.stack_id.clone()
        };

        let mut members = vec![
            member("StackName", row.stack),
            member("TemplateBody", &template("after")),
        ];
        if row.preserves {
            members.push(member("DisableRollback", "true"));
        }
        let (code, updated) = call(&router, "UpdateStack", &members).await;

        if row.accepted {
            assert_eq!(
                code, 200,
                "case {:?}: a stack in {} must take an update, got {code} {updated}",
                row.case, row.seeded
            );
            continue;
        }

        assert_eq!(
            code, 400,
            "case {:?}: a stack in {} must refuse an update, got {code} {updated}",
            row.case, row.seeded
        );
        assert_eq!(
            xml_text(&updated, "Code").as_deref(),
            Some("ValidationError"),
            "case {:?}: expected a ValidationError, got {updated}",
            row.case
        );
        // A status that takes no update at all is refused by the sentence that names it; a
        // stack whose own operation failed and was left standing is mid-change, and its
        // refusal names the two ways on from that state instead.
        let expected = if service_com_amazonaws_cloudformation_cloudformation::state::status_preserved_a_failure(row.seeded) {
            stack_is_in_a_non_terminal_state(row.seeded)
        } else {
            stack_cannot_be_updated(&stack_id, row.seeded)
        };
        assert_eq!(
            xml_text(&updated, "Message").as_deref(),
            Some(expected.as_str()),
            "case {:?}: expected the refusal to read {expected:?}, got {updated}",
            row.case
        );
        // A refused update changes nothing: the stack is still in the status it was in.
        let (code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            code, 200,
            "case {:?}: DescribeStacks must answer 200, got {code} {described}",
            row.case
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(row.seeded),
            "case {:?}: expected the refused update to leave the stack in {}, got \
             {described}",
            row.case,
            row.seeded
        );
    }
}
