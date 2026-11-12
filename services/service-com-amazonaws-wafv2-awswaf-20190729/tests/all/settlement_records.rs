// SPDX-License-Identifier: CC0-1.0

//! `ListSettlementRecords`, end to end over the wire.
//!
//! The operation "retrieves individual settlement transaction records for
//! monetization", so the RULE these suites assert is: the answer is a selection
//! over the scope's settlement ledger — the transactions whose settlement instant
//! the `TimeWindow` contains and which satisfy every filter the request applied
//! (production transactions only, unless a `CurrencyMode` filter asks otherwise),
//! ordered by the field `SortBy` names in the direction `SortOrder` names, cut into
//! pages of at most `Limit` records — and the call REPORTS: it changes nothing a
//! later caller sees.
//!
//! Each suite asserts that over a TABLE spanning the class rather than over one
//! request: windows that cover all, part and none of the ledger, every documented
//! filter name with one and several values, every `SortBy` × `SortOrder` pair,
//! every page size, and each refusal — the regional namespace, a window past the
//! published maximum, an unknown filter and a value outside an enum-restricted
//! filter's vocabulary.
//!
//! WAF records a settlement in its DATA plane, when a rule's `Monetize` action
//! settles a payment for a request a web ACL evaluated, and this emulator serves
//! only the WAFv2 control plane, so no operation fills that ledger: the selection
//! cases seed it through the state API ([`seed_ledger`]) and then drive every
//! assertion through the router's front door, the same door an SDK uses.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::ops::types::SettlementRecord;
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    ComAmazonawsWafv2Awswaf20190729State, SettlementRecordEntry, MAX_MONETIZATION_WINDOW_DAYS,
    SCOPE_CLOUDFRONT, SCOPE_REGIONAL,
};

use crate::parameter_validation::{error_code, error_message};
use crate::web_acl_lifecycle::{create_ip_set, create_web_acl, Waf};

/// The operation under test, named once.
const OPERATION: &str = "ListSettlementRecords";

/// The one `Currency` the model declares
/// (`smithy_query(query="shape",shape="Currency")`).
pub(crate) const CURRENCY: &str = "USDC";

/// The instant every window and every seeded transaction below is expressed
/// relative to (epoch seconds for 2026-01-01T00:00:00Z), so the whole suite is
/// deterministic and never races a wall clock.
pub(crate) const BASE_SECS: i64 = 1_767_225_600;

/// A `TimeWindow` spanning `[BASE_SECS + from, BASE_SECS + to]`, in the
/// epoch-seconds form an awsJson1_1 body carries a timestamp in.
pub(crate) fn window(from: i64, to: i64) -> Value {
    json!({ "StartTime": BASE_SECS + from, "EndTime": BASE_SECS + to })
}

/// A request for `scope` over `time_window`, carrying the three members the model
/// marks `@required`.
pub(crate) fn request(scope: &str, time_window: Value) -> Value {
    json!({ "Scope": scope, "Currency": CURRENCY, "TimeWindow": time_window })
}

