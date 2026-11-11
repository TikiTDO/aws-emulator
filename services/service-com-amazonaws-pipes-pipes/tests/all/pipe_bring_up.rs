// SPDX-License-Identifier: CC0-1.0

//! Whether a pipe is BROUGHT UP at all — end to end over the wire, with IAM, Amazon SQS and
//! CloudWatch mounted beside pipes on one engine (the harness of `tests/all/pipe_metrics.rs`,
//! reused rather than copied).
//!
//! The RULE asserted over the tables below: a lifecycle call that puts a pipe on its way to
//! a healthy state completes only if the pipe could POLL ITS SOURCE — the source has to be a
//! resource the service that owns it holds, and the pipe's execution role has to be one this
//! service can assume AND one whose policies allow the call that TAKES that source's records
//! — so a pipe that could not settles in the terminal failure state of the move that was in
//! flight (`CREATE_FAILED` for the creation, `START_FAILED` for the start, `UPDATE_FAILED`
//! for the reconfiguration), carrying a non-empty `StateReason` that says which, and it runs
//! nothing: nothing is consumed, nothing is delivered and it publishes no counter at all.
//!
//! The TARGET side is the other half of the same rule and is deliberately NOT part of it: a
//! pipe whose role may not write to its target comes UP and fails each execution at the
//! target stage (`tests/all/pipe_metrics.rs`, `tests/all/pipe_source_checkout.rs`), which is
//! what lets it deliver the record it withheld as soon as that grant is in force. The last
//! two rows of the create table are that control, held here beside the refusals so the
//! asymmetry is asserted and not assumed.
//!
//! `tests/all/pipe_unassumable_role.rs` is the trust document's own table — the first thing
//! this rule reads — and carries the rows that generalise over what a trust document says.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_pipes_pipes::state::execution_role::{
    SERVICE_PRINCIPAL, STATE_REASON_LENGTH_MAX,
};
use service_com_amazonaws_pipes_pipes::state::metrics::PIPE_COUNTERS;
use service_com_amazonaws_pipes_pipes::state::pipe_state::{
    CREATE_FAILED, RUNNING, START_FAILED, STOPPED, UPDATE_FAILED,
};
use service_com_amazonaws_pipes_pipes::state::TRANSIENT_PIPE_STATES;
use service_com_amazonaws_pipes_pipes::state::USER_INITIATED_STATE_REASON;
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_data_plane::{
    collect_at_least, counted, given_queue, send, sqs, QUIET, VISIBLE_ATTRIBUTE,
};
use crate::pipe_lifecycle::{call, describe, Reply, REGION};
use crate::pipe_metrics::{emulator, given_role_stating, series, withdraw_from_role};

/// How long a pipe is given to settle out of the state a lifecycle call left it in. Well
/// above the state-settle span, so a row fails only when the pipe never settled at all.
const BUDGET: Duration = Duration::from_secs(10);

/// How long between two looks at a pipe while waiting for it to settle.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// `TargetParameters.InputTemplate`, so a delivered message carries the record's own body
/// and a delivery can be attributed: `<$.body>` selects the `body` of the record envelope.
const BODY_TEMPLATE: &str = "<$.body>";

/// The ARN form Amazon SQS gives a queue of this account and Region, for the row whose
/// source queue is never created at all: there is no `CreateQueue` answer to read one off.
fn unheld_queue_arn(name: &str) -> String {
    format!(
        "arn:aws:sqs:{REGION}:{}:{name}",
        engine::emulator_account_id()
    )
}

/// Every Amazon SQS call a pipe with a queue source and a queue target makes, allowed on
/// exactly the resource each is made on.
fn every_call(source: &str, target: &str) -> Value {
    json!([
        {
            "Effect": "Allow",
            "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"],
            "Resource": source,
        },
        { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": target },
    ])
}

/// The delivery and nothing else: a role that may write the target and may not take the
/// source's records.
fn the_target_only(target: &str) -> Value {
    json!([{ "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": target }])
}

/// Delete the queue at `url`, so a pipe's `Source` keeps its well-formed ARN while the
/// resource that ARN names is gone.
async fn delete_queue(router: &MultiServiceRouter, case: &str, url: &str) {
    let (status, body) = sqs(router, "DeleteQueue", &json!({ "QueueUrl": url })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteQueue answered {status} {body}"
    );
}

