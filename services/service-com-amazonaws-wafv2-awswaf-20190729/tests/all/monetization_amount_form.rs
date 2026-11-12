// SPDX-License-Identifier: CC0-1.0

//! The textual form of a `MonetizationAmountValue`, end to end over the wire.
//!
//! `MonetizationAmountValue` is a STRING in the model with no pattern
//! (`smithy_query(query="shape",shape="MonetizationAmountValue")`), so its form is
//! the service's own, and the RULE this suite asserts is: every member of that
//! shape a response carries is written as a fixed-scale decimal — an integer part,
//! `.`, and exactly [`SCALE`] fractional digits, the smallest unit of the one
//! `Currency` the model declares — whatever places the quantity behind it was
//! written in, so a client can hand the member straight to a fixed-scale decimal
//! parser.
//!
//! The suite asserts that over a TABLE of amounts spanning the class: whole
//! numbers, fewer places than the scale, exactly the scale, finer than the scale,
//! a figure much larger than one unit, and a ledger value that spells no number at
//! all. Each row is asserted on every operation that serves the shape — the
//! settlement listing (`SettlementRecord$Amount`), the revenue summary
//! (`RevenueBreakdown$TotalAmount` / `$VerifiedAmount` / `$UnverifiedAmount`) and
//! the revenue time series (`DataPointEntry$TotalAmount`).
//!
//! WAF records a settlement in its DATA plane, when a rule's `Monetize` action
//! settles a payment for a request a web ACL evaluated, and this emulator serves
//! only the WAFv2 control plane, so no operation fills that ledger: these cases
//! seed it through the state API (`crate::settlement_records::seed_ledger`) and
//! then drive every assertion through the router's front door, the same door an
//! SDK uses.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::ops::types::SettlementRecord;
use service_com_amazonaws_wafv2_awswaf_20190729::state::{SettlementRecordEntry, SCOPE_CLOUDFRONT};

use crate::settlement_records::{
    at, base_record, entry, monetized_web_acl, request, seed_ledger, window, with,
};
use crate::web_acl_lifecycle::Waf;

/// The operations that serve a `MonetizationAmountValue`, named once.
const LISTING: &str = "ListSettlementRecords";
const SUMMARY: &str = "GetRevenueStatisticsSummary";
const SERIES: &str = "GetRevenueStatisticsTimeSeries";

/// How many fractional digits every reported amount carries: the smallest unit of
/// `USDC`, the one denomination `Currency` declares
/// (`smithy_query(query="shape",shape="Currency")`), is one millionth of a token,
/// and a recorded response writes a zero balance as `0.000000`.
const SCALE: usize = 6;

/// A ledger of two settled transactions ten seconds apart, the first paid by a
/// verified bot and the second by an unverified one, so one query reports each
/// amount on its own tier and both in the total.
fn ledger(
    web_acl_arn: &str,
    verified_amount: &str,
    unverified_amount: &str,
) -> Vec<SettlementRecordEntry> {
    [(10, verified_amount, true), (20, unverified_amount, false)]
        .into_iter()
        .map(|(seconds, amount, verified)| {
            entry(
                SettlementRecord {
                    timestamp: at(seconds),
                    amount: amount.to_string(),
                    status: "SETTLED".to_string(),
                    verified,
                    network: Some("BASE".to_string()),
                    transaction_id: Some(format!("tx-{seconds}")),
                    request_id: Some(format!("req-{seconds}")),
                    ..base_record(web_acl_arn)
                },
                "monetize-ai",
                ("CF", "E1AMOUNTS"),
            )
        })
        .collect()
}

/// Assert that `value` is written in the one form a `MonetizationAmountValue`
/// takes: digits, `.`, and exactly [`SCALE`] digits.
fn assert_fixed_scale(value: &str, member: &str, case: &str) {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    let (integer, fraction) = value.split_once('.').unwrap_or((value, ""));
    assert!(
        digits(integer) && fraction.len() == SCALE && digits(fraction),
        "case {case:?}: {member} must be an integer part, '.' and exactly {SCALE} \
         fractional digits, got {value:?}"
    );
}

/// The string a response member carries, or the empty string when it carries none.
fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

