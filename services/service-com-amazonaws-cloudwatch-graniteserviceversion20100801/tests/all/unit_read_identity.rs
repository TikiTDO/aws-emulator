// SPDX-License-Identifier: CC0-1.0

//! A datapoint's UNIT, and what each metric read does with it, driven over the wire the
//! service speaks through the shared `harness`.
//!
//! Two rules, each asserted over a TABLE whose rows differ along what the rule generalises
//! over rather than along the one input any recording happens to carry.
//!
//! 1. The unit is part of a datapoint's READ identity and not of the metric CATALOGUE's: "If
//!    you omit Unit, all data that was collected with any unit is returned, along with the
//!    corresponding units that were specified when the data was reported to CloudWatch. If you
//!    specify a unit, the operation returns only data that was collected with that unit
//!    specified. If you specify a unit that does not match the data collected, the results of
//!    the operation are null. CloudWatch does not perform unit conversions."
//!    (`GetMetricStatisticsInput$Unit`, in the same words on `GetMetricDataInput`) — so both
//!    read paths select by the unit a request names, a unit nothing was published with is an
//!    empty SUCCESS, and `ListMetrics` still reports one metric however many units were
//!    published under it ("a metric is uniquely defined by a name, a namespace, and zero or
//!    more dimensions" — no unit).
//! 2. A value outside the set the enumerated type its member targets declares is refused by
//!    the validator of the OPERATION it arrived on: the read whose model declares this
//!    service's parameter faults answers `InvalidParameterValueException` naming the parameter
//!    and the
//!    whole accepted set as the service announces it, and the read whose model declares none of them keeps this service's
//!    own request-validation class with the framework's counting clause naming the member path
//!    and that
//!    set. (`GetMetricStatistics` models `InvalidParameterValueException`,
//!    `InvalidParameterCombinationException`, `MissingRequiredParameterException` and
//!    `InternalServiceFault`; `GetMetricData` models `InvalidNextToken` alone.)

use serde_json::{json, Value};

use crate::harness::{build_router, call, error_code, error_message, ok};

/// The namespace every series in this suite is published under, per row.
const NAMESPACE: &str = "MyApp/UnitIdentity";

/// The dimension set every series in this suite carries: one dimension, so the metric's
/// catalogue identity is a single name-and-dimension pair whatever units ride on it.
fn dimensions() -> Value {
    json!([{"Name": "Probe", "Value": "unit-identity"}])
}

/// The values `StandardUnit` declares, in the order the model declares them
/// (`smithy_query(query="shape",shape="StandardUnit")`): the 27 units a datapoint may be
/// published with, which is also the set a refusal of a value outside it enumerates.
///
/// Written out here rather than taken from the crate's own generated
/// `STANDARD_UNIT_VALUES`, which holds the same 27 values ALPHABETICALLY: the constraint
/// clause lists them in the order the model declares them, so the order is part of what this
/// suite asserts and has to be stated independently of the generated copy.
pub const STANDARD_UNITS: [&str; 27] = [
    "Seconds",
    "Microseconds",
    "Milliseconds",
    "Bytes",
    "Kilobytes",
    "Megabytes",
    "Gigabytes",
    "Terabytes",
    "Bits",
    "Kilobits",
    "Megabits",
    "Gigabits",
    "Terabits",
    "Percent",
    "Count",
    "Bytes/Second",
    "Kilobytes/Second",
    "Megabytes/Second",
    "Gigabytes/Second",
    "Terabytes/Second",
    "Bits/Second",
    "Kilobits/Second",
    "Megabits/Second",
    "Gigabits/Second",
    "Terabits/Second",
    "Count/Second",
    "None",
];

/// A minute-aligned instant three minutes in the past: a CLOSED whole-minute bucket, so no
/// partial period can move underneath a read, and recent enough to sit in every retention
/// tier.
fn closed_minute() -> f64 {
    (crate::harness::now_secs() / 60.0).floor() * 60.0 - 3.0 * 60.0
}

