// SPDX-License-Identifier: CC0-1.0

//! The `AWS/ACMPrivateCA` metric series a private CA publishes about the issuances it
//! serves, asserted where a client reads them: through CloudWatch's own `ListMetrics` and
//! `GetMetricStatistics`, with both services mounted on ONE engine exactly as `core/router`
//! assembles them.
//!
//! The RULE under test, from the monitoring documentation of Amazon Web Services Private CA
//! (its `AWS/ACMPrivateCA` metrics `Success`, "the issuances that succeeded", applying only
//! to `IssueCertificate`; `Failure`, the issuance attempts that failed; and `Time`, "the
//! time in milliseconds between an issuance request and the completion or failure of
//! issuance"): every issuance request a certificate authority COMPLETES contributes one
//! `Success` count and one `Time` observation of its own latency to the series of THAT CA,
//! and a request refused before the CA takes it on contributes nothing at all — so a CA
//! whose requests were refused has no `Failure` series rather than a zero one, and a CA that
//! served no issuance has no series at all.
//!
//! The table's rows differ along what the rule generalises over: how many issuance requests
//! reach one CA, the validity form each asks for, whether the request is completed or
//! refused (and in which refusal class), and whether a completed request issues a new
//! certificate or replays one. Every row runs against a CA of its own on ONE emulator, so
//! each row's own series is also the assertion that the dimension is honoured rather than
//! aggregated away. The certificate each CA signs for its own activation is an issuance
//! request too, and none of the expected counts holds it: a CA accounts for the issuances it
//! serves once it holds the certificate they are signed under.

use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::certificate_issuance_rules::root_ca_valid_for;
use crate::private_ca_lifecycle::{blob, csr_pem, pem, string, Emulator};

// The namespace, dimension and metric names the series are published under, spelled out
// here rather than read from the crate, so a re-authored name fails this suite. They are
// the ones the service's monitoring documentation publishes for private-CA issuance.
/// The namespace Amazon Web Services Private CA publishes into.
const NAMESPACE: &str = "AWS/ACMPrivateCA";
/// The dimension a series carries: the certificate authority it accounts for.
const CA_DIMENSION: &str = "CertificateAuthorityArn";
/// The metric counting the issuances a CA completed.
const SUCCESS: &str = "Success";
/// The metric counting the issuances that failed after the CA took them on.
const FAILURE: &str = "Failure";
/// The metric carrying how long an issuance took.
const TIME: &str = "Time";

/// The `X-Amz-Target` prefix the CloudWatch model declares, and the protocol it serves.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// A SigV4 `Authorization` whose credential scope names CloudWatch's signing name in the
/// region this suite's CAs live in. The router verifies no signature here; the scope is read
/// for routing.
const CLOUDWATCH_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTTESTTEST/20260101/eu-west-1/monitoring/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The aggregation period every read below asks for: the one-minute resolution these series
/// are published at.
const PERIOD_SECS: i64 = 60;

/// The period as seconds — a read's windows are anchored at the requested start rounded down
/// to the minute, so a datapoint's own stamp may precede the requested `StartTime` by at most
/// one period.
#[allow(clippy::cast_precision_loss)]
const PERIOD_SECS_F64: f64 = PERIOD_SECS as f64;

/// How many seconds either side of the calls a read's window spans.
const WINDOW_SECS: f64 = 180.0;

/// One issuance request a row makes: the case it is named by, the members it carries over
/// [`request`]'s base, and the modelled error code the service refuses it with — `None` for a
/// request the CA completes.
struct Call {
    case: &'static str,
    members: Value,
    refused_with: Option<&'static str>,
}

/// One row of the table: the window the CA's own certificate holds, the issuance requests
/// made against it, and how many issuances that CA must then account for.
struct Row {
    case: &'static str,
    common_name: &'static str,
    ca_validity: Value,
    calls: Vec<Call>,
    completed: f64,
}

/// Wall-clock now as epoch seconds, the clock a metric window is measured against.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// An `IssueCertificate` body for `ca_arn` over `csr`, with `members` written over it: a row
/// states only the members its case is about.
fn request(ca_arn: &str, csr: &str, members: &Value) -> Value {
    let mut body = json!({
        "CertificateAuthorityArn": ca_arn,
        "Csr": blob(csr),
        "SigningAlgorithm": "SHA256WITHRSA",
    });
    for (member, value) in members.as_object().expect("a row states its members") {
        body[member] = value.clone();
    }
    body
}

