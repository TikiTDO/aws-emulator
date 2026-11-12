// SPDX-License-Identifier: CC0-1.0

//! WHICH resources a pipe's execution role lets its stages reach, and what happens to the
//! records when it does not — end to end over the wire, with IAM and Amazon SQS mounted
//! beside pipes on one engine (the harness of `tests/all/pipe_metrics.rs` and
//! `tests/all/pipe_data_plane.rs`, reused rather than copied).
//!
//! `tests/all/pipe_metrics.rs` asserts WHICH STAGE fails when the role bars its call and
//! what the pipe then publishes about itself. The three rules here are the other half of
//! the same rule, and none is covered there:
//!
//! 1. The decision is about the (action, RESOURCE) pair the call is made on, not about the
//!    action alone: a grant is read with IAM's wildcards, a grant that names another queue
//!    does not reach this one, a deny on the target resource refuses the delivery, and a
//!    call the role may not make leaves what it would have consumed or deleted on the
//!    source. The rows differ along what the rule generalises over — which stage's call the
//!    policy covers (receive, delete, send), and how it names the action and the resource
//!    (exactly, by wildcard, on another queue, by an explicit deny).
//! 2. The role is read from the pipe's record when each stage makes its call, so
//!    `UpdatePipe` switches the identity a RUNNING pipe's deliveries are authorized by:
//!    delivery after the update is authorized by the NEW role, a pipe left on a role that
//!    may not reach the target it was just re-pointed at delivers nothing, and the record
//!    it could not move is still delivered once a role that may reach the target is
//!    supplied. It is read per CALL and not once per batch, so a role switched while a
//!    batch is in flight is the one that authorizes the calls that batch has not made yet
//!    — its delivery and the deletion of what it delivered.
//! 3. The REST of the configuration is read from that same record: records polled under a
//!    configuration `UpdatePipe` has since replaced are not delivered under it, but under
//!    the configuration the pipe carries when the batch is decided.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_pipes_pipes::state::execution_role::SERVICE_PRINCIPAL;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pipes_pipes::state::pipe_state::RUNNING;

use crate::pipe_data_plane::{
    collect_at_least, counted, given_queue, receive_now, send, sqs, wait_for_counted, LOOK_AGAIN,
    QUIET,
};
use crate::pipe_lifecycle::{call, Reply, REGION};
use crate::pipe_metrics::{emulator, given_role_stating};

/// `TargetParameters.InputTemplate`, so a delivered message carries the record's own body
/// and a row can attribute it: `<$.body>` selects the `body` of the record envelope.
const BODY_TEMPLATE: &str = "<$.body>";

/// How long a delivery the pipe has to make is given before the row fails. Well above the
/// poll interval and the state-settle span, so a row fails only when the pipe did not
/// deliver at all.
const BUDGET: Duration = Duration::from_secs(25);

/// The visibility timeout the re-role source is set to, so a record the pipe took and then
/// could not deliver comes back within this suite's budget instead of after the Amazon SQS
/// default of 30 seconds.
const SHORT_VISIBILITY: &str = "1";

/// Give the queue at `url` a visibility timeout of `seconds`: "the length of time, in
/// seconds, that a message received from a queue will be invisible to other receiving
/// components" (`SetQueueAttributes$Attributes`).
///
/// `pub(crate)` because `tests/all/pipe_filter_reinstall.rs` configures the span of its
/// source's checkout the same way, through this one path.
pub(crate) async fn set_visibility(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    seconds: &str,
) {
    let (status, body) = sqs(
        router,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": { "VisibilityTimeout": seconds },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetQueueAttributes answered {status} {body}"
    );
}

/// A role trusting EventBridge Pipes and carrying `statements` as its one inline policy;
/// answered with the ARN IAM persisted.
///
/// The role is written through the ONE path `tests/all/pipe_metrics.rs` writes one by;
/// every row here differs only in the statements, because what each asserts is which
/// RESOURCE a statement names.
async fn given_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    statements: Value,
) -> String {
    let arn = given_role_stating(router, name, SERVICE_PRINCIPAL, statements).await;
    assert!(
        !arn.is_empty(),
        "case {case:?}: CreateRole answered an empty Arn for {name}"
    );
    arn
}