/// `CreatePipe` for one case: the source and target it names, the role it acts as, and the
/// `DesiredState` it asks for when it asks for one.
async fn create_pipe(
    router: &MultiServiceRouter,
    name: &str,
    source_arn: &str,
    target_arn: &str,
    role_arn: &str,
    desired_state: Option<&str>,
) -> Reply {
    let mut body = json!({
        "Source": source_arn,
        "Target": target_arn,
        "RoleArn": role_arn,
        "SourceParameters": { "SqsQueueParameters": { "BatchSize": 1 } },
        "TargetParameters": { "InputTemplate": BODY_TEMPLATE },
    });
    if let Some(desired_state) = desired_state {
        body["DesiredState"] = json!(desired_state);
    }
    call(router, "POST", &format!("/v1/pipes/{name}"), Some(body), REGION).await
}

/// `DescribePipe` once the pipe has settled out of whatever move was in flight, or a panic
/// naming the state it was still passing through when the budget ran out.
async fn settled(router: &MultiServiceRouter, case: &str, name: &str) -> Reply {
    let deadline = Instant::now() + BUDGET;
    loop {
        let last = describe(router, name).await;
        assert_eq!(
            last.status, 200,
            "case {case:?}: DescribePipe answered {} {}",
            last.status, last.body
        );
        let state = last.string("CurrentState");
        // The states a pipe PASSES THROUGH on its way out of a lifecycle call, read from
        // the crate's own list so this cannot drift from it.
        if !TRANSIENT_PIPE_STATES.contains(&state.as_str()) {
            return last;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the pipe was still {state} after {BUDGET:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Assert the pipe `name` reports no datapoint of any counter: a pipe that was never brought
/// up publishes nothing at all, which is what tells it from one that ran and was refused.
async fn publishes_nothing(router: &MultiServiceRouter, case: &str, name: &str) {
    for metric in PIPE_COUNTERS {
        let (points, sum) = series(router, name, metric).await;
        assert_eq!(
            (points, sum),
            (0, 0.0),
            "case {case:?}: a pipe that never ran must publish no {metric} series, got \
             {points} datapoint(s) summing to {sum}"
        );
    }
}

/// Assert the `StateReason` of a failed move names the source and carries `mentions`, within
/// the bound the member that reports it has.
fn reason_says(case: &str, reason: &str, source_arn: &str, mentions: &[&str]) {
    assert!(
        reason.contains(source_arn),
        "case {case:?}: expected the StateReason to name the source {source_arn}, got \
         {reason:?}"
    );
    for mention in mentions {
        assert!(
            reason.contains(mention),
            "case {case:?}: expected the StateReason to carry {mention:?}, got {reason:?}"
        );
    }
    assert!(
        reason.chars().count() <= STATE_REASON_LENGTH_MAX,
        "case {case:?}: the StateReason is {} characters, past the \
         {STATE_REASON_LENGTH_MAX} a PipeStateReason may carry: {reason:?}",
        reason.chars().count()
    );
}

/// One row of the create table: how its source and role are wired, and where the creation
/// lands.
struct Row {
    case: &'static str,
    /// Whether the row's source queue is a resource the account holds when the pipe is
    /// created. A row whose source is not held either never created it (`created_source`
    /// is false) or created it and deleted it again, which is a well-formed queue ARN
    /// naming nothing.
    held_source: bool,
    /// Whether the row's source queue is created at all.
    created_source: bool,
    /// The statements the row's execution role carries, built from its source and target.
    policy: fn(&str, &str) -> Value,
    /// The `DesiredState` the creation asks for, or `None` to leave it at the default.
    desired_state: Option<&'static str>,
    /// The `CurrentState` the pipe has to settle in.
    settles_in: &'static str,
    /// Text the settled `StateReason` has to carry beside the source ARN.
    reason_mentions: &'static [&'static str],
    /// Whether the pipe, having been brought up, must move its record to its target.
    delivers: bool,
}

/// What one row was given, so the assertions can read the resources it named.
struct Prepared<'row> {
    row: &'row Row,
    source_arn: String,
    /// The url of the row's source queue, for a row that created one.
    source_url: Option<String>,
    target_url: String,
    /// The body the row sent to its source, for a row whose source was there to take one.
    marker: Option<String>,
}

/// RULE: `CreatePipe` completes only if the pipe it creates could poll its source — the
/// source is a resource its owner holds, and the role the pipe acts as may make the call that
/// takes that source's records — and a creation that could not settles in `CREATE_FAILED`
/// with a `StateReason` saying which, having consumed nothing, delivered nothing and
/// published nothing.
///
/// The rows differ along what the rule generalises over: WHY the pipe could not poll (its
/// source was deleted, its source was never created, its role's policies state nothing about
/// the source read, they allow it on another queue only, they deny it), HOW a policy names
/// the call it allows (exactly, by wildcard), and which `DesiredState` the creation asked for
/// — plus the two controls that fix the rule's edge: a role that may read the source and not
/// write the target brings the pipe UP, because the target is the data plane's to fail.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_pipe_brings_up_only_a_pipe_that_could_poll_its_source() {
    let rows = [
        Row {
            case: "source-queue-deleted-before-the-pipe",
            held_source: false,
            created_source: true,
            policy: every_call,
            desired_state: Some(RUNNING),
            settles_in: CREATE_FAILED,
            reason_mentions: &["does not exist"],
            delivers: false,
        },
        Row {
            // The same failure where the queue was never created at all, and the creation
            // asked for STOPPED: a creation that cannot be completed lands in
            // `CREATE_FAILED` whichever state it was asked to reach.
            case: "source-queue-never-created",
            held_source: false,
            created_source: false,
            policy: every_call,
            desired_state: Some(STOPPED),
            settles_in: CREATE_FAILED,
            reason_mentions: &["does not exist"],
            delivers: false,
        },
        Row {
            // The policies in force on the role say nothing about taking the source's
            // records, which withholds that call.
            case: "role-stating-nothing-about-the-source-read",
            held_source: true,
            created_source: true,
            policy: |_source, target| the_target_only(target),
            desired_state: Some(RUNNING),
            settles_in: CREATE_FAILED,
            reason_mentions: &["sqs:ReceiveMessage"],
            delivers: false,
        },
        Row {
            // The read allowed on another queue, which does not reach this one.
            case: "role-reading-another-queue-only",
            held_source: true,
            created_source: true,
            policy: |source, target| {
                json!([
                    {
                        "Effect": "Allow",
                        "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage"],
                        "Resource": format!("{source}-elsewhere"),
                    },
                    { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": target },
                ])
            },
            desired_state: Some(RUNNING),
            settles_in: CREATE_FAILED,
            reason_mentions: &["sqs:ReceiveMessage"],
            delivers: false,
        },
        Row {
            // Allowed everything and then denied the one call: IAM's deny wins, so the
            // pipe could never take a record.
            case: "role-denied-the-source-read",
            held_source: true,
            created_source: true,
            policy: |source, _target| {
                json!([
                    { "Effect": "Allow", "Action": "sqs:*", "Resource": "*" },
                    { "Effect": "Deny", "Action": "sqs:ReceiveMessage", "Resource": source },
                ])
            },
            desired_state: None,
            settles_in: CREATE_FAILED,
            reason_mentions: &["sqs:ReceiveMessage"],
            delivers: false,
        },
        Row {
            // CONTROL: the source read allowed under a wildcard action and read with IAM's
            // own wildcards — the pipe comes up and moves its record.
            case: "role-reading-the-source-by-wildcard",
            held_source: true,
            created_source: true,
            policy: |source, target| {
                json!([
                    { "Effect": "Allow", "Action": "sqs:*Message", "Resource": source },
                    { "Effect": "Allow", "Action": "sqs:Send*", "Resource": target },
                ])
            },
            desired_state: None,
            settles_in: RUNNING,
            reason_mentions: &[],
            delivers: true,
        },
        Row {
            // CONTROL: the source side is in place and the TARGET side is not, which is a
            // pipe that comes up and fails its deliveries — not one that is never brought
            // up.
            case: "role-reading-the-source-and-not-writing-the-target",
            held_source: true,
            created_source: true,
            policy: |source, _target| {
                json!([{
                    "Effect": "Allow",
                    "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage"],
                    "Resource": source,
                }])
            },
            desired_state: Some(RUNNING),
            settles_in: RUNNING,
            reason_mentions: &[],
            delivers: false,
        },
    ];

    let router = emulator();
    // Every row's pipe is created first, so the rows' settle spans overlap instead of being
    // paid one after another.
    let mut prepared: Vec<Prepared<'_>> = Vec::new();
    for row in &rows {
        let case = row.case;
        let (target_url, target_arn) = given_queue(&router, &format!("{case}-tgt")).await;
        let (source_url, source_arn) = if row.created_source {
            let (url, arn) = given_queue(&router, &format!("{case}-src")).await;
            (Some(url), arn)
        } else {
            (None, unheld_queue_arn(&format!("{case}-src")))
        };
        let role = given_role_stating(
            &router,
            &format!("{case}-role"),
            SERVICE_PRINCIPAL,
            (row.policy)(&source_arn, &target_arn),
        )
        .await;
        // A record on the source before the pipe is created, so "it consumed nothing" is a
        // statement about a source that had something to give.
        let marker = match (row.held_source, source_url.as_deref()) {
            (true, Some(url)) => {
                let marker = format!("{case}-record");
                send(&router, url, std::slice::from_ref(&marker)).await;
                Some(marker)
            }
            _ => None,
        };
        if !row.held_source {
            if let Some(url) = source_url.as_deref() {
                delete_queue(&router, case, url).await;
            }
        }
        let created = create_pipe(
            &router,
            case,
            &source_arn,
            &target_arn,
            &role,
            row.desired_state,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe answered {} {}",
            created.status, created.body
        );
        prepared.push(Prepared {
            row,
            source_arn,
            source_url,
            target_url,
            marker,
        });
    }

    for entry in &prepared {
        let case = entry.row.case;
        let reported = settled(&router, case, case).await;
        let state = reported.string("CurrentState");
        assert_eq!(
            state, entry.row.settles_in,
            "case {case:?}: expected the pipe to settle in {}, it reports {state} reason={:?}",
            entry.row.settles_in,
            reported.member("StateReason")
        );
        let reason = reported.string("StateReason");
        if entry.row.settles_in == CREATE_FAILED {
            reason_says(case, &reason, &entry.source_arn, entry.row.reason_mentions);
        } else {
            // A move that COULD be completed leaves the user-initiated reason its caller
            // drove it with.
            assert_eq!(
                reason, USER_INITIATED_STATE_REASON,
                "case {case:?}: expected the pipe that was brought up to report the \
                 user-initiated reason, got {reason:?}"
            );
        }
    }

    // What the pipe that was brought up DID: it moved its record.
    for entry in prepared.iter().filter(|entry| entry.row.delivers) {
        let case = entry.row.case;
        let marker = entry.marker.clone().unwrap_or_default();
        let delivered = collect_at_least(&router, &entry.target_url, 1, BUDGET).await;
        assert_eq!(
            delivered,
            vec![marker.clone()],
            "case {case:?}: expected the pipe that was brought up to deliver {marker:?}, got \
             {delivered:?}"
        );
    }

    // And what a pipe that was NEVER brought up did: nothing. The quiet window every
    // "nothing happened" is decided after is waited out once for the whole table.
    tokio::time::sleep(QUIET).await;
    for entry in prepared
        .iter()
        .filter(|entry| entry.row.settles_in == CREATE_FAILED)
    {
        let case = entry.row.case;
        publishes_nothing(&router, case, case).await;
        let held = counted(&router, &entry.target_url, &[VISIBLE_ATTRIBUTE]).await;
        assert_eq!(
            held, 0,
            "case {case:?}: expected the target of a pipe that never ran to hold nothing, it \
             holds {held}"
        );
        if let (Some(url), Some(marker)) = (entry.source_url.as_deref(), entry.marker.as_deref()) {
            let left = counted(&router, url, &[VISIBLE_ATTRIBUTE]).await;
            assert_eq!(
                left, 1,
                "case {case:?}: expected the record {marker:?} to be left on the source {}, \
                 which holds {left}",
                entry.source_arn
            );
        }
    }
}

