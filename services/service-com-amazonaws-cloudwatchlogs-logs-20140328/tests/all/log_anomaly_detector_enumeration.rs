// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListLogAnomalyDetectors`: "Retrieves a list of the log anomaly
//! detectors in the account."
//!
//! The rules under test:
//!
//! * the enumeration is of the detectors the STORE holds, each reported through the one
//!   projection every reader of a detector shares — so a listed row carries exactly what
//!   `GetLogAnomalyDetector` reports for the same ARN — in one total order, and it follows
//!   every create, update and delete the account makes;
//! * `filterLogGroupArn` is used "to optionally filter the results to only include anomaly
//!   detectors that are associated with the specified log group", so it selects the detectors
//!   whose watch list names that LOG GROUP, while a request naming no filter enumerates the
//!   whole account;
//! * the log group a filter names is the only resource the request names, so one the account
//!   does not hold is refused as a missing resource, and a value that is not a log group's
//!   ARN as a bad parameter;
//! * one page carries at most the `limit` the request named, and "if you don't specify a
//!   value, the default maximum value of 50 items is used"
//!   (`ListLogAnomalyDetectorsInput$limit`, `@range(1..50)`);
//! * `nextToken` is "the token for the next set of items to return", so it comes back only
//!   while detectors remain past the page handed out, and walking it reports every selected
//!   detector exactly once in that one order;
//! * that token is a position within ONE filtered enumeration, so a continuation that
//!   changes the filter — and a token another paginated operation minted — is a bad
//!   parameter.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so an
//! answer that is not computed from the shared store cannot pass.
//!
//! The tables span what the rules generalise over rather than one request: a detector
//! configured with nothing but the log group it watches, with each optional member on its
//! own (every value of the model's `EvaluationFrequency` enum, both published bounds of
//! `anomalyVisibilityTime`, each form a `filterPattern` takes, a KMS key, tags) and with all
//! of them at once; filters naming each of several log groups, in both ARN forms, including
//! a group no detector watches, and filters that name no log group at all (a value that is
//! not an ARN, another kind of CloudWatch Logs resource, another service's resource); page
//! sizes at both published bounds of the `limit` range and one past each of them, over
//! fewer, exactly and more than the default page; and tokens carried back into a request
//! whose filter changed in each way one can.

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    log_group_arn, resource_not_found_message, ANOMALY_DETECTOR_STATUS_ANALYZING,
    ANOMALY_DETECTOR_STATUS_PAUSED, INVALID_NEXT_TOKEN_MESSAGE,
};

use crate::harness::{Logs, REGION};

/// The operation under test, and the ones that write and read back what it enumerates.
const LIST: &str = "ListLogAnomalyDetectors";
const CREATE: &str = "CreateLogAnomalyDetector";
const GET: &str = "GetLogAnomalyDetector";
const UPDATE: &str = "UpdateLogAnomalyDetector";
const DELETE: &str = "DeleteLogAnomalyDetector";

/// The code CloudWatch Logs answers for a parameter it will not accept, which is what this
/// operation declares for such a refusal ("A parameter is specified incorrectly").
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The code answered when the resource a request names is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// How many detectors one page carries when the request names no `limit`: "The maximum
/// number of items to return. If you don't specify a value, the default maximum value of 50
/// items is used." (`ListLogAnomalyDetectorsInput$limit`)
const DEFAULT_LIMIT: usize = 50;

/// The published bounds of `ListLogAnomalyDetectorsLimit`. Source:
/// `smithy_query(query="operation",operation="ListLogAnomalyDetectors")` — `limit` targets
/// `ListLogAnomalyDetectorsLimit`, `@range(min 1, max 50)`.
const SMALLEST_PAGE: i64 = 1;
const LARGEST_PAGE: i64 = 50;

/// Every value of the model's `EvaluationFrequency` enum, in the order the model lists them.
/// Source: `smithy_query(query="shape",shape="EvaluationFrequency")`.
const EVALUATION_FREQUENCIES: [&str; 6] = [
    "ONE_MIN",
    "FIVE_MIN",
    "TEN_MIN",
    "FIFTEEN_MIN",
    "THIRTY_MIN",
    "ONE_HOUR",
];

/// The published bounds of `AnomalyVisibilityTime`. Source:
/// `smithy_query(query="operation",operation="CreateLogAnomalyDetector")` — `@range(7..90)`.
const SHORTEST_VISIBILITY: i64 = 7;
const LONGEST_VISIBILITY: i64 = 90;