/// The attributes that together say how many records a queue HOLDS: the ones waiting on it
/// plus the ones a consumer has taken and not deleted ("messages that are in flight",
/// `ApproximateNumberOfMessagesNotVisible`) — which is what a record the pipe could not
/// move is one of.
const HELD_ATTRIBUTES: [&str; 2] = ["ApproximateNumberOfMessages", IN_FLIGHT_ATTRIBUTE];

/// The attribute that says how many records a consumer has TAKEN from a queue and not
/// deleted: "messages that are in flight" (`ApproximateNumberOfMessagesNotVisible`) —
/// which is what the record of a batch the pipe is still working on is.
///
/// `pub(crate)` because `tests/all/pipe_desired_state.rs` asks the same question of a
/// queue — has the poller taken this record yet? — and reads it through this one name.
pub(crate) const IN_FLIGHT_ATTRIBUTE: &str = "ApproximateNumberOfMessagesNotVisible";

/// How many records the queue at `url` still holds, through this crate's ONE queue-attribute
/// read.
async fn held(router: &MultiServiceRouter, url: &str) -> i64 {
    counted(router, url, &HELD_ATTRIBUTES).await
}

/// Wait until the queue at `url` holds `want` records, answering what it held last.
async fn wait_for_held(router: &MultiServiceRouter, url: &str, want: i64, budget: Duration) -> i64 {
    wait_for_counted(router, url, &HELD_ATTRIBUTES, want, budget).await
}

/// `CreatePipe`: an SQS source, an SQS target, the role the row names, and the template
/// that makes a delivered message carry the record's body.
async fn create_pipe(
    router: &MultiServiceRouter,
    name: &str,
    source_arn: &str,
    target_arn: &str,
    role_arn: &str,
) -> Reply {
    create_batching_pipe(router, name, source_arn, target_arn, role_arn, None).await
}

/// That same `CreatePipe`, under the source batching parameters a row names: `BatchSize`
/// and `MaximumBatchingWindowInSeconds`, which together are how long a poll that has not
/// filled its batch keeps collecting.
async fn create_batching_pipe(
    router: &MultiServiceRouter,
    name: &str,
    source_arn: &str,
    target_arn: &str,
    role_arn: &str,
    batching: Option<(i64, Duration)>,
) -> Reply {
    let mut body = json!({
        "Source": source_arn,
        "Target": target_arn,
        "RoleArn": role_arn,
        "DesiredState": "RUNNING",
        "TargetParameters": { "InputTemplate": BODY_TEMPLATE },
    });
    if let Some((batch_size, window)) = batching {
        body["SourceParameters"] = json!({
            "SqsQueueParameters": {
                "BatchSize": batch_size,
                "MaximumBatchingWindowInSeconds": window.as_secs(),
            },
        });
    }
    call(
        router,
        "POST",
        &format!("/v1/pipes/{name}"),
        Some(body),
        REGION,
    )
    .await
}

/// `UpdatePipe` with the `RoleArn` every update carries and, when the caller supplies one,
/// a new `Target`.
async fn update_pipe(
    router: &MultiServiceRouter,
    name: &str,
    role_arn: &str,
    target_arn: Option<&str>,
) -> Reply {
    update_templated_pipe(router, name, role_arn, target_arn, BODY_TEMPLATE).await
}

/// That same `UpdatePipe`, putting the `TargetParameters.InputTemplate` the caller names
/// in force: what the TARGET is handed instead of the event.
async fn update_templated_pipe(
    router: &MultiServiceRouter,
    name: &str,
    role_arn: &str,
    target_arn: Option<&str>,
    template: &str,
) -> Reply {
    let mut body = json!({
        "RoleArn": role_arn,
        "TargetParameters": { "InputTemplate": template },
    });
    if let Some(target_arn) = target_arn {
        body["Target"] = json!(target_arn);
    }
    call(
        router,
        "PUT",
        &format!("/v1/pipes/{name}"),
        Some(body),
        REGION,
    )
    .await
}