/// The table: one to three issuance requests per CA, every validity form, three refusal
/// classes, and a repeat of a request a CA has already answered.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one issuance",
            common_name: "one-issuance-root",
            ca_validity: json!({"Type": "DAYS", "Value": 90}),
            calls: vec![Call {
                case: "a month inside the CA's window",
                members: json!({"Validity": {"Type": "DAYS", "Value": 30}}),
                refused_with: None,
            }],
            completed: 1.0,
        },
        Row {
            case: "three issuances, each stating its validity in another form",
            common_name: "three-issuances-root",
            ca_validity: json!({"Type": "YEARS", "Value": 10}),
            calls: vec![
                Call {
                    case: "thirty days",
                    members: json!({"Validity": {"Type": "DAYS", "Value": 30}}),
                    refused_with: None,
                },
                Call {
                    case: "two months",
                    members: json!({"Validity": {"Type": "MONTHS", "Value": 2}}),
                    refused_with: None,
                },
                Call {
                    case: "one year",
                    members: json!({"Validity": {"Type": "YEARS", "Value": 1}}),
                    refused_with: None,
                },
            ],
            completed: 3.0,
        },
        Row {
            case: "two issuances served and one refused for a validity past the CA's own",
            common_name: "bounded-root",
            ca_validity: json!({"Type": "DAYS", "Value": 2}),
            calls: vec![
                Call {
                    case: "a day inside the CA's window",
                    members: json!({
                        "Validity": {"Type": "DAYS", "Value": 1},
                        "IdempotencyToken": "first-day",
                    }),
                    refused_with: None,
                },
                Call {
                    case: "a week past the CA's window",
                    members: json!({"Validity": {"Type": "DAYS", "Value": 7}}),
                    refused_with: Some("ValidationException"),
                },
                Call {
                    case: "a day inside the CA's window again",
                    members: json!({
                        "Validity": {"Type": "DAYS", "Value": 1},
                        "IdempotencyToken": "second-day",
                    }),
                    refused_with: None,
                },
            ],
            completed: 2.0,
        },
        Row {
            case: "a request repeated under the token it first carried",
            common_name: "repeated-root",
            ca_validity: json!({"Type": "DAYS", "Value": 90}),
            calls: vec![
                Call {
                    case: "the request as it stands",
                    members: json!({
                        "Validity": {"Type": "DAYS", "Value": 30},
                        "IdempotencyToken": "repeated",
                    }),
                    refused_with: None,
                },
                Call {
                    case: "the same request again",
                    members: json!({
                        "Validity": {"Type": "DAYS", "Value": 30},
                        "IdempotencyToken": "repeated",
                    }),
                    refused_with: None,
                },
            ],
            // Two issuance requests the CA completed, one certificate issued: the series
            // counts the issuances that succeeded, and a replay succeeded.
            completed: 2.0,
        },
        Row {
            case: "every request refused",
            common_name: "refused-root",
            ca_validity: json!({"Type": "DAYS", "Value": 90}),
            calls: vec![
                Call {
                    case: "a Csr that is not a certification request",
                    members: json!({
                        "Validity": {"Type": "DAYS", "Value": 30},
                        "Csr": blob(&pem("CERTIFICATE REQUEST", "not a certification request")),
                    }),
                    refused_with: Some("MalformedCSRException"),
                },
                Call {
                    case: "a signing algorithm of another key family",
                    members: json!({
                        "Validity": {"Type": "DAYS", "Value": 30},
                        "SigningAlgorithm": "SHA256WITHECDSA",
                    }),
                    refused_with: Some("InvalidArgsException"),
                },
                Call {
                    case: "a validity that resolves into the past",
                    members: json!({"Validity": {"Type": "ABSOLUTE", "Value": 1}}),
                    refused_with: Some("InvalidArgsException"),
                },
            ],
            completed: 0.0,
        },
    ]
}

/// One `awsJson1_0` CloudWatch call through the router both services are mounted on,
/// requiring a 200 — the protocol and target prefix that service serves.
async fn cloudwatch(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://monitoring.eu-west-1.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("host", "monitoring.eu-west-1.amazonaws.com")
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        )
        .header("authorization", CLOUDWATCH_AUTHORIZATION)
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the request builds");
    let response = emulator.router().dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} {parsed:?}"
    );
    parsed
}

/// The datapoints `GetMetricStatistics` reports for one CA's series of `metric` over
/// `[start, end]`, each carrying the `statistics` asked for.
async fn datapoints(
    emulator: &Emulator,
    case: &str,
    metric: &str,
    ca_arn: &str,
    start: f64,
    end: f64,
    statistics: &[&str],
) -> Vec<Value> {
    let answered = cloudwatch(
        emulator,
        case,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": [{"Name": CA_DIMENSION, "Value": ca_arn}],
            "StartTime": start,
            "EndTime": end,
            "Period": PERIOD_SECS,
            "Statistics": statistics,
        }),
    )
    .await;
    answered
        .get("Datapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Whether `ListMetrics` surfaces a series of `metric` dimensioned on `ca_arn` — the read a
/// client polls for before it graphs or alarms on one private CA.
async fn series_is_listed(emulator: &Emulator, case: &str, metric: &str, ca_arn: &str) -> bool {
    let listed = cloudwatch(
        emulator,
        case,
        "ListMetrics",
        &json!({"Namespace": NAMESPACE, "MetricName": metric}),
    )
    .await;
    listed
        .get("Metrics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .any(|series| {
            series["Dimensions"]
                .as_array()
                .is_some_and(|dimensions| {
                    dimensions.iter().any(|dimension| {
                        dimension["Name"] == json!(CA_DIMENSION)
                            && dimension["Value"] == json!(ca_arn)
                    })
                })
        })
}

