// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `AWS/SQS` series a queue publishes about itself — the backlog
//! gauges it is read into and the counters its data-plane calls are counted into — asserted
//! where a client reads them: through CloudWatch's own `GetMetricStatistics` and
//! `ListMetrics`, with both services mounted on one shared engine exactly as `core/router`
//! assembles them.
//!
//! RULE under test: every message a queue holds is counted in exactly ONE of the three
//! `AWS/SQS` series dimensioned by `QueueName` — `ApproximateNumberOfMessagesVisible`,
//! `ApproximateNumberOfMessagesNotVisible` and `ApproximateNumberOfMessagesDelayed` — by
//! where that message stands at the instant the queue was read, and a queue that exists is
//! read once a minute whether or not a client calls SQS. So a message sent with a positive
//! `DelaySeconds` is counted in the delayed series and NEVER in the visible one while the
//! delay is in force, it counts nothing towards the age of the oldest message the queue could
//! deliver, it moves to the visible series when the delay elapses with nobody calling, an idle
//! queue reports zero in all three series, and a queue that does not exist has no series at
//! all. The data-plane calls the queue serves are counted into its own sent, received,
//! deleted and empty-receive counters.
//!
//! Each gauge is read as the PEAK it reported over the window (`Maximum`), which is what a
//! client watching a queue's backlog reads: a series whose peak is zero never counted the
//! message, whatever moment a reader happened to look at. A counter is read as its total
//! (`Sum`), which is the traffic the window carried. Because a reading is taken once a
//! minute, each step a test drives is followed by [`sample_again`], which makes the next
//! minute's reading due through the engine seam rather than waiting for the clock.
//!
//! The table below spans what the rule generalises over: how many messages are sent, how many
//! of them are delayed, the delay value (none, zero, one minute, the documented maximum of 900
//! seconds), where the delay comes from (the send's own member on a standard queue, the queue
//! attribute on a FIFO queue, which is the only place a FIFO queue takes one) and the queue
//! type. The recorded case — one standard queue, one message sent with a delay — is ONE row.
//! The tests after it drive the rest: a delivery and a delete moving the count between the
//! series, the counters over real calls, the age of the oldest message over five holdings, a
//! delay elapsing with no SQS call in between, two queues keeping their own series apart, and
//! the same series read over the query wire a legacy client speaks.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
// The namespace, dimension, metric names and unit under test, read from this service's own
// publication rule so the suite and the publisher cannot disagree about which series is meant.
use service_com_amazonaws_sqs_amazonsqs::state::queue_depth_metrics::{
    APPROXIMATE_AGE_OF_OLDEST_MESSAGE, APPROXIMATE_NUMBER_OF_MESSAGES_DELAYED,
    APPROXIMATE_NUMBER_OF_MESSAGES_NOT_VISIBLE, APPROXIMATE_NUMBER_OF_MESSAGES_VISIBLE,
    BYTES_UNIT, COUNT_UNIT, DEPTH_PUBLISH_INTERVAL, NAMESPACE, NUMBER_OF_EMPTY_RECEIVES,
    NUMBER_OF_MESSAGES_DELETED, NUMBER_OF_MESSAGES_RECEIVED, NUMBER_OF_MESSAGES_SENT,
    QUEUE_NAME_DIMENSION, SECONDS_UNIT, SENT_MESSAGE_SIZE,
};
use service_com_amazonaws_sqs_amazonsqs::state::{queue_name_from_url, ComAmazonawsSqsAmazonsqsState};
use service_com_amazonaws_sqs_amazonsqs::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::support::{create_ok, fifo_attributes, form_encode, messages, ok, xml_element, REGION};

/// The `X-Amz-Target` prefix the CloudWatch model declares.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// The endpoint a CloudWatch client addresses, and the region every call here is made in.
const CLOUDWATCH_HOST: &str = "monitoring.us-east-1.amazonaws.com";

/// How many seconds either side of the calls a metric read spans: a window wide enough that
/// every sample of a test falls inside it whichever minute the clock is in when it runs.
const WINDOW_SECS: f64 = 300.0;