async fn describe_pipe(router: &MultiServiceRouter, name: &str) -> Reply {
    call(router, "GET", &format!("/v1/pipes/{name}"), None, REGION).await
}

/// The queues one row's policy statements are written against.
struct Resources {
    source: String,
    target: String,
    /// A queue the pipe does NOT deliver to, for the row whose grant names the wrong one.
    other: String,
}

/// One row of the resource-by-resource table.
struct Row {
    case: &'static str,
    /// The statements of the role's inline policy, written against the row's own queues.
    policy: fn(&Resources) -> Value,
    /// How many messages the target must receive.
    delivered: usize,
    /// Whether the row's record is delivered AGAIN once its invisibility on the source
    /// lapses, because the role may not delete it: the row then asserts at least
    /// [`Row::delivered`] rather than exactly that many, since how many copies have
    /// arrived by the time it looks depends only on how long the run took.
    redelivers: bool,
    /// How many records the SOURCE must still hold afterwards: a call the role may not
    /// make does not happen, so nothing it would have consumed or deleted leaves the queue.
    left_on_source: i64,
}

/// Every Amazon SQS call a pipe with an SQS source and an SQS target makes, allowed on
/// exactly the resource each is made on.
fn exact_grant(arns: &Resources) -> Value {
    json!([
        {
            "Effect": "Allow",
            "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage"],
            "Resource": arns.source,
        },
        { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": arns.target },
    ])
}

/// Only the calls the SOURCE poll makes: the pipe may take the source's records and
/// delete them, and may deliver to nothing.
fn source_only_grant(arns: &Resources) -> Value {
    json!([
        {
            "Effect": "Allow",
            "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage"],
            "Resource": arns.source,
        },
    ])
}