/// A KMS key ARN of the form `DetectorKmsKeyArn`'s `@pattern` admits, for the rows that
/// assign one: "Optionally assigns a KMS key to secure this anomaly detector".
const KMS_KEY: &str = "arn:aws:kms:eu-west-2:000000000000:key/aaaabbbb-cccc-dddd-eeee-ffff00001111";

/// How often, and how many times, a case looks again for the status the clock owes a
/// detector. The product is far longer than the spans the create arms, so a loaded host
/// settles well inside it and no case is bounded by how fast the host runs.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);
const POLL_ATTEMPTS: usize = 2_400;

/// One log group, created through the API.
async fn create_group(logs: &Logs, name: &str) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": name}))
        .await;
}

/// One anomaly detector over `group`, created through the API with the optional members
/// `extra` names; answers the ARN the create minted for it.
async fn create_detector(logs: &Logs, group: &str, extra: &Value) -> String {
    let mut body = json!({"logGroupArnList": [log_group_arn(REGION, group, false)]});
    if let (Some(target), Some(source)) = (body.as_object_mut(), extra.as_object()) {
        for (member, value) in source {
            target.insert(member.clone(), value.clone());
        }
    }
    let created = logs.ok(CREATE, &body).await;
    created
        .get("anomalyDetectorArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{CREATE} hands back an ARN, got {created}"))
        .to_string()
}

/// One `ListLogAnomalyDetectors` page: the detectors it listed and the token it handed back.
async fn page(logs: &Logs, request: &Value) -> (Vec<Value>, Option<String>) {
    let answered = logs.ok(LIST, request).await;
    let detectors = answered
        .get("anomalyDetectors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{request}: {LIST} returns anomalyDetectors, got {answered}"))
        .clone();
    let token = answered
        .get("nextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    (detectors, token)
}

/// The `anomalyDetectorArn`s of the detectors in one page, in the order it listed them.
fn arns(listed: &[Value]) -> Vec<String> {
    listed
        .iter()
        .map(|detector| {
            detector
                .get("anomalyDetectorArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("every listed detector carries an ARN, got {detector}")
                })
                .to_string()
        })
        .collect()
}

/// `request`, with every member of `extra` added to it.
fn with(request: &Value, extra: &Value) -> Value {
    let mut merged = request.clone();
    if let (Some(target), Some(source)) = (merged.as_object_mut(), extra.as_object()) {
        for (member, value) in source {
            target.insert(member.clone(), value.clone());
        }
    }
    merged
}

/// The detector under `arn` as `GetLogAnomalyDetector` reports it.
async fn reported(logs: &Logs, arn: &str) -> Value {
    logs.ok(GET, &json!({"anomalyDetectorArn": arn})).await
}

/// Wait until the detector under `arn` reports the status the clock carries it to on its own
/// and stops there (`ANALYZING`), looking again until it does rather than for a fixed time,
/// so cases that compare two reads are not racing the lifecycle.
async fn await_analyzing(logs: &Logs, case: &str, arn: &str) {
    for _ in 0..POLL_ATTEMPTS {
        let status = reported(logs, arn)
            .await
            .get("anomalyDetectorStatus")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if status == ANOMALY_DETECTOR_STATUS_ANALYZING {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case}: the detector {arn} must reach {ANOMALY_DETECTOR_STATUS_ANALYZING}, and \
         still reports an earlier status after {POLL_ATTEMPTS} looks"
    );
}

/// One created detector: what its row varies, and the optional members its create named.
struct Row {
    case: String,
    extra: Value,
}

