// SPDX-License-Identifier: CC0-1.0

//! What a LISTING says about WHY each pipe is in the state it reports, end to end over the
//! wire, with IAM and Amazon SQS mounted beside pipes on one engine (the harness of
//! `tests/all/pipe_metrics.rs` and `tests/all/pipe_bring_up.rs`, reused rather than copied).
//!
//! RULE: a pipe's `StateReason` says how it came to be in the state it is in, and every read
//! that carries the member reports the pipe's OWN reason — so each entry of a listing carries
//! the same reason a read of that one pipe reports: the user-initiated token for a state its
//! caller drove it into, and the sentence saying why for a move that could not be completed.
//!
//! `tests/all/pipe_state_reason.rs` is the table over the lifecycle calls and listing shapes
//! of pipes that all rest where their caller asked them to, and `tests/all/pipe_bring_up.rs`
//! is the table over the moves that could not be completed, read one pipe at a time. What is
//! asserted here and in neither of them is the pipes with DIFFERING reasons in ONE listing:
//! that each entry carries its OWN reason, which is what tells a caller-stopped pipe from a
//! failed one without describing every listed pipe in turn.

use service_com_amazonaws_pipes_pipes::state::execution_role::SERVICE_PRINCIPAL;
use service_com_amazonaws_pipes_pipes::state::pipe_state::{
    CREATE_FAILED, RUNNING, START_FAILED, STOPPED,
};
use service_com_amazonaws_pipes_pipes::state::USER_INITIATED_STATE_REASON;
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_bring_up::{create_pipe, delete_queue, every_call, reason_says, settled};
use crate::pipe_data_plane::given_queue;
use crate::pipe_lifecycle::{call, list, REGION};
use crate::pipe_metrics::{emulator, given_role_stating, withdraw_from_role};
use crate::pipe_state_reason::listed_reasons;

/// How many `Limit=1` pages the paged walk reads before it gives up, so a listing that
/// never stops handing out tokens fails the case instead of looping for ever. Well above
/// the number of pipes the fixture holds.
const PAGE_BUDGET: usize = 16;

/// What a pipe's `StateReason` has to say, as the state it rests in was reached.
enum Reason {
    /// The pipe rests where its caller asked it to: the fixed user-initiated token.
    UserInitiated,
    /// A move that could not be completed: the sentence names the source the pipe could not
    /// poll and carries `mentions` beside it, saying what stopped it.
    Failure {
        source_arn: String,
        mentions: &'static [&'static str],
    },
}

/// One pipe of the fixture: its name, the state it comes to rest in, and the reason it
/// therefore reports.
struct Listed {
    case: &'static str,
    state: &'static str,
    reason: Reason,
}

/// Assert the `StateReason` `read` reported for the pipe `case` is the one the state it
/// rests in was reached by.
fn assert_reason(case: &str, read: &str, reported: Option<&str>, expected: &Reason) {
    let Some(reason) = reported else {
        panic!("case {case:?}: {read} returned the pipe with no StateReason at all");
    };
    match expected {
        Reason::UserInitiated => assert_eq!(
            reason, USER_INITIATED_STATE_REASON,
            "case {case:?}: expected {read} to report the user-initiated reason \
             {USER_INITIATED_STATE_REASON} for a state the caller drove, got {reason:?}"
        ),
        Reason::Failure {
            source_arn,
            mentions,
        } => {
            // The failure sentence's own reading — it names the source, says what stopped
            // the move and stays within the member's bound — is `tests/all/pipe_bring_up.rs`
            // ONE reading of it, so this suite asserts no second version of it.
            reason_says(
                &format!("{case} as {read} reports it"),
                reason,
                source_arn,
                mentions,
            );
            assert_ne!(
                reason, USER_INITIATED_STATE_REASON,
                "case {case:?}: a move that could not be completed must not be reported as \
                 user-initiated, {read} reported {reason:?}"
            );
        }
    }
}

/// The `StateReason` the entries of one listing reported for the pipe `case`, or `None` when
/// the entry carried none. Which pipes a listing returned at all is asserted separately, off
/// the same entries, so `None` here is the member missing and not the pipe missing.
fn reason_for<'entries>(
    entries: &'entries [(String, Option<String>)],
    case: &str,
) -> Option<&'entries str> {
    entries
        .iter()
        .find(|(name, _)| name == case)
        .and_then(|(_, reason)| reason.as_deref())
}

