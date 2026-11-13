// SPDX-License-Identifier: CC0-1.0

//! The sentence `DeleteStack` refuses a FORCED deletion with, end to end over the awsQuery
//! wire.
//!
//! RULE under test: **a `DeletionMode` `FORCE_DELETE_STACK` deletion `DeleteStack` cannot
//! start is refused `400 ValidationError` whose message states the invalid operation on the
//! stack, named by its STACK ID, and then the one status in which that mode may be activated.
//! Everything outside the brackets is fixed text: neither the status the stack is actually in
//! nor the identifier form the request spelled the stack with is part of the sentence.**
//!
//! `DeleteStackInput$DeletionMode` is "STANDARD — use the standard behavior. Specifying this
//! value is the same as not specifying this parameter. FORCE_DELETE_STACK — Delete the stack
//! if it's stuck in a DELETE_FAILED state due to resource deletion failure"
//! (`smithy_query(query="operation", operation="DeleteStack")`), and the operation declares no
//! error for a mode its target does not admit, so the refusal is the service-common
//! `ValidationError`. `forced_deletion_is_gated_by_status.rs` is the suite for WHICH statuses
//! are refused; this one is the suite for the WORDING, over the two things a caller can vary
//! without changing the answer: the status the stack sits in, and whether the request named
//! the stack by name or by its id.
//!
//! The sentence is spelled out here rather than imported from the crate, because the sentence
//! IS the requirement: a reworded message must fail this suite.

use std::time::SystemTime;

use service_com_amazonaws_cloudformation_cloudformation::state::{deletion_mode, status};

use crate::harness::{router_with_engine, store, wait_for_the_transition, REGION};
use crate::stack_lifecycle::{call, create_stack, member, xml_text};

/// The template every row's stack is created from: one parameter in Systems Manager, so the
/// stack really holds a resource when the forced deletion arrives.
const TEMPLATE: &str = r#"{"Resources":{"Param":{"Type":"AWS::SSM::Parameter",
     "Properties":{"Type":"String","Value":"forced-delete-wording"}}}}"#;

/// Which identifier form the row's request spells the stack with — the refusal names the
/// stack id either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    /// `StackName` carrying the stack's name.
    ByName,
    /// `StackName` carrying the stack's id, which is what the member also accepts.
    ById,
}

/// One row: the status the forced deletion arrives at, and how the request names the stack.
struct Row {
    case: &'static str,
    /// Unique per row, so each row drives its own stack.
    stack: &'static str,
    seeded: &'static str,
    named: Named,
}

const ROWS: &[Row] = &[
    Row {
        case: "a create that completed, named by name",
        stack: "forced-wording-created-by-name",
        seeded: status::CREATE_COMPLETE,
        named: Named::ByName,
    },
    Row {
        case: "a create that completed, named by id",
        stack: "forced-wording-created-by-id",
        seeded: status::CREATE_COMPLETE,
        named: Named::ById,
    },
    Row {
        case: "an update that completed, named by name",
        stack: "forced-wording-updated-by-name",
        seeded: status::UPDATE_COMPLETE,
        named: Named::ByName,
    },
    Row {
        case: "a create that was rolled back, named by id",
        stack: "forced-wording-rolled-back-by-id",
        seeded: status::ROLLBACK_COMPLETE,
        named: Named::ById,
    },
    Row {
        case: "an update rollback that could not put a resource back, named by name",
        stack: "forced-wording-update-rollback-failed",
        seeded: status::UPDATE_ROLLBACK_FAILED,
        named: Named::ByName,
    },
    Row {
        case: "a stack a CREATE change set brought into existence, named by id",
        stack: "forced-wording-in-review-by-id",
        seeded: status::REVIEW_IN_PROGRESS,
        named: Named::ById,
    },
];

#[tokio::test]
async fn the_forced_deletion_refusal_names_the_stack_id_and_the_status_it_needs() {
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
        // `forced_deletion_is_gated_by_status.rs` seeds the statuses the gate is held to.
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

        let identifier = match row.named {
            Named::ByName => row.stack.to_string(),
            Named::ById => stack_id.clone(),
        };
        let (status_code, refused) = call(
            &router,
            "DeleteStack",
            &[
                member("StackName", &identifier),
                member("DeletionMode", deletion_mode::FORCE_DELETE_STACK),
            ],
        )
        .await;

        assert_eq!(
            status_code, 400,
            "case {:?}: a stack in {} must refuse the forced deletion, got {status_code} \
             {refused}",
            row.case, row.seeded
        );
        let expected = format!(
            "Invalid operation on stack [{stack_id}]. You can activate DeletionMode \
             FORCE_DELETE_STACK in a delete stack operation only when the stack is in the \
             DELETE_FAILED state."
        );
        let answered = (
            xml_text(&refused, "Code").unwrap_or_default(),
            xml_text(&refused, "Message").unwrap_or_default(),
        );
        assert_eq!(
            answered,
            ("ValidationError".to_string(), expected.clone()),
            "case {:?}: expected the refusal to read {expected:?}, got {answered:?}",
            row.case
        );
        // The status the stack is in is NOT a variable part of the sentence: the refusal
        // states the status the mode NEEDS, not the one it found.
        assert!(
            !answered.1.contains(row.seeded),
            "case {:?}: the refusal must not name the status the stack is in, got {:?}",
            row.case,
            answered.1
        );
    }
}
