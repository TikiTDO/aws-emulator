// SPDX-License-Identifier: CC0-1.0

//! An invocation that did NOT reach its target is REPORTED: end to end over the wire
//! through the real `MultiServiceRouter`, with Amazon SQS and IAM mounted beside
//! EventBridge Scheduler on ONE engine — so a failed invocation is asserted where a
//! client observes it, in the messages the schedule's dead-letter queue holds.
//!
//! RULE: an occurrence whose invocation does not reach its target — the target's own
//! API refuses the call, or the permissions of `Target.RoleArn`, which the invocation
//! is made under, withhold it — is delivered to the queue `Target.DeadLetterConfig.Arn`
//! names, carrying the payload the target was to be handed and, beside it, the detail
//! of the failure that ended the invocation. An invocation the target accepted is not
//! reported there, and neither is one whose execution role may not write to that
//! queue.
//!
//! The rows span the class rather than one recorded case: they differ along the KIND
//! of failure (the target does not exist, the target refuses the request, the
//! permissions withhold the call), along whether the invocation reaches its target at
//! all, and along whether the execution role may write to the dead-letter queue. What
//! the retry policy admits before a failure becomes the invocation's last, over the
//! whole range of both its members, is asserted without a wait in
//! `src/state/dead_letter.rs`'s own tests.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_scheduler_awschronosservice::state::invocation;
use smithy_server_runtime::MultiServiceRouter;

use crate::schedule_invocation::{
    at_expression_body, emulator, given_execution_role_for, given_queue, peek, scheduler,
    seconds_from_now, sqs, sqs_response, POLL_INTERVAL, REGION,
};
use crate::wire_harness::account_id;

/// How long a row waits for the message it expects. The occurrence every row states
/// is seconds ahead; the rest is room for the clock's tick, the refusal and the
/// dead-letter delivery.
const PATIENCE: Duration = Duration::from_secs(80);

/// How long a row watches a queue it expects NOTHING to reach: well past the
/// occurrence its own expression names, so an invocation this emulator would have
/// performed, and a report it would have delivered, have both had their chance.
const QUIET_WINDOW: Duration = Duration::from_secs(15);

/// The error code AWS refuses an unauthorized call with, which is what a report of a
/// call the invocation's own permissions withheld carries.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// What the row's schedule points its target at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetQueue {
    /// A standard queue of this account, which takes the invocation.
    Standard,
    /// A queue of this account that was never created, which its own API refuses a
    /// message to.
    Absent,
    /// A FIFO queue, which refuses a message carrying no message group — the row's
    /// target states none.
    Fifo,
}

/// What the row expects to observe once the occurrence has passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The target was invoked, and nothing was reported to the dead-letter queue.
    Delivered,
    /// The failure was reported to the dead-letter queue. `Some(code)` where the code
    /// is the service's own for that refusal; `None` where the row discovers it from
    /// the target's own API, which is what a report of it must carry.
    DeadLettered(Option<&'static str>),
    /// Neither queue was reached at all.
    Nothing,
}

/// One row: the schedule written, what its execution role may write to, and what is
/// observed.
struct Row {
    case: &'static str,
    target: TargetQueue,
    /// Whether the role's permissions allow the invocation's own call.
    may_invoke: bool,
    /// Whether they allow the dead-letter message's delivery.
    may_dead_letter: bool,
    /// How many retries the schedule's `RetryPolicy` admits, or `None` for a schedule
    /// stating no policy at all.
    retries: Option<i32>,
    expected: Expected,
}

const ROWS: &[Row] = &[
    Row {
        case: "a target queue that does not exist",
        target: TargetQueue::Absent,
        may_invoke: true,
        may_dead_letter: true,
        // A refusal of the request itself is the invocation's last attempt whatever
        // the policy would still have admitted, so no retry is made of it.
        retries: Some(2),
        expected: Expected::DeadLettered(None),
    },
    Row {
        case: "a FIFO target the schedule names no message group for",
        target: TargetQueue::Fifo,
        may_invoke: true,
        may_dead_letter: true,
        retries: None,
        expected: Expected::DeadLettered(None),
    },
    Row {
        case: "an execution role that may not write to the target queue",
        target: TargetQueue::Standard,
        may_invoke: false,
        may_dead_letter: true,
        retries: None,
        expected: Expected::DeadLettered(Some(ACCESS_DENIED)),
    },
    Row {
        case: "an invocation the target accepts",
        target: TargetQueue::Standard,
        may_invoke: true,
        may_dead_letter: true,
        retries: Some(0),
        expected: Expected::Delivered,
    },
    Row {
        case: "an execution role that may write to neither queue",
        target: TargetQueue::Standard,
        may_invoke: false,
        may_dead_letter: false,
        retries: None,
        expected: Expected::Nothing,
    },
];