/// The configurations this suite enumerates: nothing but the watched log group, each
/// optional member on its own — every `EvaluationFrequency` value, both published bounds of
/// `anomalyVisibilityTime`, each form a `filterPattern` takes, a KMS key, tags — and all of
/// them at once.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "nothing but the log group it watches".to_string(),
            extra: json!({}),
        },
        Row {
            case: "a detector name".to_string(),
            extra: json!({"detectorName": "named-detector"}),
        },
        Row {
            case: "the empty filter pattern, which FilterPattern's length 0.. admits"
                .to_string(),
            extra: json!({"filterPattern": ""}),
        },
        Row {
            case: "a term filter pattern".to_string(),
            extra: json!({"filterPattern": "ERROR"}),
        },
        Row {
            case: "a bracketed filter pattern".to_string(),
            extra: json!({"filterPattern": "[ip, user, timestamp]"}),
        },
        Row {
            case: "a KMS key".to_string(),
            extra: json!({"kmsKeyId": KMS_KEY}),
        },
        Row {
            case: "tags, which no AnomalyDetector member reports".to_string(),
            extra: json!({"tags": {"team": "observability"}}),
        },
        Row {
            case: "every optional member at once".to_string(),
            extra: json!({
                "detectorName": "fully-configured",
                "evaluationFrequency": "FIFTEEN_MIN",
                "filterPattern": "WARN",
                "kmsKeyId": KMS_KEY,
                "anomalyVisibilityTime": 30,
                "tags": {"team": "observability", "env": "test"},
            }),
        },
    ];
    for frequency in EVALUATION_FREQUENCIES {
        rows.push(Row {
            case: format!("the evaluation frequency {frequency}"),
            extra: json!({"evaluationFrequency": frequency}),
        });
    }
    for visibility in [SHORTEST_VISIBILITY, LONGEST_VISIBILITY] {
        rows.push(Row {
            case: format!("the anomaly visibility time {visibility}"),
            extra: json!({"anomalyVisibilityTime": visibility}),
        });
    }
    rows
}

/// Every detector the account holds is listed, in one total order, each carrying exactly
/// what `GetLogAnomalyDetector` reports for the same ARN.
#[tokio::test]
async fn every_stored_detector_is_listed_as_the_shared_projection() {
    let logs = Logs::new();
    let group = "enumeration/watched";
    create_group(&logs, group).await;

    // Before anything is created the account holds no detectors, and an empty enumeration
    // hands back no token.
    let (empty, token) = page(&logs, &json!({})).await;
    assert!(
        empty.is_empty(),
        "case an account with no detectors: expected no detectors, got {empty:?}"
    );
    assert_eq!(
        token, None,
        "case an account with no detectors: expected no token, got {token:?}"
    );

    let rows = rows();
    let mut created: Vec<(String, String)> = Vec::new();
    for row in &rows {
        let arn = create_detector(&logs, group, &row.extra).await;
        created.push((row.case.clone(), arn));
    }
    // Compared against a read, so both are taken once the clock owes the detector nothing
    // further and neither read is racing a transition.
    for (case, arn) in &created {
        await_analyzing(&logs, case, arn).await;
    }

    let (listed, token) = page(&logs, &json!({})).await;
    assert_eq!(
        token, None,
        "case the whole account: {} detectors fit one default page, got {token:?}",
        rows.len()
    );
    let listed_arns = arns(&listed);
    let mut expected_arns: Vec<String> = created.iter().map(|(_, arn)| arn.clone()).collect();
    expected_arns.sort();
    assert_eq!(
        listed_arns, expected_arns,
        "case the whole account: every detector is listed once, in one total order, got \
         {listed_arns:?}"
    );

    for (case, arn) in &created {
        let row = listed
            .iter()
            .find(|detector| {
                detector.get("anomalyDetectorArn").and_then(Value::as_str) == Some(arn.as_str())
            })
            .unwrap_or_else(|| panic!("case {case}: the detector {arn} is listed, got {listed:?}"));
        // The listed row is the detector's own projection: what the single-detector read
        // reports, plus the ARN that identifies it.
        let mut without_arn: Map<String, Value> = row
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: a listed detector is a structure, got {row}"))
            .clone();
        without_arn.remove("anomalyDetectorArn");
        let got = reported(&logs, arn).await;
        assert_eq!(
            Value::Object(without_arn),
            got,
            "case {case}: the listed row must carry what {GET} reports, got {row} and {got}"
        );
        assert_eq!(
            row.get("logGroupArnList"),
            Some(&json!([log_group_arn(REGION, group, false)])),
            "case {case}: the row names the log group the create named, got {row}"
        );
        assert_eq!(
            row.get("tags"),
            None,
            "case {case}: no AnomalyDetector member reports tags, got {row}"
        );

        // Every configuration member the create named is reported back on the row.
        let extra = created
            .iter()
            .position(|(stored, _)| stored == case)
            .map(|at| rows[at].extra.clone())
            .unwrap_or(Value::Null);
        for (member, value) in extra.as_object().into_iter().flatten() {
            if member == "tags" {
                continue;
            }
            assert_eq!(
                row.get(member),
                Some(value),
                "case {case}: the row reports {member} as the create named it, got {row}"
            );
        }
    }
}

