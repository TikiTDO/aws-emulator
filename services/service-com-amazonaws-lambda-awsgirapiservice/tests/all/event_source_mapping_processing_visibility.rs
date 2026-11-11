// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how long the batch an event source mapping is PROCESSING stays out
//! of its queue: `CreateEventSourceMapping` "creates a mapping between an event source and
//! an Lambda Event Function. Lambda reads items from the event source and invokes the
//! function."
//!
//! The rule under test, in one sentence: a poller that takes a batch out of a queue to hand
//! it to a function keeps that batch invisible to every other consumer for as long as the
//! processing attempt it was handed to may last — the queue's own `VisibilityTimeout` or the
//! function's `Timeout`, whichever is the longer, since one attempt cannot outlast "the
//! amount of time that Lambda allows a function to run before stopping it" — so no message
//! is handed over a second time while the first attempt at it may still be running. The hold
//! is that span and no more: once no attempt can still be running, the message is the
//! queue's to hand over again, which the second test below pins by pausing the mapping and
//! watching the message come back.
//!
//! That is what the comparison a queue mapping is ACCEPTED against assumes (a queue whose
//! `VisibilityTimeout` is shorter than the function's `Timeout` is refused); the hold is
//! what keeps it true afterwards, because `SetQueueAttributes` can shorten a queue's
//! visibility timeout at any time and Amazon SQS knows nothing about the mappings that poll
//! it. Every row therefore states the pair the mapping is created on and then sets the
//! queue's timeout to the value under test, exactly as a client would.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with BOTH
//! services on ONE engine — the queue is created, re-configured and read through Amazon SQS
//! exactly as a client does it, and the mapping through Lambda — so a hold applied to
//! anything other than the stored queue fails here. The rows differ along what the rule
//! generalises over: which of the two spans is the longer one, how long each is (including a
//! queue that hides a delivery for no time at all), how many messages are waiting and how
//! many of them one batch may hold.
//!
//! Nothing here runs a deployment package, so no batch is ever processed: that is the
//! invocation this emulator refuses, and it is why these rows assert where the messages ARE
//! rather than what a function returned.

use std::time::{Duration, Instant};

use serde_json::json;
use service_com_amazonaws_lambda_awsgirapiservice::state::LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    call, call_sqs, create_mapping, create_queue, create_timed_function, get_mapping,
    processing_result_within_budget, queue_attribute, router_with_queues, set_visibility_timeout,
    MAPPINGS_URI,
};

/// The `State` a mapping reports once it is polling its event source.
const STATE_ENABLED: &str = "Enabled";

/// The `State` a mapping reports when its `Enabled` setting is false: "when false, Lambda
/// pauses polling and invocation."
const STATE_DISABLED: &str = "Disabled";

/// How long a row waits for work the mapping owes — settling into `Enabled`, taking the
/// waiting messages out of the queue — before it is a failure. Well above the poller's own
/// wake-up interval and the state-settle span, so a row fails only when the work was not
/// done at all.
const DEADLINE: Duration = Duration::from_secs(20);

/// How often a row re-reads while it waits.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// How far past the SHORTER of the two spans a row looks at its queue: long enough that a
/// hold cut to that span has demonstrably lapsed, and short enough to be well inside the
/// longer span the rule holds the batch for.
const SLACK: Duration = Duration::from_millis(700);

/// The `(visible, in flight)` message counts the queue `url` addresses reports.
async fn depths(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
) -> (Option<String>, Option<String>) {
    let visible = queue_attribute(router, case, url, "ApproximateNumberOfMessages").await;
    let in_flight =
        queue_attribute(router, case, url, "ApproximateNumberOfMessagesNotVisible").await;
    (visible, in_flight)
}

/// One row: the queue's `VisibilityTimeout` while it is being polled, the mapped function's
/// `Timeout`, how many messages are waiting and how many one batch may hold.
struct Row {
    case: &'static str,
    visibility_timeout: i64,
    function_timeout: i64,
    messages: usize,
    batch_size: i64,
}

impl Row {
    /// The span the batch must stay in flight for: the longer of the two, which is the rule
    /// under test.
    fn hold(&self) -> i64 {
        self.visibility_timeout.max(self.function_timeout)
    }

    /// The shorter of the two spans — the instant a hold cut to only one of them would have
    /// handed the batch back at, which is why a row looks just past it.
    fn probe_after(&self) -> Duration {
        Duration::from_secs(
            u64::try_from(self.visibility_timeout.min(self.function_timeout)).unwrap_or(0),
        ) + SLACK
    }
}