/// Every row: a failed invocation is reported to the schedule's dead-letter queue
/// with the failure's own detail, and nothing else is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_invocation_is_reported_to_the_dead_letter_queue() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&emulator, case, &format!("dlq-{index}")).await;
        let (target_url, target_arn) = given_target(&emulator, case, index, row.target).await;

        let mut granted: Vec<&str> = Vec::new();
        if row.may_invoke {
            granted.push(&target_arn);
        }
        if row.may_dead_letter {
            granted.push(&dead_letter_arn);
        }
        let role =
            given_execution_role_for(&emulator, case, &format!("role-{index}"), &granted).await;

        let marker = format!("{{\"marker\":\"{index}\"}}");
        let occurrence = seconds_from_now(4);
        let mut target = json!({
            "Arn": target_arn,
            "RoleArn": role,
            "Input": marker,
            "DeadLetterConfig": { "Arn": dead_letter_arn },
        });
        if let Some(retries) = row.retries {
            target["RetryPolicy"] = json!({ "MaximumRetryAttempts": retries });
        }
        let name = format!("schedule-{index}");
        let (status, created) = scheduler(
            &emulator,
            "POST",
            &format!("/schedules/{name}"),
            Some(json!({
                "ScheduleExpression": format!("at({})", at_expression_body(occurrence)),
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": target,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateSchedule answered {status} {created}"
        );
        let schedule_arn = created["ScheduleArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ScheduleArn in {created}"))
            .to_string();

        match row.expected {
            Expected::Delivered => {
                let delivered = messages_once(&emulator, &target_url, 1, PATIENCE).await;
                let body = delivered
                    .first()
                    .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
                    .unwrap_or_else(|| panic!("case {case:?}: the target was never invoked"));
                assert_eq!(
                    body, marker,
                    "case {case:?}: expected the schedule's own Input {marker:?}, got {body:?}"
                );
                stays_empty(&emulator, case, &dead_letter_url, "the dead-letter queue").await;
            }
            Expected::Nothing => {
                stays_empty(&emulator, case, &dead_letter_url, "the dead-letter queue").await;
                stays_empty(&emulator, case, &target_url, "the target queue").await;
            }
            Expected::DeadLettered(code) => {
                // The code the report must carry is the one the target's own API
                // answers that call with, so a row that does not state one asks the
                // target itself rather than restating its vocabulary here.
                let expected_code = match code {
                    Some(code) => code.to_string(),
                    None => refusal_code(&emulator, case, &target_url).await,
                };
                let reported = messages_once(&emulator, &dead_letter_url, 1, PATIENCE).await;
                let message = reported
                    .first()
                    .unwrap_or_else(|| panic!("case {case:?}: nothing was dead-lettered"));
                assert_eq!(
                    message["Body"].as_str(),
                    Some(marker.as_str()),
                    "case {case:?}: expected the undelivered payload {marker:?}, got {message}"
                );
                let attribute = |name: &str| {
                    message["MessageAttributes"][name]["StringValue"]
                        .as_str()
                        .map(ToString::to_string)
                };
                for (name, expected) in [
                    ("ERROR_CODE", expected_code.clone()),
                    ("SCHEDULE_ARN", schedule_arn.clone()),
                    ("TARGET_ARN", target_arn.clone()),
                    (
                        "SCHEDULED_TIME",
                        invocation::scheduled_time_text(occurrence),
                    ),
                    // A refusal of the request is never retried, so the report of one
                    // counts no retry even where the policy admitted two.
                    ("RETRY_ATTEMPTS", "0".to_string()),
                ] {
                    assert_eq!(
                        attribute(name),
                        Some(expected.clone()),
                        "case {case:?}: expected {name} to carry {expected:?}, got {message}"
                    );
                }
                let detail = attribute("ERROR_MESSAGE").unwrap_or_default();
                assert!(
                    !detail.is_empty(),
                    "case {case:?}: expected the failure's message to be reported, got {message}"
                );
                if code == Some(ACCESS_DENIED) {
                    // AWS's own authorization refusal: the call that was withheld and
                    // the resource it was on.
                    for expected in ["is not authorized to perform: sqs:SendMessage", &target_arn] {
                        assert!(
                            detail.contains(expected),
                            "case {case:?}: expected the refusal to name {expected:?}, got \
                             {detail:?}"
                        );
                    }
                }
                let execution = attribute("EXECUTION_ID").unwrap_or_default();
                // "8-4-4-4-12": the form EventBridge Scheduler names an invocation in.
                let groups: Vec<usize> = execution.split('-').map(str::len).collect();
                assert_eq!(
                    groups,
                    vec![8, 4, 4, 4, 12],
                    "case {case:?}: expected an execution id in the published form, got \
                     {execution:?}"
                );
                assert_eq!(
                    message["MessageAttributes"]["RETRY_ATTEMPTS"]["DataType"].as_str(),
                    Some("Number"),
                    "case {case:?}: expected the retry count reported as a figure, got {message}"
                );
                assert_eq!(
                    attribute("EXHAUSTED_RETRY_CONDITION"),
                    None,
                    "case {case:?}: expected no exhausted-retry condition for a refusal no \
                     retry was made of, got {message}"
                );
                // Nothing reached the target: the report is of an invocation that did
                // not arrive, never of one that did.
                if row.target == TargetQueue::Standard {
                    let held = peek(&emulator, &target_url).await;
                    assert!(
                        held.is_empty(),
                        "case {case:?}: expected the target to hold nothing, got {held:?}"
                    );
                }
            }
        }
    }
}