/// Every call of [`exact_grant`] but the source DELETE: the pipe may take a record and
/// deliver it, and may not delete what it delivered.
fn undeletable_grant(arns: &Resources) -> Value {
    json!([
        { "Effect": "Allow", "Action": "sqs:ReceiveMessage", "Resource": arns.source },
        { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": arns.target },
    ])
}

/// RULE: each stage's call is decided for the (action, resource) pair it is made on, read
/// with IAM's own wildcards and its deny-wins order; a call the policies in force on the
/// pipe's role do not allow is not made, so what it would have consumed or deleted stays
/// on the source.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stage_reaches_only_the_resources_its_role_allows_it_on() {
    let rows = [
        Row {
            case: "every call allowed on exactly the resource it is made on",
            policy: exact_grant,
            delivered: 1,
            redelivers: false,
            left_on_source: 0,
        },
        Row {
            case: "the send allowed by an action wildcard and the queue by a resource one",
            policy: |arns| {
                json!([
                    {
                        "Effect": "Allow",
                        "Action": "sqs:*Message",
                        "Resource": arns.source,
                    },
                    {
                        "Effect": "Allow",
                        "Action": "sqs:Send*",
                        "Resource": format!("{}*", arns.target),
                    },
                ])
            },
            delivered: 1,
            redelivers: false,
            left_on_source: 0,
        },
        Row {
            case: "the send allowed on another queue only",
            policy: |arns| {
                json!([
                    {
                        "Effect": "Allow",
                        "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage"],
                        "Resource": arns.source,
                    },
                    { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": arns.other },
                ])
            },
            delivered: 0,
            redelivers: false,
            left_on_source: 1,
        },
        Row {
            case: "the send allowed on every queue and then denied on the target",
            policy: |arns| {
                json!([
                    { "Effect": "Allow", "Action": "sqs:*", "Resource": "*" },
                    {
                        "Effect": "Deny",
                        "Action": "sqs:SendMessage",
                        "Resource": arns.target,
                    },
                ])
            },
            delivered: 0,
            redelivers: false,
            left_on_source: 1,
        },
        Row {
            case: "the source receive withheld, with the delivery allowed",
            policy: |arns| {
                json!([
                    { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": arns.target },
                ])
            },
            delivered: 0,
            redelivers: false,
            left_on_source: 1,
        },
        Row {
            case: "the source delete withheld, with the delivery allowed",
            policy: |arns| {
                json!([
                    {
                        "Effect": "Allow",
                        "Action": "sqs:ReceiveMessage",
                        "Resource": arns.source,
                    },
                    { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": arns.target },
                ])
            },
            // Delivered, and then left on the source: the at-least-once a pipe that may
            // not delete its records produces — so the record is delivered AGAIN each time
            // its invisibility lapses, and the row asserts at least one delivery.
            delivered: 1,
            redelivers: true,
            left_on_source: 1,
        },
    ];

    let router = emulator();
    // Every row's pipe is created first and every source then written to, so the rows'
    // settle spans and delivery waits overlap instead of being paid one after another.
    let mut prepared: Vec<(&Row, Resources, String, String, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let (source_url, source_arn) = given_queue(&router, &format!("role-source-{index}")).await;
        let (target_url, target_arn) = given_queue(&router, &format!("role-target-{index}")).await;
        let (_, other_arn) = given_queue(&router, &format!("role-other-{index}")).await;
        let arns = Resources {
            source: source_arn.clone(),
            target: target_arn.clone(),
            other: other_arn,
        };
        let role = given_role(&router, case, &format!("role-{index}"), (row.policy)(&arns)).await;
        let name = format!("role-pipe-{index}");
        let created = create_pipe(&router, &name, &source_arn, &target_arn, &role).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe answered {} {}",
            created.status, created.body
        );
        let marker = format!("record-{index}");
        send(&router, &source_url, std::slice::from_ref(&marker)).await;
        prepared.push((row, arns, source_url, target_url, marker));
    }

    // One instant every row's "nothing was delivered" is decided at, so the quiet window
    // is waited out once for the whole table.
    let quiet_until = Instant::now() + QUIET + QUIET;
    for (row, arns, source_url, target_url, marker) in &prepared {
        let case = row.case;
        if row.delivered == 0 {
            let remaining = quiet_until.saturating_duration_since(Instant::now());
            if !remaining.is_zero() {
                tokio::time::sleep(remaining).await;
            }
        }
        let got = collect_at_least(&router, target_url, row.delivered, BUDGET).await;
        if row.redelivers {
            assert!(
                got.len() >= row.delivered,
                "case {case:?}: expected at least {} message(s) on the target {}, got {got:?}",
                row.delivered,
                arns.target
            );
        } else {
            assert_eq!(
                got.len(),
                row.delivered,
                "case {case:?}: expected {} message(s) on the target {}, got {got:?}",
                row.delivered,
                arns.target
            );
        }
        for body in &got {
            assert_eq!(
                body, marker,
                "case {case:?}: the target {} received {body:?}, not the record the source \
                 was sent",
                arns.target
            );
        }
        let left = wait_for_held(&router, source_url, row.left_on_source, BUDGET).await;
        assert_eq!(
            left, row.left_on_source,
            "case {case:?}: expected the source {} to hold {} record(s), got {left}",
            arns.source, row.left_on_source
        );
    }
}

/// RULE: the execution role is read from the pipe's record on every cycle, so `UpdatePipe`
/// switches the identity a RUNNING pipe's deliveries are authorized by — a role that may
/// reach the new target delivers, a role that may not delivers nothing, and the records
/// left behind are moved as soon as a role that may reach it is in force.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_pipe_switches_the_role_a_running_pipe_is_authorized_by() {
    let case = "a re-roled pipe";
    let router = emulator();
    let (source_url, source_arn) = given_queue(&router, "rerole-source").await;
    set_visibility(&router, case, &source_url, SHORT_VISIBILITY).await;
    let (first_url, first_arn) = given_queue(&router, "rerole-target-a").await;
    let (second_url, second_arn) = given_queue(&router, "rerole-target-b").await;

    // Two roles, each able to send to ONE of the targets, so which queue a record reaches
    // is decided by which role the pipe is acting as.
    let source_access = json!({
        "Effect": "Allow",
        "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage"],
        "Resource": source_arn,
    });
    let first_role = given_role(
        &router,
        case,
        "rerole-a",
        json!([
            source_access,
            { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": first_arn },
        ]),
    )
    .await;
    let second_role = given_role(
        &router,
        case,
        "rerole-b",
        json!([
            source_access,
            { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": second_arn },
        ]),
    )
    .await;

    let name = "rerole-pipe";
    let created = create_pipe(&router, name, &source_arn, &first_arn, &first_role).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreatePipe answered {} {}",
        created.status, created.body
    );

    // The pipe as created: the first role, delivering into the first target.
    send(&router, &source_url, &["before-update".to_string()]).await;
    let got = collect_at_least(&router, &first_url, 1, BUDGET).await;
    assert_eq!(
        got,
        vec!["before-update".to_string()],
        "case {case:?}: expected the record on the first target {first_arn} under the \
         first role, got {got:?}"
    );

    // Re-roled AND re-pointed: the identity and the target of a RUNNING pipe both move.
    let updated = update_pipe(&router, name, &second_role, Some(&second_arn)).await;
    assert_eq!(
        updated.status, 200,
        "case {case:?}: UpdatePipe answered {} {}",
        updated.status, updated.body
    );
    let described = describe_pipe(&router, name).await;
    assert_eq!(
        described.string("RoleArn"),
        second_role,
        "case {case:?}: DescribePipe reports the role {} after the update, not {second_role}",
        described.string("RoleArn")
    );
    assert_eq!(
        described.string("Target"),
        second_arn,
        "case {case:?}: DescribePipe reports the target {} after the update, not {second_arn}",
        described.string("Target")
    );
    send(&router, &source_url, &["after-update".to_string()]).await;
    let got = collect_at_least(&router, &second_url, 1, BUDGET).await;
    assert_eq!(
        got,
        vec!["after-update".to_string()],
        "case {case:?}: expected the record on the second target {second_arn} under the \
         new role, got {got:?}"
    );

    // The negative control: the pipe is left on the FIRST role while its target stays the
    // second queue, which that role may not send to — so nothing is delivered and the
    // record stays on the source.
    let re_roled = update_pipe(&router, name, &first_role, None).await;
    assert_eq!(
        re_roled.status, 200,
        "case {case:?}: UpdatePipe answered {} {}",
        re_roled.status, re_roled.body
    );
    send(&router, &source_url, &["under-the-old-role".to_string()]).await;
    tokio::time::sleep(QUIET + QUIET).await;
    let got = receive_now(&router, &second_url).await;
    assert!(
        got.is_empty(),
        "case {case:?}: the first role may not send to {second_arn}, so nothing may be \
         delivered there, got {got:?}"
    );
    let left = held(&router, &source_url).await;
    assert_eq!(
        left, 1,
        "case {case:?}: expected the undelivered record to stay on the source {source_arn}, \
         which held {left}"
    );

    // And back: the record the old role could not move is delivered as soon as the pipe
    // acts as a role that may reach the target, with no record lost in between.
    let re_roled = update_pipe(&router, name, &second_role, None).await;
    assert_eq!(
        re_roled.status, 200,
        "case {case:?}: UpdatePipe answered {} {}",
        re_roled.status, re_roled.body
    );
    let got = collect_at_least(&router, &second_url, 1, BUDGET).await;
    assert_eq!(
        got,
        vec!["under-the-old-role".to_string()],
        "case {case:?}: expected the record the old role could not move on {second_arn} \
         once the new role is in force, got {got:?}"
    );
}

