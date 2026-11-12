// SPDX-License-Identifier: CC0-1.0

//! Which stack statuses `ContinueUpdateRollback` can be called from, and how the refusal from
//! every other one is WORDED, end to end over the awsQuery wire.
//!
//! RULE under test: **the operation "continues rolling back a stack from
//! UPDATE_ROLLBACK_FAILED to UPDATE_ROLLBACK_COMPLETE state", so it is served only for a
//! stack in `UPDATE_ROLLBACK_FAILED`; from any other `StackStatus` it is refused with HTTP
//! 400 and a `ValidationError` whose message is the status gate's own FIXED sentence —
//! `RollbackUpdatedStack cannot be called from current stack status`, naming the call
//! CloudFormation's control plane makes for this operation and saying the stack's current
//! status is not one it can be made from, with no part of this run in it — and the stack is
//! left exactly where it was.**
//!
//! The sentence is spelled out here rather than imported from the crate, because the sentence
//! IS the requirement: a reworded message, one naming some other call, or one that
//! interpolates the status or the stack it was called against must fail this suite.
//!
//! The rows span the class the rule generalises over: every value of the model's `StackStatus`
//! enum, read with
//! `smithy_query(query="shape",shape="StackStatus",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")`
//! — the in-progress statuses, the completed ones, the failed ones, the cleanup phases and the
//! import statuses this service never puts a stack into itself — each seeded on a stack whose
//! create completed, plus `UPDATE_ROLLBACK_FAILED`, the one status the call IS served from.
//! `DELETE_COMPLETE` is left out: such a stack no longer exists, so what it answers is the
//! stack-not-found refusal of this operation rather than a refusal about a status
//! (`stack_refusal_wording.rs` pins that one).

use std::collections::BTreeSet;
use std::time::SystemTime;

use service_com_amazonaws_cloudformation_cloudformation::state::status;

use crate::harness::{router_with_engine, store, wait_for_the_transition, REGION};
use crate::stack_lifecycle::{call, create_stack, describe_stack, member, xml_text};

/// The template these stacks are deployed from: one topic, really created in Amazon SNS, so
/// the stack the continuation IS served for has a resource to re-declare there.
const TEMPLATE: &str = r#"{"Resources":{
     "Topic":{"Type":"AWS::SNS::Topic","Properties":{"DisplayName":"continue-gate"}}}}"#;

/// The sentence CloudFormation answers when the continuation arrives at a stack whose status
/// it cannot be started from: the call its control plane makes, and that the current status
/// is not one the call can be made from — whichever status that is.
const CANNOT_CONTINUE: &str = "RollbackUpdatedStack cannot be called from current stack status";

/// Every `StackStatus` the model declares except `UPDATE_ROLLBACK_FAILED` (the one status the
/// call is served from) and `DELETE_COMPLETE` (a stack that no longer exists).
///
/// The names are the model's own enum values; the ones this service puts a stack into have a
/// constant in `state::status`, and the two import-rollback statuses it never produces are
/// spelled out from the enum, because the gate must word a status whatever put the stack
/// there.
const REFUSED_FROM: &[&str] = &[
    status::CREATE_IN_PROGRESS,
    status::CREATE_FAILED,
    status::CREATE_COMPLETE,
    status::ROLLBACK_IN_PROGRESS,
    status::ROLLBACK_FAILED,
    status::ROLLBACK_COMPLETE,
    status::DELETE_IN_PROGRESS,
    status::DELETE_FAILED,
    status::UPDATE_IN_PROGRESS,
    status::UPDATE_COMPLETE_CLEANUP_IN_PROGRESS,
    status::UPDATE_COMPLETE,
    status::UPDATE_FAILED,
    status::UPDATE_ROLLBACK_IN_PROGRESS,
    status::UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS,
    status::UPDATE_ROLLBACK_COMPLETE,
    status::REVIEW_IN_PROGRESS,
    status::IMPORT_IN_PROGRESS,
    status::IMPORT_COMPLETE,
    "IMPORT_ROLLBACK_IN_PROGRESS",
    status::IMPORT_ROLLBACK_COMPLETE,
    "IMPORT_ROLLBACK_FAILED",
];