/// `filterLogGroupArn` selects the detectors associated with the log group it names,
/// whichever of the two ARN forms that group carries it is written in.
#[tokio::test]
async fn the_filter_selects_the_detectors_of_one_log_group() {
    let logs = Logs::new();
    let groups = [
        "enumeration/alpha",
        "enumeration/beta",
        "enumeration/unwatched",
    ];
    for group in groups {
        create_group(&logs, group).await;
    }
    // Two detectors on one group, one on another, none on the third, so a filter has to
    // select a set of several, a set of one and an empty one.
    let mut by_group: Vec<(&str, Vec<String>)> = Vec::new();
    for (group, count) in [(groups[0], 2usize), (groups[1], 1), (groups[2], 0)] {
        let mut arns = Vec::new();
        for index in 0..count {
            arns.push(
                create_detector(&logs, group, &json!({"detectorName": format!("{group}-{index}")}))
                    .await,
            );
        }
        arns.sort();
        by_group.push((group, arns));
    }
    let mut whole_account: Vec<String> = by_group
        .iter()
        .flat_map(|(_, arns)| arns.clone())
        .collect();
    whole_account.sort();

    // (case, the filter the request names, the detectors it selects)
    let mut cases: Vec<(String, Value, Vec<String>)> = vec![(
        "no filter at all".to_string(),
        json!({}),
        whole_account.clone(),
    )];
    for (group, arns) in &by_group {
        // The ARN form `LogGroupArn`'s `@pattern` (`^[\w#+=/:,.@-]*$`) admits: the
        // `LogGroup$logGroupArn` form, without the trailing `:*` the `LogGroup$arn` form
        // carries. Source: `smithy_query(query="operation",operation="ListLogAnomalyDetectors")`.
        cases.push((
            format!("the {group} group"),
            json!({"filterLogGroupArn": log_group_arn(REGION, group, false)}),
            arns.clone(),
        ));
    }
    for (case, request, expected) in cases {
        let (listed, token) = page(&logs, &request).await;
        assert_eq!(
            arns(&listed),
            expected,
            "case {case}: expected {expected:?}, got {listed:?}"
        );
        assert_eq!(
            token, None,
            "case {case}: every selected detector fits one default page, got {token:?}"
        );
    }

    // The log group a filter names is the only resource the request names, so one the
    // account does not hold is a missing resource rather than an empty page.
    let absent = log_group_arn(REGION, "enumeration/never-created", false);
    let (code, message) = logs
        .refusal(LIST, &json!({"filterLogGroupArn": absent}))
        .await;
    assert_eq!(
        code, NOT_FOUND,
        "case a filter naming a log group the account does not hold: expected {NOT_FOUND}, got \
         {code} / {message}"
    );
    assert_eq!(
        message,
        resource_not_found_message("log group"),
        "case a filter naming a log group the account does not hold: got {message:?}"
    );

    // (case, a filter value that is not a log group's ARN, the sentence it is refused with)
    let detector_arn = by_group
        .iter()
        .flat_map(|(_, arns)| arns.clone())
        .next()
        .unwrap_or_else(|| panic!("a detector was created"));
    let refused: Vec<(String, String, String)> = vec![
        (
            "a filter that is not an ARN at all".to_string(),
            "enumeration/alpha".to_string(),
            "1 validation error detected: Unable to parse filterLogGroupArn enumeration/alpha"
                .to_string(),
        ),
        (
            "a filter naming another kind of CloudWatch Logs resource".to_string(),
            detector_arn.clone(),
            format!("filterLogGroupArn must be a log group ARN, got '{detector_arn}'"),
        ),
        (
            "a filter naming another service's resource".to_string(),
            "arn:aws:s3:::enumeration-bucket".to_string(),
            "filterLogGroupArn must be a log group ARN, got \
             'arn:aws:s3:::enumeration-bucket'"
                .to_string(),
        ),
        (
            "a filter naming a log stream of a group".to_string(),
            format!(
                "{}:log-stream:enumeration-stream",
                log_group_arn(REGION, groups[0], false)
            ),
            format!(
                "filterLogGroupArn must be a log group ARN, got '{}:log-stream:enumeration-stream'",
                log_group_arn(REGION, groups[0], false)
            ),
        ),
    ];
    for (case, filter, sentence) in refused {
        let (code, message) = logs
            .refusal(LIST, &json!({"filterLogGroupArn": filter}))
            .await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case}: expected {INVALID_PARAMETER}, got {code} / {message}"
        );
        assert_eq!(
            message, sentence,
            "case {case}: expected {sentence:?}, got {message:?}"
        );
    }
}

