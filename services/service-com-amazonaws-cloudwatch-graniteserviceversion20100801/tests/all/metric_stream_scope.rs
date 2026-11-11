// SPDX-License-Identifier: CC0-1.0

//! What a metric stream's configuration may NAME, what it reads back as, and how a
//! configuration this service cannot serve is refused.
//!
//! Four rules are under test, each tabled along what it generalises over rather than along the
//! one request that showed it:
//!
//! 1. an ARN a configuration names another AWS resource by is in the scope of the stream being
//!    configured — this account, and, when the resource it names is Region-scoped, this
//!    stream's Region — or the whole request is refused and nothing is stored;
//! 2. a collection member that arrived EMPTY constrains nothing, which is what leaving it out
//!    means, so `GetMetricStream` reads it back as the omitted form;
//! 3. `IncludeLinkedAccountsMetrics` is the setting the stream HAS, so a configuration that
//!    does not ask for source-account metrics reads back as the documented default rather
//!    than as nothing;
//! 4. a refused configuration names the member and the rule it broke, and a value outside a
//!    modelled enumerated type is refused under this service's own validation code — on every
//!    operation that models such a member, not only on the metric-stream writer.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    build_router, call, call_in_region, error_code, error_message, merged, ok, DEFAULT_REGION,
};

/// The account every ARN this emulator serves carries (`engine`'s own account id, which
/// `PutMetricStream` mints a stream's ARN under).
const ACCOUNT: &str = "000000000000";

/// An account that is not this one: the same number of digits, so it is a well-formed account
/// id and only its value differs.
const OTHER_ACCOUNT: &str = "000000000001";

/// The error class a parameter value this operation cannot use is refused with
/// (`PutMetricStream`'s modelled `InvalidParameterValueException`, HTTP 400).
const INVALID_VALUE: &str = "InvalidParameterValueException";

/// The error class a request carrying two members that cannot be served together is refused
/// with (`PutMetricStream`'s modelled `InvalidParameterCombinationException`, HTTP 400).
const INVALID_COMBINATION: &str = "InvalidParameterCombinationException";

/// The class this service answers a value its model refuses with, as the body names it: the
/// request-validation class, whose legacy query code travels in the header.
const VALIDATION_CODE: &str = "ValidationException";

/// The envelope the boundary walk states a modelled constraint in.
const VALIDATION_ENVELOPE: &str = "1 validation error detected";

/// A Firehose delivery stream's ARN in `region` and `account`: Firehose is Region-scoped, so
/// its ARN names a Region.
fn firehose_arn(region: &str, account: &str) -> String {
    format!("arn:aws:firehose:{region}:{account}:deliverystream/metrics")
}

/// An IAM role's ARN in `account`: IAM is global, so the ARN names no Region.
fn role_arn(account: &str) -> String {
    format!("arn:aws:iam::{account}:role/MetricStreamRole")
}

/// How CloudWatch states an ARN naming a resource in another location than the stream: the
/// member that carried it and the resource it has to match.
fn wrong_location(parameter: &str) -> String {
    format!("{parameter} must be in the same region and AWS partition as the Metric Stream")
}

/// How CloudWatch states an ARN naming a resource in another account than the stream.
fn wrong_account(parameter: &str) -> String {
    format!("{parameter} must be in the same account as the Metric Stream")
}

/// A `PutMetricStream` request for `name` whose destination and role are both in `region` and
/// this account, with `extra` merged over it.
fn put_request(name: &str, region: &str, extra: &Value) -> Value {
    merged(
        json!({
            "Name": name,
            "FirehoseArn": firehose_arn(region, ACCOUNT),
            "RoleArn": role_arn(ACCOUNT),
            "OutputFormat": "json",
        }),
        extra,
    )
}

/// The stream `name` as `GetMetricStream` reports it, requiring the read to succeed.
///
/// A metric stream lives in the Region its write was signed for, so the read is signed for
/// the same one: the same NAME in another Region is another stream.
async fn stream_of(router: &MultiServiceRouter, name: &str) -> Value {
    stream_of_in_region(router, DEFAULT_REGION, name).await
}