/// The catalog: the queue's timeout below the function's (including a queue that hides a
/// delivery for no time at all, the published minimum of `VisibilityTimeout`) and above it,
/// one message and several, and a batch ceiling both above and below what is waiting.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a queue that hides nothing, a five second function",
            visibility_timeout: 0,
            function_timeout: 5,
            messages: 1,
            batch_size: 10,
        },
        Row {
            case: "a one second queue, an eight second function, three messages",
            visibility_timeout: 1,
            function_timeout: 8,
            messages: 3,
            batch_size: 10,
        },
        Row {
            case: "a one second queue, a five second function, two messages one per batch",
            visibility_timeout: 1,
            function_timeout: 5,
            messages: 2,
            batch_size: 1,
        },
        Row {
            case: "a four second queue, a one second function",
            visibility_timeout: 4,
            function_timeout: 1,
            messages: 1,
            batch_size: 10,
        },
    ]
}

#[tokio::test]
async fn a_polled_batch_stays_in_flight_for_as_long_as_its_attempt_may_last() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = router_with_queues();
        let function = format!("esm-hold-fn-{index}");
        // The pair is created the way Lambda accepts one — a queue that outlasts its
        // function — and the timeout under test is set afterwards, which is the only way a
        // client reaches a shorter one.
        let (url, arn) = create_queue(
            &router,
            case,
            &format!("esm-hold-queue-{index}"),
            &json!({ "VisibilityTimeout": row.hold().to_string() }),
        )
        .await;
        create_timed_function(&router, case, &function, row.function_timeout).await;

        let created = create_mapping(
            &router,
            &json!({
                "FunctionName": function,
                "EventSourceArn": arn,
                "BatchSize": row.batch_size,
            }),
        )
        .await;
        assert_eq!(
            created.status,
            202,
            "case {case:?}: expected CreateEventSourceMapping to answer 202, got {} {}",
            created.status,
            created.text()
        );
        let uuid = created
            .string("UUID")
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));

        // The mapping polls from the moment it reports `Enabled`, so the queue is given the
        // timeout under test before anything is waiting on it.
        let deadline = Instant::now() + DEADLINE;
        let mut state = None;
        while Instant::now() < deadline {
            state = get_mapping(&router, &uuid).await.string("State");
            if state.as_deref() == Some(STATE_ENABLED) {
                break;
            }
            tokio::time::sleep(LOOK_AGAIN).await;
        }
        assert_eq!(
            state.as_deref(),
            Some(STATE_ENABLED),
            "case {case:?}: the mapping must settle into {STATE_ENABLED} before it polls, \
             got {state:?}"
        );
        set_visibility_timeout(&router, case, &url, row.visibility_timeout).await;

        for message in 0..row.messages {
            let (status, sent) = call_sqs(
                &router,
                "SendMessage",
                &json!({ "QueueUrl": url, "MessageBody": format!("held-{index}-{message}") }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected SendMessage to answer 200, got {status} {sent}"
            );
        }

        // Every waiting message is taken out of the queue by the poller. A batch ceiling
        // below what is waiting takes several passes to get through them, and nothing is
        // deleted, so the wait is for all of them to be in flight at once.
        let waiting = row.messages.to_string();
        let deadline = Instant::now() + DEADLINE;
        let mut in_flight = None;
        while Instant::now() < deadline {
            in_flight = queue_attribute(
                &router,
                case,
                &url,
                "ApproximateNumberOfMessagesNotVisible",
            )
            .await;
            if in_flight.as_deref() == Some(waiting.as_str()) {
                break;
            }
            tokio::time::sleep(LOOK_AGAIN).await;
        }
        assert_eq!(
            in_flight.as_deref(),
            Some(waiting.as_str()),
            "case {case:?}: the mapping's poller must take all {} waiting message(s) out of \
             the queue, got {in_flight:?} in flight",
            row.messages
        );

        // Past the shorter of the two spans, and well inside the longer one the rule holds
        // the batch for: nothing has come back, so nothing can have been handed to a second
        // consumer while the attempt it was handed to may still be running.
        tokio::time::sleep(row.probe_after()).await;
        let (visible, in_flight) = depths(&router, case, &url).await;
        assert_eq!(
            (visible.as_deref(), in_flight.as_deref()),
            (Some("0"), Some(waiting.as_str())),
            "case {case:?}: {} second(s) into an attempt a {} second function may take, all \
             {} message(s) must still be in flight with the poller and none visible, got \
             {visible:?} visible and {in_flight:?} in flight",
            row.probe_after().as_secs_f32(),
            row.function_timeout,
            row.messages
        );

        // And the mapping is still the one that holds them: it reports what it polled for,
        // and the attempt it made on the batch.
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            (reading.status, reading.string("State")),
            (200, Some(STATE_ENABLED.to_string())),
            "case {case:?}: the mapping must still report {STATE_ENABLED}, got {} {}",
            reading.status,
            reading.text()
        );
        // The batch going out of the queue is the attempt STARTING: what it came to is
        // reported once the invocation it was handed to has been decided, so it is waited
        // for rather than read at the instant the hold was probed at.
        let result = processing_result_within_budget(&router, case, &uuid)
            .await
            .unwrap_or_default();
        assert_eq!(
            result, LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED,
            "case {case:?}: nothing here runs a deployment package, so the attempt this \
             mapping made on its batch must be reported as the unsuccessful one it was, got \
             {result:?} in {}",
            reading.text()
        );
    }
}