/// The period a read folds that window into: one minute, the resolution these gauges are
/// published at.
const PERIOD_SECS: i64 = 60;

/// How long a test waits for a sample the CLOCK owes, and how often it looks: a delay
/// elapsing is not published by a call, so a reader polls for it the way a client does.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLL_ATTEMPTS: usize = 200;

/// The three series, in the order a row's expected depth states them.
const SERIES: [&str; 3] = [
    APPROXIMATE_NUMBER_OF_MESSAGES_VISIBLE,
    APPROXIMATE_NUMBER_OF_MESSAGES_NOT_VISIBLE,
    APPROXIMATE_NUMBER_OF_MESSAGES_DELAYED,
];

/// One message a row sends: its body, and the `DelaySeconds` the send carries for it.
type Sent = (&'static str, Option<i64>);

/// One row of the table: a case name, the queue it creates, the one batch it sends, and the
/// `(visible, not visible, delayed)` PEAK every series must then report.
struct Row {
    case: &'static str,
    name: &'static str,
    attributes: Value,
    sent: Vec<Sent>,
    expected: (f64, f64, f64),
}

/// The table. Every row sends its messages in ONE call, so the peak each series reports is the
/// depth that call left the queue standing at.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a message sent with no delay is counted as visible and nowhere else",
            name: "depth-ready",
            attributes: json!({}),
            sent: vec![("ready", None)],
            expected: (1.0, 0.0, 0.0),
        },
        Row {
            case: "a message sent with a delay is counted as delayed and nowhere else",
            name: "depth-delayed",
            attributes: json!({}),
            // The longest delay `SendMessageRequest$DelaySeconds` documents: "Valid values:
            // 0 to 900".
            sent: vec![("delayed", Some(900))],
            expected: (0.0, 0.0, 1.0),
        },
        Row {
            case: "a batch carrying one delayed and one ready message splits between the two \
                   series",
            name: "depth-split",
            attributes: json!({}),
            sent: vec![("delayed", Some(120)), ("ready", None)],
            expected: (1.0, 0.0, 1.0),
        },
        Row {
            case: "a delay of zero is no delay at all",
            name: "depth-zero-delay",
            attributes: json!({}),
            sent: vec![("ready", Some(0))],
            expected: (1.0, 0.0, 0.0),
        },
        Row {
            case: "every message of a batch is counted",
            name: "depth-many",
            attributes: json!({}),
            sent: vec![
                ("one", Some(60)),
                ("two", Some(60)),
                ("three", Some(60)),
                ("four", None),
                ("five", None),
            ],
            expected: (2.0, 0.0, 3.0),
        },
        Row {
            case: "a FIFO queue's delay comes from its queue attribute, and is counted the \
                   same way",
            name: "depth-fifo-delayed.fifo",
            attributes: fifo_attributes(&[("DelaySeconds", "300")]),
            sent: vec![("one", None), ("two", None)],
            expected: (0.0, 0.0, 2.0),
        },
        Row {
            case: "a FIFO queue with no delay counts its messages as visible",
            name: "depth-fifo-ready.fifo",
            attributes: fifo_attributes(&[]),
            sent: vec![("one", None), ("two", None)],
            expected: (2.0, 0.0, 0.0),
        },
        Row {
            case: "a queue nothing has been sent to reports zero in all three series",
            name: "depth-untouched",
            attributes: json!({}),
            sent: Vec::new(),
            expected: (0.0, 0.0, 0.0),
        },
    ]
}