/// What takes the pipe's source side away AFTER the pipe has been created healthy.
#[derive(Clone, Copy)]
enum Break {
    /// Its source queue is deleted, so its `Source` names nothing.
    SourceDeleted,
    /// The policies of the role it HOLDS are rewritten to withhold the source read.
    SourceReadWithdrawn,
    /// It is re-roled onto another role that may not take the source's records.
    AnotherRole,
}

/// One row of the later-move table: what is taken away, which call is then made, and the
/// terminal failure state that move has.
struct Move {
    case: &'static str,
    broken: Break,
    /// Whether the move is a `StartPipe`; otherwise it is an `UpdatePipe` asking for
    /// `RUNNING`.
    starts: bool,
    settles_in: &'static str,
    /// Text the settled `StateReason` has to carry beside the source ARN.
    reason_mentions: &'static [&'static str],
}

/// RULE: the same reading holds for every LATER lifecycle call that puts a pipe on its way to
/// running — a start and a reconfiguration each settle in their OWN terminal failure state
/// (`START_FAILED`, `UPDATE_FAILED`) with the `StateReason` saying why, and the pipe runs
/// nothing afterwards.
///
/// The rows differ along which side of the source went away (the resource, the permission of
/// the role in force, the role itself) and which call was in flight when it did.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_later_move_fails_when_the_pipe_can_no_longer_poll_its_source() {
    let moves = [
        Move {
            case: "start-after-the-source-was-deleted",
            broken: Break::SourceDeleted,
            starts: true,
            settles_in: START_FAILED,
            reason_mentions: &["does not exist"],
        },
        Move {
            case: "start-after-the-source-read-was-withdrawn",
            broken: Break::SourceReadWithdrawn,
            starts: true,
            settles_in: START_FAILED,
            reason_mentions: &["sqs:ReceiveMessage"],
        },
        Move {
            case: "update-onto-a-role-that-may-not-read-the-source",
            broken: Break::AnotherRole,
            starts: false,
            settles_in: UPDATE_FAILED,
            reason_mentions: &["sqs:ReceiveMessage"],
        },
        Move {
            case: "update-after-the-source-was-deleted",
            broken: Break::SourceDeleted,
            starts: false,
            settles_in: UPDATE_FAILED,
            reason_mentions: &["does not exist"],
        },
    ];

    for row in &moves {
        let case = row.case;
        let router = emulator();
        let (target_url, target_arn) = given_queue(&router, &format!("{case}-tgt")).await;
        let (source_url, source_arn) = given_queue(&router, &format!("{case}-src")).await;
        let role_name = format!("{case}-role");
        let role = given_role_stating(
            &router,
            &role_name,
            SERVICE_PRINCIPAL,
            every_call(&source_arn, &target_arn),
        )
        .await;
        let created = create_pipe(
            &router,
            case,
            &source_arn,
            &target_arn,
            &role,
            Some(STOPPED),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe answered {} {}",
            created.status, created.body
        );
        let resting = settled(&router, case, case).await.string("CurrentState");
        assert_eq!(
            resting, STOPPED,
            "case {case:?}: expected the healthy pipe to rest STOPPED before the move, it \
             reports {resting}"
        );
        let marker = format!("{case}-record");
        send(&router, &source_url, std::slice::from_ref(&marker)).await;

        // Take the source side away, then make the move.
        let mut role_for_the_move = role.clone();
        match row.broken {
            Break::SourceDeleted => delete_queue(&router, case, &source_url).await,
            Break::SourceReadWithdrawn => {
                // The same one path `tests/all/pipe_metrics.rs` narrows a live role by.
                withdraw_from_role(&router, case, &role_name, &["sqs:SendMessage"]).await;
            }
            Break::AnotherRole => {
                role_for_the_move = given_role_stating(
                    &router,
                    &format!("{case}-role-b"),
                    SERVICE_PRINCIPAL,
                    the_target_only(&target_arn),
                )
                .await;
            }
        }
        let answered = if row.starts {
            call(
                &router,
                "POST",
                &format!("/v1/pipes/{case}/start"),
                None,
                REGION,
            )
            .await
        } else {
            call(
                &router,
                "PUT",
                &format!("/v1/pipes/{case}"),
                Some(json!({ "RoleArn": role_for_the_move, "DesiredState": RUNNING })),
                REGION,
            )
            .await
        };
        assert_eq!(
            answered.status, 200,
            "case {case:?}: the move answered {} {}",
            answered.status, answered.body
        );

        let reported = settled(&router, case, case).await;
        let state = reported.string("CurrentState");
        assert_eq!(
            state, row.settles_in,
            "case {case:?}: expected the move to settle in {}, it reports {state} reason={:?}",
            row.settles_in,
            reported.member("StateReason")
        );
        reason_says(
            case,
            &reported.string("StateReason"),
            &source_arn,
            row.reason_mentions,
        );

        // And the pipe ran nothing: its target is empty, and the record it was sent is
        // still on the source wherever that source is still there.
        tokio::time::sleep(QUIET).await;
        publishes_nothing(&router, case, case).await;
        let delivered = counted(&router, &target_url, &[VISIBLE_ATTRIBUTE]).await;
        assert_eq!(
            delivered, 0,
            "case {case:?}: expected the target of a pipe that never ran to hold nothing, it \
             holds {delivered}"
        );
        if !matches!(row.broken, Break::SourceDeleted) {
            let left = counted(&router, &source_url, &[VISIBLE_ATTRIBUTE]).await;
            assert_eq!(
                left, 1,
                "case {case:?}: expected the record {marker:?} to be left on the source \
                 {source_arn}, which holds {left}"
            );
        }
    }
}
