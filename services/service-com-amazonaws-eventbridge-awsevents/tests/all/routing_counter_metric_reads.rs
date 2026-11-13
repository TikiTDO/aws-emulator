// SPDX-License-Identifier: CC0-1.0

//! A matched rule's AWS/Events routing counters are READABLE through CloudWatch's
//! metric reads — repeatedly, at every granularity a just-published series is
//! served at, and under the unit the counters are published in — end to end over
//! the wire with CloudWatch mounted beside EventBridge on one engine.
//!
//! The RULE asserted here: CloudWatch serves the datapoints a service appended to
//! its own namespace on the same terms as any other stored series.
//! `GetMetricStatistics` aggregates the series its (namespace, metric name,
//! dimension set) names into the period windows the request asks for, at every
//! granularity that series is still readable at, reporting the unit the
//! datapoints were stored with and selecting on the request's `Unit` when it names
//! one ("If you specify a unit, the operation returns only data that was collected
//! with that unit specified. […] If you specify a unit that does not match the
//! data collected, the results of the operation are null",
//! `GetMetricStatisticsInput$Unit`); `ListMetrics` enumerates exactly the
//! dimension sets datapoints were published under, narrowed to the series that
//! carry a `DimensionFilter`'s dimension. Both are REPORTING operations: repeating
//! either answers the same values, and a dimension set nothing was published under
//! stays absent however often it is asked for ("If a specific combination of
//! dimensions was not published, you can't retrieve statistics for it",
//! `GetMetricStatisticsInput$Dimensions`).
//!
//! The tables span the class rather than the one recorded reading: the rows differ
//! along the BUS the rules live on and HOW MANY matched events the round publishes
//! (so a count is a count rather than a flag) and whether the rule carries a
//! target at all; along the PERIOD the series is read at (four multiples of a
//! minute, of which the recorded reading used one); along the `Unit` the read
//! names (none, the one the counters carry, and three the model declares that they
//! do not); along the DIMENSION SET the read names (the published one, three
//! looser or other ones, and one that is the published set plus a third
//! dimension); and along the PASS, the same reads being taken three times over so
//! that a read that created, consumed or shifted anything fails a later pass.
//!
//! The harness (the router with EventBridge, SQS and CloudWatch on one engine) is
//! the one `target_dispatch` owns.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    EVENT_BUS_NAME_DIMENSION, INVOCATIONS_METRIC, MATCHED_EVENTS_METRIC, METRIC_NAMESPACE,
    METRIC_UNIT_COUNT, RULE_NAME_DIMENSION, SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC,
    TRIGGERED_RULES_METRIC,
};

use crate::target_dispatch::{
    bus_dimensions, cloudwatch, drain, emulator, events, given_queue, ordered, rule_dimensions,
    rule_name_dimensions, rule_series, series_filtered_by, Emulator, DETAIL_TYPE, SOURCE,
};

/// The rule the published events match, and the one on the same bus they do not.
const MATCHING_RULE: &str = "hit";
const SIBLING_RULE: &str = "miss";

/// The source the sibling rule pins, which nothing published below carries.
const OTHER_SOURCE: &str = "com.demo.nomatch";

/// The two counters a pattern match is accounted on, which are the series this
/// suite reads.
const ROUTING_METRICS: [&str; 2] = [MATCHED_EVENTS_METRIC, TRIGGERED_RULES_METRIC];

/// A dimension a reader may name that EventBridge publishes no per-rule routing
/// counter under: the name of an event SOURCE, which is a property of what was
/// published rather than of the bus's routing table. Stated here as the literal a
/// client would send, so it names a series this store holds nothing for.
const EVENT_SOURCE_NAME_DIMENSION: &str = "EventSourceName";

/// How far back and how far ahead of now a read's window reaches, in seconds: wide
/// enough that a just-published datapoint is inside it whichever minute it landed
/// in, and reaching past now the way a client watching a live counter reads it.
const WINDOW_BEFORE_SECS: f64 = 1_200.0;
const WINDOW_AFTER_SECS: f64 = 300.0;

/// How long a read is retried for before its absence is taken as an answer, and
/// how long one wait between two reads is. Generous: the deadline is only there so
/// a counter that never arrives fails an assertion instead of hanging, and nothing
/// below asserts on how long anything took.
const READ_BUDGET: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The periods a just-published regular-resolution series is read at here. "For
/// metrics with regular resolution, a period can be as short as one minute (60
/// seconds) and must be a multiple of 60." (`GetMetricStatisticsInput$Period`);
/// the three the retention tiers are stated in are 60, 300 and 3600
/// (`GetMetricStatistics`), and 120 is a multiple of a minute none of those names.
const PERIODS: [i64; 4] = [60, 120, 300, 3_600];

