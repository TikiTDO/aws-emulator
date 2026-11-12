// SPDX-License-Identifier: CC0-1.0

//! The Kinesis CONSUMER LISTING, end to end over the wire: `RegisterStreamConsumer` ->
//! `ListStreamConsumers`.
//!
//! RULE: `ListStreamConsumers` REPORTS the enhanced fan-out consumers registered with the
//! stream its `StreamARN` addresses, computed from the records `RegisterStreamConsumer`
//! persisted and writing nothing of its own. Each consumer is projected onto the `Consumer`
//! shape — the name it was registered under, the ARN Kinesis generated for it, the status the
//! clock has reached (`CREATING` until the transition the registration armed falls due,
//! `ACTIVE` from then on) and the instant it was registered — so a listed consumer never
//! disagrees with the `ConsumerDescription` a `DescribeStreamConsumer` of the same consumer
//! answers with. A page holds at most `MaxResults` of them (100 by default, and at most 100),
//! and a `NextToken` is handed back exactly when the stream carries a consumer past the page;
//! that cursor resumes precisely where the page stopped and is refused once it no longer names
//! a page — with `InvalidArgumentException` and "Invalid NextToken." when it is not a cursor
//! this listing issued (it does not decode and verify, or it was minted for another listing),
//! and with `ExpiredNextTokenException` only when a cursor it DID issue is outside its
//! lifetime. The listing is scoped to the one
//! stream the request names — a `StreamARN` this account and region hold no stream under is
//! `ResourceNotFoundException`, as is a `StreamCreationTimestamp` that is not that stream's,
//! and a stream whose create or removal is still due is `ResourceInUseException`.
//!
//! The recorded flow this was derived from lists the consumers of ONE stream and asks for an
//! invalid `MaxResults`, which is two rows of the tables below. The rows differ along what the
//! rule generalises over: how many consumers the stream carries (none, one, several, as many as
//! may be registered at once), the form and length of each `ConsumerName` inside its
//! `@pattern`/`@length`, the order the registrations arrived in, the page size the request asks
//! for (including each published bound of `MaxResults` and one past either), the lifecycle
//! status of the stream and of the consumer, how the request names the stream, and the region
//! the request is signed for.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    next_token_conflict_message, next_token_expired_message, stream_not_active_message,
    stream_not_found_message, ConsumerPageToken, ShardPageToken, CONSUMER_ACTIVATION_DELAY,
    CONSUMER_ACTIVE, CONSUMER_CREATING, INVALID_NEXT_TOKEN_MESSAGE, MAX_CONSUMERS_CREATING,
    MAX_LIST_STREAM_CONSUMERS_LIMIT, NEXT_TOKEN_STREAM_MISMATCH_MESSAGE, PAGE_TOKEN_TTL,
    STREAM_CREATING,
};

use crate::stream_tags::{
    error_code, error_message, past_activation, stream_arn, Emulator, REGION,
};

/// The published bounds of `MaxResults`
/// (`smithy_query(query="operation",operation="ListStreamConsumers")` -> `MaxResults` range
/// 1..10000).
const MAX_RESULTS_BOUNDS: (i64, i64) = (1, 10_000);

/// The `@length` maximum of `ConsumerName`, in characters
/// (`smithy_query(query="shape",shape="ConsumerName")` -> len 1..128).
const CONSUMER_NAME_MAX_LENGTH: usize = 128;

/// How long past a transition's due instant a case waits before reading the settled state.
const SETTLE_SLACK: Duration = Duration::from_millis(400);

/// A `ConsumerName` of `length` characters inside the modelled `^[a-zA-Z0-9_.-]+$`.
fn name_of_length(length: usize) -> String {
    std::iter::repeat_n('c', length).collect()
}

/// Create a provisioned stream named `name` in `region`, wait out the transition the create
/// armed, and answer its ARN.
async fn active_stream(emulator: &Emulator, case: &str, region: &str, name: &str) -> String {
    emulator
        .ok_in(
            case,
            region,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
    let summary = emulator
        .ok_in(
            case,
            region,
            "DescribeStreamSummary",
            &json!({ "StreamName": name }),
        )
        .await;
    summary["StreamDescriptionSummary"]["StreamARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: DescribeStreamSummary reported no StreamARN"))
        .to_string()
}