#[tokio::test]
async fn every_reported_monetization_amount_is_written_at_the_currency_scale() {
    // Each row differs in what the rule generalises over: how many decimal places
    // the quantity behind the amount was written in, how large it is, and whether
    // it spells a number at all. The expected figures are that quantity at the
    // scale every reported amount is written to — a quantity finer than the
    // smallest unit rounded to the nearest one, and a value that spells no number
    // contributing nothing.
    for (case, amounts, expected) in [
        (
            "whole numbers",
            ("3", "4"),
            ("3.000000", "4.000000", "7.000000"),
        ),
        (
            "fewer places than the scale",
            ("0.5", "1.25"),
            ("0.500000", "1.250000", "1.750000"),
        ),
        (
            "three places",
            ("0.004", "0.250"),
            ("0.004000", "0.250000", "0.254000"),
        ),
        (
            "exactly the smallest unit",
            ("0.000001", "0.000002"),
            ("0.000001", "0.000002", "0.000003"),
        ),
        (
            "finer than the smallest unit",
            ("0.0000004", "0.0000006"),
            ("0.000000", "0.000001", "0.000001"),
        ),
        (
            "much larger than one unit",
            ("1234567.891234", "0.108766"),
            ("1234567.891234", "0.108766", "1234568.000000"),
        ),
        (
            "a ledger value that spells no number",
            ("not-an-amount", "2.5"),
            ("0.000000", "2.500000", "2.500000"),
        ),
    ] {
        let (verified_amount, unverified_amount) = amounts;
        let (expected_verified, expected_unverified, expected_total) = expected;
        let (waf, engine) = Waf::sharing_engine();
        let arn = monetized_web_acl(&waf, "amount-form-acl").await;
        seed_ledger(
            &engine,
            SCOPE_CLOUDFRONT,
            ledger(&arn, verified_amount, unverified_amount),
        );
        let time_window = window(0, 3_600);

        // The listing reports each transaction's own amount.
        let listing = waf
            .ok(LISTING, request(SCOPE_CLOUDFRONT, time_window.clone()))
            .await;
        let listed: Vec<String> = listing["Settlements"]
            .as_array()
            .map(|records| {
                records
                    .iter()
                    .map(|record| text(&record["Amount"]))
                    .collect()
            })
            .unwrap_or_default();
        for amount in &listed {
            assert_fixed_scale(amount, "SettlementRecord$Amount", case);
        }
        let mut expected_listed = vec![
            expected_verified.to_string(),
            expected_unverified.to_string(),
        ];
        let mut sorted = listed.clone();
        sorted.sort();
        expected_listed.sort();
        assert_eq!(
            sorted, expected_listed,
            "case {case:?}: the listing reports each amount at the currency scale, \
             got {listed:?} in {listing}"
        );

        // The summary reports the sum and the two tiers.
        let summary = waf
            .ok(SUMMARY, request(SCOPE_CLOUDFRONT, time_window.clone()))
            .await;
        let breakdown = &summary["RevenueBreakdown"];
        for member in ["TotalAmount", "VerifiedAmount", "UnverifiedAmount"] {
            assert_fixed_scale(
                &text(&breakdown[member]),
                &format!("RevenueBreakdown${member}"),
                case,
            );
        }
        let reported = (
            text(&breakdown["TotalAmount"]),
            text(&breakdown["VerifiedAmount"]),
            text(&breakdown["UnverifiedAmount"]),
        );
        assert_eq!(
            reported,
            (
                expected_total.to_string(),
                expected_verified.to_string(),
                expected_unverified.to_string()
            ),
            "case {case:?}: expected (TotalAmount, VerifiedAmount, \
             UnverifiedAmount) ({expected_total:?}, {expected_verified:?}, \
             {expected_unverified:?}), got {reported:?} in {summary}"
        );

        // The series reports the same sum on the bucket both transactions fall in.
        let series = waf
            .ok(
                SERIES,
                with(
                    request(SCOPE_CLOUDFRONT, time_window),
                    json!({ "StatisticType": "DATE_HISTOGRAM", "Interval": "DAILY" }),
                ),
            )
            .await;
        let point_amounts: Vec<String> = series["DataPoints"]
            .as_array()
            .map(|points| {
                points
                    .iter()
                    .map(|point| text(&point["TotalAmount"]))
                    .collect()
            })
            .unwrap_or_default();
        for amount in &point_amounts {
            assert_fixed_scale(amount, "DataPointEntry$TotalAmount", case);
        }
        assert_eq!(
            point_amounts,
            vec![expected_total.to_string()],
            "case {case:?}: the one bucket both transactions fall in reports the \
             total at the currency scale, got {point_amounts:?} in {series}"
        );
    }
}

#[tokio::test]
async fn a_scope_that_settled_nothing_reports_a_zero_balance_at_the_currency_scale() {
    // The form does not depend on the ledger holding anything: a window that
    // settled nothing reports zero, and zero is written at the same scale as every
    // other amount.
    let waf = Waf::new();
    let summary = waf
        .ok(SUMMARY, request(SCOPE_CLOUDFRONT, window(0, 3_600)))
        .await;
    let breakdown = &summary["RevenueBreakdown"];
    for member in ["TotalAmount", "VerifiedAmount", "UnverifiedAmount"] {
        let reported = text(&breakdown[member]);
        assert_fixed_scale(&reported, member, "a scope that settled nothing");
        assert_eq!(
            reported, "0.000000",
            "case {:?}: {member} must report a zero balance at the currency scale, \
             got {reported:?} in {summary}",
            "a scope that settled nothing"
        );
    }
}