/// One statistic summed over every datapoint of a span, which is how a client reads the
/// volume a series accounts for over a window.
fn total(points: &[Value], statistic: &str) -> f64 {
    points
        .iter()
        .filter_map(|point| point.get(statistic).and_then(Value::as_f64))
        .sum()
}

/// The largest value any datapoint of a span reports for `statistic`.
fn largest(points: &[Value], statistic: &str) -> f64 {
    points
        .iter()
        .filter_map(|point| point.get(statistic).and_then(Value::as_f64))
        .fold(0.0, f64::max)
}

/// RULE: a CA accounts for every issuance request it completes, and for nothing it refused.
#[tokio::test]
async fn a_private_ca_counts_the_issuances_it_serves_in_the_acmprivateca_namespace() {
    let emulator = Emulator::with_cloudwatch();
    let opened = now_secs();
    let csr = csr_pem("a-counted-leaf");

    // (the row, the CA it ran against, how long its own calls took)
    let mut served: Vec<(Row, String, f64)> = Vec::new();
    for row in rows() {
        let arn = root_ca_valid_for(&emulator, row.case, row.common_name, row.ca_validity.clone())
            .await;
        let started = Instant::now();
        for call in &row.calls {
            let case = format!("{}: {}", row.case, call.case);
            let body = request(&arn, &csr, &call.members);
            match call.refused_with {
                None => {
                    let issued = emulator.ok(&case, "IssueCertificate", &body).await;
                    let certificate_arn = string(&issued, "CertificateArn", &case);
                    assert!(
                        certificate_arn.starts_with(&format!("{arn}/certificate/")),
                        "case {case:?}: expected a certificate of {arn:?}, got \
                         {certificate_arn:?}"
                    );
                }
                Some(code) => {
                    emulator
                        .refused(&case, "IssueCertificate", &body, code)
                        .await;
                }
            }
        }
        let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        served.push((row, arn, elapsed_ms));
    }

    let start = opened - WINDOW_SECS;
    let end = now_secs() + WINDOW_SECS;

    for (row, arn, elapsed_ms) in &served {
        let case = row.case;
        let expected = row.completed;

        // `Success` counts the issuances this CA completed, and only this CA's: the
        // dimension is part of the series' identity, so one row's calls never land in
        // another's.
        let successes = datapoints(
            &emulator,
            case,
            SUCCESS,
            arn,
            start,
            end,
            &["Sum", "SampleCount"],
        )
        .await;
        let counted = total(&successes, "Sum");
        assert!(
            (counted - expected).abs() < 0.5,
            "case {case:?}: {SUCCESS} for {arn} must sum to the {expected} issuances the CA \
             completed, got {counted} from {successes:?}"
        );
        let observations = total(&successes, "SampleCount");
        assert!(
            (observations - expected).abs() < 0.5,
            "case {case:?}: {SUCCESS} must carry one observation per completed issuance \
             ({expected}), got {observations} from {successes:?}"
        );

        // `Time` observes the same event, in milliseconds: one observation per completed
        // issuance, each a positive latency no longer than the wall clock the calls took.
        let times = datapoints(
            &emulator,
            case,
            TIME,
            arn,
            start,
            end,
            &["SampleCount", "Average", "Maximum"],
        )
        .await;
        let timed = total(&times, "SampleCount");
        assert!(
            (timed - expected).abs() < 0.5,
            "case {case:?}: {TIME} must carry one observation per completed issuance \
             ({expected}), got {timed} from {times:?}"
        );
        if expected > 0.0 {
            let average = largest(&times, "Average");
            let slowest = largest(&times, "Maximum");
            assert!(
                average > 0.0,
                "case {case:?}: {TIME} must observe a positive latency, got {average} from \
                 {times:?}"
            );
            assert!(
                slowest <= *elapsed_ms,
                "case {case:?}: {TIME} must be bounded by the {elapsed_ms} ms the calls took, \
                 got {slowest} from {times:?}"
            );
        }

        // Each datapoint carries the unit its metric is measured in, and falls inside the
        // window it was read over.
        for (metric, points, unit) in [
            (SUCCESS, &successes, "Count"),
            (TIME, &times, "Milliseconds"),
        ] {
            for point in points {
                assert_eq!(
                    point.get("Unit").and_then(Value::as_str),
                    Some(unit),
                    "case {case:?}: {metric} must be reported in {unit}, got {point:?}"
                );
                let stamp = point
                    .get("Timestamp")
                    .and_then(Value::as_f64)
                    .unwrap_or_else(|| panic!("case {case:?}: a datapoint carried no Timestamp"));
                assert!(
                    (start - PERIOD_SECS_F64..=end).contains(&stamp),
                    "case {case:?}: the {metric} datapoint at {stamp} must fall inside the \
                     window {start}..={end}"
                );
            }
        }

        // A series exists exactly where the CA completed an issuance: a CA whose every
        // request was refused is absent from the listing rather than present at zero.
        for metric in [SUCCESS, TIME] {
            let listed = series_is_listed(&emulator, case, metric, arn).await;
            assert_eq!(
                listed,
                expected > 0.0,
                "case {case:?}: ListMetrics must surface a {metric} series for {arn} only \
                 where the CA completed an issuance ({expected})"
            );
        }

        // A request refused before the CA takes it on is no failed issuance, so no
        // `Failure` series names this CA and it reports no datapoint at all.
        assert!(
            !series_is_listed(&emulator, case, FAILURE, arn).await,
            "case {case:?}: a synchronous refusal must publish no {FAILURE} series for {arn}"
        );
        let failures = datapoints(&emulator, case, FAILURE, arn, start, end, &["Sum"]).await;
        assert!(
            failures.is_empty(),
            "case {case:?}: a synchronous refusal must publish no {FAILURE} datapoint, got \
             {failures:?}"
        );
    }
}