/// The `Unit` values a read may name, with whether a series published in `Count`
/// answers it. The names are the model's own `StandardUnit` values
/// (`smithy_query(query="shape",shape="StandardUnit")`): the unit these counters
/// carry, and three the model declares that they do not — one of them `None`, the
/// value that stands for a measurement whose publisher named no unit.
const UNIT_PROBES: [(Option<&str>, bool); 5] = [
    (None, true),
    (Some("Count"), true),
    (Some("Count/Second"), false),
    (Some("Bytes"), false),
    (Some("None"), false),
];

/// Seconds since the epoch, as a read's `StartTime` / `EndTime` carry it.
fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// One row: the bus the two rules live on, how many matching events the round
/// publishes, and whether the rules carry a queue target.
struct Row {
    case: &'static str,
    bus: &'static str,
    events: usize,
    targeted: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a custom bus, one matched event, a queue target",
            bus: "probe-bus",
            events: 1,
            targeted: true,
        },
        Row {
            case: "a custom bus whose name carries a dot, three matched events, no target",
            bus: "probe.bus-2",
            events: 3,
            targeted: false,
        },
    ]
}

/// Create the row's custom bus.
async fn given_bus(emulator: &Emulator, case: &str, bus: &str) {
    let (status, created) = events(emulator, "CreateEventBus", &json!({ "Name": bus })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateEventBus answered {status} {created}"
    );
}