/// A `GetMetricStatistics` request over `metric`'s one dimension set, for the range around
/// `bucket`, carrying `unit` when the row names one.
fn statistics_request(metric: &str, bucket: f64, unit: Option<&str>) -> Value {
    let mut request = json!({
        "Namespace": NAMESPACE,
        "MetricName": metric,
        "Dimensions": dimensions(),
        "StartTime": bucket - 120.0,
        "EndTime": bucket + 300.0,
        "Period": 60,
        "Statistics": ["Sum", "SampleCount"],
    });
    if let (Some(unit), Value::Object(members)) = (unit, &mut request) {
        members.insert("Unit".to_string(), json!(unit));
    }
    request
}

/// A `GetMetricData` request retrieving the same series' `Sum` at the same granularity,
/// carrying `unit` on its `MetricStat` when the row names one.
fn data_request(metric: &str, bucket: f64, unit: Option<&str>) -> Value {
    let mut stat = json!({
        "Metric": {
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": dimensions(),
        },
        "Period": 60,
        "Stat": "Sum",
    });
    if let (Some(unit), Value::Object(members)) = (unit, &mut stat) {
        members.insert("Unit".to_string(), json!(unit));
    }
    json!({
        "StartTime": bucket - 120.0,
        "EndTime": bucket + 300.0,
        "MetricDataQueries": [{"Id": "q", "MetricStat": stat}],
    })
}

/// The `(Unit, Sum, SampleCount)` of every datapoint a `GetMetricStatistics` answer carries,
/// ordered by unit then time stamp so a row compares a SET of per-unit series rather than one
/// serialization order.
fn series_of(body: &Value) -> Vec<(String, f64, f64)> {
    let mut points: Vec<(String, f64, f64)> = body
        .get("Datapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| {
            (
                point
                    .get("Unit")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                point.get("Sum").and_then(Value::as_f64).unwrap_or(f64::NAN),
                point
                    .get("SampleCount")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::NAN),
            )
        })
        .collect();
    points.sort_by(|left, right| left.0.cmp(&right.0));
    points
}

/// The `(StatusCode, Values)` of the one result a `GetMetricData` answer carries.
fn retrieved(body: &Value) -> (String, Vec<f64>) {
    let result = body
        .get("MetricDataResults")
        .and_then(Value::as_array)
        .and_then(|results| results.first().cloned())
        .unwrap_or(Value::Null);
    (
        result
            .get("StatusCode")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        result
            .get("Values")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_f64)
            .collect(),
    )
}

