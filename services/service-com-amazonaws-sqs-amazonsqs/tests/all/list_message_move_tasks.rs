// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListMessageMoveTasks`, driven over the wire (`POST /` with
//! `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared harness in
//! [`super::support`].
//!
//! The rule under test: `ListMessageMoveTasks` reports the message movement tasks
//! recorded under the source queue the request names and no other queue's, most recent
//! first, at most `MaxResults` of them (default 1, upper limit 10), each entry's handle,
//! queues, status and `ApproximateNumberOfMessages*` counts read off the task record and
//! the clock — each member populated only in the condition the model documents it for, so
//! the handle appears exactly while the task is `RUNNING` and `DestinationArn` only for a
//! task whose start named one. Reporting them changes nothing a later caller sees. A
//! `SourceArn` that names no queue of this account, and a `MaxResults` outside the
//! documented range, are refused.
//!
//! Each table spans the class the rule generalises over rather than one example: the
//! listing rows walk the recorded task count (none, one, several) crossed with the
//! `MaxResults` forms (absent, below the recorded count, at the published upper limit),
//! the refusal rows walk each published bound of `MaxResults` and one value past it
//! plus every way a `SourceArn` can fail to name a queue of this account, and the progress
//! rows walk the size a task is started over against how much of it the source queue can
//! hand over, the order those messages were sent in, and each form the movement rate takes.

use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    DEFAULT_MESSAGE_MOVE_TASK_RESULTS, MAX_MESSAGE_MOVE_TASKS_PER_QUEUE, MAX_MESSAGES_PER_SECOND,
    MIN_MESSAGES_PER_SECOND,
};

use super::support::{
    account, call, create_ok, dead_letter_queue, emulator, error_code, expected_arn, ok, send_all,
    OTHER_ACCOUNT,
};
use smithy_server_runtime::MultiServiceRouter;

/// How long a loop waits for the state the clock owes — the running task to finish, or
/// the next start to be accepted — before giving up. Far longer than any row's own
/// schedule, so a slow machine cannot fail a row the emulator is still working through.
const DEADLINE: Duration = Duration::from_secs(20);

/// How long a polling loop pauses between two attempts.
const PAUSE: Duration = Duration::from_millis(50);

/// A task's status while it still has messages to move, and the one it reports once it has
/// moved everything its source queue had to give.
const RUNNING: &str = "RUNNING";
const COMPLETED: &str = "COMPLETED";

/// The statuses a task this store records can be in: "Possible values are: RUNNING,
/// COMPLETED, CANCELLING, CANCELLED, and FAILED"
/// (`ListMessageMoveTasksResultEntry$Status`) — of those, a task that takes its messages
/// when it starts and is never cancelled reaches only these two.
const REACHABLE_STATUSES: &[&str] = &[RUNNING, COMPLETED];

/// For how long a message a progress row keeps BACK from its task is hidden from it: the
/// longest delay a send may ask for ("Valid values: 0 to 900. Maximum: 15 minutes",
/// `SendMessageRequest$DelaySeconds`).
///
/// It outlasts [`DEADLINE`] many times over, which is what lets a row hold a task in
/// `RUNNING` for as long as the row runs: the source queue still holds a message the task
/// may not take, so the task cannot have finished, however fast or slow the host is.
const HIDDEN_DELAY_SECONDS: i64 = 900;