/// A rule called `name` on `bus` pinning `source`, carrying `target_arn` as its one
/// target when the row asks for one.
async fn given_rule(
    emulator: &Emulator,
    case: &str,
    bus: &str,
    name: &str,
    source: &str,
    target_arn: Option<&str>,
) {
    let pattern = json!({ "source": [source], "detail-type": [DETAIL_TYPE] }).to_string();
    let (status, body) = events(
        emulator,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": bus,
            "EventPattern": pattern,
            "State": "ENABLED",
        }),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: PutRule answered {status} {body}");
    let Some(target_arn) = target_arn else {
        return;
    };
    let (status, body) = events(
        emulator,
        "PutTargets",
        &json!({
            "Rule": name,
            "EventBusName": bus,
            "Targets": [{ "Id": "t1", "Arn": target_arn }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutTargets answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutTargets reported a failed entry: {body}"
    );
}

/// Publish one matching event onto `bus` and answer the `EventId` it was given.
async fn publish_onto(emulator: &Emulator, case: &str, bus: &str, nonce: &str) -> String {
    let (status, body) = events(
        emulator,
        "PutEvents",
        &json!({
            "Entries": [{
                "EventBusName": bus,
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": json!({ "nonce": nonce }).to_string(),
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutEvents answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutEvents reported a failed entry: {body}"
    );
    body["Entries"][0]["EventId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no EventId in {body}"))
        .to_string()
}

/// Everything the queue at `url` holds once it holds `want` messages, so a delivery
/// still in flight is read again rather than read as missing.
async fn drain_until(emulator: &Emulator, url: &str, want: usize) -> Vec<Value> {
    let deadline = Instant::now() + READ_BUDGET;
    let mut held = Vec::new();
    loop {
        held.extend(drain(emulator, url).await);
        if held.len() >= want || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The datapoints of one `GetMetricStatistics` read of the series `dimensions`
/// names in EventBridge's namespace, asking for every basic statistic a count is
/// read by.
async fn read_datapoints(
    emulator: &Emulator,
    case: &str,
    metric: &str,
    dimensions: &Value,
    period: i64,
    unit: Option<&str>,
) -> Vec<Value> {
    let now = epoch_seconds();
    let mut request = json!({
        "Namespace": METRIC_NAMESPACE,
        "MetricName": metric,
        "Dimensions": dimensions,
        "StartTime": now - WINDOW_BEFORE_SECS,
        "EndTime": now + WINDOW_AFTER_SECS,
        "Period": period,
        "Statistics": ["Sum", "SampleCount", "Minimum", "Maximum", "Average"],
    });
    if let (Some(unit), Value::Object(members)) = (unit, &mut request) {
        members.insert("Unit".to_string(), json!(unit));
    }
    let (status, stats) = cloudwatch(emulator, "GetMetricStatistics", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricStatistics answered {status} {stats}"
    );
    assert_eq!(
        stats["Label"],
        json!(metric),
        "case {case:?}: expected the read to be labelled with the metric, got {stats}"
    );
    stats["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// The total of the statistic `statistic` over `points`.
fn total(points: &[Value], statistic: &str) -> f64 {
    points
        .iter()
        .filter_map(|point| point[statistic].as_f64())
        .sum()
}

/// The datapoints of `metric` for the matched rule's own series once they account
/// for `want` matches, so a counter still being written is read again rather than
/// read as missing.
async fn settled_datapoints(
    emulator: &Emulator,
    case: &str,
    metric: &str,
    dimensions: &Value,
    want: f64,
) -> Vec<Value> {
    let deadline = Instant::now() + READ_BUDGET;
    loop {
        let points = read_datapoints(emulator, case, metric, dimensions, 60, None).await;
        if total(&points, "Sum") >= want || Instant::now() >= deadline {
            return points;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Assert every reading of the row's series, once. Called repeatedly with a
/// different `pass`: a reporting operation answers the same thing every time, so a
/// read that created, consumed or shifted anything fails a later pass.
async fn assert_reads(emulator: &Emulator, row: &Row, pass: u32, matched: f64) {
    let case = format!("{} (pass {pass})", row.case);
    let case = case.as_str();
    let published = rule_dimensions(row.bus, MATCHING_RULE);

    for metric in ROUTING_METRICS {
        // The series the counter IS published under, read at every granularity a
        // just-published regular-resolution series is served at: one occurrence
        // per matched event, each of value 1, in `Count`.
        for period in PERIODS {
            for (unit, served) in UNIT_PROBES {
                let named = unit.unwrap_or("no unit");
                let points =
                    read_datapoints(emulator, case, metric, &published, period, unit).await;
                if !served {
                    assert!(
                        points.is_empty(),
                        "case {case:?}: expected {metric} read at period {period}s under \
                         {named} to answer with no datapoint at all, got {points:?}"
                    );
                    continue;
                }
                let summed = total(&points, "Sum");
                let occurrences = total(&points, "SampleCount");
                assert_eq!(
                    (summed, occurrences),
                    (matched, matched),
                    "case {case:?}: expected {metric} read at period {period}s under {named} \
                     to hold {matched} occurrence(s) summing to {matched}, got {points:?}"
                );
                for point in &points {
                    assert_eq!(
                        point["Unit"],
                        json!(METRIC_UNIT_COUNT),
                        "case {case:?}: expected every {metric} datapoint at period \
                         {period}s under {named} in {METRIC_UNIT_COUNT}, got {point}"
                    );
                    for statistic in ["Minimum", "Maximum", "Average"] {
                        let got = point[statistic].as_f64();
                        assert_eq!(
                            got,
                            Some(1.0),
                            "case {case:?}: expected every {metric} datapoint at period \
                             {period}s under {named} to report {statistic} 1, got {got:?} \
                             in {point}"
                        );
                    }
                }
            }
        }

        // And every other dimension set a client may name: a series nothing was
        // published under answers with no datapoint at all rather than a zero one.
        let absent: [(&str, Value); 5] = [
            ("the bus with no rule", bus_dimensions(row.bus)),
            ("the rule name with no bus", rule_name_dimensions(MATCHING_RULE)),
            (
                "the whole identity of the rule the events did not match",
                rule_dimensions(row.bus, SIBLING_RULE),
            ),
            (
                "the name of the event's source",
                json!([{ "Name": EVENT_SOURCE_NAME_DIMENSION, "Value": SOURCE }]),
            ),
            (
                "the published set plus a third dimension",
                json!([
                    { "Name": EVENT_BUS_NAME_DIMENSION, "Value": row.bus },
                    { "Name": RULE_NAME_DIMENSION, "Value": MATCHING_RULE },
                    { "Name": EVENT_SOURCE_NAME_DIMENSION, "Value": SOURCE },
                ]),
            ),
        ];
        for (dimensioned, dimensions) in absent {
            let points = read_datapoints(emulator, case, metric, &dimensions, 60, None).await;
            assert!(
                points.is_empty(),
                "case {case:?}: expected {metric} read under {dimensioned} to answer with no \
                 datapoint at all, got {points:?}"
            );
        }
    }

    // The series CloudWatch holds in the namespace: the matched rule's counters,
    // each under the dimension set(s) that counter is published under, and nothing
    // for the rule that matched nothing.
    let mut metrics = ROUTING_METRICS.to_vec();
    if row.targeted {
        metrics.push(INVOCATIONS_METRIC);
        metrics.push(SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC);
    }
    let expected = rule_series(row.bus, MATCHING_RULE, &metrics);
    let (status, listed) = cloudwatch(
        emulator,
        "ListMetrics",
        &json!({ "Namespace": METRIC_NAMESPACE }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListMetrics answered {status} {listed}"
    );
    let series = listed["Metrics"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        ordered(&series),
        ordered(&expected),
        "case {case:?}: expected one series per dimension set of each counter the matched \
         rule reports, got {series:?}"
    );

    // The same enumeration narrowed by a `DimensionFilter`: the filter answers the
    // series carrying that dimension, so naming the BUS leaves out the sets that do
    // not carry it, and naming a dimension no series carries answers nothing.
    let by_bus: Vec<Value> = metrics
        .iter()
        .map(|metric| {
            json!({
                "Namespace": METRIC_NAMESPACE,
                "MetricName": metric,
                "Dimensions": rule_dimensions(row.bus, MATCHING_RULE),
            })
        })
        .collect();
    let filtered = series_filtered_by(emulator, EVENT_BUS_NAME_DIMENSION, row.bus).await;
    assert_eq!(
        ordered(&filtered),
        ordered(&by_bus),
        "case {case:?}: expected the {EVENT_BUS_NAME_DIMENSION} filter to answer the series \
         that carry it, got {filtered:?}"
    );
    let filtered = series_filtered_by(emulator, EVENT_SOURCE_NAME_DIMENSION, SOURCE).await;
    assert!(
        filtered.is_empty(),
        "case {case:?}: expected the {EVENT_SOURCE_NAME_DIMENSION} filter to answer no series \
         at all, got {filtered:?}"
    );

    // And the same enumeration narrowed to the series that have reported recently:
    // "To filter the results to show only metrics that have had data points
    // published in the past three hours, specify this parameter with a value of
    // PT3H." (`ListMetricsInput$RecentlyActive`) — every series here was published
    // in this test's own run, so the filter leaves the set as it is.
    let (status, listed) = cloudwatch(
        emulator,
        "ListMetrics",
        &json!({ "Namespace": METRIC_NAMESPACE, "RecentlyActive": "PT3H" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListMetrics answered {status} {listed}"
    );
    let recent = listed["Metrics"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        ordered(&recent),
        ordered(&expected),
        "case {case:?}: expected the recently-active filter to answer every series just \
         published, got {recent:?}"
    );
}

/// How many times every reading is taken: more than once, so a read that is not a
/// pure report of the store fails a later pass.
const PASSES: u32 = 3;

#[tokio::test]
async fn a_matched_rules_routing_counters_are_repeatably_readable() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();
        given_bus(&emulator, case, row.bus).await;
        let target = if row.targeted {
            Some(given_queue(&emulator, "deliveries").await)
        } else {
            None
        };
        let target_arn = target.as_ref().map(|(_, arn)| arn.as_str());
        given_rule(&emulator, case, row.bus, MATCHING_RULE, SOURCE, target_arn).await;
        given_rule(
            &emulator,
            case,
            row.bus,
            SIBLING_RULE,
            OTHER_SOURCE,
            target_arn,
        )
        .await;

        let mut ids = Vec::new();
        for round in 0..row.events {
            ids.push(publish_onto(&emulator, case, row.bus, &format!("n-{round}")).await);
        }

        // The delivery the match caused, read where a consumer reads it: one
        // message per published event, from the matched rule's target only, each
        // carrying the source that matched and the id its publication returned.
        if let Some((url, _)) = &target {
            let delivered = drain_until(&emulator, url, row.events).await;
            assert_eq!(
                delivered.len(),
                row.events,
                "case {case:?}: expected the matched rule's target to hold every event, \
                 got {delivered:?}"
            );
            let mut envelopes: Vec<(String, String)> = delivered
                .iter()
                .map(|message| {
                    let body = message["Body"].as_str().unwrap_or_default();
                    let envelope: Value = serde_json::from_str(body).unwrap_or_else(|error| {
                        panic!("case {case:?}: a delivered body is not JSON ({error}): {body}")
                    });
                    (
                        envelope["source"].as_str().unwrap_or_default().to_string(),
                        envelope["id"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            envelopes.sort();
            let mut expected: Vec<(String, String)> = ids
                .iter()
                .map(|id| (SOURCE.to_string(), id.clone()))
                .collect();
            expected.sort();
            assert_eq!(
                envelopes, expected,
                "case {case:?}: expected the delivered envelopes to name the matched source \
                 and the ids the publications returned, got {envelopes:?}"
            );
        }

        let matched = f64::from(u32::try_from(row.events).unwrap_or(u32::MAX));
        // Wait for the accounting of the round to be complete before the readings
        // begin, so an incomplete count is read again rather than asserted on.
        for metric in ROUTING_METRICS {
            let points = settled_datapoints(
                &emulator,
                case,
                metric,
                &rule_dimensions(row.bus, MATCHING_RULE),
                matched,
            )
            .await;
            let summed = total(&points, "Sum");
            assert_eq!(
                summed, matched,
                "case {case:?}: expected {metric} for the matched rule to account for \
                 {matched} match(es), got {summed} in {points:?}"
            );
        }

        for pass in 1..=PASSES {
            assert_reads(&emulator, &row, pass, matched).await;
        }
    }
}
