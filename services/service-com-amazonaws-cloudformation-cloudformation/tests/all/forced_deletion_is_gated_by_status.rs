// SPDX-License-Identifier: CC0-1.0

//! Which stack statuses take a FORCED `DeleteStack`, end to end over the awsQuery wire.
//!
//! RULE under test: **`DeletionMode` `FORCE_DELETE_STACK` deletes a stack that is stuck in
//! `DELETE_FAILED` due to a resource deletion failure, so a deletion it would START is
//! accepted only from that status; from every other status a stack holds, the request is
//! refused `400 ValidationError` whose message names the stack by its stack id and states the
//! one status in which the mode may be activated — fixed text otherwise, carrying neither the
//! status the stack is in nor the identifier form the request used — and nothing of the stack
//! is touched. The standard mode — named, or not named at all — is
//! not gated this way: it is how a stack is deleted from the statuses a deletion starts from.
//! A repeat of a deletion already in flight starts nothing, so it is the same no-op success
//! under either mode.**
//!
//! `DeleteStackInput$DeletionMode` is "STANDARD — use the standard behavior. Specifying this
//! value is the same as not specifying this parameter. FORCE_DELETE_STACK — Delete the stack
//! if it's stuck in a DELETE_FAILED state due to resource deletion failure"
//! (`smithy_query(query="operation", operation="DeleteStack")`): the forced form exists for
//! the one state in which a resource an owning service refused to delete has already failed
//! the deletion, and under it such a resource is ABANDONED rather than deleted —
//! `forced_stack_deletion.rs` is the suite for what the mode then does.
//!
//! The rows are the `StackStatus` values this service puts a stack in (the model's
//! `StackStatus` enum), each seeded on a stack whose create completed: the operations that
//! leave a stack in most of them need an owning service to refuse something, which no wire
//! call of this suite can make happen. Two rows are neither accepted nor refused:
//! `DELETE_COMPLETE`, a stack that is already gone, and `DELETE_IN_PROGRESS`, a deletion
//! already under way — both are the no-op success a request for a state the store is already
//! reaching gets.
//!
//! The sentence is spelled out here rather than imported from the crate, because the sentence
//! IS the requirement: a reworded message must fail this suite.

use std::time::SystemTime;

use service_com_amazonaws_cloudformation_cloudformation::state::{deletion_mode, status};

use crate::harness::{router_with_engine, store, wait_for_the_transition, REGION};
use crate::stack_lifecycle::{call, create_stack, describe_stack, member, xml_text};

/// The template every row's stack is created from: one parameter in Systems Manager, so the
/// stack really holds a resource when the forced deletion arrives.
const TEMPLATE: &str = r#"{"Resources":{"Param":{"Type":"AWS::SSM::Parameter",
     "Properties":{"Type":"String","Value":"forced-delete-gate"}}}}"#;

/// The refusal a forced deletion gets from a stack that is not stuck in a failed deletion:
/// the stack's id, then the one status the mode may be activated in.
fn force_delete_needs_a_failed_deletion(stack_id: &str) -> String {
    format!(
        "Invalid operation on stack [{stack_id}]. You can activate DeletionMode {} in a \
         delete stack operation only when the stack is in the {} state.",
        deletion_mode::FORCE_DELETE_STACK,
        status::DELETE_FAILED
    )
}

/// What a forced deletion gets from a stack in the row's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Forced {
    /// Carried out: the one status the mode is documented for.
    Accepted,
    /// Refused, with the sentence above.
    Refused,
    /// Neither: the deletion the request asks for needs starting no longer, or not at all —
    /// a stack that is already gone, and one whose deletion is already under way.
    NothingToStart,
}

/// One row: the status the stack reports when the forced deletion arrives.
struct Row {
    case: &'static str,
    /// Unique per row, so each row drives its own stack.
    stack: &'static str,
    seeded: &'static str,
    forced: Forced,
}

const ROWS: &[Row] = &[
    Row {
        case: "a deletion an owning service refused",
        stack: "forced-gate-delete-failed",
        seeded: status::DELETE_FAILED,
        forced: Forced::Accepted,
    },
    Row {
        case: "a create still in flight",
        stack: "forced-gate-creating",
        seeded: status::CREATE_IN_PROGRESS,
        forced: Forced::Refused,
    },
    Row {
        case: "a create that completed",
        stack: "forced-gate-created",
        seeded: status::CREATE_COMPLETE,
        forced: Forced::Refused,
    },
    Row {
        case: "a create that failed with its resources preserved",
        stack: "forced-gate-create-failed",
        seeded: status::CREATE_FAILED,
        forced: Forced::Refused,
    },
    Row {
        case: "a create that was rolled back",
        stack: "forced-gate-rolled-back",
        seeded: status::ROLLBACK_COMPLETE,
        forced: Forced::Refused,
    },
    Row {
        case: "a rollback that could not remove what the create had made",
        stack: "forced-gate-rollback-failed",
        seeded: status::ROLLBACK_FAILED,
        forced: Forced::Refused,
    },
    Row {
        case: "an update still in flight",
        stack: "forced-gate-updating",
        seeded: status::UPDATE_IN_PROGRESS,
        forced: Forced::Refused,
    },
    Row {
        case: "an update that completed",
        stack: "forced-gate-updated",
        seeded: status::UPDATE_COMPLETE,
        forced: Forced::Refused,
    },
    Row {
        case: "an update that failed with its resources preserved",
        stack: "forced-gate-update-failed",
        seeded: status::UPDATE_FAILED,
        forced: Forced::Refused,
    },
    Row {
        case: "an update rollback that could not put a resource back",
        stack: "forced-gate-update-rollback-failed",
        seeded: status::UPDATE_ROLLBACK_FAILED,
        forced: Forced::Refused,
    },
    Row {
        case: "a deletion already in flight",
        stack: "forced-gate-deleting",
        seeded: status::DELETE_IN_PROGRESS,
        forced: Forced::NothingToStart,
    },
    Row {
        case: "a stack a CREATE change set brought into existence",
        stack: "forced-gate-in-review",
        seeded: status::REVIEW_IN_PROGRESS,
        forced: Forced::Refused,
    },
    Row {
        case: "a stack whose deletion completed",
        stack: "forced-gate-deleted",
        seeded: status::DELETE_COMPLETE,
        forced: Forced::NothingToStart,
    },
];