/// The same read, signed for `region`.
async fn stream_of_in_region(router: &MultiServiceRouter, region: &str, name: &str) -> Value {
    let (status, body) =
        call_in_region(router, region, "GetMetricStream", &json!({"Name": name})).await;
    assert_eq!(
        status, 200,
        "GetMetricStream in {region}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// Assert that `name` names no stream: a refused configuration stores nothing.
async fn assert_absent(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, body) = call(router, "GetMetricStream", &json!({"Name": name})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "case {case:?}: a refused PutMetricStream must store nothing, got {status} {body}"
    );
}

/// A filter naming `namespace` and `count` generated metric names, for the rows that probe the
/// published "up to 1000 total names" bound.
fn filter_with_metric_names(namespace: &str, count: usize) -> Value {
    let names: Vec<String> = (0..count).map(|index| format!("Metric{index}")).collect();
    json!({"Namespace": namespace, "MetricNames": names})
}

#[tokio::test]
async fn an_arn_outside_the_streams_account_or_region_is_refused_and_stores_nothing() {
    // (case, the Region the request is signed for, the FirehoseArn, the RoleArn, the refusal
    // the rule owes — `None` for a configuration in scope). The rows differ along WHICH member
    // carries the foreign ARN, which dimension it is foreign in (Region, account, both,
    // partition), and which Region the stream itself lives in, so the rule reads "the stream's
    // own account and Region" rather than one fixed pair. The scope rule judges what an ARN
    // NAMES, so a value that is no ARN names no account or Region to compare and is left to
    // the member's own form rule — a separate rule, which no recording covers, so the last row
    // pins that such a value is not refused by THIS one.
    let cases: Vec<(&str, &str, String, String, Option<String>)> = vec![
        (
            "both ARNs in the stream's own account and Region",
            DEFAULT_REGION,
            firehose_arn(DEFAULT_REGION, ACCOUNT),
            role_arn(ACCOUNT),
            None,
        ),
        (
            "the same configuration in another Region",
            "us-west-2",
            firehose_arn("us-west-2", ACCOUNT),
            role_arn(ACCOUNT),
            None,
        ),
        (
            "a destination one Region away",
            DEFAULT_REGION,
            firehose_arn("eu-west-1", ACCOUNT),
            role_arn(ACCOUNT),
            Some(wrong_location("FirehoseArn")),
        ),
        (
            "a destination in the Region the stream is NOT in",
            "us-west-2",
            firehose_arn(DEFAULT_REGION, ACCOUNT),
            role_arn(ACCOUNT),
            Some(wrong_location("FirehoseArn")),
        ),
        (
            "a destination in another partition, and so in another Region",
            DEFAULT_REGION,
            format!("arn:aws-cn:firehose:cn-north-1:{ACCOUNT}:deliverystream/metrics"),
            role_arn(ACCOUNT),
            Some(wrong_location("FirehoseArn")),
        ),
        (
            "a destination in this Region but another partition",
            DEFAULT_REGION,
            format!("arn:aws-us-gov:firehose:{DEFAULT_REGION}:{ACCOUNT}:deliverystream/metrics"),
            role_arn(ACCOUNT),
            Some(wrong_location("FirehoseArn")),
        ),
        (
            "a role in another partition, which names no Region of its own",
            DEFAULT_REGION,
            firehose_arn(DEFAULT_REGION, ACCOUNT),
            format!("arn:aws-cn:iam::{ACCOUNT}:role/MetricStreamRole"),
            Some(wrong_location("RoleArn")),
        ),
        (
            "a destination in this Region but another account",
            DEFAULT_REGION,
            firehose_arn(DEFAULT_REGION, OTHER_ACCOUNT),
            role_arn(ACCOUNT),
            Some(wrong_account("FirehoseArn")),
        ),
        (
            "a role in another account",
            DEFAULT_REGION,
            firehose_arn(DEFAULT_REGION, ACCOUNT),
            role_arn(OTHER_ACCOUNT),
            Some(wrong_account("RoleArn")),
        ),
        (
            "a role in another account, in the other Region too",
            "us-west-2",
            firehose_arn("us-west-2", ACCOUNT),
            role_arn(OTHER_ACCOUNT),
            Some(wrong_account("RoleArn")),
        ),
        (
            "a destination foreign in both dimensions, reported as the location it names",
            DEFAULT_REGION,
            firehose_arn("ap-south-1", OTHER_ACCOUNT),
            role_arn(ACCOUNT),
            Some(wrong_location("FirehoseArn")),
        ),
        (
            "a destination that is no ARN, which this rule does not judge",
            DEFAULT_REGION,
            "metrics-delivery-stream".to_string(),
            role_arn(ACCOUNT),
            None,
        ),
    ];

    for (case, region, firehose, role, refusal) in cases {
        let router = build_router();
        let name = "scoped-stream";
        let request = json!({
            "Name": name,
            "FirehoseArn": firehose,
            "RoleArn": role,
            "OutputFormat": "json",
        });

        let (status, body) = call_in_region(&router, region, "PutMetricStream", &request).await;
        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a configuration in the stream's own scope must be accepted, \
                     got {status} {body}"
                );
                let got = stream_of_in_region(&router, region, name).await;
                assert_eq!(
                    (
                        got.get("FirehoseArn").and_then(Value::as_str),
                        got.get("RoleArn").and_then(Value::as_str)
                    ),
                    (
                        request.get("FirehoseArn").and_then(Value::as_str),
                        request.get("RoleArn").and_then(Value::as_str)
                    ),
                    "case {case:?}: both ARNs must round-trip, got {got}"
                );
            }
            Some(message) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, INVALID_VALUE),
                    "case {case:?}: expected HTTP 400 {INVALID_VALUE}, got {status} {body}"
                );
                assert_eq!(
                    error_message(&body),
                    message,
                    "case {case:?}: expected the refusal to read {message:?}, got {body}"
                );
                assert_absent(&router, case, name).await;
            }
        }
    }

    // A request that breaks the ARN-scope rule AND another of the operation's rules is told
    // about the destination: there is no stream to configure filters for until the
    // configuration names one this account can reach in this Region.
    let router = build_router();
    let (status, body) = call(
        &router,
        "PutMetricStream",
        &json!({
            "Name": "scoped-stream",
            "FirehoseArn": firehose_arn("eu-west-1", ACCOUNT),
            "RoleArn": role_arn(ACCOUNT),
            "OutputFormat": "json",
            "IncludeFilters": [{"Namespace": "AWS/EC2"}],
            "ExcludeFilters": [{"Namespace": "AWS/S3"}],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str(), error_message(&body)),
        (400, INVALID_VALUE, wrong_location("FirehoseArn")),
        "case \"a foreign destination beside two filter lists\": the destination must be the \
         refusal reported, got {status} {body}"
    );
    assert_absent(
        &router,
        "a foreign destination beside two filter lists",
        "scoped-stream",
    )
    .await;
}