#[tokio::test]
async fn the_hold_lapses_once_the_attempt_can_no_longer_be_running() {
    // The other side of the rule: the hold is the span the attempt may last, so it is
    // BOUNDED by that span rather than indefinite — once it is over, the message is the
    // queue's to hand over again. The mapping is PAUSED first ("when false, Lambda pauses
    // polling and invocation"), because a running poller takes the message straight back
    // the instant it becomes visible and nothing could be observed between the two.
    let case = "a queue that hides nothing, a three second function, paused mid-hold";
    let visibility_timeout = 0;
    let function_timeout = 3;
    let router = router_with_queues();
    let (url, arn) = create_queue(
        &router,
        case,
        "esm-hold-lapses-queue",
        &json!({ "VisibilityTimeout": function_timeout.to_string() }),
    )
    .await;
    create_timed_function(&router, case, "esm-hold-lapses-fn", function_timeout).await;
    let created = create_mapping(
        &router,
        &json!({ "FunctionName": "esm-hold-lapses-fn", "EventSourceArn": arn }),
    )
    .await;
    assert_eq!(
        created.status,
        202,
        "case {case:?}: expected CreateEventSourceMapping to answer 202, got {} {}",
        created.status,
        created.text()
    );
    let uuid = created
        .string("UUID")
        .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));

    let deadline = Instant::now() + DEADLINE;
    let mut state = None;
    while Instant::now() < deadline {
        state = get_mapping(&router, &uuid).await.string("State");
        if state.as_deref() == Some(STATE_ENABLED) {
            break;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    assert_eq!(
        state.as_deref(),
        Some(STATE_ENABLED),
        "case {case:?}: the mapping must settle into {STATE_ENABLED} before it polls, got \
         {state:?}"
    );
    set_visibility_timeout(&router, case, &url, visibility_timeout).await;

    let (status, sent) = call_sqs(
        &router,
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": "held-then-released" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected SendMessage to answer 200, got {status} {sent}"
    );

    let deadline = Instant::now() + DEADLINE;
    let mut in_flight = None;
    while Instant::now() < deadline {
        in_flight = queue_attribute(
            &router,
            case,
            &url,
            "ApproximateNumberOfMessagesNotVisible",
        )
        .await;
        if in_flight.as_deref() == Some("1") {
            break;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    assert_eq!(
        in_flight.as_deref(),
        Some("1"),
        "case {case:?}: the mapping's poller must take the message out of the queue, got \
         {in_flight:?} in flight"
    );

    let paused = call(
        &router,
        "PUT",
        &format!("{MAPPINGS_URI}/{uuid}"),
        &json!({ "Enabled": false }),
    )
    .await;
    assert_eq!(
        paused.status,
        202,
        "case {case:?}: expected UpdateEventSourceMapping to answer 202, got {} {}",
        paused.status,
        paused.text()
    );
    let deadline = Instant::now() + DEADLINE;
    let mut state = None;
    while Instant::now() < deadline {
        state = get_mapping(&router, &uuid).await.string("State");
        if state.as_deref() == Some(STATE_DISABLED) {
            break;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    assert_eq!(
        state.as_deref(),
        Some(STATE_DISABLED),
        "case {case:?}: the paused mapping must settle into {STATE_DISABLED}, got {state:?}"
    );

    // Past the span the attempt could have lasted: the message is the queue's again, and
    // nothing deleted it, because no deployment package ran.
    tokio::time::sleep(
        Duration::from_secs(u64::try_from(function_timeout).unwrap_or(0)) + SLACK,
    )
    .await;
    let (visible, in_flight) = depths(&router, case, &url).await;
    assert_eq!(
        (visible.as_deref(), in_flight.as_deref()),
        (Some("1"), Some("0")),
        "case {case:?}: once no attempt can still be running the message must be visible on \
         its queue again, got {visible:?} visible and {in_flight:?} in flight"
    );
}