#[tokio::test]
async fn every_message_a_queue_holds_is_counted_in_exactly_one_aws_sqs_series() {
    for row in rows() {
        let (router, engine) = emulator_with_cloudwatch();
        let url = create_ok(&router, row.name, &row.attributes).await;
        if !row.sent.is_empty() {
            send_batch(&router, &url, &row.sent).await;
        }
        // The gauges are sampled once a minute, so the reading that carries what the send
        // left behind is the NEXT minute's: the suite reaches it through the engine seam
        // instead of waiting for the clock.
        sample_again(&router, &engine).await;

        let (visible, not_visible, delayed) = row.expected;
        let expected = [Some(visible), Some(not_visible), Some(delayed)];
        let reported = peaks(&router, row.name).await;
        assert_eq!(
            reported, expected,
            "case {}: expected {SERIES:?} to peak at {expected:?} over the window, got \
             {reported:?}",
            row.case
        );

        // A client discovers those series through `ListMetrics`, dimensioned on the queue;
        // one the store does not hold has none of them. Every queue that exists is sampled,
        // so it carries all four gauges; the sent counters appear only for a queue something
        // was really sent to.
        let indexed = indexed_series(&router, row.name).await;
        let mut discoverable: Vec<String> = SERIES
            .iter()
            .chain([&APPROXIMATE_AGE_OF_OLDEST_MESSAGE])
            .map(|name| (*name).to_string())
            .collect();
        if !row.sent.is_empty() {
            discoverable.push(NUMBER_OF_MESSAGES_SENT.to_string());
            discoverable.push(SENT_MESSAGE_SIZE.to_string());
        }
        discoverable.sort();
        assert_eq!(
            indexed, discoverable,
            "case {}: expected ListMetrics to index {discoverable:?} for the queue, got \
             {indexed:?}",
            row.case
        );
        let unknown = indexed_series(&router, "depth-no-such-queue").await;
        assert!(
            unknown.is_empty(),
            "case {}: a queue that does not exist must have no series, got {unknown:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_delivery_moves_the_count_from_the_visible_series_to_the_not_visible_one() {
    let (router, engine) = emulator_with_cloudwatch();
    let name = "depth-delivery";
    let url = create_ok(&router, name, &json!({})).await;
    send_batch(&router, &url, &[("one", None)]).await;
    sample_again(&router, &engine).await;
    assert_eq!(
        peaks(&router, name).await,
        [Some(1.0), Some(0.0), Some(0.0)],
        "case before the delivery: the message must be counted as visible alone"
    );

    let handed = ok(
        &router,
        "ReceiveMessage",
        json!({"QueueUrl": url, "MaxNumberOfMessages": 1}),
    )
    .await;
    let delivered = messages(&handed);
    assert_eq!(
        delivered.len(),
        1,
        "case the delivery: expected one message, got {handed}"
    );
    sample_again(&router, &engine).await;
    let reported = peaks(&router, name).await;
    assert_eq!(
        reported[1],
        Some(1.0),
        "case after the delivery: expected the in-flight series to have reported 1, got \
         {reported:?}"
    );
    let fell = troughs(&router, name).await;
    assert_eq!(
        fell[0],
        Some(0.0),
        "case after the delivery: expected the visible series to have fallen to 0, got \
         {fell:?}"
    );

    let receipt = delivered[0]["ReceiptHandle"]
        .as_str()
        .unwrap_or_else(|| panic!("case the delivery: no ReceiptHandle in {handed}"))
        .to_string();
    ok(
        &router,
        "DeleteMessage",
        json!({"QueueUrl": url, "ReceiptHandle": receipt}),
    )
    .await;
    sample_again(&router, &engine).await;
    let reported = troughs(&router, name).await;
    for (index, series) in SERIES.iter().enumerate() {
        assert_eq!(
            reported[index],
            Some(0.0),
            "case after the delete: expected {series} to have reported an empty queue, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_delay_that_elapses_moves_the_message_into_the_visible_series_with_no_further_call() {
    let (router, engine) = emulator_with_cloudwatch();
    let name = "depth-delay-elapses";
    let url = create_ok(&router, name, &json!({})).await;
    send_batch(&router, &url, &[("delayed", Some(900))]).await;
    sample_again(&router, &engine).await;
    assert_eq!(
        peaks(&router, name).await,
        [Some(0.0), Some(0.0), Some(1.0)],
        "case while the delay is in force: the message must be counted as delayed alone"
    );

    // The delay lapses WITHOUT any SQS call: the instant the message becomes available moves
    // into the past through the engine seam, the next minute's reading is made due the same
    // way, and from here on only CloudWatch is read — so what the polling observes is a
    // sample the CLOCK took.
    make_available(&engine, &url);
    reopen_sample_slot(&engine);
    let moved = poll_for(&router, name, APPROXIMATE_NUMBER_OF_MESSAGES_VISIBLE, 1.0).await;
    assert!(
        moved,
        "case after the delay lapsed: expected the visible series to report 1 with no further \
         SQS call, got {:?}",
        peaks(&router, name).await
    );
    let fell = troughs(&router, name).await;
    assert_eq!(
        fell[2],
        Some(0.0),
        "case after the delay lapsed: expected the delayed series to have fallen to 0, got \
         {fell:?}"
    );

    // The same split the gauges report is the one the queue itself reports, so the metric and
    // the attribute cannot disagree about what the queue holds.
    let attributes = ok(
        &router,
        "GetQueueAttributes",
        json!({"QueueUrl": url, "AttributeNames": ["All"]}),
    )
    .await;
    for (attribute, expected) in [
        ("ApproximateNumberOfMessages", "1"),
        ("ApproximateNumberOfMessagesDelayed", "0"),
    ] {
        assert_eq!(
            attributes["Attributes"][attribute].as_str(),
            Some(expected),
            "case after the delay lapsed: expected {attribute} to be {expected}, got \
             {attributes}"
        );
    }
}

#[tokio::test]
async fn the_data_plane_calls_a_queue_serves_are_counted_into_its_own_counters() {
    let (router, _engine) = emulator_with_cloudwatch();
    let name = "depth-traffic";
    let url = create_ok(&router, name, &json!({})).await;
    // Two messages of different sizes, so the size counter is asserted on a value the
    // bodies decide rather than on one figure.
    let bodies = ["one", "two-is-longer"];
    send_batch(&router, &url, &[(bodies[0], None), (bodies[1], None)]).await;
    let sent_bytes: f64 = bodies.iter().map(|body| body.len() as f64).sum();
    for (metric, expected) in [
        (NUMBER_OF_MESSAGES_SENT, 2.0),
        (SENT_MESSAGE_SIZE, sent_bytes),
    ] {
        let counted = total(&router, name, metric).await;
        assert_eq!(
            counted,
            Some(expected),
            "case after the sends: expected {metric} to total {expected}, got {counted:?}"
        );
    }
    let size_unit = unit_of(&router, name, SENT_MESSAGE_SIZE).await;
    assert_eq!(
        size_unit,
        Some(BYTES_UNIT.to_string()),
        "case after the sends: a message size is measured in bytes, got {size_unit:?}"
    );

    let handed = ok(
        &router,
        "ReceiveMessage",
        json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    let delivered = messages(&handed);
    assert_eq!(
        delivered.len(),
        2,
        "case the delivery: expected both messages, got {handed}"
    );
    let received = total(&router, name, NUMBER_OF_MESSAGES_RECEIVED).await;
    assert_eq!(
        received,
        Some(2.0),
        "case after the delivery: expected both messages counted as received, got {received:?}"
    );

    // Both messages are in flight, so this poll gets nothing: an empty receive, which is
    // counted as one whatever the queue holds.
    ok(
        &router,
        "ReceiveMessage",
        json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    let empty = total(&router, name, NUMBER_OF_EMPTY_RECEIVES).await;
    assert_eq!(
        empty,
        Some(1.0),
        "case after the empty poll: expected one empty receive, got {empty:?}"
    );

    let receipts: Vec<Value> = delivered
        .iter()
        .enumerate()
        .map(|(index, message)| {
            json!({"Id": format!("d{index}"), "ReceiptHandle": message["ReceiptHandle"]})
        })
        .collect();
    ok(
        &router,
        "DeleteMessageBatch",
        json!({"QueueUrl": url, "Entries": receipts}),
    )
    .await;
    let deleted = total(&router, name, NUMBER_OF_MESSAGES_DELETED).await;
    assert_eq!(
        deleted,
        Some(2.0),
        "case after the deletes: expected both messages counted as deleted, got {deleted:?}"
    );
    // The counters count real calls, so a period nothing called in has no datapoint: the
    // queue nothing was sent to has no sent series at all.
    let untouched = create_ok(&router, "depth-traffic-idle", &json!({})).await;
    assert!(untouched.ends_with("depth-traffic-idle"));
    let unsent = total(&router, "depth-traffic-idle", NUMBER_OF_MESSAGES_SENT).await;
    assert_eq!(
        unsent, None,
        "case the idle queue: a queue nothing was sent to counts no send, got {unsent:?}"
    );
}

#[tokio::test]
async fn a_queue_reports_how_long_the_oldest_message_it_can_deliver_has_been_in_it() {
    /// What a row does to the one message it sends before the age is read.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Holding {
        /// Left where the send put it: available for retrieval.
        Available,
        /// Still inside its `DelaySeconds`, so the queue cannot hand it over at all.
        Delayed,
        /// Handed to a consumer and not deleted, so the queue is still waiting to hear
        /// about it.
        InFlight,
    }

    // (case, how long ago the queue enqueued the message, what is holding it, the age the
    // gauge must then report in seconds)
    let rows = [
        (
            "a message just sent",
            Duration::from_secs(0),
            Holding::Available,
            0.0,
        ),
        (
            "a message five minutes old",
            Duration::from_secs(300),
            Holding::Available,
            300.0,
        ),
        (
            "a message a day old",
            Duration::from_secs(86_400),
            Holding::Available,
            86_400.0,
        ),
        (
            "a message inside its delay, however long it has been there",
            Duration::from_secs(600),
            Holding::Delayed,
            0.0,
        ),
        (
            "a message in flight with a consumer",
            Duration::from_secs(600),
            Holding::InFlight,
            600.0,
        ),
    ];
    for (case, age, holding, expected) in rows {
        let (router, engine) = emulator_with_cloudwatch();
        let name = "depth-oldest-message";
        let url = create_ok(&router, name, &json!({})).await;
        let delay = (holding == Holding::Delayed).then_some(900);
        send_batch(&router, &url, &[("one", delay)]).await;
        if holding == Holding::InFlight {
            let handed = ok(
                &router,
                "ReceiveMessage",
                json!({"QueueUrl": url, "MaxNumberOfMessages": 1}),
            )
            .await;
            assert_eq!(
                messages(&handed).len(),
                1,
                "case {case:?}: expected the message to be handed over, got {handed}"
            );
        }
        enqueued_earlier(&engine, &url, age);
        sample_again(&router, &engine).await;

        let reported = bounds(&router, name, APPROXIMATE_AGE_OF_OLDEST_MESSAGE)
            .await
            .map(|(_, largest)| largest);
        assert_eq!(
            reported,
            Some(expected),
            "case {case:?}: expected the age of the oldest message to peak at {expected} \
             seconds, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn each_queue_publishes_its_own_series_under_its_own_name() {
    let (router, engine) = emulator_with_cloudwatch();
    let ready = create_ok(&router, "depth-tenant-one", &json!({})).await;
    let delayed = create_ok(&router, "depth-tenant-two", &json!({})).await;
    send_batch(&router, &ready, &[("one", None)]).await;
    send_batch(&router, &delayed, &[("one", Some(600)), ("two", Some(600))]).await;
    sample_again(&router, &engine).await;

    assert_eq!(
        peaks(&router, "depth-tenant-one").await,
        [Some(1.0), Some(0.0), Some(0.0)],
        "case the first queue: its own depth alone"
    );
    assert_eq!(
        peaks(&router, "depth-tenant-two").await,
        [Some(0.0), Some(0.0), Some(2.0)],
        "case the second queue: its own depth alone"
    );

    // The queue name is part of the series' identity, so a read that names no dimension, and
    // one that names a queue nothing published for, select no datapoint at all.
    for dimensions in [
        json!([]),
        json!([{"Name": QUEUE_NAME_DIMENSION, "Value": "depth-tenant-three"}]),
    ] {
        let points = read_series(
            &router,
            APPROXIMATE_NUMBER_OF_MESSAGES_VISIBLE,
            dimensions.clone(),
        )
        .await;
        assert!(
            points.is_empty(),
            "case the dimension set {dimensions}: expected no datapoint, got {points:?}"
        );
    }
}

#[tokio::test]
async fn the_series_reads_the_same_over_the_query_wire_a_legacy_client_speaks() {
    let (router, engine) = emulator_with_cloudwatch();
    let name = "depth-query-form";
    let url = create_ok(&router, name, &json!({})).await;
    send_batch(&router, &url, &[("delayed", Some(900))]).await;
    sample_again(&router, &engine).await;

    let now = now_secs();
    let start = iso8601(now - WINDOW_SECS);
    let end = iso8601(now + WINDOW_SECS);
    let form = [
        ("Action", "GetMetricStatistics".to_string()),
        ("Version", "2010-08-01".to_string()),
        ("Namespace", NAMESPACE.to_string()),
        (
            "MetricName",
            APPROXIMATE_NUMBER_OF_MESSAGES_DELAYED.to_string(),
        ),
        ("Dimensions.member.1.Name", QUEUE_NAME_DIMENSION.to_string()),
        ("Dimensions.member.1.Value", name.to_string()),
        ("StartTime", start),
        ("EndTime", end),
        ("Period", PERIOD_SECS.to_string()),
        ("Statistics.member.1", "Maximum".to_string()),
    ]
    .iter()
    .map(|(member, value)| format!("{member}={}", form_encode(value)))
    .collect::<Vec<String>>()
    .join("&");

    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", CLOUDWATCH_HOST)
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/monitoring/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(form.into_bytes()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case the query form: expected HTTP 200, got {status} with {body}"
    );
    assert_eq!(
        xml_element(&body, "Maximum"),
        Some("1".to_string()),
        "case the query form: expected the delayed series to report the one delayed message, \
         got {body}"
    );
}

/// Take the next minute's reading of every queue this store holds, and answer once it is in
/// the metric store.
///
/// The gauges are sampled once a minute, so a suite that drives several depths in one minute
/// would otherwise read only the first of them. The next reading is made DUE through the
/// engine seam — the recorded sample moves a minute into the past, exactly as the
/// clock-driven suites move a message's instants — and then one SQS call takes it, which is
/// what every request of this service does on its way out.
async fn sample_again(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) {
    reopen_sample_slot(engine);
    ok(router, "ListQueues", json!({})).await;
}

/// Move the last reading recorded for every queue a minute into the past, so the next look at
/// the store takes a fresh one.
fn reopen_sample_slot(engine: &engine::InMemoryEngine) {
    let store = engine.state::<ComAmazonawsSqsAmazonsqsState>();
    let mut store = store.lock();
    for sampled in store.last_sampled_at.values_mut() {
        *sampled = sampled
            .checked_sub(DEPTH_PUBLISH_INTERVAL)
            .unwrap_or(*sampled);
    }
}

/// The emulator under test: SQS and CloudWatch mounted on ONE engine, which is what makes the
/// metric store the depth samples are published into the same one CloudWatch reads.
///
/// Assembled here rather than in [`super::support`] because mounting CloudWatch beside this
/// service is this suite's own concern; the engine is handed back so the suite can reach the
/// clock's bookkeeping ([`reopen_sample_slot`]) and a message's instants.
fn emulator_with_cloudwatch() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// Send `sent` to the queue at `url` as ONE `SendMessageBatch` call, so the depth the queue is
/// left standing at is reached by a single change.
async fn send_batch(router: &MultiServiceRouter, url: &str, sent: &[Sent]) {
    let fifo = url.ends_with(".fifo");
    let entries: Vec<Value> = sent
        .iter()
        .enumerate()
        .map(|(index, (body, delay))| {
            let mut entry = json!({"Id": format!("e{index}"), "MessageBody": *body});
            if let Some(delay) = delay {
                entry["DelaySeconds"] = json!(delay);
            }
            if fifo {
                // A FIFO send carries the group it is ordered in and the token it is
                // deduplicated by; the delay of a FIFO queue is the queue's own attribute.
                entry["MessageGroupId"] = json!("group");
                entry["MessageDeduplicationId"] = json!(format!("d{index}"));
            }
            entry
        })
        .collect();
    let answered = ok(
        router,
        "SendMessageBatch",
        json!({"QueueUrl": url, "Entries": entries}),
    )
    .await;
    let failed = answered.get("Failed").and_then(Value::as_array);
    assert!(
        failed.is_none_or(|entries| entries.is_empty()),
        "case sending {sent:?}: expected every entry to be accepted, got {answered}"
    );
}

/// Make every message the queue at `url` holds available now: the instant its delay would
/// elapse at moves into the past, which is the transition the CLOCK owes without any call.
fn make_available(engine: &engine::InMemoryEngine, url: &str) {
    let name = queue_name_from_url(url)
        .unwrap_or_else(|| panic!("case making {url} available: the url names no queue"))
        .to_string();
    let store = engine.state::<ComAmazonawsSqsAmazonsqsState>();
    let mut store = store.lock();
    let queue = store
        .queue_mut(REGION, &name)
        .unwrap_or_else(|| panic!("case making {url} available: no queue named {name}"));
    for message in &mut queue.messages {
        message.available_at = message
            .available_at
            .checked_sub(Duration::from_secs(1_800))
            .unwrap_or(message.available_at);
    }
}

/// Make every message the queue at `url` holds as if the queue had enqueued it `ago`: the
/// instant its retention runs from, which is the instant the age of the oldest message is
/// counted from.
fn enqueued_earlier(engine: &engine::InMemoryEngine, url: &str, ago: Duration) {
    let name = queue_name_from_url(url)
        .unwrap_or_else(|| panic!("case ageing {url}: the url names no queue"))
        .to_string();
    let store = engine.state::<ComAmazonawsSqsAmazonsqsState>();
    let mut store = store.lock();
    let queue = store
        .queue_mut(REGION, &name)
        .unwrap_or_else(|| panic!("case ageing {url}: no queue named {name}"));
    for message in &mut queue.messages {
        message.retention_from = message
            .retention_from
            .checked_sub(ago)
            .unwrap_or(message.retention_from);
    }
}

/// The PEAK each of the three series reported for `queue` over the whole window, in [`SERIES`]
/// order — the largest count it ever carried — or `None` for a series holding no datapoint at
/// all.
///
/// The peak is what a client watching a backlog reads, and what makes exclusivity assertable:
/// a series whose peak is zero never counted the message at any instant of the window.
async fn peaks(router: &MultiServiceRouter, queue: &str) -> [Option<f64>; 3] {
    let mut reported = [None; 3];
    for (index, metric) in SERIES.iter().enumerate() {
        reported[index] = bounds(router, queue, metric).await.map(|(_, largest)| largest);
    }
    reported
}

/// The same, for the smallest count each series carried: what tells a count that FELL back
/// from one that never moved.
async fn troughs(router: &MultiServiceRouter, queue: &str) -> [Option<f64>; 3] {
    let mut reported = [None; 3];
    for (index, metric) in SERIES.iter().enumerate() {
        reported[index] = bounds(router, queue, metric)
            .await
            .map(|(smallest, _)| smallest);
    }
    reported
}

/// The smallest and largest value `metric` reports for `queue` across the window, or `None`
/// when the series holds no datapoint. Every datapoint it does hold has to carry the unit that
/// measurement is published in.
async fn bounds(router: &MultiServiceRouter, queue: &str, metric: &str) -> Option<(f64, f64)> {
    let points = read_series(
        router,
        metric,
        json!([{"Name": QUEUE_NAME_DIMENSION, "Value": queue}]),
    )
    .await;
    let unit = expected_unit(metric);
    for point in &points {
        assert_eq!(
            point.get("Unit").and_then(Value::as_str),
            Some(unit),
            "case {metric} of {queue}: expected every datapoint in {unit}, got {point}"
        );
    }
    let smallest = points
        .iter()
        .filter_map(|point| point.get("Minimum").and_then(Value::as_f64))
        .fold(f64::INFINITY, f64::min);
    let largest = points
        .iter()
        .filter_map(|point| point.get("Maximum").and_then(Value::as_f64))
        .fold(f64::NEG_INFINITY, f64::max);
    (smallest.is_finite() && largest.is_finite()).then_some((smallest, largest))
}

/// The unit a series is published in: a message size in bytes, the age of a message in
/// seconds, and every other series — a number of messages or of calls — as a count.
fn expected_unit(metric: &str) -> &'static str {
    match metric {
        SENT_MESSAGE_SIZE => BYTES_UNIT,
        APPROXIMATE_AGE_OF_OLDEST_MESSAGE => SECONDS_UNIT,
        _ => COUNT_UNIT,
    }
}

/// What `metric` totals for `queue` across the window — the `Sum` of every datapoint it holds
/// — or `None` when the series holds none, which is what a counter nothing contributed to
/// reports.
async fn total(router: &MultiServiceRouter, queue: &str, metric: &str) -> Option<f64> {
    let points = read_series(
        router,
        metric,
        json!([{"Name": QUEUE_NAME_DIMENSION, "Value": queue}]),
    )
    .await;
    if points.is_empty() {
        return None;
    }
    Some(
        points
            .iter()
            .filter_map(|point| point.get("Sum").and_then(Value::as_f64))
            .sum(),
    )
}

/// The unit `metric`'s datapoints carry for `queue`, as every one of them states it.
async fn unit_of(router: &MultiServiceRouter, queue: &str, metric: &str) -> Option<String> {
    let points = read_series(
        router,
        metric,
        json!([{"Name": QUEUE_NAME_DIMENSION, "Value": queue}]),
    )
    .await;
    points
        .first()
        .and_then(|point| point.get("Unit").and_then(Value::as_str))
        .map(ToString::to_string)
}

/// `GetMetricStatistics` over one `AWS/SQS` series across the window around the calls: its
/// datapoints, each carrying the statistics asked for and the unit they were published in.
async fn read_series(
    router: &MultiServiceRouter,
    metric: &str,
    dimensions: Value,
) -> Vec<Value> {
    let now = now_secs();
    let answered = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": dimensions,
            "StartTime": now - WINDOW_SECS,
            "EndTime": now + WINDOW_SECS,
            "Period": PERIOD_SECS,
            "Statistics": ["Minimum", "Maximum", "Sum", "SampleCount"],
        }),
    )
    .await;
    answered
        .get("Datapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `AWS/SQS` metric names `ListMetrics` indexes for `queue`, sorted: which series a client
/// discovering the queue's backlog finds.
async fn indexed_series(router: &MultiServiceRouter, queue: &str) -> Vec<String> {
    let listed = cloudwatch(
        router,
        "ListMetrics",
        &json!({
            "Namespace": NAMESPACE,
            "Dimensions": [{"Name": QUEUE_NAME_DIMENSION, "Value": queue}],
        }),
    )
    .await;
    let mut names: Vec<String> = listed
        .get("Metrics")
        .and_then(Value::as_array)
        .map(|metrics| {
            metrics
                .iter()
                .filter_map(|metric| {
                    metric
                        .get("MetricName")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Poll `metric` until the largest value it reports for `queue` reaches `wanted`, answering
/// whether it did inside the budget.
///
/// The way a client watches a queue's depth: nothing but CloudWatch is called, so what the
/// polling observes is what the clock published.
async fn poll_for(
    router: &MultiServiceRouter,
    queue: &str,
    metric: &str,
    wanted: f64,
) -> bool {
    for _ in 0..POLL_ATTEMPTS {
        if bounds(router, queue, metric)
            .await
            .is_some_and(|(_, largest)| largest >= wanted)
        {
            return true;
        }
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
    false
}

/// One `awsJson1_0` CloudWatch call, requiring a 200 — the protocol and target prefix that
/// service serves.
async fn cloudwatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", CLOUDWATCH_HOST)
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/monitoring/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.body().to_vec();
    let parsed: Value = serde_json::from_slice(&payload).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// Wall-clock now as epoch seconds, the clock a metric window is measured against.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// `secs` as the ISO-8601 instant the query wire carries a timestamp as.
fn iso8601(secs: f64) -> String {
    ::aws_smithy_types::DateTime::from_secs_f64(secs)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an instant the date-time format can render")
}