/// The instant the create stored for the stream `name` of `region`, which is what a cursor
/// carries beside the stream's ARN.
fn stream_created_at(emulator: &Emulator, case: &str, region: &str, name: &str) -> SystemTime {
    let state = emulator.store();
    let store = state.lock();
    store
        .stream_in(SystemTime::now(), region, name)
        .unwrap_or_else(|| panic!("case {case}: the store holds no stream named {name}"))
        .created_at
}

/// Register `consumer_name` with the stream `stream_arn` in `region` and answer the
/// `Consumer` the registration reported.
async fn register(
    emulator: &Emulator,
    case: &str,
    region: &str,
    stream_arn: &str,
    consumer_name: &str,
) -> Value {
    emulator
        .ok_in(
            case,
            region,
            "RegisterStreamConsumer",
            &json!({ "StreamARN": stream_arn, "ConsumerName": consumer_name }),
        )
        .await["Consumer"]
        .clone()
}

/// The `Consumers` a `ListStreamConsumers` response carries.
fn listed(case: &str, response: &Value) -> Vec<Value> {
    response
        .get("Consumers")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case}: ListStreamConsumers must carry Consumers, got {response}")
        })
        .clone()
}

/// The consumer names a listing reported, in the order it reported them.
fn listed_names(case: &str, response: &Value) -> Vec<String> {
    listed(case, response)
        .iter()
        .map(|consumer| {
            consumer
                .get("ConsumerName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `NextToken` a listing handed back, when it handed one back.
fn next_token(response: &Value) -> Option<String> {
    response
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// List the consumers of `stream_arn` in one call, asking for `max_results` when a row names
/// one.
async fn list(emulator: &Emulator, case: &str, stream_arn: &str, max_results: Option<i64>) -> Value {
    let mut body = json!({ "StreamARN": stream_arn });
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    emulator
        .ok(case, "ListStreamConsumers", &body)
        .await
}

/// Walk every page of the listing of `stream_arn`, asserting no page is longer than
/// `page_size` and that a cursor comes back exactly when a consumer is left over, and answer
/// the consumer names the pages reported in order.
async fn every_page(
    emulator: &Emulator,
    case: &str,
    stream_arn: &str,
    max_results: Option<i64>,
    page_size: usize,
) -> Vec<String> {
    let mut reported: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One page per consumer is the worst case; the bound keeps a cursor that never advances
    // from looping forever.
    for _ in 0..=MAX_CONSUMERS_CREATING {
        let mut body = json!({ "StreamARN": stream_arn });
        if let Some(max_results) = max_results {
            body["MaxResults"] = json!(max_results);
        }
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let response = emulator.ok(case, "ListStreamConsumers", &body).await;
        let page = listed_names(case, &response);
        assert!(
            page.len() <= page_size,
            "case {case}: a page holds at most {page_size} consumers, got {} in {response}",
            page.len()
        );
        reported.extend(page);
        token = next_token(&response);
        if token.is_none() {
            return reported;
        }
    }
    panic!("case {case}: the listing of {stream_arn} never ran out of pages")
}

/// Assert `listed` is the consumer `registered` reported: the same name, the same ARN and the
/// same registration instant, with `expected_status` the status the clock has reached.
fn assert_agrees(case: &str, listed: &Value, registered: &Value, expected_status: &str) {
    for member in ["ConsumerName", "ConsumerARN", "ConsumerCreationTimestamp"] {
        assert_eq!(
            listed.get(member),
            registered.get(member),
            "case {case}: the listed consumer's {member} must be the one the registration \
             reported, got {listed:?} beside {registered:?}"
        );
    }
    assert_eq!(
        listed.get("ConsumerStatus").and_then(Value::as_str),
        Some(expected_status),
        "case {case}: expected the status {expected_status}, got {listed:?}"
    );
}

/// One row of the listing table: the consumers a stream carries, in the order they are
/// registered.
struct ListCase {
    case: &'static str,
    stream: &'static str,
    registered: Vec<String>,
}

/// The table the report rule is checked over. The rows differ along how many consumers the
/// stream carries, the order the registrations arrive in, and the form and length of each
/// `ConsumerName` inside its published `@pattern` and `@length`.
fn list_cases() -> Vec<ListCase> {
    vec![
        ListCase {
            case: "no-consumers-at-all",
            stream: "listing-none",
            registered: Vec::new(),
        },
        ListCase {
            case: "one-consumer",
            stream: "listing-one",
            registered: vec!["solo".to_string()],
        },
        ListCase {
            // Registered in an order that is not the order a name comparison gives, so the
            // listing cannot be reporting insertion order by accident.
            case: "registration-order-is-not-name-order",
            stream: "listing-order",
            registered: vec![
                "zeta".to_string(),
                "alpha".to_string(),
                "middle".to_string(),
            ],
        },
        ListCase {
            // The published `@length` bounds of `ConsumerName` and a name using the
            // punctuation its `@pattern` admits.
            case: "name-forms-inside-the-pattern",
            stream: "listing-names",
            registered: vec![
                "a".to_string(),
                "A.b_c-9".to_string(),
                name_of_length(CONSUMER_NAME_MAX_LENGTH),
            ],
        },
        ListCase {
            // As many consumers as may be registered at once: "you cannot have more than 5
            // consumers in a CREATING status at the same time" (`RegisterStreamConsumer`).
            case: "as-many-as-may-be-registered-at-once",
            stream: "listing-many",
            registered: (0..MAX_CONSUMERS_CREATING)
                .map(|index| format!("batch-{index}"))
                .collect(),
        },
    ]
}

// 1. Every registered consumer is reported, and agrees with what a read of it reports
#[tokio::test]
async fn a_listing_reports_every_consumer_registered_with_the_stream() {
    let emulator = Emulator::new();

    for row in list_cases() {
        let case = row.case;
        let arn = active_stream(&emulator, case, REGION, row.stream).await;
        let mut registered: Vec<Value> = Vec::new();
        for name in &row.registered {
            registered.push(register(&emulator, case, REGION, &arn, name).await);
        }

        let response = list(&emulator, case, &arn, None).await;
        let reported = listed_names(case, &response);
        let mut expected = row.registered.clone();
        expected.sort();
        let mut sorted_reported = reported.clone();
        sorted_reported.sort();
        assert_eq!(
            sorted_reported, expected,
            "case {case}: expected the listing to report every registered consumer, got \
             {reported:?}"
        );

        // Each listed consumer is the stored record, which is also what the registration
        // reported and what a read of that one consumer reports.
        for consumer in listed(case, &response) {
            let name = consumer
                .get("ConsumerName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let minted = registered
                .iter()
                .find(|minted| minted.get("ConsumerName") == consumer.get("ConsumerName"))
                .unwrap_or_else(|| {
                    panic!("case {case}: the listing reported an unregistered consumer {name}")
                });
            assert_agrees(case, &consumer, minted, CONSUMER_CREATING);

            let described = emulator
                .ok(
                    case,
                    "DescribeStreamConsumer",
                    &json!({ "StreamARN": arn, "ConsumerName": name }),
                )
                .await["ConsumerDescription"]
                .clone();
            assert_agrees(case, &described, &consumer, CONSUMER_CREATING);
        }

        // A read changes nothing a later caller sees: the same request answers the same body,
        // and the stream's consumer count is the one the registrations left.
        let again = list(&emulator, case, &arn, None).await;
        assert_eq!(
            listed_names(case, &again),
            reported,
            "case {case}: expected a second listing to report the same consumers, got {again}"
        );
        let counted = emulator
            .ok(
                case,
                "DescribeStreamSummary",
                &json!({ "StreamARN": arn }),
            )
            .await["StreamDescriptionSummary"]["ConsumerCount"]
            .as_u64()
            .unwrap_or_default();
        assert_eq!(
            counted as usize,
            row.registered.len(),
            "case {case}: expected the listing to leave {} consumers registered, got {counted}",
            row.registered.len()
        );
    }
}

// 1b. The status reported is the one the clock has reached, for every consumer listed
#[tokio::test]
async fn a_listing_reports_the_status_the_clock_has_reached() {
    let emulator = Emulator::new();
    let case = "status-follows-the-clock";
    let arn = active_stream(&emulator, case, REGION, "status-stream").await;
    let names = ["early", "late"];
    for name in names {
        register(&emulator, case, REGION, &arn, name).await;
    }

    let creating = list(&emulator, case, &arn, None).await;
    for consumer in listed(case, &creating) {
        assert_eq!(
            consumer.get("ConsumerStatus").and_then(Value::as_str),
            Some(CONSUMER_CREATING),
            "case {case}: a consumer whose transition is still due is reported {CONSUMER_CREATING}, \
             got {consumer:?}"
        );
    }

    // The transition is owed at a stored instant, so waiting past it is all it takes.
    tokio::time::sleep(CONSUMER_ACTIVATION_DELAY + SETTLE_SLACK).await;
    let active = list(&emulator, case, &arn, None).await;
    for consumer in listed(case, &active) {
        assert_eq!(
            consumer.get("ConsumerStatus").and_then(Value::as_str),
            Some(CONSUMER_ACTIVE),
            "case {case}: a consumer past its transition is reported {CONSUMER_ACTIVE}, got \
             {consumer:?}"
        );
    }
}

/// One row of the paging table: the page size a request asks for, and the page size the rule
/// clamps it to.
struct PageRow {
    case: &'static str,
    max_results: Option<i64>,
    page_size: usize,
}

// 2. `MaxResults` bounds a page, and `NextToken` resumes exactly where it stopped
#[tokio::test]
async fn a_listing_pages_by_max_results() {
    let emulator = Emulator::new();
    let case = "paging";
    let arn = active_stream(&emulator, case, REGION, "paging-stream").await;
    let held: Vec<String> = (0..MAX_CONSUMERS_CREATING)
        .map(|index| format!("page-{index}"))
        .collect();
    for name in &held {
        register(&emulator, case, REGION, &arn, name).await;
    }
    // The whole listing in one call is the order every page must agree with.
    let whole = listed_names(case, &list(&emulator, case, &arn, None).await);
    assert_eq!(
        whole.len(),
        held.len(),
        "case {case}: expected the stream to carry {} consumers, got {whole:?}",
        held.len()
    );

    let rows = [
        PageRow {
            case: "one-at-a-time",
            max_results: Some(MAX_RESULTS_BOUNDS.0),
            page_size: 1,
        },
        PageRow {
            case: "two-at-a-time",
            max_results: Some(2),
            page_size: 2,
        },
        PageRow {
            case: "exactly-the-registered-consumers",
            max_results: Some(held.len() as i64),
            page_size: held.len(),
        },
        PageRow {
            case: "no-max-results-at-all",
            max_results: None,
            page_size: MAX_LIST_STREAM_CONSUMERS_LIMIT as usize,
        },
        PageRow {
            // "If you specify a value greater than 100, at most 100 results are returned"
            // (`ListStreamConsumersInput$MaxResults`), so the modelled ceiling is clamped
            // to 100.
            case: "past-the-served-ceiling",
            max_results: Some(MAX_RESULTS_BOUNDS.1),
            page_size: MAX_LIST_STREAM_CONSUMERS_LIMIT as usize,
        },
    ];

    for row in &rows {
        let paged = every_page(&emulator, row.case, &arn, row.max_results, row.page_size).await;
        assert_eq!(
            paged, whole,
            "case {}: expected the pages to report every consumer of the stream in the order \
             one call reports them, got {paged:?}",
            row.case
        );
    }

    // A cursor resumes AT the consumer it names: the first page's own consumers are not
    // repeated and none is skipped.
    let case = "resumed";
    let first = list(&emulator, case, &arn, Some(2)).await;
    let token = next_token(&first)
        .unwrap_or_else(|| panic!("case {case}: a truncated listing hands back a NextToken"));
    let resumed = emulator
        .ok(
            case,
            "ListStreamConsumers",
            &json!({ "StreamARN": arn, "NextToken": token }),
        )
        .await;
    assert_eq!(
        listed_names(case, &resumed),
        whole[2..].to_vec(),
        "case {case}: expected the cursor to resume at the consumer it names, got {resumed}"
    );
    // A listing that reports every consumer it holds hands back no cursor at all.
    assert!(
        next_token(&resumed).is_none(),
        "case {case}: expected no NextToken once the last consumer is reported, got {resumed}"
    );
}

// 2b. Each published bound of `MaxResults` is served and a value one past either is refused
#[tokio::test]
async fn a_max_results_outside_its_published_range_is_refused() {
    let emulator = Emulator::new();
    let case = "max-results-range";
    let arn = active_stream(&emulator, case, REGION, "range-stream").await;
    register(&emulator, case, REGION, &arn, "range-consumer").await;

    for max_results in [MAX_RESULTS_BOUNDS.0, MAX_RESULTS_BOUNDS.1] {
        let served = list(&emulator, "max-results-inside-the-range", &arn, Some(max_results)).await;
        assert_eq!(
            listed_names("max-results-inside-the-range", &served),
            vec!["range-consumer".to_string()],
            "case max-results-{max_results}: a MaxResults inside the modelled range is \
             served, got {served}"
        );
    }
    for max_results in [MAX_RESULTS_BOUNDS.0 - 1, MAX_RESULTS_BOUNDS.1 + 1] {
        let (status, response) = emulator
            .call(
                "ListStreamConsumers",
                &json!({ "StreamARN": arn, "MaxResults": max_results }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case max-results-{max_results}: a MaxResults outside the modelled range is \
             refused, got {status} with body {response}"
        );
    }
}

/// One refusal row: the request body and the modelled error it must answer with.
struct RefusalCase {
    case: String,
    body: Value,
    expected_code: &'static str,
    /// The message the rule that produces this refusal words, when it fixes one.
    expected_message: Option<String>,
}

// 3. A cursor names the page it resumes, or it names none
#[tokio::test]
async fn a_next_token_that_names_no_page_is_refused() {
    let emulator = Emulator::new();
    let case = "cursor-refusals";
    let arn = active_stream(&emulator, case, REGION, "cursor-stream").await;
    for name in ["cursor-a", "cursor-b"] {
        register(&emulator, case, REGION, &arn, name).await;
    }
    let created_at = stream_created_at(&emulator, case, REGION, "cursor-stream");

    let expired = ConsumerPageToken::new(
        &arn,
        created_at,
        "cursor-b",
        SystemTime::now() - PAGE_TOKEN_TTL - Duration::from_secs(1),
    )
    .encode();
    // A token minted at an instant that has not arrived names no page this service served.
    let future = ConsumerPageToken::new(
        &arn,
        created_at,
        "cursor-b",
        SystemTime::now() + Duration::from_secs(3_600),
    )
    .encode();
    // A token naming this ARN but another creation instant names a stream this account and
    // region do not hold: a name a delete released and a later create took again.
    let other_incarnation = ConsumerPageToken::new(
        &arn,
        created_at - Duration::from_secs(60),
        "cursor-b",
        SystemTime::now(),
    )
    .encode();
    // A cursor of ANOTHER listing of the same stream names no page of this one.
    let other_listing =
        ShardPageToken::new(&arn, created_at, "shardId-000000000000", SystemTime::now()).encode();
    let live = ConsumerPageToken::new(&arn, created_at, "cursor-b", SystemTime::now()).encode();

    let cases = [
        RefusalCase {
            case: "a NextToken past its lifetime".to_string(),
            body: json!({ "StreamARN": arn, "NextToken": expired }),
            expected_code: "ExpiredNextTokenException",
            expected_message: Some(next_token_expired_message()),
        },
        RefusalCase {
            case: "a NextToken minted in the future".to_string(),
            body: json!({ "StreamARN": arn, "NextToken": future }),
            expected_code: "ExpiredNextTokenException",
            expected_message: Some(next_token_expired_message()),
        },
        RefusalCase {
            case: "a NextToken this service never issued".to_string(),
            body: json!({ "StreamARN": arn, "NextToken": "not-a-token-this-service-issued" }),
            expected_code: "InvalidArgumentException",
            expected_message: Some(INVALID_NEXT_TOKEN_MESSAGE.to_string()),
        },
        RefusalCase {
            case: "a NextToken minted for another listing".to_string(),
            body: json!({ "StreamARN": arn, "NextToken": other_listing }),
            expected_code: "InvalidArgumentException",
            expected_message: Some(INVALID_NEXT_TOKEN_MESSAGE.to_string()),
        },
        RefusalCase {
            case: "a NextToken naming another incarnation of the stream".to_string(),
            body: json!({ "StreamARN": arn, "NextToken": other_incarnation }),
            expected_code: "ResourceNotFoundException",
            expected_message: Some(stream_not_found_message(&arn)),
        },
        RefusalCase {
            case: "a live NextToken paired with a StreamCreationTimestamp".to_string(),
            body: json!({
                "StreamARN": arn,
                "NextToken": live.clone(),
                "StreamCreationTimestamp": 1.0_f64,
            }),
            expected_code: "InvalidArgumentException",
            expected_message: Some(next_token_conflict_message("StreamCreationTimestamp")),
        },
        RefusalCase {
            case: "a live NextToken paired with another stream's StreamARN".to_string(),
            body: json!({
                "StreamARN": stream_arn(REGION, "cursor-stream-other"),
                "NextToken": live.clone(),
            }),
            expected_code: "InvalidArgumentException",
            expected_message: Some(NEXT_TOKEN_STREAM_MISMATCH_MESSAGE.to_string()),
        },
    ];

    for refusal in &cases {
        let (status, response) = emulator
            .call("ListStreamConsumers", &refusal.body)
            .await;
        assert_eq!(
            status, 400,
            "case {}: expected a 400, got {status} with body {response}",
            refusal.case
        );
        assert_eq!(
            error_code(&response),
            refusal.expected_code,
            "case {}: expected {}, got {response}",
            refusal.case,
            refusal.expected_code
        );
        if let Some(expected) = &refusal.expected_message {
            assert_eq!(
                &error_message(&response),
                expected,
                "case {}: expected the message {expected:?}, got {response}",
                refusal.case
            );
        }
    }

    // The live cursor those rows paired wrongly is served on its own, so each refusal is the
    // pairing and not the cursor.
    let served = emulator
        .ok(
            "a live NextToken on its own",
            "ListStreamConsumers",
            &json!({ "StreamARN": arn, "NextToken": live }),
        )
        .await;
    assert_eq!(
        listed_names("a live NextToken on its own", &served),
        vec!["cursor-b".to_string()],
        "case a live NextToken on its own: expected the page the cursor names, got {served}"
    );
}

// 4. The stream the listing is of has to be one this account and region serve
#[tokio::test]
async fn a_listing_refuses_a_stream_it_cannot_serve() {
    let emulator = Emulator::new();
    let case = "stream-refusals";
    let arn = active_stream(&emulator, case, REGION, "served-stream").await;
    register(&emulator, case, REGION, &arn, "served-consumer").await;

    // A stream of ANOTHER region: a stream "is also scoped by Amazon Web Services Region".
    let elsewhere = "eu-west-1";
    let elsewhere_arn = active_stream(&emulator, case, elsewhere, "elsewhere-stream").await;

    // A stream whose create has not fallen due yet is not in the ACTIVE state.
    emulator
        .ok(
            case,
            "CreateStream",
            &json!({ "StreamName": "creating-stream", "ShardCount": 1 }),
        )
        .await;
    let creating_arn = stream_arn(REGION, "creating-stream");

    let cases = [
        RefusalCase {
            case: "a StreamARN this account and region hold no stream under".to_string(),
            body: json!({ "StreamARN": stream_arn(REGION, "never-created") }),
            expected_code: "ResourceNotFoundException",
            expected_message: Some(stream_not_found_message(&stream_arn(
                REGION,
                "never-created",
            ))),
        },
        RefusalCase {
            case: "a stream of another region".to_string(),
            body: json!({ "StreamARN": elsewhere_arn }),
            expected_code: "ResourceNotFoundException",
            expected_message: Some(stream_not_found_message(&elsewhere_arn)),
        },
        RefusalCase {
            case: "a stream whose create is still due".to_string(),
            body: json!({ "StreamARN": creating_arn }),
            expected_code: "ResourceInUseException",
            expected_message: Some(stream_not_active_message(
                "creating-stream",
                STREAM_CREATING,
            )),
        },
        RefusalCase {
            case: "a StreamCreationTimestamp that is not this stream's".to_string(),
            body: json!({ "StreamARN": arn, "StreamCreationTimestamp": 1.0_f64 }),
            expected_code: "ResourceNotFoundException",
            expected_message: Some(stream_not_found_message(&arn)),
        },
    ];

    for refusal in &cases {
        let (status, response) = emulator
            .call("ListStreamConsumers", &refusal.body)
            .await;
        assert_eq!(
            status, 400,
            "case {}: expected a 400, got {status} with body {response}",
            refusal.case
        );
        assert_eq!(
            error_code(&response),
            refusal.expected_code,
            "case {}: expected {}, got {response}",
            refusal.case,
            refusal.expected_code
        );
        if let Some(expected) = &refusal.expected_message {
            assert_eq!(
                &error_message(&response),
                expected,
                "case {}: expected the message {expected:?}, got {response}",
                refusal.case
            );
        }
    }

    // The stream's OWN creation instant names it, so the member that distinguishes two
    // streams of one name serves the listing rather than refusing it.
    let created_at = emulator
        .ok(
            case,
            "DescribeStreamSummary",
            &json!({ "StreamARN": arn }),
        )
        .await["StreamDescriptionSummary"]["StreamCreationTimestamp"]
        .clone();
    let served = emulator
        .ok(
            "its own StreamCreationTimestamp",
            "ListStreamConsumers",
            &json!({ "StreamARN": arn, "StreamCreationTimestamp": created_at }),
        )
        .await;
    assert_eq!(
        listed_names("its own StreamCreationTimestamp", &served),
        vec!["served-consumer".to_string()],
        "case its own StreamCreationTimestamp: expected the stream's consumers, got {served}"
    );

    // The other region's own listing is served there: the refusal above is the scoping, not
    // the stream.
    let elsewhere_served = emulator
        .ok_in(
            "the other region's own listing",
            elsewhere,
            "ListStreamConsumers",
            &json!({ "StreamARN": elsewhere_arn }),
        )
        .await;
    assert!(
        listed_names("the other region's own listing", &elsewhere_served).is_empty(),
        "case the other region's own listing: expected no consumers, got {elsewhere_served}"
    );
}

// 5. The listing is of ONE stream's consumers, whatever other streams carry
#[tokio::test]
async fn a_listing_reports_only_the_consumers_of_the_stream_it_names() {
    let emulator = Emulator::new();
    let case = "scoping";
    let one = active_stream(&emulator, case, REGION, "scoped-one").await;
    let two = active_stream(&emulator, case, REGION, "scoped-two").await;

    // "For a given Kinesis data stream, each consumer must have a unique name. However,
    // consumer names don't have to be unique across data streams"
    // (`RegisterStreamConsumerInput$ConsumerName`), so the shared name is two consumers.
    for (stream, names) in [(&one, ["shared", "only-one"]), (&two, ["shared", "only-two"])] {
        for name in names {
            register(&emulator, case, REGION, stream, name).await;
        }
    }

    for (stream, expected) in [
        (&one, vec!["only-one".to_string(), "shared".to_string()]),
        (&two, vec!["only-two".to_string(), "shared".to_string()]),
    ] {
        let response = list(&emulator, case, stream, None).await;
        let mut reported = listed_names(case, &response);
        reported.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case}: expected only the consumers of {stream}, got {response}"
        );
        // The shared name resolves to the consumer of the stream that was listed: a
        // consumer's ARN is its own stream's ARN extended.
        for consumer in listed(case, &response) {
            let consumer_arn = consumer
                .get("ConsumerARN")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                consumer_arn.starts_with(&format!("{stream}/consumer/")),
                "case {case}: expected a ConsumerARN of {stream}, got {consumer_arn}"
            );
        }
    }
}
