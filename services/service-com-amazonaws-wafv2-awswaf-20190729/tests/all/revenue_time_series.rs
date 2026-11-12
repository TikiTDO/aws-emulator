// SPDX-License-Identifier: CC0-1.0

//! `GetRevenueStatisticsTimeSeries`, end to end over the wire.
//!
//! The operation "retrieves time series data for monetization revenue … aggregated
//! at the specified interval for the given time window", so the RULE these suites
//! assert is: the answer is the scope's settlement ledger — the transactions whose
//! settlement instant the `TimeWindow` contains and which satisfy every filter the
//! request applied (production transactions only, unless a `CurrencyMode` filter
//! asks otherwise) — split into `Interval`-wide buckets that start on a whole
//! multiple of the interval from the Unix epoch and, when the request names a
//! `GroupBy` dimension, into one point per value of that dimension in the bucket;
//! each point reports one HTTP 402 answer per selected transaction, the count of
//! those whose `Status` is `SETTLED`, and — for the `DATE_HISTOGRAM` statistic —
//! the exact decimal sum of the `Amount` of exactly those settled transactions,
//! written in the fixed-scale decimal form this service reports a
//! `MonetizationAmountValue` in; a
//! bucket no transaction falls in is no point; the series is cut into pages of at
//! most `Limit` points; and the call REPORTS: it changes nothing a later caller
//! sees.
//!
//! Every suite asserts that over a TABLE rather than over one request: every
//! modelled `IntervalType`, every `GroupByType` dimension, both
//! `TimeSeriesStatisticType`s, windows that cover all, part and none of the ledger,
//! every page size over a five-point series, and each refusal — the regional
//! namespace, a window past the published maximum, an unknown filter, a value
//! outside an enum-restricted filter's vocabulary and a member outside its
//! modelled constraint.
//!
//! WAF records a settlement in its DATA plane, when a rule's `Monetize` action
//! settles a payment for a request a web ACL evaluated, and this emulator serves
//! only the WAFv2 control plane, so no operation fills that ledger: the aggregation
//! cases seed it through the state API (`crate::settlement_records::seed_ledger`)
//! and then drive every assertion through the router's front door, the same door an
//! SDK uses.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::ops::types::SettlementRecord;
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    monetization_amount_total, SettlementRecordEntry, MAX_MONETIZATION_WINDOW_DAYS,
    SCOPE_CLOUDFRONT, SCOPE_REGIONAL,
};

use crate::settlement_records::{
    at, base_record, entry, filter, ledger_len, monetized_web_acl, request, seed_ledger, window,
    with, BASE_SECS,
};
use crate::web_acl_lifecycle::Waf;

/// The operation under test, named once.
const OPERATION: &str = "GetRevenueStatisticsTimeSeries";

/// The summary over the same ledger, for the cross-check that the series and the
/// summary report the same selection.
const SUMMARY: &str = "GetRevenueStatisticsSummary";

/// The revenue statistic: "DATE_HISTOGRAM for revenue over time"
/// (`smithy_query(query="shape",shape="TimeSeriesStatisticType")`).
const DATE_HISTOGRAM: &str = "DATE_HISTOGRAM";

/// The traffic statistic: "PAYMENT_TRAFFIC for payment traffic patterns".
const PAYMENT_TRAFFIC: &str = "PAYMENT_TRAFFIC";

/// The amount a bucket that settled nothing comes to: the sum of no amount,
/// written at the scale every reported amount is written to
/// (`state::MONETIZATION_AMOUNT_SCALE`).
const NO_REVENUE: &str = "0.000000";

/// Whole seconds in one day, the unit the published window maximum is stated in.
const DAY_SECS: i64 = 24 * 60 * 60;

/// Every value of the model's `IntervalType`, with the width in seconds of the
/// bucket each one aggregates into
/// (`smithy_query(query="shape",shape="IntervalType")`).
const INTERVALS: &[(&str, i64)] = &[
    ("MINUTELY", 60),
    ("FIVE_MINUTELY", 5 * 60),
    ("HOURLY", 60 * 60),
    ("DAILY", DAY_SECS),
];