/// How long a poll that has not filled its batch keeps collecting:
/// `MaximumBatchingWindowInSeconds`, "the maximum length of a time to wait for events"
/// (`PipeSourceSqsQueueParameters`). The rows below reconfigure the pipe DURING this span,
/// which is what puts the change on a batch the pipe has already taken.
const IN_FLIGHT_WINDOW: Duration = Duration::from_secs(2);

/// The `BatchSize` those rows poll under: more than the one record each sends, so the
/// poll waits its whole window out instead of returning as soon as the batch is full.
const IN_FLIGHT_BATCH_SIZE: i64 = 10;

/// Wait until the pipe `name` reports the `CurrentState` `want`, answering what it
/// reported last: only a pipe the clock has moved into `RUNNING` polls its source at all.
///
/// `pub(crate)` because `tests/all/pipe_desired_state.rs` waits for a pipe to settle on the
/// state its own `UpdatePipe` asked for, and waits through this one read.
pub(crate) async fn wait_for_state(
    router: &MultiServiceRouter,
    name: &str,
    want: &str,
    budget: Duration,
) -> String {
    let deadline = Instant::now() + budget;
    loop {
        let reported = describe_pipe(router, name).await.string("CurrentState");
        if reported == want || Instant::now() >= deadline {
            return reported;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One row of the mid-batch switch table: the role the pipe polls its record under, the
/// role `UpdatePipe` puts in force while that record is in flight, and what the two
/// queues hold once the batch has landed.
struct Switch {
    case: &'static str,
    /// The statements of the role the pipe is CREATED with — the one its poll is made as.
    before: fn(&Resources) -> Value,
    /// The statements of the role put in force while the batch is in flight, which is the
    /// one every call that batch has not made yet is authorized by.
    after: fn(&Resources) -> Value,
    /// How many messages the target holds once the batch has landed.
    delivered: usize,
    /// How many records the source still holds: one a delivery was withheld for, and one
    /// a delete was withheld for, both stay on the queue.
    left_on_source: i64,
}

/// RULE: a stage's call is authorized by the execution role the pipe's record carries AT
/// THE MOMENT THAT CALL IS MADE, never by one read earlier, so a role `UpdatePipe` put in
/// force while a batch was in flight decides every call of that batch still to come —
/// nothing is delivered or deleted as an identity the pipe no longer carries, and a call
/// the new role allows is made even though the role the batch began under did not.
///
/// The rows differ along what the rule generalises over: which stage's call the switch
/// changes the answer for (the target delivery, the source delete) and which way it
/// changes it (withdrawn, granted).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_role_switched_mid_batch_authorizes_the_calls_that_batch_has_not_made() {
    let rows = [
        Switch {
            case: "the delivery withdrawn while the batch is in flight",
            before: exact_grant,
            after: source_only_grant,
            delivered: 0,
            left_on_source: 1,
        },
        Switch {
            case: "the delivery granted while the batch is in flight",
            before: source_only_grant,
            after: exact_grant,
            delivered: 1,
            left_on_source: 0,
        },
        Switch {
            case: "the source delete withdrawn while the batch is in flight",
            before: exact_grant,
            after: undeletable_grant,
            delivered: 1,
            left_on_source: 1,
        },
    ];

    let router = emulator();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let (source_url, source_arn) = given_queue(&router, &format!("switch-source-{index}")).await;
        let (target_url, target_arn) = given_queue(&router, &format!("switch-target-{index}")).await;
        let (_, other_arn) = given_queue(&router, &format!("switch-other-{index}")).await;
        let arns = Resources {
            source: source_arn.clone(),
            target: target_arn.clone(),
            other: other_arn,
        };
        let before = given_role(
            &router,
            case,
            &format!("switch-before-{index}"),
            (row.before)(&arns),
        )
        .await;
        let after = given_role(
            &router,
            case,
            &format!("switch-after-{index}"),
            (row.after)(&arns),
        )
        .await;

        let name = format!("switch-pipe-{index}");
        let created = create_batching_pipe(
            &router,
            &name,
            &source_arn,
            &target_arn,
            &before,
            Some((IN_FLIGHT_BATCH_SIZE, IN_FLIGHT_WINDOW)),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe answered {} {}",
            created.status, created.body
        );
        let reported = wait_for_state(&router, &name, RUNNING, BUDGET).await;
        assert_eq!(
            reported, RUNNING,
            "case {case:?}: the pipe reports {reported}, so it polls nothing"
        );

        let marker = format!("in-flight-{index}");
        send(&router, &source_url, std::slice::from_ref(&marker)).await;
        // The batch is IN FLIGHT once the poller has taken the record and not deleted it:
        // the pipe is inside its batching window with its delivery still to come, which
        // is the instant the role is switched at.
        let taken = wait_for_counted(&router, &source_url, &[IN_FLIGHT_ATTRIBUTE], 1, BUDGET).await;
        assert_eq!(
            taken, 1,
            "case {case:?}: expected the poller to have taken the record of {source_arn}, \
             in flight {taken}"
        );
        let switched = update_pipe(&router, &name, &after, None).await;
        assert_eq!(
            switched.status, 200,
            "case {case:?}: UpdatePipe answered {} {}",
            switched.status, switched.body
        );

        // The batching window has to close before the delivery is attempted at all, so a
        // row that must see nothing delivered looks only once it has, and then waits the
        // quiet window out on top.
        let got = if row.delivered == 0 {
            tokio::time::sleep(IN_FLIGHT_WINDOW + QUIET).await;
            receive_now(&router, &target_url).await
        } else {
            collect_at_least(&router, &target_url, row.delivered, BUDGET).await
        };
        assert_eq!(
            got.len(),
            row.delivered,
            "case {case:?}: expected {} message(s) on the target {target_arn}, got {got:?}",
            row.delivered
        );
        for body in &got {
            assert_eq!(
                body, &marker,
                "case {case:?}: the target {target_arn} received {body:?}, not the record \
                 the source was sent"
            );
        }
        let left = wait_for_held(&router, &source_url, row.left_on_source, BUDGET).await;
        assert_eq!(
            left, row.left_on_source,
            "case {case:?}: expected the source {source_arn} to hold {} record(s), got {left}",
            row.left_on_source
        );
    }
}

/// The visibility timeout the re-configured source is set to, as the seconds a
/// `VisibilityTimeout` attribute carries. It is longer than [`IN_FLIGHT_WINDOW`] by more
/// than a delivery takes, so a record a batch is still collecting cannot be taken twice
/// by that batch, nor come back before the batch that delivered it has deleted it — and
/// short enough that a record the pipe left behind comes back inside this suite's budget,
/// instead of after the Amazon SQS default of 30 seconds.
const REDELIVERY_VISIBILITY: &str = "4";

/// A `TargetParameters.InputTemplate` that is NOT [`BODY_TEMPLATE`]: it wraps the record's
/// body in a JSON object, so which template was in force when the delivery was made is
/// read off the delivered message.
const WRAPPED_TEMPLATE: &str = r#"{"moved":"<$.body>"}"#;

/// What [`WRAPPED_TEMPLATE`] renders for a record whose body is `body`.
fn wrapped(body: &str) -> String {
    format!(r#"{{"moved":"{body}"}}"#)
}

/// One row of the reconfigured-mid-batch table: which member of the pipe's configuration
/// `UpdatePipe` moves while a batch is in flight, and what the delivery then looks like.
struct Reconfigured {
    case: &'static str,
    /// Whether the update re-points the pipe at the SECOND queue; a row that does not
    /// leaves the first as the target.
    retargets: bool,
    /// The `TargetParameters.InputTemplate` the update puts in force.
    template: &'static str,
    /// What the delivered message must carry, given the record's body.
    delivered: fn(&str) -> String,
}

/// RULE: the configuration a batch is moved under is the pipe's own, read when the batch is
/// decided — so records polled under a configuration `UpdatePipe` has since replaced are not
/// delivered under it: nothing reaches the target the pipe has left, nothing is delivered
/// through a template it no longer carries, and the records leave the source once the
/// configuration it does carry has moved them.
///
/// The rows differ along which member of that configuration moved: the `Target` the
/// delivery is made to, and the `InputTemplate` it is made with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_batch_in_flight_is_not_delivered_under_a_configuration_the_pipe_has_left() {
    let rows = [
        Reconfigured {
            case: "the target re-pointed while the batch is in flight",
            retargets: true,
            template: BODY_TEMPLATE,
            delivered: |body| body.to_string(),
        },
        Reconfigured {
            case: "the input template replaced while the batch is in flight",
            retargets: false,
            template: WRAPPED_TEMPLATE,
            delivered: wrapped,
        },
    ];

    let router = emulator();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let (source_url, source_arn) =
            given_queue(&router, &format!("moved-source-{index}")).await;
        let (first_url, first_arn) = given_queue(&router, &format!("moved-first-{index}")).await;
        let (second_url, second_arn) =
            given_queue(&router, &format!("moved-second-{index}")).await;
        set_visibility(&router, case, &source_url, REDELIVERY_VISIBILITY).await;
        // ONE role, which may deliver to either queue: where the record lands is then
        // decided by the pipe's own configuration and by nothing about its permissions.
        let role = given_role(
            &router,
            case,
            &format!("moved-role-{index}"),
            json!([
                {
                    "Effect": "Allow",
                    "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage"],
                    "Resource": source_arn,
                },
                { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": first_arn },
                { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": second_arn },
            ]),
        )
        .await;

        let name = format!("moved-pipe-{index}");
        let created = create_batching_pipe(
            &router,
            &name,
            &source_arn,
            &first_arn,
            &role,
            Some((IN_FLIGHT_BATCH_SIZE, IN_FLIGHT_WINDOW)),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe answered {} {}",
            created.status, created.body
        );
        let reported = wait_for_state(&router, &name, RUNNING, BUDGET).await;
        assert_eq!(
            reported, RUNNING,
            "case {case:?}: the pipe reports {reported}, so it polls nothing"
        );

        let marker = format!("moved-{index}");
        send(&router, &source_url, std::slice::from_ref(&marker)).await;
        let taken = wait_for_counted(&router, &source_url, &[IN_FLIGHT_ATTRIBUTE], 1, BUDGET).await;
        assert_eq!(
            taken, 1,
            "case {case:?}: expected the poller to have taken the record of {source_arn}, \
             in flight {taken}"
        );
        let moved = update_templated_pipe(
            &router,
            &name,
            &role,
            row.retargets.then_some(second_arn.as_str()),
            row.template,
        )
        .await;
        assert_eq!(
            moved.status, 200,
            "case {case:?}: UpdatePipe answered {} {}",
            moved.status, moved.body
        );

        // The record is delivered once the configuration the pipe now carries has moved
        // it: the batch it was polled into is decided by the configuration the pipe carries
        // then, and not by the one it was polled under.
        let (landed_url, landed_arn) = if row.retargets {
            (&second_url, &second_arn)
        } else {
            (&first_url, &first_arn)
        };
        let want = (row.delivered)(&marker);
        let got = collect_at_least(&router, landed_url, 1, BUDGET).await;
        assert_eq!(
            got,
            vec![want.clone()],
            "case {case:?}: expected {want:?} on {landed_arn} under the configuration the \
             pipe carries now, got {got:?}"
        );
        if row.retargets {
            let stale = receive_now(&router, &first_url).await;
            assert!(
                stale.is_empty(),
                "case {case:?}: the pipe was re-pointed away from {first_arn} before the \
                 batch was delivered, so nothing may reach it, got {stale:?}"
            );
        }
        let left = wait_for_held(&router, &source_url, 0, BUDGET).await;
        assert_eq!(
            left, 0,
            "case {case:?}: expected the delivered record to leave the source {source_arn}, \
             which held {left}"
        );
    }
}