#[tokio::test]
async fn the_continuation_is_refused_from_every_other_status_naming_the_call_and_that_status() {
    // The rows are the statuses themselves, so a duplicate would silently shrink the table,
    // and neither excluded status may appear among them.
    let listed: BTreeSet<&str> = REFUSED_FROM.iter().copied().collect();
    assert_eq!(
        listed.len(),
        REFUSED_FROM.len(),
        "expected each status to be listed once, got {REFUSED_FROM:?}"
    );
    for excluded in [status::UPDATE_ROLLBACK_FAILED, status::DELETE_COMPLETE] {
        assert!(
            !listed.contains(excluded),
            "expected {excluded:?} to be left out of the refusal table, got {REFUSED_FROM:?}"
        );
    }

    let (router, engine) = router_with_engine();
    let stack_name = "continue-gate-refused";
    let (code, body) = create_stack(&router, stack_name, TEMPLATE, &[]).await;
    assert_eq!(
        code, 200,
        "expected CreateStack to succeed, got {code} {body}"
    );
    wait_for_the_transition().await;

    // Every sentence the table collects: the gate words one refusal whatever status it
    // refused from, so the set this fills must hold exactly that sentence.
    let mut answered: BTreeSet<String> = BTreeSet::new();
    for seeded in REFUSED_FROM {
        // The status the call arrives at, seeded directly: most of these need an operation
        // this suite cannot drive — an owning service that refuses something, an import, a
        // cleanup phase in flight — and the gate's rule is about the status, whatever put the
        // stack in it.
        let stack_id = {
            let state = store(&engine);
            let mut held = state.lock();
            // The create's own clock-owed completion is materialised FIRST, so the status
            // seeded below is not overwritten by the transition a later call settles.
            held.settle(SystemTime::now());
            let stack = held
                .resolve_mut(REGION, stack_name)
                .expect("the stack just created");
            stack.status = (*seeded).to_string();
            stack.stack_id.clone()
        };

        let (code, refused) = call(
            &router,
            "ContinueUpdateRollback",
            &[member("StackName", stack_name)],
        )
        .await;
        assert_eq!(
            code, 400,
            "case {seeded:?}: expected HTTP 400, got {code} {refused}"
        );
        assert_eq!(
            xml_text(&refused, "Type").as_deref(),
            Some("Sender"),
            "case {seeded:?}: expected a Sender fault, got {refused}"
        );
        assert_eq!(
            xml_text(&refused, "Code").as_deref(),
            Some("ValidationError"),
            "case {seeded:?}: expected a ValidationError, got {refused}"
        );
        let message = xml_text(&refused, "Message");
        assert_eq!(
            message.as_deref(),
            Some(CANNOT_CONTINUE),
            "case {seeded:?}: expected the refusal to read {CANNOT_CONTINUE:?}, got {message:?}"
        );
        // The sentence carries the call and nothing of this run: not the status the stack is
        // really in, not the stack's id, and not the name the request sent.
        let sentence = message.unwrap_or_default();
        for leaked in [stack_id.as_str(), stack_name, *seeded] {
            assert!(
                !sentence.contains(leaked),
                "case {seeded:?}: expected the refusal to name no part of the stack, got \
                 {leaked:?} in {sentence:?}"
            );
        }
        answered.insert(sentence);

        // A refused continuation changes nothing: the stack is still in the status it was in.
        let (code, described) = describe_stack(&router, stack_name).await;
        assert_eq!(
            code, 200,
            "case {seeded:?}: expected the stack to stay readable, got {code} {described}"
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(*seeded),
            "case {seeded:?}: expected the refused call to leave the stack where it was, got \
             {described}"
        );
    }

    assert_eq!(
        answered.into_iter().collect::<Vec<String>>(),
        vec![CANNOT_CONTINUE.to_string()],
        "expected the one fixed sentence from every status"
    );
}

#[tokio::test]
async fn the_continuation_is_served_from_the_one_status_it_resumes_a_rollback_from() {
    let (router, engine) = router_with_engine();
    let stack_name = "continue-gate-served";
    let (code, body) = create_stack(&router, stack_name, TEMPLATE, &[]).await;
    assert_eq!(
        code, 200,
        "expected CreateStack to succeed, got {code} {body}"
    );
    wait_for_the_transition().await;

    // The one status the call resumes from, seeded directly: an owning service that refuses to
    // put a resource back is what produces it, and no wire call of this suite can make a
    // service refuse.
    {
        let state = store(&engine);
        let mut held = state.lock();
        held.settle(SystemTime::now());
        let stack = held
            .resolve_mut(REGION, stack_name)
            .expect("the stack just created");
        stack.status = status::UPDATE_ROLLBACK_FAILED.to_string();
    }

    let (code, served) = call(
        &router,
        "ContinueUpdateRollback",
        &[member("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {:?}: expected the continuation to be served, got {code} {served}",
        status::UPDATE_ROLLBACK_FAILED
    );

    // ...and it really resumes the rollback: the stack reaches the status the operation
    // documents as its destination, read back through `DescribeStacks`.
    wait_for_the_transition().await;
    let (code, described) = describe_stack(&router, stack_name).await;
    assert_eq!(
        code, 200,
        "expected the stack to be readable, got {code} {described}"
    );
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::UPDATE_ROLLBACK_COMPLETE),
        "case {:?}: expected the resumed rollback to reach {}, got {described}",
        status::UPDATE_ROLLBACK_FAILED,
        status::UPDATE_ROLLBACK_COMPLETE
    );
}