#[tokio::test]
async fn an_empty_collection_reads_back_as_the_omitted_form() {
    // (case, what the configuration carries, what GetMetricStream must report for each member
    // the row names). The rows differ along WHICH collection arrived empty and whether a
    // sibling entry of the same list carried something, so the rule is "an empty collection is
    // the omitted form" rather than a case about one member.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "an exclude filter whose MetricNames list is empty",
            json!({"ExcludeFilters": [{"Namespace": "AWS/EC2", "MetricNames": []}]}),
            json!({"ExcludeFilters": [{"Namespace": "AWS/EC2"}]}),
        ),
        (
            "an include filter whose MetricNames list is empty",
            json!({"IncludeFilters": [{"Namespace": "AWS/S3", "MetricNames": []}]}),
            json!({"IncludeFilters": [{"Namespace": "AWS/S3"}]}),
        ),
        (
            "one empty MetricNames list beside one that names metrics",
            json!({"IncludeFilters": [
                {"Namespace": "AWS/EC2", "MetricNames": []},
                {"Namespace": "AWS/Lambda", "MetricNames": ["Errors"]},
            ]}),
            json!({"IncludeFilters": [
                {"Namespace": "AWS/EC2"},
                {"Namespace": "AWS/Lambda", "MetricNames": ["Errors"]},
            ]}),
        ),
        (
            "an explicitly empty StatisticsConfigurations",
            json!({"StatisticsConfigurations": []}),
            json!({"StatisticsConfigurations": null}),
        ),
        (
            "an explicitly empty filter list",
            json!({"ExcludeFilters": []}),
            json!({"ExcludeFilters": null}),
        ),
        (
            "every collection of the configuration empty at once",
            json!({
                "IncludeFilters": [],
                "ExcludeFilters": [],
                "StatisticsConfigurations": [],
            }),
            json!({
                "IncludeFilters": null,
                "ExcludeFilters": null,
                "StatisticsConfigurations": null,
            }),
        ),
        (
            "a statistics configuration that carries entries is kept as submitted",
            json!({"StatisticsConfigurations": [{
                "IncludeMetrics": [{"Namespace": "MyApp", "MetricName": "Latency"}],
                "AdditionalStatistics": ["p99"],
            }]}),
            json!({"StatisticsConfigurations": [{
                "IncludeMetrics": [{"Namespace": "MyApp", "MetricName": "Latency"}],
                "AdditionalStatistics": ["p99"],
            }]}),
        ),
    ];

    for (case, extra, expected) in cases {
        let router = build_router();
        let name = "normalised-stream";
        ok(
            &router,
            "PutMetricStream",
            &put_request(name, DEFAULT_REGION, &extra),
        )
        .await;

        let got = stream_of(&router, name).await;
        for (member, value) in expected.as_object().expect("the row states an object") {
            let reported = got.get(member.as_str());
            if value.is_null() {
                assert!(
                    reported.is_none(),
                    "case {case:?}: an empty {member} must read back as the omitted form, got \
                     {got}"
                );
            } else {
                assert_eq!(
                    reported,
                    Some(value),
                    "case {case:?}: expected {member} to read back as {value}, got {got}"
                );
            }
        }
    }
}