/// Every metric entry `ListMetrics` reports for this suite's namespace, following the token it
/// hands back so a row counts the whole catalogue rather than one page of it.
async fn catalogue(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<Value> {
    let mut entries = Vec::new();
    let mut token: Option<String> = None;
    // A bounded walk: a catalogue that handed back a token forever would otherwise hang the
    // suite rather than fail it.
    for _ in 0..10 {
        let mut request = json!({"Namespace": NAMESPACE});
        if let (Some(cursor), Value::Object(members)) = (token.as_ref(), &mut request) {
            members.insert("NextToken".to_string(), json!(cursor));
        }
        let page = ok(router, "ListMetrics", &request).await;
        entries.extend(
            page.get("Metrics")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    entries
}

/// One row of the unit-identity table: what one write publishes at ONE instant for ONE metric,
/// and the units a read may ask for.
struct UnitRow {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The `(unit, value)` observations the row publishes, all at one instant. The EMPTY unit
    /// is a datum whose publisher declares none, which is stored under [`NO_UNIT_DECLARED`].
    published: Vec<(&'static str, f64)>,
}

/// The `StandardUnit` value a datapoint published with no unit of its own is stored and read
/// back under: `None`, the value the enumerated type declares for a measurement that HAS no
/// unit (`smithy_query(query="shape",shape="StandardUnit")`) — so a publisher that declares
/// nothing and one that declares `None` publish into the SAME bucket.
const NO_UNIT_DECLARED: &str = "None";

/// The unit a datum published under the row's `unit` cell is stored under: the declared one,
/// and [`NO_UNIT_DECLARED`] where the row declares none.
fn stored_unit(unit: &str) -> &str {
    if unit.is_empty() {
        NO_UNIT_DECLARED
    } else {
        unit
    }
}

/// The runtime flavour is the one the booted emulator serves on, so the write path's
/// evaluation pass runs the way it does in production rather than inline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_unit_selects_its_own_datapoints_on_both_read_paths() {
    let router = build_router();
    let bucket = closed_minute();

    // The rows differ along what the rule generalises over: how many distinct units one
    // instant carries, which units those are (including the rate units and `None`, which is a
    // unit of its own and not a wildcard), whether a datum declares a unit at all, and how
    // many observations each unit carries. The units are the model's own `StandardUnit`
    // values, and one row names units no report of this behaviour cites.
    let rows = vec![
        UnitRow {
            case: "three units at one instant",
            published: vec![("Bytes", 100.0), ("Kilobytes", 7.0), ("None", 3.0)],
        },
        UnitRow {
            case: "one unit alone",
            published: vec![("Percent", 42.5)],
        },
        UnitRow {
            case: "two rate units, one of them twice",
            published: vec![
                ("Bytes/Second", 1.5),
                ("Count/Second", 4.0),
                ("Bytes/Second", 2.5),
            ],
        },
        UnitRow {
            case: "units no report of this behaviour cites",
            published: vec![("Terabits", 2.0), ("Microseconds", 900.0), ("Gigabytes", 6.0)],
        },
        UnitRow {
            case: "a datum whose publisher declares no unit at all",
            published: vec![("", 11.0)],
        },
        UnitRow {
            case: "a declared None and an undeclared unit, which are one bucket",
            published: vec![("None", 3.0), ("", 4.0), ("Bytes", 8.0)],
        },
    ];

    for (index, UnitRow { case, published }) in rows.into_iter().enumerate() {
        let metric = format!("Payload{index}");
        // ONE write, every datum at the same instant for the same metric and dimension set,
        // differing only in the unit it declares — or in declaring none.
        let data: Vec<Value> = published
            .iter()
            .map(|(unit, value)| {
                let mut datum = json!({
                    "MetricName": metric,
                    "Dimensions": dimensions(),
                    "Value": value,
                    "Timestamp": bucket,
                });
                if let (false, Value::Object(members)) = (unit.is_empty(), &mut datum) {
                    members.insert("Unit".to_string(), json!(unit));
                }
                datum
            })
            .collect();
        ok(
            &router,
            "PutMetricData",
            &json!({"Namespace": NAMESPACE, "MetricData": data}),
        )
        .await;

        // What each published unit's own bucket holds: its observations folded, and nothing of
        // any other unit's — "CloudWatch does not perform unit conversions".
        let mut expected: Vec<(String, f64, f64)> = Vec::new();
        for (unit, _) in &published {
            let unit = stored_unit(unit);
            if expected.iter().any(|(held, _, _)| held == unit) {
                continue;
            }
            let own: Vec<f64> = published
                .iter()
                .filter(|(published_unit, _)| stored_unit(published_unit) == unit)
                .map(|(_, value)| *value)
                .collect();
            #[allow(clippy::cast_precision_loss)]
            let count = own.len() as f64;
            expected.push((unit.to_string(), own.iter().sum::<f64>(), count));
        }
        expected.sort_by(|left, right| left.0.cmp(&right.0));

        for (unit, sum, count) in &expected {
            // The aggregating read selects that unit's datapoints and echoes the unit each of
            // them carries.
            let selected = ok(
                &router,
                "GetMetricStatistics",
                &statistics_request(&metric, bucket, Some(unit)),
            )
            .await;
            let got = series_of(&selected);
            assert_eq!(
                got,
                vec![(unit.clone(), *sum, *count)],
                "case {case:?}: a read of {unit:?} must answer with that unit's datapoint \
                 alone, got {got:?} from {selected}"
            );
            // And so does the retrieving read, through the same selection on its `MetricStat`.
            let retrieved_series = ok(
                &router,
                "GetMetricData",
                &data_request(&metric, bucket, Some(unit)),
            )
            .await;
            let got = retrieved(&retrieved_series);
            assert_eq!(
                got,
                ("Complete".to_string(), vec![*sum]),
                "case {case:?}: retrieving {unit:?} must answer with that unit's value, got \
                 {got:?} from {retrieved_series}"
            );
        }

        // A read naming NO unit answers with one datapoint per published unit, each carrying
        // its own unit and its own aggregate — never one cross-unit aggregate.
        let unitless = ok(
            &router,
            "GetMetricStatistics",
            &statistics_request(&metric, bucket, None),
        )
        .await;
        let got = series_of(&unitless);
        assert_eq!(
            got, expected,
            "case {case:?}: a read naming no unit must answer with one datapoint per published \
             unit {expected:?}, got {got:?} from {unitless}"
        );

        // A valid unit nothing was published with is an empty SUCCESS on both paths, and it
        // stays empty however often it is asked for. The units are taken from the declared set
        // and span it: the first and the last value the model declares, and two the rows above
        // never publish.
        let never_published: Vec<&str> = STANDARD_UNITS
            .iter()
            .copied()
            .filter(|unit| {
                !published
                    .iter()
                    .any(|(published, _)| stored_unit(published) == *unit)
            })
            .collect();
        for unit in [
            never_published.first().copied(),
            never_published.last().copied(),
            Some("Terabytes"),
            Some("Count"),
        ]
        .into_iter()
        .flatten()
        {
            if published
                .iter()
                .any(|(published, _)| stored_unit(published) == unit)
            {
                continue;
            }
            for attempt in 0..2 {
                let empty = ok(
                    &router,
                    "GetMetricStatistics",
                    &statistics_request(&metric, bucket, Some(unit)),
                )
                .await;
                let got = series_of(&empty);
                assert!(
                    got.is_empty(),
                    "case {case:?}: reading {unit:?} (attempt {attempt}) must answer with no \
                     datapoint, got {got:?} from {empty}"
                );
                let empty = ok(
                    &router,
                    "GetMetricData",
                    &data_request(&metric, bucket, Some(unit)),
                )
                .await;
                let got = retrieved(&empty);
                assert_eq!(
                    got,
                    ("Complete".to_string(), Vec::new()),
                    "case {case:?}: retrieving {unit:?} (attempt {attempt}) must answer \
                     complete with no value, got {got:?} from {empty}"
                );
            }
        }
    }

    // The CATALOGUE's identity excludes the unit: one entry per metric and dimension set,
    // however many units were published under it.
    let entries = catalogue(&router).await;
    assert_eq!(
        entries.len(),
        6,
        "the catalogue must hold one entry per metric and dimension set, whatever units were \
         published under it, got {entries:?}"
    );
    for entry in &entries {
        assert_eq!(
            entry.get("Dimensions"),
            Some(&dimensions()),
            "every catalogue entry names the one dimension set its series was published with, \
             got {entry}"
        );
    }
}

/// One row of the refusal table: a read, the unit value it carries, and the refusal that read's
/// own validator answers with.
struct RefusalRow {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The operation the request is sent to.
    operation: &'static str,
    /// The request itself.
    request: Value,
    /// The wire error code the refusal must carry.
    code: &'static str,
    /// The whole sentence the refusal must state.
    message: String,
}

/// What the read whose model declares the parameter faults says about a `parameter` holding a
/// value outside `StandardUnit`: the parameter and the whole accepted set as the service
/// announces it, and nothing of the submitted value — the one sentence the suite that owns the
/// announced sets states ([`crate::announced_value_sets`]).
fn parameter_in_set(parameter: &str) -> String {
    crate::announced_value_sets::unit_in_set_message(parameter)
}

/// What the read whose model declares NONE of them says about the member at `path` holding
/// `submitted`, a value outside `StandardUnit`: this service's own validation code carries the
/// framework's counting clause, which quotes the submitted value, names the member path and
/// lists the whole declared set in the order the SET ITSELF is walked — the order this service
/// is recorded listing an enumerated type's values in, bucket by bucket
/// (`service_common::validation_message::hashed_enum_order`), rather than the model's
/// declaration order or a sorted one.
fn request_validation_clause(path: &str, submitted: &str) -> String {
    format!(
        "1 validation error detected: Value '{submitted}' at '{path}' failed to satisfy \
         constraint: Member must satisfy enum value set: [{}]",
        ::service_common::validation_message::hashed_enum_order(&STANDARD_UNITS).join(", ")
    )
}

/// A `GetMetricData` request whose `position`th query (1-based) carries `unit`, the others a
/// unit the model declares — so the member path a refusal names comes from the request.
fn data_request_with_unit_at(bucket: f64, position: usize, unit: &str) -> Value {
    let queries: Vec<Value> = (1..=position)
        .map(|index| {
            json!({
                "Id": format!("q{index}"),
                "MetricStat": {
                    "Metric": {
                        "Namespace": NAMESPACE,
                        "MetricName": "Payload0",
                        "Dimensions": dimensions(),
                    },
                    "Period": 60,
                    "Stat": "Sum",
                    "Unit": if index == position { unit } else { "Bytes" },
                },
            })
        })
        .collect();
    json!({
        "StartTime": bucket - 120.0,
        "EndTime": bucket + 300.0,
        "MetricDataQueries": queries,
    })
}

#[tokio::test]
async fn a_unit_outside_the_declared_set_is_refused_by_the_validator_of_the_read_it_arrived_on() {
    let router = build_router();
    let bucket = closed_minute();
    let metric = "Payload0";

    // One series to read, so every row below is refused for its unit and not for a metric that
    // does not exist.
    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{
                "MetricName": metric,
                "Dimensions": dimensions(),
                "Value": 100.0,
                "Unit": "Bytes",
                "Timestamp": bucket,
            }],
        }),
    )
    .await;

    // The rows differ along what the rule generalises over: WHICH read the value arrived on —
    // and so which validator answers — HOW the value misses the declared set (a word that
    // names no unit, a declared value in another case, a declared value with its rate suffix
    // dropped, the empty value), and WHERE in the request it sits, since the member path a
    // refusal names is taken from the request.
    let mut rows: Vec<RefusalRow> = Vec::new();
    for value in ["Furlongs", "bytes", "Bytes/Minute", ""] {
        let mut request = statistics_request(metric, bucket, None);
        if let Value::Object(members) = &mut request {
            members.insert("Unit".to_string(), json!(value));
        }
        rows.push(RefusalRow {
            case: "the aggregating read, whose model declares the parameter faults",
            operation: "GetMetricStatistics",
            request,
            code: "InvalidParameterValueException",
            message: parameter_in_set("Unit"),
        });
        rows.push(RefusalRow {
            case: "the retrieving read, whose model declares none of them",
            operation: "GetMetricData",
            request: data_request_with_unit_at(bucket, 1, value),
            code: "ValidationException",
            message: request_validation_clause("metricDataQueries.1.member.metricStat.unit", value),
        });
        // The same value further along the query list: the index in the member path is the
        // request's, not a fixed one.
        rows.push(RefusalRow {
            case: "the retrieving read, the offending query third in the list",
            operation: "GetMetricData",
            request: data_request_with_unit_at(bucket, 3, value),
            code: "ValidationException",
            message: request_validation_clause("metricDataQueries.3.member.metricStat.unit", value),
        });
    }

    for RefusalRow {
        case,
        operation,
        request,
        code,
        message,
    } in &rows
    {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *code),
            "case {case:?} ({operation}): expected HTTP 400 {code}, got {status} with {body}"
        );
        let stated = error_message(&body);
        assert_eq!(
            &stated, message,
            "case {case:?} ({operation}): expected {message:?}, got {stated:?}"
        );
    }

    // A refused read changes nothing: the series still reads back as it was published.
    let after = ok(
        &router,
        "GetMetricStatistics",
        &statistics_request(metric, bucket, Some("Bytes")),
    )
    .await;
    let got = series_of(&after);
    assert_eq!(
        got,
        vec![("Bytes".to_string(), 100.0, 1.0)],
        "a refused read must leave the stored datapoints alone, got {got:?} from {after}"
    );
}