/// A role called `{case}-role` allowing every call a pipe over `source_arn`/`target_arn`
/// makes; answers the ARN IAM persisted. One role per pipe, so narrowing one leaves the
/// others' grants where they were.
async fn role_for(
    router: &MultiServiceRouter,
    case: &str,
    source_arn: &str,
    target_arn: &str,
) -> String {
    given_role_stating(
        router,
        &format!("{case}-role"),
        SERVICE_PRINCIPAL,
        every_call(source_arn, target_arn),
    )
    .await
}

/// RULE: every entry of a `ListPipes` page carries the pipe's own `StateReason` — the same
/// value a read of that one pipe reports — whatever listing shape returned it and however
/// the pipe came to be in the state it rests in.
///
/// The rows differ along what the rule generalises over: how the pipe reached its state (a
/// creation into the state it asked for, a later stop, a creation that could not be
/// completed, a start that could not be completed), the state it rests in (`RUNNING`,
/// `STOPPED`, `CREATE_FAILED`, `START_FAILED`), the reason it therefore reports (the
/// user-initiated token, two different failure sentences), and the shape of the read
/// (unpaged, each modelled filter, and every page of a paged walk).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_listing_reports_the_reason_each_listed_pipe_reached_its_state_by() {
    let router = emulator();
    let (_target_url, target_arn) = given_queue(&router, "reason-target").await;

    // A pipe resting in the state its CREATION asked for.
    let running = "reason-running";
    let (_url, running_source) = given_queue(&router, "reason-running-src").await;
    let running_role = role_for(&router, running, &running_source, &target_arn).await;
    let created = create_pipe(
        &router,
        running,
        &running_source,
        &target_arn,
        &running_role,
        Some(RUNNING),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {running:?}: CreatePipe answered {} {}",
        created.status, created.body
    );

    // A pipe a later STOP drove out of the state its creation asked for.
    let stopped = "reason-stopped";
    let (_url, stopped_source) = given_queue(&router, "reason-stopped-src").await;
    let stopped_role = role_for(&router, stopped, &stopped_source, &target_arn).await;
    let created = create_pipe(
        &router,
        stopped,
        &stopped_source,
        &target_arn,
        &stopped_role,
        Some(RUNNING),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {stopped:?}: CreatePipe answered {} {}",
        created.status, created.body
    );

    // A pipe whose CREATION could not be completed: its source queue is gone before the
    // pipe names it, so there is nothing for the pipe to poll.
    let create_failed = "reason-create-failed";
    let (gone_url, gone_source) = given_queue(&router, "reason-create-failed-src").await;
    let create_failed_role = role_for(&router, create_failed, &gone_source, &target_arn).await;
    delete_queue(&router, create_failed, &gone_url).await;
    let created = create_pipe(
        &router,
        create_failed,
        &gone_source,
        &target_arn,
        &create_failed_role,
        Some(RUNNING),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {create_failed:?}: CreatePipe answered {} {}",
        created.status, created.body
    );

    // A pipe created healthy and STOPPED, whose START then could not be completed because
    // the role it acts as lost the call that takes its source's records.
    let start_failed = "reason-start-failed";
    let (_url, start_failed_source) = given_queue(&router, "reason-start-failed-src").await;
    let start_failed_role =
        role_for(&router, start_failed, &start_failed_source, &target_arn).await;
    let created = create_pipe(
        &router,
        start_failed,
        &start_failed_source,
        &target_arn,
        &start_failed_role,
        Some(STOPPED),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {start_failed:?}: CreatePipe answered {} {}",
        created.status, created.body
    );

    // Every creation has landed before the later moves are asked for: a lifecycle call
    // arriving while one is in flight is refused.
    for case in [running, stopped, create_failed, start_failed] {
        settled(&router, case, case).await;
    }

    let answered = call(
        &router,
        "POST",
        &format!("/v1/pipes/{stopped}/stop"),
        None,
        REGION,
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {stopped:?}: StopPipe answered {} {}",
        answered.status, answered.body
    );
    withdraw_from_role(
        &router,
        start_failed,
        &format!("{start_failed}-role"),
        &["sqs:SendMessage"],
    )
    .await;
    let answered = call(
        &router,
        "POST",
        &format!("/v1/pipes/{start_failed}/start"),
        None,
        REGION,
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {start_failed:?}: StartPipe answered {} {}",
        answered.status, answered.body
    );

    let fixture = [
        Listed {
            case: running,
            state: RUNNING,
            reason: Reason::UserInitiated,
        },
        Listed {
            case: stopped,
            state: STOPPED,
            reason: Reason::UserInitiated,
        },
        Listed {
            case: create_failed,
            state: CREATE_FAILED,
            reason: Reason::Failure {
                source_arn: gone_source,
                mentions: &["does not exist"],
            },
        },
        Listed {
            case: start_failed,
            state: START_FAILED,
            reason: Reason::Failure {
                source_arn: start_failed_source,
                mentions: &["sqs:ReceiveMessage"],
            },
        },
    ];

    // What a read of ONE pipe reports, once every move has come to rest: the state it
    // settled in and the reason that state was reached by.
    for pipe in &fixture {
        let case = pipe.case;
        let reported = settled(&router, case, case).await;
        let state = reported.string("CurrentState");
        assert_eq!(
            state, pipe.state,
            "case {case:?}: expected the pipe to rest in {}, it reports {state} reason={:?}",
            pipe.state,
            reported.member("StateReason")
        );
        assert_reason(
            case,
            "DescribePipe",
            reported.member("StateReason").as_str(),
            &pipe.reason,
        );
    }

    // (the listing's case, its query, the pipes it has to return)
    let listings: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "unpaged",
            String::new(),
            vec![create_failed, running, start_failed, stopped],
        ),
        (
            "name-prefix",
            format!("?NamePrefix={create_failed}"),
            vec![create_failed],
        ),
        (
            "name-prefix-matching-two",
            "?NamePrefix=reason-st".to_string(),
            vec![start_failed, stopped],
        ),
        (
            "current-state-filter",
            format!("?CurrentState={START_FAILED}"),
            vec![start_failed],
        ),
        (
            "desired-state-filter",
            format!("?DesiredState={STOPPED}"),
            vec![stopped],
        ),
        (
            "limit-covering-every-match",
            "?Limit=10".to_string(),
            vec![create_failed, running, start_failed, stopped],
        ),
    ];
    for (listing, query, expected) in listings {
        let listed = list(&router, &query).await;
        assert_eq!(
            listed.status, 200,
            "case {listing:?}: ListPipes answered {} {}",
            listed.status, listed.body
        );
        let reasons = listed_reasons(&listed);
        let names: Vec<&str> = reasons.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names, expected,
            "case {listing:?}: expected the listing to return {expected:?}, got {names:?}"
        );
        for pipe in fixture.iter().filter(|pipe| expected.contains(&pipe.case)) {
            assert_reason(
                pipe.case,
                &format!("the {listing:?} listing"),
                reason_for(&reasons, pipe.case),
                &pipe.reason,
            );
        }
    }

    // A paged walk at the smallest limit: the member is on the entry of EVERY page, and
    // the pages together are the whole inventory, in name order, with a reason on each pipe.
    let mut walked: Vec<(String, Option<String>)> = Vec::new();
    let mut query = "?Limit=1".to_string();
    for page in 1..=PAGE_BUDGET {
        let listed = list(&router, &query).await;
        assert_eq!(
            listed.status, 200,
            "case \"page-{page}\": ListPipes answered {} {}",
            listed.status, listed.body
        );
        let reasons = listed_reasons(&listed);
        assert_eq!(
            reasons.len(),
            1,
            "case \"page-{page}\": expected one entry on a Limit=1 page, got {listed:?}"
        );
        walked.extend(reasons);
        let token = listed.string("NextToken");
        if token.is_empty() {
            break;
        }
        query = format!("?Limit=1&NextToken={token}");
    }
    let walked_names: Vec<&str> = walked.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        walked_names,
        vec![create_failed, running, start_failed, stopped],
        "case \"paged-walk\": expected the pages to enumerate every pipe, got {walked_names:?}"
    );
    for pipe in &fixture {
        assert_reason(
            pipe.case,
            "the paged walk",
            reason_for(&walked, pipe.case),
            &pipe.reason,
        );
    }
}