/// The queue the row's target names, and the url Amazon SQS addresses it by.
///
/// An ABSENT queue is named by the ARN a queue of that name would carry and never
/// created, so the url is the one the invocation itself resolves that ARN to.
async fn given_target(
    router: &MultiServiceRouter,
    case: &str,
    index: usize,
    target: TargetQueue,
) -> (String, String) {
    match target {
        TargetQueue::Standard => given_queue(router, case, &format!("target-{index}")).await,
        TargetQueue::Absent => {
            let name = format!("absent-{index}");
            (
                format!(
                    "https://sqs.{REGION}.amazonaws.com/{}/{name}",
                    account_id()
                ),
                format!("arn:aws:sqs:{REGION}:{}:{name}", account_id()),
            )
        }
        TargetQueue::Fifo => {
            let name = format!("target-{index}.fifo");
            let (status, created) = sqs(
                router,
                "CreateQueue",
                &json!({ "QueueName": name, "Attributes": { "FifoQueue": "true" } }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateQueue answered {status} {created}"
            );
            let url = created["QueueUrl"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {created}"))
                .to_string();
            (url, format!("arn:aws:sqs:{REGION}:{}:{name}", account_id()))
        }
    }
}

/// The error code a client sees when Amazon SQS refuses a plain `SendMessage` to
/// `url`.
///
/// The probe is refused for the same reason the invocation is, so this is the code
/// the service reports for that failure rather than one this suite states. It is read
/// the way a client reads it: Amazon SQS's protocol is query-compatible, so a refusal
/// carries the Query code in a header of its own beside the modelled name in the
/// body, and the Query code is the one an SDK surfaces.
async fn refusal_code(router: &MultiServiceRouter, case: &str, url: &str) -> String {
    let answered = sqs_response(
        router,
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": "a probe" }),
        true,
    )
    .await;
    let status = answered.status().as_u16();
    let query_code = answered
        .headers()
        .get("x-amzn-query-error")
        .and_then(|value| value.to_str().ok())
        // `<code>;<fault>`: the code is the first field.
        .and_then(|value| value.split(';').next())
        .map(ToString::to_string);
    let body = String::from_utf8_lossy(answered.body()).to_string();
    assert!(
        status >= 400,
        "case {case:?}: expected SendMessage to {url} to be refused, got {status} {body}"
    );
    if let Some(code) = query_code.filter(|code| !code.is_empty()) {
        return code;
    }
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let kind = parsed["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no error code in {body}"))
        .to_string();
    // An awsJson error type may be qualified by its namespace; the code is the name.
    kind.rsplit('#').next().unwrap_or(&kind).to_string()
}

/// The messages the queue at `url` holds once it holds at least `wanted`, or whatever
/// it holds after `patience` — left on the queue, with every message attribute they
/// carry.
async fn messages_once(
    router: &MultiServiceRouter,
    url: &str,
    wanted: usize,
    patience: Duration,
) -> Vec<Value> {
    let deadline = Instant::now() + patience;
    loop {
        let held = received(router, url).await;
        if held.len() >= wanted || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Assert the queue at `url` stays empty for as long as a row watches it, looking
/// again until the window closes.
async fn stays_empty(router: &MultiServiceRouter, case: &str, url: &str, what: &str) {
    let deadline = Instant::now() + QUIET_WINDOW;
    while Instant::now() < deadline {
        let held = received(router, url).await;
        assert!(
            held.is_empty(),
            "case {case:?}: expected {what} to hold nothing, got {held:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every message waiting on the queue at `url`, with its message attributes, left on
/// the queue: a report is read for its attributes, and a row that looks again must
/// still see it.
async fn received(router: &MultiServiceRouter, url: &str) -> Vec<Value> {
    let (status, answered) = sqs(
        router,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "VisibilityTimeout": 0,
            "MessageAttributeNames": ["All"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "receive {url}: ReceiveMessage answered {status} {answered}"
    );
    answered["Messages"].as_array().cloned().unwrap_or_default()
}