#[tokio::test]
async fn include_linked_accounts_metrics_reads_back_as_the_setting_the_stream_has() {
    // (case, what the configuration asks for, what the stream must then report). "If you are
    // creating a metric stream in a monitoring account, specify true to include metrics from
    // source accounts in the metric stream."
    // (`PutMetricStreamInput$IncludeLinkedAccountsMetrics`) — a stream that does not ask for
    // them streams none, so the omitted form is the false setting rather than no setting.
    let cases: Vec<(&str, Value, bool)> = vec![
        ("the member omitted", json!({}), false),
        (
            "the member omitted beside a filter",
            json!({"IncludeFilters": [{"Namespace": "AWS/EC2"}]}),
            false,
        ),
        (
            "source-account metrics asked for",
            json!({"IncludeLinkedAccountsMetrics": true}),
            true,
        ),
        (
            "source-account metrics declined explicitly",
            json!({"IncludeLinkedAccountsMetrics": false}),
            false,
        ),
    ];

    for (case, extra, expected) in cases {
        let router = build_router();
        let name = "linked-accounts-stream";
        ok(
            &router,
            "PutMetricStream",
            &put_request(name, DEFAULT_REGION, &extra),
        )
        .await;
        let got = stream_of(&router, name).await;
        assert_eq!(
            got.get("IncludeLinkedAccountsMetrics").and_then(Value::as_bool),
            Some(expected),
            "case {case:?}: expected IncludeLinkedAccountsMetrics to read back as {expected}, \
             got {got}"
        );
    }

    // An update "completely overwrites" the configuration, so a stream created with
    // source-account metrics and updated without them has the default again.
    let router = build_router();
    let name = "relinked-stream";
    ok(
        &router,
        "PutMetricStream",
        &put_request(
            name,
            DEFAULT_REGION,
            &json!({"IncludeLinkedAccountsMetrics": true}),
        ),
    )
    .await;
    ok(
        &router,
        "PutMetricStream",
        &put_request(name, DEFAULT_REGION, &json!({})),
    )
    .await;
    let got = stream_of(&router, name).await;
    assert_eq!(
        got.get("IncludeLinkedAccountsMetrics").and_then(Value::as_bool),
        Some(false),
        "case \"an update that stops asking for source-account metrics\": expected the \
         documented default, got {got}"
    );
}

/// One row of the configuration-rule table: the case, what the configuration carries, and the
/// refusal the rule owes it — the error class and the message — or `None` when the
/// configuration is inside every rule and accepted.
type ConfigurationRow = (&'static str, Value, Option<(&'static str, String)>);