/// One page carries at most the `limit` the request named, and at most 50 when it named
/// none; a token comes back exactly while detectors remain past the page.
#[tokio::test]
async fn a_page_carries_at_most_the_limit_and_defaults_to_fifty() {
    // (case, how many detectors the account holds, the limit the request names)
    let rows: Vec<(&str, usize, Option<i64>)> = vec![
        ("one detector, no limit", 1, None),
        ("three detectors, no limit", 3, None),
        ("as many detectors as the default page", DEFAULT_LIMIT, None),
        (
            "one detector past the default page",
            DEFAULT_LIMIT + 1,
            None,
        ),
        ("the smallest page the model admits", 3, Some(SMALLEST_PAGE)),
        ("a page of two", 3, Some(2)),
        ("a page as large as the account's detectors", 3, Some(3)),
        (
            "the largest page the model admits, over more detectors",
            DEFAULT_LIMIT + 1,
            Some(LARGEST_PAGE),
        ),
        (
            "the largest page the model admits, over fewer detectors",
            3,
            Some(LARGEST_PAGE),
        ),
    ];
    for (case, detectors, limit) in rows {
        let logs = Logs::new();
        let group = "enumeration/paged";
        create_group(&logs, group).await;
        for index in 0..detectors {
            create_detector(&logs, group, &json!({"detectorName": format!("paged-{index}")}))
                .await;
        }

        let mut request = json!({});
        if let Some(limit) = limit {
            request["limit"] = json!(limit);
        }
        let expected_page = match limit {
            Some(limit) => usize::try_from(limit).unwrap_or(0),
            None => DEFAULT_LIMIT,
        }
        .min(detectors);
        let (listed, token) = page(&logs, &request).await;
        assert_eq!(
            listed.len(),
            expected_page,
            "case {case}: expected {expected_page} detectors on the first page, got {} ({:?})",
            listed.len(),
            arns(&listed)
        );
        assert_eq!(
            token.is_some(),
            expected_page < detectors,
            "case {case}: a token comes back exactly while detectors remain past the page, got \
             {token:?} with {expected_page} of {detectors} held"
        );
    }

    // The published bounds of `limit` are the page sizes above; one past either of them is
    // refused for the constraint it failed, and nothing is listed.
    let logs = Logs::new();
    // (case, the limit the request names, the constraint clause the refusal states)
    let out_of_range: [(&str, i64, String); 2] = [
        (
            "one below the smallest page",
            SMALLEST_PAGE - 1,
            format!("Member must have value greater than or equal to {SMALLEST_PAGE}"),
        ),
        (
            "one past the largest page",
            LARGEST_PAGE + 1,
            format!("Member must have value less than or equal to {LARGEST_PAGE}"),
        ),
    ];
    for (case, limit, clause) in out_of_range {
        let (code, message) = logs.refusal(LIST, &json!({"limit": limit})).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case}: expected {INVALID_PARAMETER}, got {code} / {message}"
        );
        assert_eq!(
            message,
            format!(
                "1 validation error detected: Value at 'limit' failed to satisfy constraint: \
                 {clause}"
            ),
            "case {case}: the refusal names the position and the constraint, got {message:?}"
        );
    }
}