#[tokio::test]
async fn only_a_failed_deletion_takes_the_forced_deletion_mode() {
    for row in ROWS {
        let (router, engine) = router_with_engine();
        let (status_code, body) = create_stack(&router, row.stack, TEMPLATE, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must succeed, got {status_code} {body}",
            row.case
        );
        wait_for_the_transition().await;

        // The status the deletion arrives at, seeded directly, the way
        // `stack_status_accepts_an_update.rs` seeds the statuses an update is held to.
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

        let (status_code, deleted) = call(
            &router,
            "DeleteStack",
            &[
                member("StackName", row.stack),
                member("DeletionMode", deletion_mode::FORCE_DELETE_STACK),
            ],
        )
        .await;

        match row.forced {
            Forced::NothingToStart => {
                assert_eq!(
                    status_code, 200,
                    "case {:?}: a stack in {} has no deletion to start, so the call must be \
                     the no-op success, got {status_code} {deleted}",
                    row.case, row.seeded
                );
                continue;
            }
            Forced::Accepted => {
                assert_eq!(
                    status_code, 200,
                    "case {:?}: a stack in {} must take the forced deletion, got {status_code} \
                     {deleted}",
                    row.case, row.seeded
                );
                // Accepted means the deletion really STARTED, not that the call answered:
                // the stack reports the deletion in flight and then goes, so its name
                // stops resolving.
                let (_, in_flight) = describe_stack(&router, &stack_id).await;
                assert_eq!(
                    xml_text(&in_flight, "StackStatus").as_deref(),
                    Some(status::DELETE_IN_PROGRESS),
                    "case {:?}: expected the accepted deletion to be under way, got \
                     {in_flight}",
                    row.case
                );
                wait_for_the_transition().await;
                wait_for_the_transition().await;
                let (status_code, described) = describe_stack(&router, row.stack).await;
                assert_eq!(
                    status_code, 400,
                    "case {:?}: expected the deleted stack's name to resolve nothing, got \
                     {status_code} {described}",
                    row.case
                );
                continue;
            }
            Forced::Refused => {}
        }

        assert_eq!(
            status_code, 400,
            "case {:?}: a stack in {} must refuse the forced deletion, got {status_code} \
             {deleted}",
            row.case, row.seeded
        );
        let expected = force_delete_needs_a_failed_deletion(&stack_id);
        assert_eq!(
            (
                xml_text(&deleted, "Code").unwrap_or_default(),
                xml_text(&deleted, "Message").unwrap_or_default()
            ),
            ("ValidationError".to_string(), expected.clone()),
            "case {:?}: expected the refusal to read {expected:?}, got {deleted}",
            row.case
        );
        // A refused deletion touches nothing: the stack is still there, in the status it was
        // in, and its resource was not deleted either.
        let (status_code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeStacks must answer 200, got {status_code} {described}",
            row.case
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(row.seeded),
            "case {:?}: expected the refused deletion to leave the stack in {}, got \
             {described}",
            row.case,
            row.seeded
        );
    }
}

#[tokio::test]
async fn the_standard_deletion_is_not_gated_by_the_status_the_forced_one_is() {
    // The other side of the rule: the gate is the FORCED mode's, not the deletion's. A stack
    // whose create completed is deleted by the standard behaviour — whether the request names
    // `STANDARD` or names no mode at all, which the member says are the same request.
    for (case, mode) in [
        ("no DeletionMode at all", None),
        ("DeletionMode STANDARD", Some(deletion_mode::STANDARD)),
    ] {
        let router = router_with_engine().0;
        let stack = "standard-delete-is-not-gated";
        let (status_code, body) = create_stack(&router, stack, TEMPLATE, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: CreateStack must succeed, got {status_code} {body}"
        );
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        let mut members = vec![member("StackName", stack)];
        if let Some(mode) = mode {
            members.push(member("DeletionMode", mode));
        }
        let (status_code, deleted) = call(&router, "DeleteStack", &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: the standard deletion of a created stack must be taken, got \
             {status_code} {deleted}"
        );
        wait_for_the_transition().await;
        wait_for_the_transition().await;
        // And it really deleted the stack: the name resolves nothing afterwards.
        let (status_code, described) = describe_stack(&router, stack).await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected the deleted stack's name to resolve nothing, got \
             {status_code} {described}"
        );
    }
}