#[tokio::test]
async fn a_configuration_the_operation_cannot_serve_names_the_member_and_the_rule() {
    // (case, what the configuration carries, the error class, the message the refusal reads,
    // whether the configuration is accepted instead). The rows differ along which rule is
    // broken, which member breaks it, and by how much — the published 1000-name budget is
    // tested AT its bound and one past it, reached both through one filter's metric names and
    // through several filters' namespaces.
    let cases: Vec<ConfigurationRow> = vec![
        (
            "both filter lists carried at once",
            json!({
                "IncludeFilters": [{"Namespace": "AWS/EC2"}],
                "ExcludeFilters": [{"Namespace": "AWS/S3"}],
            }),
            Some((
                INVALID_COMBINATION,
                "IncludeFilters and ExcludeFilters cannot both be present".to_string(),
            )),
        ),
        (
            "both filter lists, one of them several namespaces long",
            json!({
                "IncludeFilters": [{"Namespace": "AWS/EC2"}, {"Namespace": "AWS/Lambda"}],
                "ExcludeFilters": [{"Namespace": "AWS/S3", "MetricNames": ["BucketSizeBytes"]}],
            }),
            Some((
                INVALID_COMBINATION,
                "IncludeFilters and ExcludeFilters cannot both be present".to_string(),
            )),
        ),
        (
            "one exclude filter naming 1000 metric names, 1001 names in total",
            json!({"ExcludeFilters": [filter_with_metric_names("AWS/EC2", 1000)]}),
            Some((
                INVALID_VALUE,
                "ExcludeFilters greater than 1,000".to_string(),
            )),
        ),
        (
            "the same budget broken in IncludeFilters",
            json!({"IncludeFilters": [filter_with_metric_names("AWS/EC2", 1000)]}),
            Some((
                INVALID_VALUE,
                "IncludeFilters greater than 1,000".to_string(),
            )),
        ),
        (
            "the budget broken across two exclude filters",
            json!({"ExcludeFilters": [
                filter_with_metric_names("AWS/EC2", 500),
                filter_with_metric_names("AWS/Lambda", 499),
            ]}),
            Some((
                INVALID_VALUE,
                "ExcludeFilters greater than 1,000".to_string(),
            )),
        ),
        (
            "a filter list exactly at the 1000-name budget",
            json!({"ExcludeFilters": [filter_with_metric_names("AWS/EC2", 999)]}),
            None,
        ),
        (
            "an empty MetricNames list counts no name towards the budget",
            json!({"ExcludeFilters": [
                filter_with_metric_names("AWS/EC2", 998),
                {"Namespace": "AWS/Lambda", "MetricNames": []},
            ]}),
            None,
        ),
    ];

    for (case, extra, refusal) in cases {
        let router = build_router();
        let name = "budgeted-stream";
        let (status, body) = call(
            &router,
            "PutMetricStream",
            &put_request(name, DEFAULT_REGION, &extra),
        )
        .await;
        match refusal {
            None => assert_eq!(
                status, 200,
                "case {case:?}: a configuration inside every rule must be accepted, got {status} \
                 {body}"
            ),
            Some((code, message)) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, code),
                    "case {case:?}: expected HTTP 400 {code}, got {status} {body}"
                );
                assert_eq!(
                    error_message(&body),
                    message,
                    "case {case:?}: expected the refusal to read {message:?}, got {body}"
                );
                assert_absent(&router, case, name).await;
            }
        }
    }
}