/// Walking `nextToken` reports every selected detector exactly once, in the one order the
/// unpaged read reports them in.
#[tokio::test]
async fn paging_walks_every_selected_detector_exactly_once() {
    let logs = Logs::new();
    let groups = ["enumeration/walk-a", "enumeration/walk-b"];
    for group in groups {
        create_group(&logs, group).await;
    }
    // Two groups holding a different number of detectors each, so a page boundary falls
    // inside one group's detectors, on a group boundary, and past the last one.
    for (group, count) in [(groups[0], 4usize), (groups[1], 3)] {
        for index in 0..count {
            create_detector(&logs, group, &json!({"detectorName": format!("{group}-{index}")}))
                .await;
        }
    }

    // (case, the request the enumeration starts from)
    let mut enumerations: Vec<(String, Value)> = vec![("the whole account".to_string(), json!({}))];
    for group in groups {
        enumerations.push((
            format!("the group {group}"),
            json!({"filterLogGroupArn": log_group_arn(REGION, group, false)}),
        ));
    }

    for (case, request) in enumerations {
        let (unpaged, token) = page(&logs, &request).await;
        assert_eq!(
            token, None,
            "case {case}: the unpaged read holds every selected detector, got {token:?}"
        );
        let expected = arns(&unpaged);
        let mut sorted = expected.clone();
        sorted.sort();
        assert_eq!(
            expected, sorted,
            "case {case}: the detectors are reported in one total order, got {expected:?}"
        );

        for limit in [
            None,
            Some(SMALLEST_PAGE),
            Some(2),
            Some(3),
            Some(LARGEST_PAGE),
        ] {
            let mut collected: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut paged = request.clone();
                if let Some(limit) = limit {
                    paged["limit"] = json!(limit);
                }
                if let Some(token) = &token {
                    paged["nextToken"] = json!(token);
                }
                let (listed, next) = page(&logs, &paged).await;
                if let Some(limit) = limit {
                    assert!(
                        i64::try_from(listed.len()).unwrap_or(i64::MAX) <= limit,
                        "case {case} at limit {limit}: a page carries at most {limit} \
                         detectors, got {}",
                        listed.len()
                    );
                }
                collected.extend(arns(&listed));
                token = next;
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {case} at limit {limit:?}: paging terminates, got {pages} pages for \
                     {} detectors",
                    expected.len()
                );
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                collected, expected,
                "case {case} at limit {limit:?}: expected {expected:?}, got {collected:?}"
            );
        }
    }
}

/// A `nextToken` is a position within ONE filtered enumeration: carried back into a request
/// whose filter is the same it continues that enumeration, and anything else is refused.
#[tokio::test]
async fn a_token_is_a_position_in_one_filtered_enumeration() {
    let logs = Logs::new();
    let groups = ["enumeration/token-a", "enumeration/token-b"];
    for group in groups {
        create_group(&logs, group).await;
    }
    for group in groups {
        for index in 0..2 {
            create_detector(&logs, group, &json!({"detectorName": format!("{group}-{index}")}))
                .await;
        }
    }
    let group_arn = log_group_arn(REGION, groups[0], false);
    let other_group_arn = log_group_arn(REGION, groups[1], false);

    let first = json!({"filterLogGroupArn": &group_arn, "limit": SMALLEST_PAGE});
    let (listed, token) = page(&logs, &first).await;
    let token = token.unwrap_or_else(|| panic!("a first page of one of two hands back a token"));
    let (whole_group, _) = page(&logs, &json!({"filterLogGroupArn": &group_arn})).await;
    let group_arns = arns(&whole_group);
    assert_eq!(
        arns(&listed),
        group_arns[..1].to_vec(),
        "case the first page: the enumeration starts at its first detector, got {listed:?}"
    );

    // The same filter continues the same enumeration.
    let case = "the same filter";
    let (continued, _) = page(&logs, &with(&first, &json!({"nextToken": &token}))).await;
    assert_eq!(
        arns(&continued),
        group_arns[1..2].to_vec(),
        "case {case}: the token continues that enumeration, got {continued:?}"
    );

    // A token DescribeLogGroups minted, for an enumeration of another resource altogether.
    let pages = logs.ok("DescribeLogGroups", &json!({"limit": 1})).await;
    let foreign = pages
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("DescribeLogGroups pages, got {pages}"))
        .to_string();

    // (case, the request the token is carried back into)
    let refused: Vec<(String, Value)> = vec![
        (
            "a continuation naming another log group".to_string(),
            json!({"filterLogGroupArn": other_group_arn, "nextToken": &token}),
        ),
        (
            "a continuation naming no log group at all".to_string(),
            json!({"nextToken": &token}),
        ),
        (
            "a token another paginated operation minted".to_string(),
            json!({"filterLogGroupArn": &group_arn, "nextToken": foreign}),
        ),
        (
            "a value that is not a token at all".to_string(),
            json!({"filterLogGroupArn": &group_arn, "nextToken": "not-a-token"}),
        ),
    ];
    for (case, request) in refused {
        let (code, message) = logs.refusal(LIST, &request).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case}: expected {INVALID_PARAMETER}, got {code} / {message}"
        );
        assert_eq!(
            message, INVALID_NEXT_TOKEN_MESSAGE,
            "case {case}: expected {INVALID_NEXT_TOKEN_MESSAGE:?}, got {message:?}"
        );
    }
}

