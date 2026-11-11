// SPDX-License-Identifier: CC0-1.0

//! What `DescribeContributorInsights` REPORTS, over and above the enable/disable
//! lifecycle its sibling suite pins (`crate::contributor_insights`, whose helpers this
//! one drives the operations through).
//!
//! The rules pinned here:
//!
//! 1. the report names the STORED table, as a bare table name, whatever form the request
//!    addressed it in — a bare name or the table's ARN ("You can also provide the Amazon
//!    Resource Name (ARN) of the table in this parameter",
//!    `DescribeContributorInsights$TableName`) — and echoes `IndexName` exactly when the
//!    request named one, because the setting reported is that index's rather than the
//!    table's;
//! 2. `ContributorInsightsRuleList` names the CloudWatch Contributor Insights rules the
//!    resource has — the ones an enable minted, and none once a disable has deleted them —
//!    while no `FailureException` is reported in any status, since nothing here fails a
//!    rule;
//! 3. `LastUpdateDateTime` is "Timestamp of the last time the status was changed"
//!    (`DescribeContributorInsightsOutput$LastUpdateDateTime`): absent while the status
//!    has never been set, never in the future, the same across reads that report the same
//!    status, and later once a later status is reported;
//! 4. the operation REPORTS — "Returns information about contributor insights for a given
//!    table or global secondary index" — so reading a setting, once or repeatedly, changes
//!    neither what it reports next nor what any other resource reports.
//!
//! Every rule is asserted over a SET of inputs: the table name's size and vocabulary, the
//! form it is addressed in, the resource addressed, the status it is read in, and how many
//! times it has been read.

use serde_json::{json, Value};

use crate::contributor_insights::{create_table, describe_insights, set_insights, settle};
use crate::emulator::{emulator, ok};

/// The epoch seconds a report's `LastUpdateDateTime` carries, or `None` where the report
/// omits the member.
fn last_update(report: &Value) -> Option<f64> {
    report.get("LastUpdateDateTime").and_then(Value::as_f64)
}

/// Now, in the epoch seconds a timestamp is reported in.
fn now_epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// One row of the status table below: the case it names, the action to ask for first and
/// whether to wait out its span, and the status a read then reports.
type StatusRow = (&'static str, Option<(&'static str, bool)>, &'static str);

/// The member a report omits reads as `Null`, whether the body left it out or carried it
/// null.
fn reported(report: &Value, member: &str) -> Value {
    report.get(member).cloned().unwrap_or(Value::Null)
}