/// The `Results` of a listing reply, as a list — empty when the member is absent, which
/// is how a reply that reports no task answers.
fn results(reply: &Value) -> Vec<Value> {
    reply
        .get("Results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// List the tasks of the queue `source_arn`, with `MaxResults` when one is given.
async fn list(
    router: &MultiServiceRouter,
    source_arn: &str,
    max_results: Option<i64>,
) -> Vec<Value> {
    let mut request = json!({"SourceArn": source_arn});
    if let Some(max) = max_results {
        request["MaxResults"] = json!(max);
    }
    results(&ok(router, "ListMessageMoveTasks", request).await)
}

/// One entry's string member, or `"<absent>"` so a failing assertion names what it saw.
fn text(entry: &Value, member: &str) -> String {
    entry
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
        .to_string()
}

/// One entry's `long` member. `ApproximateNumberOfMessagesMoved` and `StartedTimestamp`
/// both carry a modelled default of 0, which the wire may leave out, so an absent member
/// reads as that default.
fn number(entry: &Value, member: &str) -> i64 {
    entry.get(member).and_then(Value::as_i64).unwrap_or(0)
}

/// Start a message movement task from `source_arn`, waiting out the task already running
/// for that queue ("only one active message movement task is supported per queue at any
/// given time"), and return its handle.
async fn start_task(
    router: &MultiServiceRouter,
    case: &str,
    source_arn: &str,
    rate: Option<i64>,
) -> String {
    let mut request = json!({"SourceArn": source_arn});
    if let Some(rate) = rate {
        request["MaxNumberOfMessagesPerSecond"] = json!(rate);
    }
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (status, parsed) = call(router, "StartMessageMoveTask", &request).await;
        if status == 200 {
            let handle = parsed["TaskHandle"].as_str().unwrap_or_default().to_string();
            assert!(
                !handle.is_empty(),
                "case {case}: expected a TaskHandle, got {parsed}"
            );
            return handle;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected a start to be accepted within {DEADLINE:?}, got {status} \
             with {parsed}"
        );
        tokio::time::sleep(PAUSE).await;
    }
}

/// Now, as the epoch milliseconds a `StartedTimestamp` is reported in.
fn now_millis() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is past the Unix epoch")
            .as_millis(),
    )
    .expect("an instant that counts in an i64")
}

/// Assert the rule the handle member carries: "When this field is returned in the response
/// of the `ListMessageMoveTasks` action, it is only populated for tasks that are in RUNNING
/// status" (`ListMessageMoveTasksResultEntry$TaskHandle`) — so an entry reports the handle
/// its start answered with exactly while it is running, and nothing once it has left that
/// status.
fn assert_reported_handle(case: &str, entry: &Value, started_handle: &str) {
    let reported = entry.get("TaskHandle").and_then(Value::as_str);
    let status = text(entry, "Status");
    if status == RUNNING {
        assert_eq!(
            reported,
            Some(started_handle),
            "case {case}: expected a RUNNING task to report the handle its start answered \
             with, got {entry}"
        );
    } else {
        assert!(
            reported.is_none(),
            "case {case}: expected a task in {status} to report no handle, got {entry}"
        );
    }
}

/// A listing reports the tasks recorded under the named source queue, most recent first,
/// bounded by `MaxResults` (default 1, upper limit 10), with each entry's queues and
/// handle read off the task record.
#[tokio::test]
async fn a_listing_reports_the_most_recent_tasks_of_its_source_queue_newest_first() {
    // (case, how many tasks are started, the MaxResults the listing asks for, how many
    // entries it must report). The rows walk the recorded task count against each form
    // `MaxResults` takes: absent ("the default is 1, which provides the most recent
    // message movement task"), below the recorded count, and at the published upper limit
    // of 10.
    let rows: &[(&str, usize, Option<i64>, usize)] = &[
        ("no task started, MaxResults absent", 0, None, 0),
        ("no task started, MaxResults at the upper limit", 0, Some(10), 0),
        ("one task, MaxResults absent", 1, None, 1),
        ("three tasks, MaxResults absent", 3, None, 1),
        ("three tasks, MaxResults 2", 3, Some(2), 2),
        ("three tasks, MaxResults at the upper limit", 3, Some(10), 3),
    ];
    assert_eq!(
        (DEFAULT_MESSAGE_MOVE_TASK_RESULTS, MAX_MESSAGE_MOVE_TASKS_PER_QUEUE),
        (1, 10),
        "case the documented bounds: the default is 1 and the upper limit is 10"
    );

    for (index, (case, started, max_results, expected)) in rows.iter().enumerate() {
        let router = emulator();
        let prefix = format!("listing-{index}");
        let (_, dead_letter_arn, _) = dead_letter_queue(&router, &prefix).await;

        // Each task is identified by the window of epoch milliseconds its start call fell
        // in rather than by the handle it answered with: a handle is reported only while
        // the task is RUNNING, and a start waits out the task before it, so the older
        // tasks of a row have left that status by the time the listing is read.
        let mut starts: Vec<(i64, i64, String)> = Vec::with_capacity(*started);
        for task in 0..*started {
            let before = now_millis();
            let handle = start_task(
                &router,
                &format!("{case}: task {task}"),
                &dead_letter_arn,
                None,
            )
            .await;
            starts.push((before, now_millis(), handle));
        }

        let listed = list(&router, &dead_letter_arn, *max_results).await;
        assert_eq!(
            listed.len(),
            *expected,
            "case {case}: expected {expected} entries, got {listed:?}"
        );

        // "Gets the MOST RECENT message movement tasks (up to 10)": the newest task is the
        // first entry, so the entries are the last `expected` started, newest first.
        for (position, entry) in listed.iter().enumerate() {
            let (before, after, handle) = &starts[started - 1 - position];
            let started_timestamp = number(entry, "StartedTimestamp");
            assert!(
                (*before..=*after).contains(&started_timestamp),
                "case {case}: expected entry {position} to be the task started in \
                 {before}..={after}, got {entry}"
            );
            assert_eq!(
                text(entry, "SourceArn"),
                dead_letter_arn,
                "case {case}: expected the source queue the task resolved, got {entry}"
            );
            // "The ARN of the destination queue IF IT HAS BEEN SPECIFIED in the
            // StartMessageMoveTask request. If a DestinationArn has not been specified ...
            // this field value will be NULL"
            // (ListMessageMoveTasksResultEntry$DestinationArn): these starts named none.
            assert!(
                entry.get("DestinationArn").is_none(),
                "case {case}: a start that named no destination reports none, got {entry}"
            );
            assert!(
                REACHABLE_STATUSES.contains(&text(entry, "Status").as_str()),
                "case {case}: expected one of {REACHABLE_STATUSES:?}, got {entry}"
            );
            assert_reported_handle(case, entry, handle);
            // A task that took no message has nothing left to move and has moved nothing,
            // whatever the clock has done since.
            assert_eq!(
                (
                    number(entry, "ApproximateNumberOfMessagesMoved"),
                    number(entry, "ApproximateNumberOfMessagesToMove")
                ),
                (0, 0),
                "case {case}: a task over an empty queue moves nothing, got {entry}"
            );
            assert!(
                entry.get("MaxNumberOfMessagesPerSecond").is_none(),
                "case {case}: a start that fixed no rate reports none, got {entry}"
            );
            assert!(
                entry.get("FailureReason").is_none(),
                "case {case}: a task that has not failed reports no reason, got {entry}"
            );
        }
    }
}