/// [`request`] with the members of `extra` merged in, for the optional members.
pub(crate) fn with(mut body: Value, extra: Value) -> Value {
    if let (Some(target), Some(source)) = (body.as_object_mut(), extra.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    body
}

/// One `MonetizationFilter`.
pub(crate) fn filter(name: &str, values: &[&str]) -> Value {
    json!({ "Name": name, "Values": values })
}

/// The members every seeded transaction shares, so each one below states only what
/// it differs in.
pub(crate) fn base_record(web_acl_arn: &str) -> SettlementRecord {
    SettlementRecord {
        timestamp: ::aws_smithy_types::DateTime::from_millis(BASE_SECS * 1_000),
        payer_address: Some("0xPAYERA".to_string()),
        wallet_address: Some("0xWALLET".to_string()),
        status: "SETTLED".to_string(),
        amount: "0.001".to_string(),
        currency: Some(CURRENCY.to_string()),
        network: Some("BASE".to_string()),
        transaction_id: Some("tx-0".to_string()),
        request_id: Some("req-0".to_string()),
        source_name: Some("gptbot".to_string()),
        organization: Some("openai".to_string()),
        source_category: Some("ai".to_string()),
        intent: Some("search".to_string()),
        verified: true,
        content_path: Some("/api/v1/users".to_string()),
        web_acl_arn: Some(web_acl_arn.to_string()),
        request_timestamp: Some(::aws_smithy_types::DateTime::from_millis(BASE_SECS * 1_000)),
    }
}

/// One ledger entry: the transaction, the rule that triggered the `Monetize`
/// action, and the `(HttpSourceName, HttpSourceId)` of the resource that fronted
/// the request.
pub(crate) fn entry(
    record: SettlementRecord,
    terminating_rule: &str,
    source: (&str, &str),
) -> SettlementRecordEntry {
    SettlementRecordEntry {
        record,
        terminating_rule_name: Some(terminating_rule.to_string()),
        http_source_name: Some(source.0.to_string()),
        http_source_id: Some(source.1.to_string()),
    }
}

/// The instant `at` seconds after [`BASE_SECS`], as a modelled timestamp.
pub(crate) fn at(seconds: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis((BASE_SECS + seconds) * 1_000)
}

/// The ledger the selection cases read: six settlement transactions, four of them
/// settled on a production network (`BASE`, `SOLANA`) and two on a testnet
/// (`BASE_SEPOLIA`, `SOLANA_DEVNET`), differing in every field a documented filter
/// or a `SortBy` names.
///
/// The statuses are the model's own (`SettlementStatus`), the networks the model's
/// `BlockchainChain` values, the `HttpSourceName` values the ones
/// `MonetizationFilter$Name` lists, and the bot names, categories and
/// organizations the examples the model's filter docs give.
fn ledger_fixture(web_acl_arn: &str) -> Vec<SettlementRecordEntry> {
    let base = base_record(web_acl_arn);
    vec![
        // 0 — the earliest production transaction.
        entry(
            SettlementRecord {
                timestamp: at(10),
                amount: "0.004".to_string(),
                transaction_id: Some("tx-1".to_string()),
                request_id: Some("req-1".to_string()),
                ..base.clone()
            },
            "monetize-ai",
            ("CF", "E1CFDIST"),
        ),
        // 1 — another network, bot, organization, intent, path and status.
        entry(
            SettlementRecord {
                timestamp: at(20),
                amount: "0.010".to_string(),
                status: "PENDING".to_string(),
                network: Some("SOLANA".to_string()),
                transaction_id: Some("tx-2".to_string()),
                request_id: Some("req-2".to_string()),
                source_name: Some("claudebot".to_string()),
                organization: Some("anthropic".to_string()),
                intent: Some("training".to_string()),
                content_path: Some("/api/v2/orders".to_string()),
                payer_address: Some("0xPAYERB".to_string()),
                ..base.clone()
            },
            "monetize-ai",
            ("CF", "E1CFDIST"),
        ),
        // 2 — a search engine, a failed settlement, another path prefix.
        entry(
            SettlementRecord {
                timestamp: at(30),
                amount: "0.002".to_string(),
                status: "FAILED".to_string(),
                transaction_id: Some("tx-3".to_string()),
                request_id: Some("req-3".to_string()),
                source_name: Some("googlebot".to_string()),
                organization: Some("google".to_string()),
                source_category: Some("search_engine".to_string()),
                content_path: Some("/docs/intro".to_string()),
                verified: false,
                ..base.clone()
            },
            "monetize-crawlers",
            ("ALB", "app/prod/0123"),
        ),
        // 3 — a testnet transaction: reported only to a query that asks for TEST.
        entry(
            SettlementRecord {
                timestamp: at(40),
                amount: "0.025".to_string(),
                network: Some("BASE_SEPOLIA".to_string()),
                transaction_id: Some("tx-4".to_string()),
                request_id: Some("req-4".to_string()),
                ..base.clone()
            },
            "monetize-ai",
            ("APIGW", "a1b2c3d4e5"),
        ),
        // 4 — the other testnet.
        entry(
            SettlementRecord {
                timestamp: at(50),
                amount: "0.0005".to_string(),
                status: "DUPLICATE".to_string(),
                network: Some("SOLANA_DEVNET".to_string()),
                transaction_id: Some("tx-5".to_string()),
                request_id: Some("req-5".to_string()),
                source_name: Some("bingbot".to_string()),
                organization: Some("microsoft".to_string()),
                ..base.clone()
            },
            "monetize-crawlers",
            ("APPRUNNER", "svc-7"),
        ),
        // 5 — an hour later, so a one-hour window leaves it out.
        entry(
            SettlementRecord {
                timestamp: at(3_700),
                amount: "0.500".to_string(),
                status: "SERVICE_ERROR".to_string(),
                transaction_id: Some("tx-6".to_string()),
                request_id: Some("req-6".to_string()),
                source_name: Some("zzzbot".to_string()),
                organization: Some("example".to_string()),
                content_path: Some("/".to_string()),
                ..base
            },
            "monetize-late",
            ("COGNITO", "pool-9"),
        ),
    ]
}

/// Append `entries` to the settlement ledger of `scope`.
///
/// The one thing these suites cannot do over the wire: WAF's data plane is what
/// settles a payment, and this emulator serves only the WAFv2 control plane, so
/// nothing in the API fills that ledger.
pub(crate) fn seed_ledger(
    engine: &engine::InMemoryEngine,
    scope: &str,
    entries: Vec<SettlementRecordEntry>,
) {
    engine
        .state::<ComAmazonawsWafv2Awswaf20190729State>()
        .lock()
        .settlement_records
        .entry(scope.to_string())
        .or_default()
        .extend(entries);
}

/// How many transactions the ledger of `scope` holds.
pub(crate) fn ledger_len(engine: &engine::InMemoryEngine, scope: &str) -> usize {
    engine
        .state::<ComAmazonawsWafv2Awswaf20190729State>()
        .lock()
        .settlement_records(scope)
        .len()
}

/// The `TransactionId`s of a `Settlements` list, in the order the response carries
/// them.
fn ids(answer: &Value) -> Vec<String> {
    answer["Settlements"]
        .as_array()
        .map(|records| {
            records
                .iter()
                .map(|record| {
                    record["TransactionId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A web ACL in the CLOUDFRONT scope, for the `WebACLArn` filter and for the
/// records' own `WebAclArn`.
pub(crate) async fn monetized_web_acl(waf: &Waf, name: &str) -> String {
    let (_id, arn, _lock) = create_web_acl(waf, SCOPE_CLOUDFRONT, name, json!([])).await;
    arn
}

/// An emulator whose CLOUDFRONT ledger holds [`ledger_fixture`], with the ARN of
/// the web ACL the transactions were settled for.
async fn seeded() -> (Waf, String) {
    let (waf, engine) = Waf::sharing_engine();
    let arn = monetized_web_acl(&waf, "monetized-acl").await;
    seed_ledger(&engine, SCOPE_CLOUDFRONT, ledger_fixture(&arn));
    (waf, arn)
}

/// A `CurrencyMode` filter asking for both modes, so a case that is not about the
/// mode default sees the whole ledger.
fn both_modes() -> Value {
    filter("CurrencyMode", &["REAL", "TEST"])
}

#[tokio::test]
async fn a_scope_that_settled_nothing_reports_no_transaction_over_every_window() {
    let waf = Waf::new();
    // Windows of every width, including the widest the operation supports: the
    // answer is the empty selection, not a refusal.
    for (case, time_window) in [
        ("one hour forward", window(0, 3_600)),
        ("the three hours before", window(-10_800, 0)),
        ("one second", window(0, 1)),
        (
            "the published maximum",
            window(0, MAX_MONETIZATION_WINDOW_DAYS * 24 * 60 * 60),
        ),
    ] {
        let answer = waf
            .ok(OPERATION, request(SCOPE_CLOUDFRONT, time_window))
            .await;
        assert_eq!(
            answer["Settlements"],
            json!([]),
            "case {case:?}: a scope that settled nothing reports no transaction, \
             got {answer}"
        );
        assert!(
            answer["NextMarker"].is_null(),
            "case {case:?}: an exhausted selection carries no marker, got {answer}"
        );
    }
}

#[tokio::test]
async fn the_time_window_selects_the_transactions_it_contains() {
    let (waf, _arn) = seeded().await;

    // The fixture's transactions sit at seconds 10, 20, 30, 40, 50 and 3700 of the
    // window, the two testnet ones at 40 and 50.
    for (case, time_window, expected) in [
        (
            "the whole ledger",
            window(0, 7_200),
            vec!["tx-1", "tx-2", "tx-3", "tx-4", "tx-5", "tx-6"],
        ),
        (
            "the first hour",
            window(0, 3_600),
            vec!["tx-1", "tx-2", "tx-3", "tx-4", "tx-5"],
        ),
        ("the first half minute", window(0, 30), vec!["tx-1", "tx-2", "tx-3"]),
        // Both bounds are in the window: a one-second window on a transaction's own
        // instant selects it.
        ("one instant", window(20, 20), vec!["tx-2"]),
        ("a gap between two", window(21, 29), vec![]),
        ("before the ledger", window(-3_600, -1), vec![]),
        // A window whose end precedes its start holds no instant, so it selects
        // nothing rather than being refused.
        ("inverted", window(3_600, 0), vec![]),
    ] {
        let answer = waf
            .ok(
                OPERATION,
                with(
                    request(SCOPE_CLOUDFRONT, time_window),
                    json!({ "Filters": [both_modes()], "SortBy": "TIMESTAMP", "SortOrder": "ASC" }),
                ),
            )
            .await;
        assert_eq!(
            ids(&answer),
            expected,
            "case {case:?}: expected {expected:?}, got {answer}"
        );
    }
}

#[tokio::test]
async fn the_currency_mode_filter_decides_whether_testnet_transactions_are_reported() {
    let (waf, _arn) = seeded().await;

    // "When no CurrencyMode filter is provided, results default to REAL. To
    // retrieve test data, include a CurrencyMode filter with the value TEST."
    for (case, filters, expected) in [
        (
            "no filter at all",
            json!({}),
            vec!["tx-1", "tx-2", "tx-3", "tx-6"],
        ),
        (
            "another filter, no currency mode",
            json!({ "Filters": [filter("SourceCategory", &["ai", "search_engine"])] }),
            vec!["tx-1", "tx-2", "tx-3", "tx-6"],
        ),
        (
            "REAL",
            json!({ "Filters": [filter("CurrencyMode", &["REAL"])] }),
            vec!["tx-1", "tx-2", "tx-3", "tx-6"],
        ),
        (
            "TEST",
            json!({ "Filters": [filter("CurrencyMode", &["TEST"])] }),
            vec!["tx-4", "tx-5"],
        ),
        (
            "both",
            json!({ "Filters": [both_modes()] }),
            vec!["tx-1", "tx-2", "tx-3", "tx-4", "tx-5", "tx-6"],
        ),
    ] {
        let answer = waf
            .ok(
                OPERATION,
                with(
                    with(
                        request(SCOPE_CLOUDFRONT, window(0, 7_200)),
                        json!({ "SortBy": "TIMESTAMP", "SortOrder": "ASC" }),
                    ),
                    filters,
                ),
            )
            .await;
        assert_eq!(
            ids(&answer),
            expected,
            "case {case:?}: expected {expected:?}, got {answer}"
        );
    }
}

#[tokio::test]
async fn each_filter_selects_the_transactions_whose_own_field_it_names() {
    let (waf, arn) = seeded().await;

    // One row per documented `MonetizationFilter$Name`: the enum-restricted names,
    // the ARN-validated one, and the free-text ones — including the prefix one and
    // the two fields a `SettlementRecord` does not itself carry.
    let web_acl_arn_filter = filter("WebACLArn", &[arn.as_str()]);
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "SettlementStatus, one value",
            filter("SettlementStatus", &["PENDING"]),
            vec!["tx-2"],
        ),
        (
            "SettlementStatus, several values",
            filter("SettlementStatus", &["PENDING", "FAILED", "DUPLICATE"]),
            vec!["tx-2", "tx-3", "tx-5"],
        ),
        (
            "ChainName",
            filter("ChainName", &["SOLANA", "SOLANA_DEVNET"]),
            vec!["tx-2", "tx-5"],
        ),
        (
            "HttpSourceName",
            filter("HttpSourceName", &["ALB", "APIGW"]),
            vec!["tx-3", "tx-4"],
        ),
        ("WebACLArn", web_acl_arn_filter.clone(), vec![
            "tx-1", "tx-2", "tx-3", "tx-4", "tx-5", "tx-6",
        ]),
        (
            "SourceName",
            filter("SourceName", &["googlebot"]),
            vec!["tx-3"],
        ),
        (
            "SourceCategory",
            filter("SourceCategory", &["search_engine"]),
            vec!["tx-3"],
        ),
        ("Intent", filter("Intent", &["training"]), vec!["tx-2"]),
        (
            "Organization",
            filter("Organization", &["anthropic", "microsoft"]),
            vec!["tx-2", "tx-5"],
        ),
        // "The URI path of the request that was monetized" — a prefix, so one value
        // selects every transaction settled under it.
        (
            "UriPathPrefix, a branch",
            filter("UriPathPrefix", &["/api"]),
            vec!["tx-1", "tx-2", "tx-4", "tx-5"],
        ),
        (
            "UriPathPrefix, a whole path",
            filter("UriPathPrefix", &["/docs/intro"]),
            vec!["tx-3"],
        ),
        ("RequestId", filter("RequestId", &["req-4"]), vec!["tx-4"]),
        (
            "TransactionId",
            filter("TransactionId", &["tx-6"]),
            vec!["tx-6"],
        ),
        (
            "TerminatingRuleName",
            filter("TerminatingRuleName", &["monetize-crawlers"]),
            vec!["tx-3", "tx-5"],
        ),
        (
            "PayerAddress",
            filter("PayerAddress", &["0xPAYERB"]),
            vec!["tx-2"],
        ),
        (
            "HttpSourceId",
            filter("HttpSourceId", &["svc-7"]),
            vec!["tx-5"],
        ),
        (
            "a filter no transaction satisfies",
            filter("SourceName", &["nosuchbot"]),
            vec![],
        ),
    ];

    for (case, row, expected) in rows {
        let answer = waf
            .ok(
                OPERATION,
                with(
                    request(SCOPE_CLOUDFRONT, window(0, 7_200)),
                    json!({
                        "Filters": [both_modes(), row],
                        "SortBy": "TIMESTAMP",
                        "SortOrder": "ASC",
                    }),
                ),
            )
            .await;
        assert_eq!(
            ids(&answer),
            expected,
            "case {case:?}: expected {expected:?}, got {answer}"
        );
    }

    // Two filters are satisfied together, not one or the other: the two `ai`
    // transactions settled on `BASE`, which leaves out the `ai` one settled on the
    // Base testnet and the `search_engine` one settled on `BASE`.
    let answer = waf
        .ok(
            OPERATION,
            with(
                request(SCOPE_CLOUDFRONT, window(0, 7_200)),
                json!({
                    "Filters": [
                        both_modes(),
                        filter("SourceCategory", &["ai"]),
                        filter("ChainName", &["BASE"]),
                    ],
                    "SortBy": "TIMESTAMP",
                    "SortOrder": "ASC",
                }),
            ),
        )
        .await;
    assert_eq!(
        ids(&answer),
        vec!["tx-1".to_string(), "tx-6".to_string()],
        "case {:?}: every filter must be satisfied, got {answer}",
        "two filters"
    );
}

#[tokio::test]
async fn sort_by_and_sort_order_decide_the_order_of_the_selection() {
    let (waf, _arn) = seeded().await;

    // The fixture, by each sortable field: `TIMESTAMP` is its ledger order,
    // `AMOUNT` is 0.0005 < 0.002 < 0.004 < 0.010 < 0.025 < 0.500 (a quantity, not
    // the string it is spelled with), `NAME` is the source name and `STATUS` the
    // settlement status, both by their own value. Two transactions the field
    // cannot separate keep their ledger order, so `DESC` is `ASC` reversed.
    let by_timestamp = vec!["tx-1", "tx-2", "tx-3", "tx-4", "tx-5", "tx-6"];
    let by_amount = vec!["tx-5", "tx-3", "tx-1", "tx-2", "tx-4", "tx-6"];
    let by_name = vec!["tx-5", "tx-2", "tx-3", "tx-1", "tx-4", "tx-6"];
    let by_status = vec!["tx-5", "tx-3", "tx-2", "tx-6", "tx-1", "tx-4"];

    for (case, sort_by, expected) in [
        ("TIMESTAMP", "TIMESTAMP", by_timestamp.clone()),
        ("AMOUNT", "AMOUNT", by_amount),
        ("NAME", "NAME", by_name),
        ("STATUS", "STATUS", by_status),
    ] {
        for (order, expected) in [
            ("ASC", expected.clone()),
            ("DESC", expected.iter().rev().copied().collect()),
        ] {
            let answer = waf
                .ok(
                    OPERATION,
                    with(
                        request(SCOPE_CLOUDFRONT, window(0, 7_200)),
                        json!({
                            "Filters": [both_modes()],
                            "SortBy": sort_by,
                            "SortOrder": order,
                        }),
                    ),
                )
                .await;
            assert_eq!(
                ids(&answer),
                expected,
                "case {case:?} / {order}: expected {expected:?}, got {answer}"
            );
        }
    }

    // A request that names neither reports the newest transaction first.
    let answer = waf
        .ok(
            OPERATION,
            with(
                request(SCOPE_CLOUDFRONT, window(0, 7_200)),
                json!({ "Filters": [both_modes()] }),
            ),
        )
        .await;
    let newest_first: Vec<String> = by_timestamp
        .iter()
        .rev()
        .map(|id| (*id).to_string())
        .collect();
    assert_eq!(
        ids(&answer),
        newest_first,
        "case {:?}: expected {newest_first:?}, got {answer}",
        "no SortBy and no SortOrder"
    );
}

#[tokio::test]
async fn a_limit_cuts_the_selection_into_pages_a_marker_resumes() {
    let (waf, _arn) = seeded().await;
    let whole: Vec<String> = ["tx-1", "tx-2", "tx-3", "tx-4", "tx-5", "tx-6"]
        .iter()
        .map(|id| (*id).to_string())
        .collect();

    // Every page size from one record to one past the selection: the pages of a
    // walk concatenate to the whole selection, each page holds at most `Limit`
    // records, and only the page that ends the selection carries no marker.
    for limit in 1..=7_usize {
        let case = format!("Limit={limit}");
        let mut marker: Option<String> = None;
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let mut body = with(
                request(SCOPE_CLOUDFRONT, window(0, 7_200)),
                json!({
                    "Filters": [both_modes()],
                    "SortBy": "TIMESTAMP",
                    "SortOrder": "ASC",
                    "Limit": limit,
                }),
            );
            if let Some(marker) = marker.as_deref() {
                body["NextMarker"] = json!(marker);
            }
            let answer = waf.ok(OPERATION, body).await;
            let page = ids(&answer);
            assert!(
                page.len() <= limit,
                "case {case:?}: a page holds at most {limit} records, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: a walk must terminate, got {walked:?}"
            );
            match answer["NextMarker"].as_str() {
                Some(next) => marker = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: a walk must report the whole selection exactly once, \
             got {walked:?}"
        );
        let expected_pages = whole.len().div_ceil(limit).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
    }

    // A marker naming no transaction of this selection names no position to resume
    // from, so the page it asks for is empty and ends the walk.
    for marker in ["00000000000000009999", "not-a-cursor"] {
        let case = format!("NextMarker={marker:?}");
        let answer = waf
            .ok(
                OPERATION,
                with(
                    request(SCOPE_CLOUDFRONT, window(0, 7_200)),
                    json!({ "Filters": [both_modes()], "NextMarker": marker }),
                ),
            )
            .await;
        assert_eq!(
            answer["Settlements"],
            json!([]),
            "case {case:?}: an unknown marker resumes nothing, got {answer}"
        );
        assert!(
            answer["NextMarker"].is_null(),
            "case {case:?}: an empty page carries no marker, got {answer}"
        );
    }
}

#[tokio::test]
async fn a_reported_record_carries_the_transaction_as_the_ledger_holds_it() {
    let (waf, arn) = seeded().await;

    let answer = waf
        .ok(
            OPERATION,
            with(
                request(SCOPE_CLOUDFRONT, window(0, 7_200)),
                json!({ "Filters": [filter("TransactionId", &["tx-2"])] }),
            ),
        )
        .await;
    let record = &answer["Settlements"][0];
    // Every member the ledger holds for that transaction, as the model names it.
    for (member, expected) in [
        ("TransactionId", json!("tx-2")),
        ("RequestId", json!("req-2")),
        ("Status", json!("PENDING")),
        // "The payment amount in the specified currency": the quantity the ledger
        // holds, written at the scale every reported amount is written to
        // (`state::MONETIZATION_AMOUNT_SCALE`).
        ("Amount", json!("0.010000")),
        // "The currency for the amounts in the response" is the one the request
        // asked them in.
        ("Currency", json!(CURRENCY)),
        ("Network", json!("SOLANA")),
        ("PayerAddress", json!("0xPAYERB")),
        ("WalletAddress", json!("0xWALLET")),
        ("SourceName", json!("claudebot")),
        ("SourceCategory", json!("ai")),
        ("Organization", json!("anthropic")),
        ("Intent", json!("training")),
        ("ContentPath", json!("/api/v2/orders")),
        ("Verified", json!(true)),
        ("WebAclArn", json!(arn)),
        ("Timestamp", json!(BASE_SECS + 20)),
    ] {
        assert_eq!(
            record[member], expected,
            "case {member:?}: expected {expected}, got {record}"
        );
    }
}

#[tokio::test]
async fn listing_settlement_records_changes_nothing_a_later_caller_sees() {
    let (waf, engine) = Waf::sharing_engine();
    let arn = monetized_web_acl(&waf, "read-only-acl").await;
    seed_ledger(&engine, SCOPE_CLOUDFRONT, ledger_fixture(&arn));
    let before = ledger_len(&engine, SCOPE_CLOUDFRONT);

    let body = with(
        request(SCOPE_CLOUDFRONT, window(0, 7_200)),
        json!({ "Filters": [both_modes()], "Limit": 2 }),
    );
    let first = waf.ok(OPERATION, body.clone()).await;
    let second = waf.ok(OPERATION, body).await;
    assert_eq!(
        first, second,
        "case {:?}: a read must answer the same twice, got {first} then {second}",
        "the same query twice"
    );
    assert_eq!(
        ledger_len(&engine, SCOPE_CLOUDFRONT),
        before,
        "case {:?}: a read must not change the ledger",
        "the ledger after two reads"
    );
}

#[tokio::test]
async fn the_regional_namespace_is_refused_whatever_else_the_request_asks() {
    let (waf, engine) = Waf::sharing_engine();
    seed_ledger(
        &engine,
        SCOPE_REGIONAL,
        ledger_fixture("arn:aws:wafv2:us-west-2:123456789012:regional/webacl/x/0"),
    );

    // Monetization is a CloudFront-delivery feature, so under the regional namespace
    // this operation does not exist: the modelled `WAFInvalidOperationException`,
    // whose `Message` says what is unavailable and which scope to use. The window,
    // the filters and the page size make no difference to that.
    for (case, extra) in [
        ("bare", json!({})),
        ("filtered", json!({ "Filters": [both_modes()] })),
        ("paged", json!({ "Limit": 1 })),
    ] {
        let body = with(
            with(request(SCOPE_REGIONAL, window(0, 7_200)), extra),
            json!({}),
        );
        let refusal = waf
            .refused(OPERATION, body, "WAFInvalidOperationException", case)
            .await;
        assert_eq!(
            error_message(&refusal),
            "Monetization is not available for regional resources. Use CloudFront \
             scope to configure monetization.",
            "case {case:?}: the refusal must carry WAF's monetization sentence, got \
             {refusal}"
        );
        assert!(
            refusal["Settlements"].is_null(),
            "case {case:?}: a refusal carries no page, got {refusal}"
        );
    }
}

#[tokio::test]
async fn a_window_past_the_published_maximum_is_refused_and_the_maximum_itself_is_served() {
    let (waf, _arn) = seeded().await;
    let day = 24 * 60 * 60;

    // "The maximum supported time window is 90 days": the published bound is
    // served, and one second past it is refused.
    for (case, span) in [
        ("one day", day),
        ("half the maximum", MAX_MONETIZATION_WINDOW_DAYS / 2 * day),
        ("the maximum", MAX_MONETIZATION_WINDOW_DAYS * day),
    ] {
        let answer = waf
            .ok(OPERATION, request(SCOPE_CLOUDFRONT, window(0, span)))
            .await;
        assert!(
            answer["Settlements"].is_array(),
            "case {case:?}: a window within the maximum must be served, got {answer}"
        );
    }
    for (case, span) in [
        ("one second past the maximum", MAX_MONETIZATION_WINDOW_DAYS * day + 1),
        ("a year", 365 * day),
    ] {
        let refusal = waf
            .refused(
                OPERATION,
                request(SCOPE_CLOUDFRONT, window(0, span)),
                "WAFInvalidParameterException",
                case,
            )
            .await;
        assert_eq!(
            refusal["Parameter"],
            json!("TimeWindow"),
            "case {case:?}: the refusal must name the window, got {refusal}"
        );
        assert!(
            error_message(&refusal)
                .contains(&format!("{MAX_MONETIZATION_WINDOW_DAYS} days")),
            "case {case:?}: the refusal must state the maximum, got {refusal}"
        );
    }
}

#[tokio::test]
async fn a_filter_outside_the_documented_vocabulary_is_refused() {
    let (waf, _arn) = seeded().await;

    // A name this operation does not filter by, and a value outside an
    // enum-restricted name's vocabulary ("enum-restricted (invalid values
    // rejected)") — including the right value in the wrong case.
    for (case, row, parameter) in [
        ("an unknown name", filter("Nope", &["x"]), "Nope"),
        (
            "a name of another API",
            filter("BotCategory", &["ai"]),
            "BotCategory",
        ),
        (
            "CurrencyMode",
            filter("CurrencyMode", &["real"]),
            "real",
        ),
        (
            "ChainName",
            filter("ChainName", &["ETHEREUM"]),
            "ETHEREUM",
        ),
        (
            "SettlementStatus",
            filter("SettlementStatus", &["SETTLED", "UNKNOWN"]),
            "UNKNOWN",
        ),
        (
            "HttpSourceName",
            filter("HttpSourceName", &["CLOUDFRONT"]),
            "CLOUDFRONT",
        ),
    ] {
        let refusal = waf
            .refused(
                OPERATION,
                with(
                    request(SCOPE_CLOUDFRONT, window(0, 7_200)),
                    json!({ "Filters": [row] }),
                ),
                "WAFInvalidParameterException",
                case,
            )
            .await;
        assert_eq!(
            refusal["Parameter"],
            json!(parameter),
            "case {case:?}: the refusal must carry the rejected value, got {refusal}"
        );
        assert!(
            refusal["Settlements"].is_null(),
            "case {case:?}: a refusal carries no page, got {refusal}"
        );
    }

    // `WebACLArn` is "a valid WAFv2 web ACL ARN", so an ARN that is malformed or
    // names a resource of another kind is the ARN failure WAF words for every
    // parameter that takes one.
    let (_ip_set_id, ip_set_arn, _ip_set_lock) =
        create_ip_set(&waf, SCOPE_CLOUDFRONT, "not-a-web-acl").await;
    for (case, value) in [
        ("not an ARN at all", "web-acl".to_string()),
        ("an IP set", ip_set_arn),
    ] {
        let refusal = waf
            .refused(
                OPERATION,
                with(
                    request(SCOPE_CLOUDFRONT, window(0, 7_200)),
                    json!({ "Filters": [filter("WebACLArn", &[value.as_str()])] }),
                ),
                "WAFInvalidParameterException",
                case,
            )
            .await;
        assert_eq!(
            refusal["Field"],
            json!("RESOURCE_ARN"),
            "case {case:?}: an ARN failure names the ARN, got {refusal}"
        );
        assert_eq!(
            refusal["Parameter"],
            json!(value),
            "case {case:?}: the refusal must carry the rejected ARN, got {refusal}"
        );
    }
}

#[tokio::test]
async fn a_member_outside_its_modelled_constraint_is_refused_by_the_request_validator() {
    let (waf, _arn) = seeded().await;

    // `Limit` is `range 1..100`, `Scope` and `Currency` are enums, and `Filters` is
    // `length 1..20`: every one of those is the request validator's own refusal,
    // which names the member in lowerCamel.
    for (case, extra, member) in [
        ("Limit=0", json!({ "Limit": 0 }), "limit"),
        ("Limit=101", json!({ "Limit": 101 }), "limit"),
        ("Filters=[]", json!({ "Filters": [] }), "filters"),
    ] {
        let (status, body) = waf
            .call(
                OPERATION,
                with(request(SCOPE_CLOUDFRONT, window(0, 3_600)), extra),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a member-constraint failure must be refused, got status \
             {status} and body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: a member-constraint failure is the request validator's \
             own refusal, got {body}"
        );
        assert!(
            error_message(&body)
                .contains(&format!("at '{member}' failed to satisfy constraint:")),
            "case {case:?}: the clause must name {member}, got {body}"
        );
    }

    for (case, body) in [
        (
            "an unmodelled scope",
            json!({ "Scope": "GLOBAL", "Currency": CURRENCY, "TimeWindow": window(0, 3_600) }),
        ),
        (
            "an unmodelled currency",
            json!({ "Scope": SCOPE_CLOUDFRONT, "Currency": "USD", "TimeWindow": window(0, 3_600) }),
        ),
    ] {
        let (status, parsed) = waf.call(OPERATION, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unmodelled enum value must be refused, got status \
             {status} and body {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case:?}: an enum failure is the request validator's own refusal, \
             got {parsed}"
        );
    }
}