#[tokio::test]
async fn the_report_names_the_stored_table_and_echoes_only_an_index_the_request_named() {
    let router = emulator();

    // Rows differ in the stored name's size and vocabulary, which is what the addressing
    // rule generalises over: the shortest a table name may be, one carrying every
    // punctuation character the name form permits, and a long one. Both come from
    // `smithy_query(query="shape", shape="TableName")` — length 3..255, pattern
    // `^[a-zA-Z0-9_.-]+$`.
    let names = [
        "Ab3".to_string(),
        "a.b-c_D9".to_string(),
        "Insights".repeat(20),
    ];
    for name in &names {
        create_table(&router, name, &["Reported.Index"]).await;
    }
    settle().await;

    for name in &names {
        let described = ok(&router, "DescribeTable", json!({"TableName": name})).await;
        let arn = described["Table"]["TableArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            arn.ends_with(&format!("table/{name}")),
            "case {name:?}: the stored table has an ARN to address it by, got {described}"
        );

        for (form, addressed) in [("its bare name", name.clone()), ("its table ARN", arn)] {
            for (resource, index) in [
                ("the table itself", None),
                ("one of its indexes", Some("Reported.Index")),
            ] {
                let report = describe_insights(&router, &addressed, index).await;
                assert_eq!(
                    (reported(&report, "TableName"), reported(&report, "IndexName")),
                    (
                        json!(name),
                        index.map(|index| json!(index)).unwrap_or(Value::Null)
                    ),
                    "case {name:?} addressed by {form}, {resource}: the report names the stored \
                     table bare and echoes only an index the request named, got {report}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_rules_the_resource_has_are_named_and_no_failure_is_reported_in_any_status() {
    let router = emulator();
    create_table(&router, "InsightsReported", &["Reported.Index"]).await;
    settle().await;

    // Rows span the statuses a setting can be read in: never set, then each half of both
    // transitions. The action column is what this row asks for first, and whether it waits
    // out the transient span before reading.
    let cases: [StatusRow; 5] = [
        ("never set", None, "DISABLED"),
        ("the enable in flight", Some(("ENABLE", false)), "ENABLING"),
        ("the enable settled", Some(("ENABLE", true)), "ENABLED"),
        ("the disable in flight", Some(("DISABLE", false)), "DISABLING"),
        ("the disable settled", Some(("DISABLE", true)), "DISABLED"),
    ];

    for (case, action, expected) in cases {
        if let Some((action, wait)) = action {
            set_insights(&router, "InsightsReported", None, action, None).await;
            if wait {
                settle().await;
            }
        }
        for (resource, index) in [
            ("the table", None),
            ("its untouched index", Some("Reported.Index")),
        ] {
            let report = describe_insights(&router, "InsightsReported", index).await;
            // The untouched index stays off through every row: only the table was named.
            let expected_status = if index.is_some() { "DISABLED" } else { expected };
            // The rules named are the ones the feature has: an enable mints them for the
            // resource it names, so the table carries them from the enable until the disable
            // deletes them, and the index that was never enabled carries none.
            let rules_expected = index.is_none() && expected.starts_with("ENABL");
            let rules = reported(&report, "ContributorInsightsRuleList");
            let named = rules.as_array().map_or(0, Vec::len);
            assert_eq!(
                (
                    reported(&report, "ContributorInsightsStatus"),
                    named > 0,
                    reported(&report, "FailureException"),
                ),
                (json!(expected_status), rules_expected, Value::Null),
                "case {case} on {resource}: the status is reported with the resource's own rules \
                 and no failure, got {report}"
            );
        }
    }
}

#[tokio::test]
async fn the_timestamp_reported_is_when_the_status_last_changed() {
    let router = emulator();
    create_table(&router, "InsightsTimed", &["Timed.Index"]).await;
    settle().await;

    // Rows are the two kinds of resource one table carries, each timing its own
    // transitions.
    for (case, index) in [("the table", None), ("its index", Some("Timed.Index"))] {
        let untouched = describe_insights(&router, "InsightsTimed", index).await;
        assert_eq!(
            last_update(&untouched),
            None,
            "case {case}: a status that has never been set carries no timestamp, got {untouched}"
        );

        set_insights(&router, "InsightsTimed", index, "ENABLE", None).await;
        let in_flight = describe_insights(&router, "InsightsTimed", index).await;
        let read_again = describe_insights(&router, "InsightsTimed", index).await;
        let requested_at = last_update(&in_flight).unwrap_or_default();
        assert!(
            in_flight["ContributorInsightsStatus"] == json!("ENABLING")
                && requested_at > 0.0
                && requested_at <= now_epoch_seconds()
                && last_update(&read_again) == Some(requested_at)
                && read_again["ContributorInsightsStatus"] == json!("ENABLING"),
            "case {case}: the in-flight status carries the past instant it was reached and reads \
             the same twice, got {in_flight} then {read_again}"
        );

        settle().await;
        let settled = describe_insights(&router, "InsightsTimed", index).await;
        let settled_again = describe_insights(&router, "InsightsTimed", index).await;
        let settled_at = last_update(&settled).unwrap_or_default();
        assert!(
            settled["ContributorInsightsStatus"] == json!("ENABLED")
                && settled_at > requested_at
                && settled_at <= now_epoch_seconds()
                && last_update(&settled_again) == Some(settled_at),
            "case {case}: the settled status carries the later instant it was reached, got \
             {settled} then {settled_again} against the in-flight {requested_at}"
        );
    }
}

#[tokio::test]
async fn reading_a_setting_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    create_table(&router, "InsightsPolled", &["Polled.Index"]).await;
    settle().await;
    set_insights(
        &router,
        "InsightsPolled",
        None,
        "ENABLE",
        Some("THROTTLED_KEYS"),
    )
    .await;
    settle().await;

    let first = describe_insights(&router, "InsightsPolled", None).await;
    assert_eq!(
        (
            reported(&first, "ContributorInsightsStatus"),
            reported(&first, "ContributorInsightsMode"),
        ),
        (json!("ENABLED"), json!("THROTTLED_KEYS")),
        "case the settled enable: the report is what the update asked for, got {first}"
    );

    // Rows differ in how many reads have gone before: a client polling the report must be
    // served the same answer every time, however often it asks, and its polling must turn
    // nothing on or off for anyone else.
    for reads in [1, 2, 8, 32] {
        for _ in 0..reads {
            describe_insights(&router, "InsightsPolled", None).await;
        }
        let report = describe_insights(&router, "InsightsPolled", None).await;
        assert_eq!(
            report, first,
            "case after {reads} further reads: the report is unchanged, got {report}"
        );

        // The index no update named is still off: reading the table did not reach it.
        let index = describe_insights(&router, "InsightsPolled", Some("Polled.Index")).await;
        assert_eq!(
            (
                reported(&index, "ContributorInsightsStatus"),
                reported(&index, "ContributorInsightsMode"),
                last_update(&index),
            ),
            (json!("DISABLED"), Value::Null, None),
            "case after {reads} further reads: the resource the reads did not name is untouched, \
             got {index}"
        );

        // The other read surface of the same store agrees, so no read has drifted one from
        // the other.
        let listed = ok(
            &router,
            "ListContributorInsights",
            json!({"TableName": "InsightsPolled"}),
        )
        .await;
        let summary = listed["ContributorInsightsSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|summary| summary["IndexName"] == Value::Null)
            .unwrap_or(Value::Null);
        assert_eq!(
            (
                reported(&summary, "ContributorInsightsStatus"),
                reported(&summary, "ContributorInsightsMode"),
            ),
            (
                reported(&first, "ContributorInsightsStatus"),
                reported(&first, "ContributorInsightsMode"),
            ),
            "case after {reads} further reads: the listing reports what the description does, \
             got {listed}"
        );
    }
}