#[tokio::test]
async fn a_value_outside_a_modelled_enum_is_refused_with_this_services_validation_code() {
    let router = build_router();
    // Each row is an operation member modelled as an ENUM, with a value the type does not
    // declare: `PutMetricStreamInput$OutputFormat` (json | opentelemetry0.7 |
    // opentelemetry1.0), `GetMetricDataInput$ScanBy` and `DescribeAlarmHistoryInput$ScanBy`
    // (TimestampDescending | TimestampAscending), `DescribeAlarmHistoryInput$HistoryItemType`
    // and `SetAlarmStateInput$StateValue` (OK | ALARM | INSUFFICIENT_DATA) — the enumerated
    // types the model declares
    // (`smithy_query(query="shape",shape="MetricStreamOutputFormat,ScanBy,HistoryItemType,\
    // RecentlyActive,StateValue")`). The member path the refusal names is the walk's own
    // lowerCamel spelling of the member. `ListMetricsInput$RecentlyActive` is the one such
    // member refused as a parameter VALUE instead, below.
    let cases: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "a metric stream's output format",
            "PutMetricStream",
            put_request(
                "enum-stream",
                DEFAULT_REGION,
                &json!({"OutputFormat": "not-a-real-format"}),
            ),
            "outputFormat",
            "not-a-real-format",
        ),
        (
            "the order a metric read scans in",
            "GetMetricData",
            json!({
                "MetricDataQueries": [{
                    "Id": "m1",
                    "MetricStat": {
                        "Metric": {"Namespace": "MyApp", "MetricName": "Latency"},
                        "Period": 60,
                        "Stat": "Sum",
                    },
                }],
                "StartTime": 1_700_000_000.0,
                "EndTime": 1_700_000_600.0,
                "ScanBy": "ValueDescending",
            }),
            "scanBy",
            "ValueDescending",
        ),
        (
            "the order an alarm history scans in",
            "DescribeAlarmHistory",
            json!({"ScanBy": "Alphabetical"}),
            "scanBy",
            "Alphabetical",
        ),
        (
            "the kind of alarm history asked for",
            "DescribeAlarmHistory",
            json!({"HistoryItemType": "Nonsense"}),
            "historyItemType",
            "Nonsense",
        ),
        (
            "the state an alarm is forced into",
            "SetAlarmState",
            json!({
                "AlarmName": "an-alarm",
                "StateValue": "PENDING",
                "StateReason": "driven by the test",
            }),
            "stateValue",
            "PENDING",
        ),
    ];

    for (case, operation, request, path, value) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, VALIDATION_CODE),
            "case {case:?}: expected HTTP 400 {VALIDATION_CODE} from {operation}, got {status} \
             {body}"
        );
        let message = error_message(&body);
        for expected in [VALIDATION_ENVELOPE, path] {
            assert!(
                message.contains(expected),
                "case {case:?}: expected the refusal to state {expected:?}, got {message:?}"
            );
        }
        // The refusal quotes the value that was submitted: a client can only tell WHICH of the
        // values it sent was rejected — a mis-cased declared value from one the type never
        // offered — from the message.
        assert!(
            message.contains(&format!("Value '{value}' at '{path}'")),
            "case {case:?}: expected the refusal to quote the submitted value {value:?} at \
             {path:?}, got {message:?}"
        );
    }

    // Each value the metric-stream enum DOES declare is accepted, so the refusal above is
    // about the value being outside the type rather than about the member being checked.
    for format in ["json", "opentelemetry0.7", "opentelemetry1.0"] {
        let request = put_request(
            "enum-stream",
            DEFAULT_REGION,
            &json!({"OutputFormat": format}),
        );
        let created = ok(&router, "PutMetricStream", &request).await;
        assert!(
            created.get("Arn").and_then(Value::as_str).is_some(),
            "case {format:?}: a declared output format must be accepted, got {created}"
        );
    }

    // The metric LISTING is the one reader that judges such a filter as a parameter VALUE
    // instead: the only client error its model declares is the parameter-value fault
    // (`smithy_query(query="operation",operation="ListMetrics")`), and the sentence is fixed
    // apart from the parameter, named as the wire spells it. The rows differ along the value
    // offered for the one window the type declares.
    for offered in ["PT4H", "pt3h", "PT3M", ""] {
        let (status, body) = call(
            &router,
            "ListMetrics",
            &json!({"RecentlyActive": offered}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {offered:?}: expected the listing's parameter-value fault, got {status} {body}"
        );
        assert_eq!(
            error_message(&body),
            "Request parameter recentlyActive value is invalid",
            "case {offered:?}: expected the listing's fixed sentence, got {body}"
        );
    }

    // And the one window the type DOES declare is applied rather than refused.
    let listed = ok(&router, "ListMetrics", &json!({"RecentlyActive": "PT3H"})).await;
    assert!(
        listed.get("Metrics").and_then(Value::as_array).is_some(),
        "the declared recency window must be applied, got {listed}"
    );
}
