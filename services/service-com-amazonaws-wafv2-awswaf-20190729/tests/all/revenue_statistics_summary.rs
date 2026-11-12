// SPDX-License-Identifier: CC0-1.0

//! `GetRevenueStatisticsSummary`, end to end over the wire.
//!
//! The operation "retrieves a summary of monetization revenue for the specified
//! time window", so the RULE this suite asserts is: the answer is ONE aggregate
//! over the scope's settlement ledger — the transactions whose settlement instant
//! the `TimeWindow` contains and which satisfy every filter the request applied
//! (production transactions only, unless a `CurrencyMode` filter asks otherwise) —
//! reporting one HTTP 402 answer per selected transaction, the count of those whose
//! `Status` is `SETTLED`, and the exact decimal sum of the `Amount` of exactly
//! those settled transactions, split by `Verified` into two tiers that add up to
//! that total, each amount written in the fixed-scale decimal form this service
//! reports a `MonetizationAmountValue` in; and the call REPORTS: it changes
//! nothing a later caller sees.
//!
//! Every suite asserts that over a TABLE rather than over one request: windows that
//! cover all, part and none of the ledger, filters over the currency mode, the
//! settlement status, the source name and the content path, a ledger spanning every
//! modelled `SettlementStatus` and both verification tiers, and each refusal — the
//! regional namespace, a window past the published maximum, an unknown filter and a
//! value outside an enum-restricted filter's vocabulary.
//!
//! WAF records a settlement in its DATA plane, when a rule's `Monetize` action
//! settles a payment for a request a web ACL evaluated, and this emulator serves
//! only the WAFv2 control plane, so no operation fills that ledger: the aggregation
//! cases seed it through the state API (`crate::settlement_records::seed_ledger`)
//! and then drive every assertion through the router's front door, the same door an
//! SDK uses.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::ops::types::SettlementRecord;
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    monetization_amount_total, SettlementRecordEntry, MAX_MONETIZATION_WINDOW_DAYS,
    SCOPE_CLOUDFRONT, SCOPE_REGIONAL,
};

use crate::settlement_records::{
    at, base_record, entry, ledger_len, monetized_web_acl, request, seed_ledger, window, with,
    CURRENCY,
};
use crate::web_acl_lifecycle::Waf;

/// The operation under test, named once.
const OPERATION: &str = "GetRevenueStatisticsSummary";

/// The listing over the same ledger, for the cross-check that both operations
/// report the same selection.
const LISTING: &str = "ListSettlementRecords";

/// The amount a selection that settled nothing comes to: the sum of no amount,
/// written at the scale every reported amount is written to
/// (`state::MONETIZATION_AMOUNT_SCALE`).
const NO_REVENUE: &str = "0.000000";

/// Whole seconds in one day, the unit the published window maximum is stated in.
const DAY_SECS: i64 = 24 * 60 * 60;

/// The figures one answer reports: `(TotalAmount, VerifiedAmount,
/// UnverifiedAmount, TotalSettled, TotalMonetizeServed)`.
type Breakdown = (String, String, String, i64, i64);