/// Every way a listing can name no queue of this account, or ask for a number of results
/// outside the documented range, is refused — and each published bound of that range is
/// accepted.
#[tokio::test]
async fn a_listing_naming_no_queue_or_asking_outside_the_result_range_is_refused() {
    let router = emulator();
    assert_ne!(
        OTHER_ACCOUNT,
        account(),
        "case the other-account row: its account id must not be this emulator's"
    );
    let (_, dead_letter_arn, _) = dead_letter_queue(&router, "refused-listing").await;

    // (case, request, expected status, expected error code — empty for the rows that must
    // be accepted). "The default is 1 ... The upper limit is 10"
    // (ListMessageMoveTasksRequest$MaxResults), so 1 and 10 are the published bounds and
    // 0, -1 and 11 are outside them. A value that names no queue in any account is not an
    // address this service can resolve, which is what `InvalidAddress` refuses; a
    // well-formed ARN of another account or service names a resource that does not exist
    // here.
    let rows: &[(&str, Value, u16, &str)] = &[
        (
            "MaxResults at the published lower bound of 1",
            json!({"SourceArn": dead_letter_arn, "MaxResults": 1}),
            200,
            "",
        ),
        (
            "MaxResults at the published upper limit of 10",
            json!({"SourceArn": dead_letter_arn, "MaxResults": 10}),
            200,
            "",
        ),
        (
            "MaxResults of zero, one below the lower bound",
            json!({"SourceArn": dead_letter_arn, "MaxResults": 0}),
            400,
            "InvalidParameterValue",
        ),
        (
            "a negative MaxResults",
            json!({"SourceArn": dead_letter_arn, "MaxResults": -1}),
            400,
            "InvalidParameterValue",
        ),
        (
            "MaxResults of 11, one past the upper limit",
            json!({"SourceArn": dead_letter_arn, "MaxResults": 11}),
            400,
            "InvalidParameterValue",
        ),
        (
            "a SourceArn no queue of this account answers to",
            json!({"SourceArn": expected_arn("refused-listing-absent")}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a SourceArn naming another account",
            json!({"SourceArn": format!("arn:aws:sqs:us-east-1:{OTHER_ACCOUNT}:refused-listing")}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a SourceArn naming another service",
            json!({"SourceArn": format!("arn:aws:sns:us-east-1:{}:refused-listing", account())}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a SourceArn that is not an ARN at all",
            json!({"SourceArn": "refused-listing"}),
            404,
            "InvalidAddress",
        ),
        (
            "a SourceArn with a segment too few",
            json!({"SourceArn": format!("arn:aws:sqs:{}:refused-listing", account())}),
            404,
            "InvalidAddress",
        ),
        (
            "a SourceArn with an empty region segment",
            json!({"SourceArn": format!("arn:aws:sqs::{}:refused-listing", account())}),
            404,
            "InvalidAddress",
        ),
        (
            "a SourceArn whose queue name is longer than a queue name may be",
            json!({
                "SourceArn": format!("arn:aws:sqs:us-east-1:{}:{}", account(), "q".repeat(81)),
            }),
            404,
            "InvalidAddress",
        ),
    ];

    // One task is recorded, so an accepted row has something to report and a refused one
    // cannot be mistaken for an empty store.
    let handle = start_task(&router, "the recorded task", &dead_letter_arn, None).await;

    for (case, request, expected_status, expected_code) in rows {
        let (status, parsed) = call(&router, "ListMessageMoveTasks", request).await;
        if *expected_status == 200 {
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {parsed}"
            );
            let listed = results(&parsed);
            assert_eq!(
                listed.len(),
                1,
                "case {case}: expected the recorded task, got {parsed}"
            );
            assert_reported_handle(case, &listed[0], &handle);
            continue;
        }
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (*expected_status, *expected_code),
            "case {case}: expected HTTP {expected_status} {expected_code}, got {status} with \
             {parsed}"
        );
        assert!(
            parsed.get("Results").is_none(),
            "case {case}: a refusal reports no task, got {parsed}"
        );
    }
}

/// One row of the progress table: the case, how many messages the source queue holds
/// visibly when the task starts, how many more of them a delay hides from it, whether those
/// hidden ones were sent AHEAD of the visible ones, and the `MaxNumberOfMessagesPerSecond`
/// the start fixes.
struct ProgressRow {
    case: &'static str,
    visible: usize,
    hidden: usize,
    hidden_first: bool,
    rate: Option<i64>,
}

/// Send `count` messages to the queue at `url` that no reader may take for
/// [`HIDDEN_DELAY_SECONDS`]: "messages with a positive `DelaySeconds` value become available
/// for processing after the delay period is finished" (`SendMessageRequest$DelaySeconds`),
/// so until then the queue holds them and neither a consumer nor a message movement task
/// can be handed one.
async fn send_hidden(router: &MultiServiceRouter, case: &str, url: &str, count: usize) {
    for number in 0..count {
        ok(
            router,
            "SendMessage",
            json!({
                "QueueUrl": url,
                "MessageBody": format!("{case}: hidden #{number}"),
                "DelaySeconds": HIDDEN_DELAY_SECONDS,
            }),
        )
        .await;
    }
}

/// A task's rate and progress are reported from its record and the clock: the number to
/// move is the size the task was started over and stays there, the moved count never goes
/// backwards and never passes what the source queue can hand over, a task that still holds
/// messages reports `RUNNING` and keeps its source queue busy, and the moved count reaches
/// the size the task was started over exactly when the task is `COMPLETED` — and reporting
/// any of it neither delivers a message nor frees the source queue for a second task.
#[tokio::test]
async fn a_tasks_reported_progress_follows_the_record_and_the_clock() {
    // The rows walk what the rule generalises over: the size the task is sized over, how
    // much of that size the source queue can hand over at all, the order those messages
    // were sent in, and each form the rate takes — absent ("if this field is left blank,
    // the system will optimize the rate"), at the lowest rate that moves anything, and at
    // the published maximum of 500
    // (`StartMessageMoveTaskRequest$MaxNumberOfMessagesPerSecond`).
    //
    // A row that hides a message from its task is the row where `RUNNING` is a state and
    // not a race: a task is COMPLETED only over a source queue with nothing left, so one
    // whose source still holds a message it may not take keeps running for as long as the
    // row runs, whatever the host's speed.
    let rows: &[ProgressRow] = &[
        ProgressRow {
            case: "three visible messages at the lowest fixed rate",
            visible: 3,
            hidden: 0,
            hidden_first: false,
            rate: Some(i64::from(MIN_MESSAGES_PER_SECOND)),
        },
        ProgressRow {
            case: "two visible messages at the rate the service optimizes",
            visible: 2,
            hidden: 0,
            hidden_first: false,
            rate: None,
        },
        ProgressRow {
            case: "one visible message at the published maximum rate",
            visible: 1,
            hidden: 0,
            hidden_first: false,
            rate: Some(i64::from(MAX_MESSAGES_PER_SECOND)),
        },
        ProgressRow {
            case: "two visible messages and one a delay hides, at the lowest fixed rate",
            visible: 2,
            hidden: 1,
            hidden_first: false,
            rate: Some(i64::from(MIN_MESSAGES_PER_SECOND)),
        },
        ProgressRow {
            case: "a message a delay hides sent ahead of two visible ones",
            visible: 2,
            hidden: 1,
            hidden_first: true,
            rate: None,
        },
        ProgressRow {
            case: "a source queue holding nothing but a message a delay hides",
            visible: 0,
            hidden: 1,
            hidden_first: true,
            rate: Some(i64::from(MIN_MESSAGES_PER_SECOND)),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let (dead_letter_url, dead_letter_arn, _) =
            dead_letter_queue(&router, &format!("progress-{index}")).await;
        let visible: Vec<String> = (0..row.visible)
            .map(|number| format!("{case}: message #{number}"))
            .collect();
        if row.hidden_first {
            send_hidden(&router, case, &dead_letter_url, row.hidden).await;
            send_all(&router, &dead_letter_url, &visible).await;
        } else {
            send_all(&router, &dead_letter_url, &visible).await;
            send_hidden(&router, case, &dead_letter_url, row.hidden).await;
        }
        // "The number of messages to be moved from the source queue. This number is
        // obtained at the time of starting the message movement task"
        // (ListMessageMoveTasksResultEntry$ApproximateNumberOfMessagesToMove): every
        // message the queue holds when the task starts, whether a delay hides it or not.
        let sized = i64::try_from(row.visible + row.hidden).expect("a small count");
        // What the task can ever hand over while this row runs: a message still inside its
        // delay is not the queue's to give away.
        let movable = i64::try_from(row.visible).expect("a small count");
        let handle = start_task(&router, case, &dead_letter_arn, row.rate).await;

        // The loop waits for the state the CLOCK owes — every message the task can take
        // handed over — rather than for a span of its own, so no row's outcome depends on
        // how fast the host got there.
        let deadline = Instant::now() + DEADLINE;
        let mut moved_before = 0;
        let mut saw_running = false;
        loop {
            let listed = list(&router, &dead_letter_arn, None).await;
            let entry = listed
                .first()
                .cloned()
                .unwrap_or_else(|| panic!("case {case}: expected one entry, got {listed:?}"));
            assert_reported_handle(case, &entry, &handle);
            assert_eq!(
                entry.get("MaxNumberOfMessagesPerSecond").and_then(Value::as_i64),
                row.rate,
                "case {case}: expected the rate the start fixed, got {entry}"
            );
            let moved = number(&entry, "ApproximateNumberOfMessagesMoved");
            let to_move = number(&entry, "ApproximateNumberOfMessagesToMove");
            // The size the task was started over does not fall as the task delivers.
            assert_eq!(
                to_move, sized,
                "case {case}: expected the {sized} messages the task was started over, got \
                 {entry}"
            );
            assert!(
                moved <= movable,
                "case {case}: expected at most the {movable} messages its source queue can \
                 hand over to have moved, got {entry}"
            );
            assert!(
                moved >= moved_before,
                "case {case}: expected the moved count never to go backwards, got {moved} \
                 after {moved_before}"
            );
            moved_before = moved;

            let status = text(&entry, "Status");
            if status == RUNNING {
                saw_running = true;
                assert!(
                    moved < to_move,
                    "case a RUNNING task ({case}): expected messages still to move, got {entry}"
                );
                if row.hidden > 0 {
                    // A listing is a report, not a turn of the crank: while the task runs,
                    // the source queue is still busy, whoever has read it. The rows asked
                    // here are the ones whose task cannot finish, so the source queue is
                    // busy at this second call too however long the host took to make it.
                    let (refused, parsed) = call(
                        &router,
                        "StartMessageMoveTask",
                        &json!({"SourceArn": dead_letter_arn}),
                    )
                    .await;
                    assert_eq!(
                        (refused, error_code(&parsed).as_str()),
                        (400, "InvalidParameterValue"),
                        "case a start while the listed task runs ({case}): expected HTTP 400 \
                         InvalidParameterValue, got {refused} with {parsed}"
                    );
                }
            } else {
                assert_eq!(
                    row.hidden, 0,
                    "case {case}: a task whose source queue still holds a message a delay \
                     hides has not moved everything that queue had to give, got {entry}"
                );
                assert_eq!(
                    (status.as_str(), moved, to_move),
                    (COMPLETED, sized, sized),
                    "case a finished task ({case}): expected every message it was started \
                     over to have moved, got {entry}"
                );
                break;
            }
            // A task that may never take the rest of its source queue is as far on as it
            // will ever get once everything it CAN take has moved, so that — and not a
            // status it cannot reach — is the condition such a row waits for.
            if row.hidden > 0 && moved == movable {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case}: expected the task's own schedule to be done within \
                 {DEADLINE:?}, last saw {moved} of {sized} moved"
            );
            tokio::time::sleep(PAUSE).await;
        }
        assert!(
            row.hidden == 0 || saw_running,
            "case {case}: expected to observe the task RUNNING over a source queue it \
             cannot drain"
        );
        if row.hidden > 0 {
            // A task that has handed over everything it may take is still not finished,
            // and the listings above have not carried it one message further: the hidden
            // message is its source queue's until the clock makes it available, so a
            // second reading of the task says exactly what the last one did.
            let again = list(&router, &dead_letter_arn, None).await;
            let entry = again.first().cloned().unwrap_or_else(|| {
                panic!("case {case}: expected the task still listed, got {again:?}")
            });
            assert_eq!(
                (
                    text(&entry, "Status"),
                    number(&entry, "ApproximateNumberOfMessagesMoved"),
                    number(&entry, "ApproximateNumberOfMessagesToMove"),
                ),
                (RUNNING.to_string(), movable, sized),
                "case {case}: expected the task still running over the message a delay \
                 hides, with the {movable} it could take moved, got {entry}"
            );
        }
    }
}

/// One row of the per-source-queue table: the case, the queue listed, and the
/// `(handle, size started over)` of every task that queue must report.
type OwnTaskRow<'a> = (&'a str, &'a String, Vec<(String, i64)>);

/// "Under a SPECIFIC source queue": a task is listed under the queue it moves messages
/// from and under no other, and reading one queue's tasks does not change what the next
/// read of either queue sees.
#[tokio::test]
async fn a_task_is_listed_only_under_the_source_queue_it_moves_from() {
    let router = emulator();
    let (first_url, first_arn, _) = dead_letter_queue(&router, "own-tasks-first").await;
    let (second_url, second_arn, _) = dead_letter_queue(&router, "own-tasks-second").await;
    // A queue that is nobody's dead-letter target can have no task started from it, so it
    // has none to report either.
    let plain_arn = expected_arn("own-tasks-plain");
    create_ok(&router, "own-tasks-plain", &json!({})).await;

    // The two queues hold a different number of messages, so the size each task was
    // started over tells the two tasks apart for as long as they are recorded — a handle
    // identifies a task only while it is running.
    send_all(&router, &first_url, &["the first queue's only message".to_string()]).await;
    send_all(
        &router,
        &second_url,
        &["second #0".to_string(), "second #1".to_string()],
    )
    .await;

    let first = start_task(&router, "the first queue's task", &first_arn, None).await;
    let second = start_task(&router, "the second queue's task", &second_arn, None).await;

    // (case, the queue listed, the (handle, size started over) of every task it reports)
    let rows: &[OwnTaskRow] = &[
        ("the first dead-letter queue", &first_arn, vec![(first.clone(), 1)]),
        ("the second dead-letter queue", &second_arn, vec![(second.clone(), 2)]),
        ("a queue no task was started from", &plain_arn, Vec::new()),
    ];

    // Listed twice: a report is idempotent, so the second pass must say exactly what the
    // first did.
    for pass in 0..2 {
        for (case, source_arn, expected) in rows {
            let listed = list(&router, source_arn, Some(10)).await;
            assert_eq!(
                listed.len(),
                expected.len(),
                "case {case} on pass {pass}: expected {} entries, got {listed:?}",
                expected.len()
            );
            for (entry, (handle, to_move)) in listed.iter().zip(expected.iter()) {
                assert_eq!(
                    text(entry, "SourceArn"),
                    **source_arn,
                    "case {case} on pass {pass}: expected the queue listed, got {entry}"
                );
                assert_eq!(
                    number(entry, "ApproximateNumberOfMessagesToMove"),
                    *to_move,
                    "case {case} on pass {pass}: expected the task started over {to_move} \
                     messages, got {entry}"
                );
                assert_reported_handle(&format!("{case} on pass {pass}"), entry, handle);
            }
        }
    }
}