/// The enumeration is of what the store holds, so it follows every update and delete the
/// account makes — and listing changes nothing a later caller reads.
#[tokio::test]
async fn the_enumeration_follows_the_detectors_lifecycle() {
    let logs = Logs::new();
    let group = "enumeration/lifecycle";
    create_group(&logs, group).await;
    let updated = create_detector(&logs, group, &json!({"detectorName": "updated"})).await;
    let untouched = create_detector(&logs, group, &json!({"detectorName": "untouched"})).await;
    for (case, arn) in [("updated", &updated), ("untouched", &untouched)] {
        await_analyzing(&logs, case, arn).await;
    }

    /// The row the enumeration carries for one ARN, or `None` once it is gone.
    async fn row(logs: &Logs, arn: &str) -> Option<Value> {
        let (listed, _) = page(logs, &json!({})).await;
        listed.into_iter().find(|detector| {
            detector.get("anomalyDetectorArn").and_then(Value::as_str) == Some(arn)
        })
    }

    let before = row(&logs, &untouched)
        .await
        .unwrap_or_else(|| panic!("case untouched: the detector is listed"));

    // (case, the update, the members the next enumeration must report for it)
    let updates: Vec<(&str, Value, Value)> = vec![
        (
            "a new evaluation frequency",
            json!({"enabled": true, "evaluationFrequency": "THIRTY_MIN"}),
            json!({
                "evaluationFrequency": "THIRTY_MIN",
                "anomalyDetectorStatus": ANOMALY_DETECTOR_STATUS_ANALYZING,
            }),
        ),
        (
            "a new filter pattern and visibility time",
            json!({
                "enabled": true,
                "filterPattern": "WARN",
                "anomalyVisibilityTime": LONGEST_VISIBILITY,
            }),
            json!({
                "filterPattern": "WARN",
                "anomalyVisibilityTime": LONGEST_VISIBILITY,
                "evaluationFrequency": "THIRTY_MIN",
                "anomalyDetectorStatus": ANOMALY_DETECTOR_STATUS_ANALYZING,
            }),
        ),
        (
            "the detector paused",
            json!({"enabled": false}),
            json!({"anomalyDetectorStatus": ANOMALY_DETECTOR_STATUS_PAUSED}),
        ),
        (
            "the detector restarted",
            json!({"enabled": true}),
            json!({"anomalyDetectorStatus": ANOMALY_DETECTOR_STATUS_ANALYZING}),
        ),
    ];
    for (case, update, expected) in updates {
        logs.ok(UPDATE, &with(&update, &json!({"anomalyDetectorArn": &updated})))
            .await;
        let listed = row(&logs, &updated)
            .await
            .unwrap_or_else(|| panic!("case {case}: the updated detector is still listed"));
        for (member, value) in expected.as_object().into_iter().flatten() {
            assert_eq!(
                listed.get(member),
                Some(value),
                "case {case}: the enumeration reports {member} as the update left it, got \
                 {listed}"
            );
        }
        let again = row(&logs, &updated)
            .await
            .unwrap_or_else(|| panic!("case {case}: the updated detector is still listed"));
        assert_eq!(
            again, listed,
            "case {case} listed twice: a report does not change what a later caller reads, got \
             {again} after {listed}"
        );
        assert_eq!(
            row(&logs, &untouched).await,
            Some(before.clone()),
            "case {case}: an update of one detector leaves the other as it was"
        );
    }

    // A deleted detector is gone from the enumeration, and from the filtered one.
    logs.ok(DELETE, &json!({"anomalyDetectorArn": &updated}))
        .await;
    assert_eq!(
        row(&logs, &updated).await,
        None,
        "case a deleted detector: it is no longer listed"
    );
    let (filtered, _) =
        page(&logs, &json!({"filterLogGroupArn": log_group_arn(REGION, group, false)})).await;
    assert_eq!(
        arns(&filtered),
        vec![untouched.clone()],
        "case a deleted detector: the group's enumeration holds only the one left, got \
         {filtered:?}"
    );
}