/// The `RevenueBreakdown` of an answer, as [`Breakdown`] states it.
fn breakdown(answer: &Value) -> Breakdown {
    let found = &answer["RevenueBreakdown"];
    let amount = |member: &str| {
        found[member]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    (
        amount("TotalAmount"),
        amount("VerifiedAmount"),
        amount("UnverifiedAmount"),
        found["TotalSettled"].as_i64().unwrap_or(-1),
        found["TotalMonetizeServed"].as_i64().unwrap_or(-1),
    )
}

/// Assert that `answer` reports `expected`, naming `case` and what it carried.
fn assert_breakdown(answer: &Value, expected: (&str, &str, &str, i64, i64), case: &str) {
    let actual = breakdown(answer);
    let expected: Breakdown = (
        expected.0.to_string(),
        expected.1.to_string(),
        expected.2.to_string(),
        expected.3,
        expected.4,
    );
    assert_eq!(
        actual, expected,
        "case {case:?}: expected (TotalAmount, VerifiedAmount, UnverifiedAmount, \
         TotalSettled, TotalMonetizeServed) {expected:?}, got {actual:?} in {answer}"
    );
    assert_eq!(
        answer["RevenueBreakdown"]["Currency"],
        json!(CURRENCY),
        "case {case:?}: the breakdown is denominated in the currency the request \
         asked for, got {answer}"
    );
}

/// One ledger entry, from the members this suite's rule turns on: when the payment
/// settled, what it came to, the status it settled under, whether the paying bot
/// was verified, and `paid`: the network it settled on, the name of the bot that
/// paid and the path it paid for.
fn settlement(
    web_acl_arn: &str,
    seconds: i64,
    amount: &str,
    status: &str,
    verified: bool,
    paid: (&str, &str, &str),
) -> SettlementRecordEntry {
    let (network, source_name, content_path) = paid;
    entry(
        SettlementRecord {
            timestamp: at(seconds),
            amount: amount.to_string(),
            status: status.to_string(),
            verified,
            network: Some(network.to_string()),
            source_name: Some(source_name.to_string()),
            content_path: Some(content_path.to_string()),
            transaction_id: Some(format!("tx-{seconds}")),
            request_id: Some(format!("req-{seconds}")),
            ..base_record(web_acl_arn)
        },
        "monetize-ai",
        ("CF", "E1REVENUE"),
    )
}

/// The ledger the aggregation cases read: nine transactions spanning every value of
/// the model's `SettlementStatus` (`smithy_query(query="shape",
/// shape="SettlementStatus")`), both verification tiers, two bot names, two content
/// paths and both currency modes — the last one settled on a testnet network
/// (`BASE_SEPOLIA`), so it lies outside the production default.
///
/// The seeded amounts are written to three decimals, so every expected sum below is
/// the exact decimal the operands spell, reported at the scale every amount is
/// written to.
fn ledger(web_acl_arn: &str) -> Vec<SettlementRecordEntry> {
    let settled = |seconds, amount, verified, paid| {
        settlement(web_acl_arn, seconds, amount, "SETTLED", verified, paid)
    };
    let unsettled = |seconds, amount, status, verified, network| {
        settlement(
            web_acl_arn,
            seconds,
            amount,
            status,
            verified,
            (network, "gptbot", "/api/v1/users"),
        )
    };
    vec![
        settled(10, "0.004", true, ("BASE", "gptbot", "/api/v1/users")),
        settled(20, "0.250", false, ("BASE", "claudebot", "/api/v1/users")),
        unsettled(30, "1.000", "PENDING", true, "BASE"),
        unsettled(40, "2.000", "FAILED", false, "SOLANA"),
        settled(50, "0.001", true, ("SOLANA", "gptbot", "/docs/intro")),
        unsettled(60, "0.500", "SKIPPED_ORIGIN_ERROR", true, "BASE"),
        unsettled(70, "0.750", "DUPLICATE", false, "BASE"),
        unsettled(80, "0.125", "SERVICE_ERROR", true, "SOLANA"),
        settled(
            90,
            "0.008",
            true,
            ("BASE_SEPOLIA", "gptbot", "/api/v1/users"),
        ),
    ]
}

/// An emulator whose CLOUDFRONT ledger holds [`ledger`], with the shared engine and
/// the ARN of the web ACL the transactions were settled for.
async fn seeded() -> (Waf, Arc<engine::InMemoryEngine>, String) {
    let (waf, engine) = Waf::sharing_engine();
    let arn = monetized_web_acl(&waf, "revenue-acl").await;
    seed_ledger(&engine, SCOPE_CLOUDFRONT, ledger(&arn));
    (waf, engine, arn)
}

/// A one-filter `Filters` list, as the member a request carries it in.
fn filters(name: &str, values: &[&str]) -> Value {
    json!({ "Filters": [{ "Name": name, "Values": values }] })
}

#[tokio::test]
async fn a_scope_that_settled_nothing_reports_a_zeroed_breakdown_over_every_window() {
    let waf = Waf::new();
    // Windows of every width, including the widest the operation supports: the
    // answer is the zeroed summary, not a refusal and not an absent breakdown.
    for (case, time_window) in [
        ("one second", window(0, 1)),
        ("one hour forward", window(0, 3_600)),
        ("the three hours before", window(-10_800, 0)),
        ("one day", window(-DAY_SECS, 0)),
        (
            "the published maximum",
            window(0, MAX_MONETIZATION_WINDOW_DAYS * DAY_SECS),
        ),
    ] {
        let answer = waf
            .ok(OPERATION, request(SCOPE_CLOUDFRONT, time_window))
            .await;
        assert_breakdown(&answer, (NO_REVENUE, NO_REVENUE, NO_REVENUE, 0, 0), case);
    }
}

#[tokio::test]
async fn the_breakdown_aggregates_the_transactions_the_window_and_the_filters_select() {
    let (waf, _engine, _arn) = seeded().await;

    // Each row differs in what the rule generalises over: how much of the ledger
    // the window covers, and which transactions a filter leaves in. The expected
    // figures are the HTTP 402 answers served (one per selected transaction), the
    // settled transactions among them, and the exact sum of their amounts split by
    // verification tier.
    for (case, time_window, extra, expected) in [
        (
            "the whole ledger, production transactions by default",
            window(0, 3_600),
            json!({}),
            ("0.255000", "0.005000", "0.250000", 3, 8),
        ),
        (
            "a window covering the first two transactions",
            window(0, 25),
            json!({}),
            ("0.254000", "0.004000", "0.250000", 2, 2),
        ),
        (
            "a window covering one unsettled transaction",
            window(35, 45),
            json!({}),
            (NO_REVENUE, NO_REVENUE, NO_REVENUE, 0, 1),
        ),
        (
            "a window after every transaction",
            window(3_000, 3_600),
            json!({}),
            (NO_REVENUE, NO_REVENUE, NO_REVENUE, 0, 0),
        ),
        (
            "the testnet mode alone",
            window(0, 3_600),
            filters("CurrencyMode", &["TEST"]),
            ("0.008000", "0.008000", NO_REVENUE, 1, 1),
        ),
        (
            "both currency modes",
            window(0, 3_600),
            filters("CurrencyMode", &["REAL", "TEST"]),
            ("0.263000", "0.013000", "0.250000", 4, 9),
        ),
        (
            "the settled status alone",
            window(0, 3_600),
            filters("SettlementStatus", &["SETTLED"]),
            ("0.255000", "0.005000", "0.250000", 3, 3),
        ),
        (
            "two statuses that settled nothing",
            window(0, 3_600),
            filters("SettlementStatus", &["FAILED", "DUPLICATE"]),
            (NO_REVENUE, NO_REVENUE, NO_REVENUE, 0, 2),
        ),
        (
            "one bot by name",
            window(0, 3_600),
            filters("SourceName", &["claudebot"]),
            ("0.250000", NO_REVENUE, "0.250000", 1, 1),
        ),
        (
            "one content path by prefix",
            window(0, 3_600),
            filters("UriPathPrefix", &["/docs"]),
            ("0.001000", "0.001000", NO_REVENUE, 1, 1),
        ),
        (
            "a content path nothing was paid for",
            window(0, 3_600),
            filters("UriPathPrefix", &["/private"]),
            (NO_REVENUE, NO_REVENUE, NO_REVENUE, 0, 0),
        ),
    ] {
        let answer = waf
            .ok(
                OPERATION,
                with(request(SCOPE_CLOUDFRONT, time_window), extra),
            )
            .await;
        assert_breakdown(&answer, expected, case);
    }
}

#[tokio::test]
async fn amounts_of_different_decimal_places_add_up_to_exactly_what_they_spell() {
    // A revenue total is a sum of money written in decimal, so it must come to
    // exactly the figure the operands spell — including the pairs a binary float
    // cannot hold (0.1 + 0.2), places finer than the operands share, and whole
    // numbers — and it is reported at the one scale every amount is written to,
    // whatever the operands' own places. A quantity finer than the currency's
    // smallest unit cannot be written at that scale, so it is reported rounded to
    // the nearest unit. A ledger value that spells no decimal number contributes
    // nothing rather than deciding the answer.
    for (case, amounts, expected) in [
        (
            "three places",
            ["0.004", "0.250", "0.001"].as_slice(),
            "0.255000",
        ),
        (
            "the classic float pair",
            ["0.1", "0.2"].as_slice(),
            "0.300000",
        ),
        ("mixed places", ["1", "2.5"].as_slice(), "3.500000"),
        ("whole numbers", ["3", "4"].as_slice(), "7.000000"),
        (
            "the currency's smallest unit",
            ["0.000001", "0.000002"].as_slice(),
            "0.000003",
        ),
        (
            "half a unit, finer than the scale holds",
            ["0.000001", "0.0000005"].as_slice(),
            "0.000002",
        ),
        ("one amount", ["12.500"].as_slice(), "12.500000"),
        (
            "a value that spells no number",
            ["12.5", "not-an-amount"].as_slice(),
            "12.500000",
        ),
        ("nothing at all", [].as_slice(), NO_REVENUE),
    ] {
        let (waf, engine) = Waf::sharing_engine();
        let arn = monetized_web_acl(&waf, &format!("acl-{}", amounts.len())).await;
        let ledger: Vec<SettlementRecordEntry> = amounts
            .iter()
            .enumerate()
            .map(|(index, amount)| {
                settlement(
                    &arn,
                    10 * (index as i64 + 1),
                    amount,
                    "SETTLED",
                    true,
                    ("BASE", "gptbot", "/api/v1/users"),
                )
            })
            .collect();
        let settled = i64::try_from(ledger.len()).unwrap_or(-1);
        seed_ledger(&engine, SCOPE_CLOUDFRONT, ledger);

        let answer = waf
            .ok(OPERATION, request(SCOPE_CLOUDFRONT, window(0, 3_600)))
            .await;
        assert_breakdown(
            &answer,
            (expected, expected, NO_REVENUE, settled, settled),
            case,
        );
    }
}

#[tokio::test]
async fn the_two_verification_tiers_always_add_up_to_the_total() {
    let (waf, _engine, _arn) = seeded().await;

    // The invariant behind the breakdown: `Verified` partitions the settled
    // transactions, so whatever a window and a filter select, the two tiers come to
    // the total.
    for (case, time_window, extra) in [
        ("the whole ledger", window(0, 3_600), json!({})),
        (
            "both currency modes",
            window(0, 3_600),
            filters("CurrencyMode", &["REAL", "TEST"]),
        ),
        ("part of the ledger", window(0, 55), json!({})),
        ("none of it", window(3_000, 3_600), json!({})),
        (
            "one bot by name",
            window(0, 3_600),
            filters("SourceName", &["gptbot"]),
        ),
    ] {
        let answer = waf
            .ok(
                OPERATION,
                with(request(SCOPE_CLOUDFRONT, time_window), extra),
            )
            .await;
        let (total, verified, unverified, _settled, _served) = breakdown(&answer);
        let tiers = monetization_amount_total([verified.as_str(), unverified.as_str()]);
        assert_eq!(
            tiers, total,
            "case {case:?}: the two tiers must come to the total, got {verified} + \
             {unverified} = {tiers} against {total}"
        );
    }
}

#[tokio::test]
async fn the_summary_and_the_listing_report_the_same_selection() {
    let (waf, _engine, _arn) = seeded().await;

    // Both operations read the one ledger over the one selection, so the summary's
    // counts and total are the listing's own records over the same request.
    for (case, extra) in [
        ("production transactions by default", json!({})),
        (
            "both currency modes",
            filters("CurrencyMode", &["REAL", "TEST"]),
        ),
        (
            "the settled status alone",
            filters("SettlementStatus", &["SETTLED"]),
        ),
        ("one bot by name", filters("SourceName", &["gptbot"])),
    ] {
        let body = with(request(SCOPE_CLOUDFRONT, window(0, 3_600)), extra);
        let summary = waf.ok(OPERATION, body.clone()).await;
        let listing = waf.ok(LISTING, body).await;

        let listed: Vec<Value> = listing["Settlements"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let settled: Vec<&Value> = listed
            .iter()
            .filter(|record| record["Status"] == json!("SETTLED"))
            .collect();
        let expected_total = monetization_amount_total(
            settled
                .iter()
                .map(|record| record["Amount"].as_str().unwrap_or_default()),
        );
        let (total, _verified, _unverified, total_settled, total_served) = breakdown(&summary);

        assert_eq!(
            total_served,
            i64::try_from(listed.len()).unwrap_or(-1),
            "case {case:?}: one HTTP 402 answer per selected transaction, got \
             {total_served} against {} listed records",
            listed.len()
        );
        assert_eq!(
            total_settled,
            i64::try_from(settled.len()).unwrap_or(-1),
            "case {case:?}: the settled count is the listing's own settled records, \
             got {total_settled} against {}",
            settled.len()
        );
        assert_eq!(
            total, expected_total,
            "case {case:?}: the total is the sum of the listed settled amounts, got \
             {total} against {expected_total}"
        );
    }
}

#[tokio::test]
async fn reporting_the_summary_changes_nothing_a_later_caller_sees() {
    let (waf, engine, _arn) = seeded().await;
    let before = ledger_len(&engine, SCOPE_CLOUDFRONT);

    let body = with(
        request(SCOPE_CLOUDFRONT, window(0, 3_600)),
        filters("CurrencyMode", &["REAL", "TEST"]),
    );
    let first = waf.ok(OPERATION, body.clone()).await;
    let second = waf.ok(OPERATION, body).await;
    assert_eq!(
        first, second,
        "case {:?}: the same query must answer the same summary twice, got {first} \
         then {second}",
        "the same query twice"
    );
    let after = ledger_len(&engine, SCOPE_CLOUDFRONT);
    assert_eq!(
        after, before,
        "case {:?}: a read must not change the ledger, got {after} against {before}",
        "the ledger after two reads"
    );
}

#[tokio::test]
async fn the_regional_namespace_is_refused_whatever_else_the_request_asks() {
    let (waf, engine) = Waf::sharing_engine();
    seed_ledger(
        &engine,
        SCOPE_REGIONAL,
        ledger("arn:aws:wafv2:us-west-2:123456789012:regional/webacl/regional-acl/0"),
    );

    // Monetization is a CloudFront-delivery feature, so under the regional namespace
    // this operation does not exist: the modelled `WAFInvalidOperationException`. The
    // window and the filters make no difference to that, and a regional ledger is not
    // summarised either.
    for (case, time_window, extra) in [
        ("bare", window(0, 3_600), json!({})),
        (
            "filtered",
            window(0, 3_600),
            filters("CurrencyMode", &["REAL", "TEST"]),
        ),
        ("over a narrow window", window(0, 1), json!({})),
        (
            "over the widest window",
            window(0, MAX_MONETIZATION_WINDOW_DAYS * DAY_SECS),
            json!({}),
        ),
    ] {
        let refusal = waf
            .refused(
                OPERATION,
                with(request(SCOPE_REGIONAL, time_window), extra),
                "WAFInvalidOperationException",
                case,
            )
            .await;
        assert_eq!(
            refusal["Message"].as_str().unwrap_or_default(),
            "Monetization is not available for regional resources. Use CloudFront \
             scope to configure monetization.",
            "case {case:?}: the refusal must carry WAF's monetization sentence, got \
             {refusal}"
        );
    }
}

#[tokio::test]
async fn a_query_outside_what_the_operation_supports_is_refused() {
    let (waf, _engine, arn) = seeded().await;

    for (case, body) in [
        (
            "a window one day past the published maximum",
            request(
                SCOPE_CLOUDFRONT,
                window(0, (MAX_MONETIZATION_WINDOW_DAYS + 1) * DAY_SECS),
            ),
        ),
        (
            "a window a year wide",
            request(SCOPE_CLOUDFRONT, window(0, 365 * DAY_SECS)),
        ),
        (
            "a filter name this operation does not filter by",
            with(
                request(SCOPE_CLOUDFRONT, window(0, 3_600)),
                filters("BotName", &["gptbot"]),
            ),
        ),
        (
            "a currency mode outside its vocabulary",
            with(
                request(SCOPE_CLOUDFRONT, window(0, 3_600)),
                filters("CurrencyMode", &["PLAY"]),
            ),
        ),
        (
            "a settlement status outside its vocabulary",
            with(
                request(SCOPE_CLOUDFRONT, window(0, 3_600)),
                filters("SettlementStatus", &["REFUNDED"]),
            ),
        ),
        (
            "a chain name outside its vocabulary",
            with(
                request(SCOPE_CLOUDFRONT, window(0, 3_600)),
                filters("ChainName", &["BITCOIN"]),
            ),
        ),
        (
            "an http source name outside its vocabulary",
            with(
                request(SCOPE_CLOUDFRONT, window(0, 3_600)),
                filters("HttpSourceName", &["LAMBDA"]),
            ),
        ),
        (
            "a web ACL filter whose value is not an ARN",
            with(
                request(SCOPE_CLOUDFRONT, window(0, 3_600)),
                filters("WebACLArn", &["revenue-acl"]),
            ),
        ),
    ] {
        waf.refused(OPERATION, body, "WAFInvalidParameterException", case)
            .await;
    }

    // The window exactly at the published maximum is inside it, and the web ACL
    // filter naming a real ARN selects the transactions settled for that web ACL:
    // the bound and the ARN form are what is refused above, not the query itself.
    for (case, body, expected) in [
        (
            "the published maximum exactly",
            request(
                SCOPE_CLOUDFRONT,
                window(0, MAX_MONETIZATION_WINDOW_DAYS * DAY_SECS),
            ),
            ("0.255000", "0.005000", "0.250000", 3, 8),
        ),
        (
            "the web ACL the transactions were settled for",
            with(
                request(SCOPE_CLOUDFRONT, window(0, 3_600)),
                filters("WebACLArn", &[arn.as_str()]),
            ),
            ("0.255000", "0.005000", "0.250000", 3, 8),
        ),
        (
            "a web ACL nothing was settled for",
            with(
                request(SCOPE_CLOUDFRONT, window(0, 3_600)),
                filters(
                    "WebACLArn",
                    &["arn:aws:wafv2:us-west-2:123456789012:global/webacl/other-acl/0"],
                ),
            ),
            (NO_REVENUE, NO_REVENUE, NO_REVENUE, 0, 0),
        ),
    ] {
        let answer = waf.ok(OPERATION, body).await;
        assert_breakdown(&answer, expected, case);
    }
}

#[tokio::test]
async fn a_member_outside_its_modelled_constraint_is_refused_by_the_request_validator() {
    let waf = Waf::new();

    // `Filters` is `length 1..20`, and `Scope` and `Currency` are enums: each of
    // those is the request validator's own refusal, before the handler runs.
    for (case, body) in [
        (
            "an empty filter list",
            with(
                request(SCOPE_CLOUDFRONT, window(0, 3_600)),
                json!({ "Filters": [] }),
            ),
        ),
        (
            "an unmodelled scope",
            json!({ "Scope": "GLOBAL", "Currency": CURRENCY, "TimeWindow": window(0, 3_600) }),
        ),
        (
            "an unmodelled currency",
            json!({ "Scope": SCOPE_CLOUDFRONT, "Currency": "USD", "TimeWindow": window(0, 3_600) }),
        ),
    ] {
        waf.refused(OPERATION, body, "ValidationException", case)
            .await;
    }
}