/// What one data point reports: `(Date in whole seconds after [`BASE_SECS`],
/// MonetizeServedCount, SettledCount, TotalAmount)`. A point carrying no
/// `TotalAmount` reports it as the empty string, so an absent amount is a case this
/// table can state.
type Point = (i64, i64, i64, String);

/// A request for `scope`'s series over `time_window` at `interval`, carrying the
/// five members the model marks `@required`.
fn series(scope: &str, time_window: Value, interval: &str) -> Value {
    with(
        request(scope, time_window),
        json!({ "StatisticType": DATE_HISTOGRAM, "Interval": interval }),
    )
}

/// The `DataPoints` of an answer, as [`Point`] states each one.
fn points(answer: &Value) -> Vec<Point> {
    answer["DataPoints"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|point| {
                    (
                        (point["Date"].as_f64().unwrap_or_default() as i64) - BASE_SECS,
                        point["MonetizeServedCount"].as_i64().unwrap_or(-1),
                        point["SettledCount"].as_i64().unwrap_or(-1),
                        point["TotalAmount"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Assert that `answer` reports `expected`, naming `case` and what it carried.
fn assert_points(answer: &Value, expected: &[(i64, i64, i64, &str)], case: &str) {
    let actual = points(answer);
    let expected: Vec<Point> = expected
        .iter()
        .map(|(at, served, settled, amount)| (*at, *served, *settled, amount.to_string()))
        .collect();
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the data points (Date, MonetizeServedCount, \
         SettledCount, TotalAmount) {expected:?}, got {actual:?} in {answer}"
    );
}

/// One ledger entry, from the members this suite's rule turns on: when the payment
/// settled, what it came to, the status it settled under, and `bot`: the name of
/// the bot that paid, its category, the intent it declared and the organization
/// operating it.
fn settlement(
    web_acl_arn: &str,
    seconds: i64,
    amount: &str,
    status: &str,
    bot: (&str, &str, &str, &str),
) -> SettlementRecordEntry {
    let (source_name, source_category, intent, organization) = bot;
    entry(
        SettlementRecord {
            timestamp: at(seconds),
            amount: amount.to_string(),
            status: status.to_string(),
            network: Some("BASE".to_string()),
            source_name: Some(source_name.to_string()),
            source_category: Some(source_category.to_string()),
            intent: Some(intent.to_string()),
            organization: Some(organization.to_string()),
            transaction_id: Some(format!("tx-{seconds}")),
            request_id: Some(format!("req-{seconds}")),
            ..base_record(web_acl_arn)
        },
        "monetize-ai",
        ("CF", "E1REVENUE"),
    )
}

/// The ledger every aggregation case reads: six transactions settled on a
/// production network, spread so that each interval of [`INTERVALS`] buckets them
/// differently — two in one minute, three in one five-minute bucket, four in one
/// day — and differing in every dimension `GroupByType` names, with two of them
/// left unsettled so a bucket's settled count is not its served count.
fn ledger(web_acl_arn: &str) -> Vec<SettlementRecordEntry> {
    const GPTBOT: (&str, &str, &str, &str) = ("gptbot", "ai", "search", "openai");
    const CLAUDEBOT: (&str, &str, &str, &str) = ("claudebot", "ai", "training", "anthropic");
    const GOOGLEBOT: (&str, &str, &str, &str) = ("googlebot", "search_engine", "search", "google");
    vec![
        settlement(web_acl_arn, 10, "0.004", "SETTLED", GPTBOT),
        settlement(web_acl_arn, 70, "0.250", "SETTLED", CLAUDEBOT),
        settlement(web_acl_arn, 200, "1.000", "PENDING", GPTBOT),
        settlement(web_acl_arn, 3_700, "0.010", "SETTLED", GOOGLEBOT),
        settlement(web_acl_arn, 90_000, "0.002", "FAILED", GPTBOT),
        settlement(web_acl_arn, 90_010, "0.500", "SETTLED", GPTBOT),
    ]
}

/// An emulator whose CLOUDFRONT ledger holds [`ledger`], with the shared engine and
/// the ARN of the web ACL the transactions were settled for.
async fn seeded() -> (Waf, std::sync::Arc<engine::InMemoryEngine>, String) {
    let (waf, engine) = Waf::sharing_engine();
    let arn = monetized_web_acl(&waf, "series-acl").await;
    seed_ledger(&engine, SCOPE_CLOUDFRONT, ledger(&arn));
    (waf, engine, arn)
}

/// The window covering every transaction of [`ledger`].
fn whole_window() -> Value {
    window(0, 100_000)
}

#[tokio::test]
async fn a_scope_that_settled_nothing_reports_no_data_point_at_any_interval() {
    let waf = Waf::new();
    // Windows of every width at every interval: the answer is the empty series,
    // not a refusal and not a run of zeroed points.
    for (interval, _width) in INTERVALS {
        for (case, time_window) in [
            ("one second", window(0, 1)),
            ("one hour forward", window(0, 3_600)),
            ("the day before", window(-DAY_SECS, 0)),
            (
                "the published maximum",
                window(0, MAX_MONETIZATION_WINDOW_DAYS * DAY_SECS),
            ),
        ] {
            let case = format!("{interval} over {case}");
            let answer = waf
                .ok(OPERATION, series(SCOPE_CLOUDFRONT, time_window, interval))
                .await;
            assert_eq!(
                answer["DataPoints"],
                json!([]),
                "case {case:?}: a scope that settled nothing reports no data point, \
                 got {answer}"
            );
            assert!(
                answer["NextMarker"].is_null(),
                "case {case:?}: an empty series ends a paginator's loop, got {answer}"
            );
        }
    }
}

#[tokio::test]
async fn the_series_aggregates_the_selected_transactions_into_interval_buckets() {
    let (waf, _engine, _arn) = seeded().await;

    // One row per modelled interval: the same six transactions, bucketed at the
    // width the request asked for. Every bucket's date is a whole multiple of that
    // width, and a bucket no transaction falls in is no point at all.
    for (case, interval, expected) in [
        (
            "to the minute",
            "MINUTELY",
            vec![
                (0, 1, 1, "0.004000"),
                (60, 1, 1, "0.250000"),
                (180, 1, 0, NO_REVENUE),
                (3_660, 1, 1, "0.010000"),
                (90_000, 2, 1, "0.500000"),
            ],
        ),
        (
            "to five minutes",
            "FIVE_MINUTELY",
            vec![
                (0, 3, 2, "0.254000"),
                (3_600, 1, 1, "0.010000"),
                (90_000, 2, 1, "0.500000"),
            ],
        ),
        (
            "to the hour",
            "HOURLY",
            vec![
                (0, 3, 2, "0.254000"),
                (3_600, 1, 1, "0.010000"),
                (90_000, 2, 1, "0.500000"),
            ],
        ),
        (
            "to the day",
            "DAILY",
            vec![(0, 4, 3, "0.264000"), (86_400, 2, 1, "0.500000")],
        ),
    ] {
        let answer = waf
            .ok(OPERATION, series(SCOPE_CLOUDFRONT, whole_window(), interval))
            .await;
        assert_points(&answer, &expected, case);
    }
}

#[tokio::test]
async fn every_bucket_starts_on_a_whole_multiple_of_the_interval_and_the_series_runs_forward() {
    let (waf, _engine, _arn) = seeded().await;

    // The invariant behind a bucket: whatever window a caller asks over, each
    // point's date is a whole multiple of the interval from the epoch and the
    // series runs oldest first — so two callers asking over different windows see
    // the same buckets.
    for (interval, width) in INTERVALS {
        for (case, time_window) in [
            ("the whole ledger", whole_window()),
            ("a window starting mid-bucket", window(37, 100_000)),
            ("a window ending mid-bucket", window(0, 90_007)),
            ("a window inside one bucket", window(100, 250)),
        ] {
            let case = format!("{interval} over {case}");
            let answer = waf
                .ok(
                    OPERATION,
                    series(SCOPE_CLOUDFRONT, time_window, interval),
                )
                .await;
            let mut previous: Option<i64> = None;
            for (date, served, _settled, _amount) in points(&answer) {
                let absolute = BASE_SECS + date;
                assert_eq!(
                    absolute % width,
                    0,
                    "case {case:?}: a bucket starts on a whole multiple of {width} \
                     seconds, got {absolute} in {answer}"
                );
                assert!(
                    served > 0,
                    "case {case:?}: a point reports a bucket that holds a \
                     transaction, got {served} served in {answer}"
                );
                if let Some(previous) = previous {
                    assert!(
                        previous < date,
                        "case {case:?}: the series runs oldest bucket first, got \
                         {previous} before {date} in {answer}"
                    );
                }
                previous = Some(date);
            }
        }
    }
}

#[tokio::test]
async fn grouping_splits_each_bucket_by_the_value_of_the_dimension() {
    let (waf, _engine, arn) = seeded().await;

    // One row per `GroupByType` dimension, each over the same two daily buckets:
    // the dimension's value splits a bucket into one point per value, ordered by
    // that value, and every point's figures are its own transactions'.
    for (case, group_by, expected) in [
        (
            "by bot name",
            "NAME",
            vec![
                (0, "claudebot", 1, 1, "0.250000"),
                (0, "googlebot", 1, 1, "0.010000"),
                (0, "gptbot", 2, 1, "0.004000"),
                (86_400, "gptbot", 2, 1, "0.500000"),
            ],
        ),
        (
            "by bot category",
            "CATEGORY",
            vec![
                (0, "ai", 3, 2, "0.254000"),
                (0, "search_engine", 1, 1, "0.010000"),
                (86_400, "ai", 2, 1, "0.500000"),
            ],
        ),
        (
            "by declared intent",
            "INTENT",
            vec![
                (0, "search", 3, 2, "0.014000"),
                (0, "training", 1, 1, "0.250000"),
                (86_400, "search", 2, 1, "0.500000"),
            ],
        ),
        (
            "by organization",
            "ORGANIZATION",
            vec![
                (0, "anthropic", 1, 1, "0.250000"),
                (0, "google", 1, 1, "0.010000"),
                (0, "openai", 2, 1, "0.004000"),
                (86_400, "openai", 2, 1, "0.500000"),
            ],
        ),
        (
            "by web ACL",
            "WEBACL",
            vec![
                (0, arn.as_str(), 4, 3, "0.264000"),
                (86_400, arn.as_str(), 2, 1, "0.500000"),
            ],
        ),
    ] {
        let answer = waf
            .ok(
                OPERATION,
                with(
                    series(SCOPE_CLOUDFRONT, whole_window(), "DAILY"),
                    json!({ "GroupBy": group_by }),
                ),
            )
            .await;
        let figures: Vec<(i64, i64, i64, String)> = expected
            .iter()
            .map(|(date, _group, served, settled, amount)| {
                (*date, *served, *settled, amount.to_string())
            })
            .collect();
        let actual = points(&answer);
        assert_eq!(
            actual, figures,
            "case {case:?}: expected the grouped figures {figures:?}, got \
             {actual:?} in {answer}"
        );

        let groups: Vec<String> = answer["DataPoints"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|point| {
                        point["GroupByValue"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let expected_groups: Vec<String> = expected
            .iter()
            .map(|(_date, group, _served, _settled, _amount)| group.to_string())
            .collect();
        assert_eq!(
            groups, expected_groups,
            "case {case:?}: each point carries the dimension value it aggregates, \
             expected {expected_groups:?}, got {groups:?} in {answer}"
        );

        // `Category` is the value "when grouped by category" and `Intent` the one
        // "when grouped by intent", so each member is carried by exactly the
        // dimension it names and left out by every other.
        for (member, dimension) in [("Category", "CATEGORY"), ("Intent", "INTENT")] {
            for (index, point) in answer["DataPoints"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                let carried = point[member].as_str();
                let expected = (group_by == dimension).then(|| expected_groups[index].as_str());
                assert_eq!(
                    carried, expected,
                    "case {case:?}: point {index} must report {member} as \
                     {expected:?}, got {carried:?} in {point}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_payment_traffic_series_reports_the_traffic_counts_and_no_revenue_amount() {
    let (waf, _engine, _arn) = seeded().await;

    // The two statistic types retrieve the same buckets over the same selection:
    // the revenue series carries each bucket's revenue amount, the traffic series
    // the counts alone.
    for (interval, _width) in INTERVALS {
        let traffic = waf
            .ok(
                OPERATION,
                with(
                    series(SCOPE_CLOUDFRONT, whole_window(), interval),
                    json!({ "StatisticType": PAYMENT_TRAFFIC }),
                ),
            )
            .await;
        let revenue = waf
            .ok(
                OPERATION,
                series(SCOPE_CLOUDFRONT, whole_window(), interval),
            )
            .await;

        let case = format!("{interval} payment traffic");
        let counts = |answer: &Value| -> Vec<(i64, i64, i64)> {
            points(answer)
                .into_iter()
                .map(|(date, served, settled, _amount)| (date, served, settled))
                .collect()
        };
        assert_eq!(
            counts(&traffic),
            counts(&revenue),
            "case {case:?}: both statistics bucket the same selection, got \
             {traffic} against {revenue}"
        );
        for (index, point) in traffic["DataPoints"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            assert!(
                point["TotalAmount"].is_null(),
                "case {case:?}: point {index} of a traffic series carries no \
                 revenue amount, got {point}"
            );
        }
    }
}

#[tokio::test]
async fn the_series_is_cut_into_pages_of_at_most_the_requested_number_of_data_points() {
    let (waf, _engine, _arn) = seeded().await;

    let whole = points(
        &waf.ok(
            OPERATION,
            series(SCOPE_CLOUDFRONT, whole_window(), "MINUTELY"),
        )
        .await,
    );
    assert_eq!(
        whole.len(),
        5,
        "the minutely series over the seeded ledger holds five points, got \
         {whole:?}"
    );

    // Every page size over a five-point series, including one that holds the whole
    // series and one wider than it: the pages concatenate to the series, each holds
    // at most `Limit` points, and only the page that ends the series carries no
    // marker.
    for limit in [1_i64, 2, 3, 4, 5, 6, 10_000] {
        let case = format!("pages of {limit}");
        let mut collected: Vec<Point> = Vec::new();
        let mut marker: Option<String> = None;
        for _page in 0..=whole.len() {
            let mut body = with(
                series(SCOPE_CLOUDFRONT, whole_window(), "MINUTELY"),
                json!({ "Limit": limit }),
            );
            if let Some(marker) = marker.as_deref() {
                body = with(body, json!({ "NextMarker": marker }));
            }
            let answer = waf.ok(OPERATION, body).await;
            let page = points(&answer);
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= limit,
                "case {case:?}: a page holds at most {limit} points, got {page:?}"
            );
            collected.extend(page);
            marker = answer["NextMarker"].as_str().map(str::to_string);
            if marker.is_none() {
                break;
            }
        }
        assert!(
            marker.is_none(),
            "case {case:?}: the last page of a series carries no marker, got \
             {marker:?}"
        );
        assert_eq!(
            collected, whole,
            "case {case:?}: the pages must concatenate to the whole series, got \
             {collected:?} against {whole:?}"
        );
    }

    // A marker naming no point of this series names no position to resume from.
    let answer = waf
        .ok(
            OPERATION,
            with(
                series(SCOPE_CLOUDFRONT, whole_window(), "MINUTELY"),
                json!({ "NextMarker": "99999999999999999999" }),
            ),
        )
        .await;
    assert_eq!(
        answer["DataPoints"],
        json!([]),
        "case {:?}: a marker naming no point resumes nowhere, got {answer}",
        "a marker past the series"
    );
}

#[tokio::test]
async fn each_bucket_selects_what_the_window_and_the_filters_select() {
    let (waf, _engine, _arn) = seeded().await;

    // The window and the filters decide WHICH transactions are bucketed, so each
    // row below changes the selection and reads the buckets it leaves.
    for (case, time_window, extra, expected) in [
        (
            "a window covering the first two transactions",
            window(0, 100),
            json!({}),
            vec![(0, 2, 2, "0.254000")],
        ),
        (
            "a window covering one unsettled transaction",
            window(150, 250),
            json!({}),
            vec![(0, 1, 0, NO_REVENUE)],
        ),
        (
            "a window after every transaction",
            window(100_000, 100_500),
            json!({}),
            vec![],
        ),
        (
            "the settled status alone",
            whole_window(),
            json!({ "Filters": [filter("SettlementStatus", &["SETTLED"])] }),
            vec![(0, 3, 3, "0.264000"), (86_400, 1, 1, "0.500000")],
        ),
        (
            "one bot by name",
            whole_window(),
            json!({ "Filters": [filter("SourceName", &["claudebot"])] }),
            vec![(0, 1, 1, "0.250000")],
        ),
        (
            "the testnet mode, which nothing settled on",
            whole_window(),
            json!({ "Filters": [filter("CurrencyMode", &["TEST"])] }),
            vec![],
        ),
        (
            "a path nothing was paid for",
            whole_window(),
            json!({ "Filters": [filter("UriPathPrefix", &["/private"])] }),
            vec![],
        ),
    ] {
        let answer = waf
            .ok(
                OPERATION,
                with(series(SCOPE_CLOUDFRONT, time_window, "DAILY"), extra),
            )
            .await;
        assert_points(&answer, &expected, case);
    }
}

#[tokio::test]
async fn the_series_and_the_summary_report_the_same_selection() {
    let (waf, _engine, _arn) = seeded().await;

    // An interval partitions the selection, so whatever interval a caller asks for,
    // the points come to the summary's own figures over the same request.
    for (interval, _width) in INTERVALS {
        let body = series(SCOPE_CLOUDFRONT, whole_window(), interval);
        let answer = waf.ok(OPERATION, body.clone()).await;
        let summary = waf.ok(SUMMARY, body).await;

        let series_points = points(&answer);
        let served: i64 = series_points
            .iter()
            .map(|(_date, served, _settled, _amount)| served)
            .sum();
        let settled: i64 = series_points
            .iter()
            .map(|(_date, _served, settled, _amount)| settled)
            .sum();
        let total = monetization_amount_total(
            series_points
                .iter()
                .map(|(_date, _served, _settled, amount)| amount.as_str()),
        );
        let breakdown = &summary["RevenueBreakdown"];

        assert_eq!(
            served,
            breakdown["TotalMonetizeServed"].as_i64().unwrap_or(-1),
            "case {interval:?}: the points' served counts come to the summary's, \
             got {served} against {summary}"
        );
        assert_eq!(
            settled,
            breakdown["TotalSettled"].as_i64().unwrap_or(-1),
            "case {interval:?}: the points' settled counts come to the summary's, \
             got {settled} against {summary}"
        );
        assert_eq!(
            monetization_amount_total([total.as_str()]),
            monetization_amount_total([breakdown["TotalAmount"].as_str().unwrap_or_default()]),
            "case {interval:?}: the points' amounts come to the summary's total, \
             got {total} against {summary}"
        );
    }
}

#[tokio::test]
async fn reporting_the_series_changes_nothing_a_later_caller_sees() {
    let (waf, engine, _arn) = seeded().await;
    let before = ledger_len(&engine, SCOPE_CLOUDFRONT);

    let body = series(SCOPE_CLOUDFRONT, whole_window(), "HOURLY");
    let first = waf.ok(OPERATION, body.clone()).await;
    let second = waf.ok(OPERATION, body).await;
    assert_eq!(
        first, second,
        "case {:?}: the same query must answer the same series twice, got {first} \
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
    // this operation does not exist: the modelled `WAFInvalidOperationException`,
    // whatever interval, grouping or window the request asks for — and a regional
    // ledger is not aggregated either.
    for (interval, _width) in INTERVALS {
        for (case, extra) in [
            ("bare", json!({})),
            ("grouped", json!({ "GroupBy": "NAME" })),
            ("as payment traffic", json!({ "StatisticType": PAYMENT_TRAFFIC })),
        ] {
            let case = format!("{interval} {case}");
            let refusal = waf
                .refused(
                    OPERATION,
                    with(series(SCOPE_REGIONAL, whole_window(), interval), extra),
                    "WAFInvalidOperationException",
                    &case,
                )
                .await;
            assert_eq!(
                refusal["Message"].as_str().unwrap_or_default(),
                "Monetization is not available for regional resources. Use CloudFront \
                 scope to configure monetization.",
                "case {case:?}: the refusal must carry WAF's monetization sentence, \
                 got {refusal}"
            );
        }
    }
}

#[tokio::test]
async fn a_query_outside_what_the_operation_supports_is_refused() {
    let (waf, _engine, _arn) = seeded().await;

    for (case, body) in [
        (
            "a window one day past the published maximum",
            series(
                SCOPE_CLOUDFRONT,
                window(0, (MAX_MONETIZATION_WINDOW_DAYS + 1) * DAY_SECS),
                "DAILY",
            ),
        ),
        (
            "a window a year wide",
            series(SCOPE_CLOUDFRONT, window(0, 365 * DAY_SECS), "DAILY"),
        ),
        (
            "a filter name this operation does not filter by",
            with(
                series(SCOPE_CLOUDFRONT, whole_window(), "DAILY"),
                json!({ "Filters": [filter("BotName", &["gptbot"])] }),
            ),
        ),
        (
            "a currency mode outside its vocabulary",
            with(
                series(SCOPE_CLOUDFRONT, whole_window(), "DAILY"),
                json!({ "Filters": [filter("CurrencyMode", &["PLAY"])] }),
            ),
        ),
        (
            "a web ACL filter whose value is not an ARN",
            with(
                series(SCOPE_CLOUDFRONT, whole_window(), "DAILY"),
                json!({ "Filters": [filter("WebACLArn", &["series-acl"])] }),
            ),
        ),
    ] {
        waf.refused(OPERATION, body, "WAFInvalidParameterException", case)
            .await;
    }

    // The window exactly at the published maximum is inside it: the bound is what
    // is refused above, not the query itself.
    let answer = waf
        .ok(
            OPERATION,
            series(
                SCOPE_CLOUDFRONT,
                window(0, MAX_MONETIZATION_WINDOW_DAYS * DAY_SECS),
                "DAILY",
            ),
        )
        .await;
    assert_points(
        &answer,
        &[(0, 4, 3, "0.264000"), (86_400, 2, 1, "0.500000")],
        "the published maximum exactly",
    );
}

#[tokio::test]
async fn a_member_outside_its_modelled_constraint_is_refused_by_the_request_validator() {
    let waf = Waf::new();

    // `Limit` is `range 1..10000`, `Filters` is `length 1..20`, and `Interval`,
    // `StatisticType`, `GroupBy`, `Scope` and `Currency` are enums: each of those
    // is the request validator's own refusal, before the handler runs.
    for (case, body) in [
        (
            "a limit below the modelled minimum",
            with(
                series(SCOPE_CLOUDFRONT, whole_window(), "DAILY"),
                json!({ "Limit": 0 }),
            ),
        ),
        (
            "a limit above the modelled maximum",
            with(
                series(SCOPE_CLOUDFRONT, whole_window(), "DAILY"),
                json!({ "Limit": 10_001 }),
            ),
        ),
        (
            "an empty filter list",
            with(
                series(SCOPE_CLOUDFRONT, whole_window(), "DAILY"),
                json!({ "Filters": [] }),
            ),
        ),
        (
            "an unmodelled interval",
            series(SCOPE_CLOUDFRONT, whole_window(), "WEEKLY"),
        ),
        (
            "an unmodelled statistic type",
            with(
                series(SCOPE_CLOUDFRONT, whole_window(), "DAILY"),
                json!({ "StatisticType": "REVENUE_HISTOGRAM" }),
            ),
        ),
        (
            "an unmodelled group-by dimension",
            with(
                series(SCOPE_CLOUDFRONT, whole_window(), "DAILY"),
                json!({ "GroupBy": "RULE" }),
            ),
        ),
        (
            "an unmodelled scope",
            series("GLOBAL", whole_window(), "DAILY"),
        ),
    ] {
        waf.refused(OPERATION, body, "ValidationException", case)
            .await;
    }
}