/// RULE (read from the other side): a CA that served no issuance has no series, and reading a
/// series reports it without changing it — so the same window answers the same volume however
/// often it is read.
#[tokio::test]
async fn reading_the_series_reports_it_without_changing_it() {
    let emulator = Emulator::with_cloudwatch();
    let opened = now_secs();
    let csr = csr_pem("a-re-read-leaf");

    let issuing = root_ca_valid_for(
        &emulator,
        "the issuing CA",
        "re-read-root",
        json!({"Type": "DAYS", "Value": 90}),
    )
    .await;
    // A CA activated exactly like the other one, asked for no certificate: the control.
    let idle = root_ca_valid_for(
        &emulator,
        "an idle CA",
        "idle-root",
        json!({"Type": "DAYS", "Value": 90}),
    )
    .await;

    let body = request(
        &issuing,
        &csr,
        &json!({"Validity": {"Type": "DAYS", "Value": 30}, "IdempotencyToken": "re-read"}),
    );
    emulator.ok("the issuing CA", "IssueCertificate", &body).await;

    let start = opened - WINDOW_SECS;
    let end = now_secs() + WINDOW_SECS;
    let read_twice = [
        total(
            &datapoints(&emulator, "first read", SUCCESS, &issuing, start, end, &["Sum"]).await,
            "Sum",
        ),
        total(
            &datapoints(&emulator, "second read", SUCCESS, &issuing, start, end, &["Sum"]).await,
            "Sum",
        ),
    ];
    assert!(
        (read_twice[0] - 1.0).abs() < 0.5 && (read_twice[1] - read_twice[0]).abs() < 0.5,
        "two reads of one window must both report the one issuance the CA served, got \
         {read_twice:?}"
    );

    // A second issuance adds its own observation rather than replacing the first.
    let second = request(
        &issuing,
        &csr,
        &json!({"Validity": {"Type": "DAYS", "Value": 30}, "IdempotencyToken": "re-read-again"}),
    );
    emulator
        .ok("the issuing CA", "IssueCertificate", &second)
        .await;
    let after = total(
        &datapoints(
            &emulator,
            "after a second issuance",
            SUCCESS,
            &issuing,
            start,
            now_secs() + WINDOW_SECS,
            &["Sum"],
        )
        .await,
        "Sum",
    );
    assert!(
        (after - 2.0).abs() < 0.5,
        "a second issuance must add its own observation to the first, expected 2, got {after}"
    );

    for metric in [SUCCESS, FAILURE, TIME] {
        assert!(
            !series_is_listed(&emulator, "an idle CA", metric, &idle).await,
            "a CA that served no issuance must have no {metric} series, and {idle} served none"
        );
        let points = datapoints(&emulator, "an idle CA", metric, &idle, start, end, &["Sum"]).await;
        assert!(
            points.is_empty(),
            "a CA that served no issuance must report no {metric} datapoint, got {points:?}"
        );
    }
}
