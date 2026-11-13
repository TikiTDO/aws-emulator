// SPDX-License-Identifier: CC0-1.0

//! `UpdateScheduledReport`, end to end over the wire, read back through
//! `GetScheduledReport`.
//!
//! The rule under test: an update is PARTIAL — "Only the parameters included in the
//! request are updated; all other properties remain unchanged"
//! (`smithy_query(query="doc",shape="UpdateScheduledReport")`). So every optional member
//! the request carries REPLACES the stored one, every member it omits leaves the stored
//! value exactly as it was, the `clear…` flags are the only way to unset an optional
//! member, and `updatedAt` moves to the instant of the call while `createdAt` does not
//! move at all.
//!
//! The rows below are a TABLE spanning that class rather than one request: they differ in
//! WHICH member is updated (name, description, dashboard, execution role, schedule
//! config, widget ids, date-range override, several at once, none at all), in the KIND of
//! change (replace a value, replace with a shorter member set, replace with the empty
//! list, unset through a `clear…` flag, leave alone through an absent member or a
//! `clear…` flag set to false) and in the FORM of the values (a cron and a rate
//! expression, an absolute and a relative date range, a schedule with and without an
//! explicit state). Every row asserts the WHOLE stored record afterwards, so a member the
//! request did not name cannot quietly change.
//!
//! Every call goes through the real router, so what these tests assert is what an SDK
//! client sees.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{call, error_code, ok_call, router, timestamp};
use super::resource_not_found::assert_not_found;
use super::scheduled_report_lifecycle::{source_dashboard, EXECUTION_ROLE_ARN};

/// A second well-formed `ServiceRoleArn` (pattern
/// `^arn:aws[-a-z0-9]*:iam::[0-9]{12}:role/[a-zA-Z0-9+=,.@_/-]+$`, source:
/// `smithy_query(query="operation",operation="UpdateScheduledReport")`), so a row can
/// replace the role the baseline report was created with.
const OTHER_EXECUTION_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/BCMDashboardsOtherRole";

/// A well-formed `DashboardArn` no dashboard is stored under, so a request naming it
/// references a missing resource rather than being malformed.
const MISSING_DASHBOARD_ARN: &str = "arn:aws:bcm-dashboards::123456789012:dashboard/no-such-board";

/// The create request every row starts from: every optional member of the stored record
/// is populated, so a row that replaces one and a row that unsets one are both visible.
fn baseline_request(dashboard_arn: &str, widget_id: &str, name: &str) -> Value {
    json!({
        "resourceTags": [{"key": "team", "value": "finops"}],
        "scheduledReport": {
            "name": name,
            "dashboardArn": dashboard_arn,
            "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
            "scheduleConfig": {
                "scheduleExpression": "cron(0 8 1 * ? *)",
                "scheduleExpressionTimeZone": "UTC",
                "state": "ENABLED"
            },
            "description": "Baseline description.",
            "widgetIds": [widget_id],
            "widgetDateRangeOverride": {
                "startTime": {"type": "ABSOLUTE", "value": "2025-01-01"},
                "endTime": {"type": "ABSOLUTE", "value": "2025-07-01"}
            }
        }
    })
}

/// One row: the update request (beyond `arn`), and the members of the stored record it
/// must change — `Some(value)` for a member that must read back as `value`, `None` for a
/// member that must be gone. Every other member must be untouched.
struct UpdateCase {
    label: &'static str,
    request: Value,
    expected: Vec<(&'static str, Option<Value>)>,
}

/// The table. `other_dashboard` and `other_widget` belong to a second stored dashboard, so
/// the rows that re-point the report name resources that exist.
fn cases(other_dashboard: &str, other_widget: &str) -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "arn only: nothing but updatedAt changes",
            request: json!({}),
            expected: vec![],
        },
        UpdateCase {
            label: "both clear flags false: nothing but updatedAt changes",
            request: json!({"clearWidgetIds": false, "clearWidgetDateRangeOverride": false}),
            expected: vec![],
        },
        UpdateCase {
            label: "name only",
            request: json!({"name": "Renamed Report-1"}),
            expected: vec![("name", Some(json!("Renamed Report-1")))],
        },
        UpdateCase {
            label: "description only",
            request: json!({"description": "A new description, with punctuation."}),
            expected: vec![(
                "description",
                Some(json!("A new description, with punctuation.")),
            )],
        },
        UpdateCase {
            // The report keeps the widget id it was created with, which belongs to the
            // dashboard it no longer points at — so the stored configuration no longer
            // resolves, and the health every view derives from the store says so.
            label: "dashboard re-pointed at another stored dashboard",
            request: json!({"dashboardArn": other_dashboard}),
            expected: vec![
                ("dashboardArn", Some(json!(other_dashboard))),
                (
                    "healthStatus",
                    Some(json!({
                        "statusCode": "UNHEALTHY",
                        "statusReasons": ["WIDGET_ID_NOT_FOUND"]
                    })),
                ),
            ],
        },
        UpdateCase {
            label: "execution role replaced",
            request: json!({"scheduledReportExecutionRoleArn": OTHER_EXECUTION_ROLE_ARN}),
            expected: vec![(
                "scheduledReportExecutionRoleArn",
                Some(json!(OTHER_EXECUTION_ROLE_ARN)),
            )],
        },
        UpdateCase {
            // A property of the submitted `scheduleConfig` is named only when that config
            // carries it, so the two properties this one names move and the time zone the
            // baseline carried stays exactly as stored.
            label: "schedule config replaced with a rate expression and DISABLED",
            request: json!({
                "scheduleConfig": {"scheduleExpression": "rate(7 days)", "state": "DISABLED"}
            }),
            expected: vec![(
                "scheduleConfig",
                Some(json!({
                    "scheduleExpression": "rate(7 days)",
                    "scheduleExpressionTimeZone": "UTC",
                    "state": "DISABLED"
                })),
            )],
        },
        UpdateCase {
            // No state is named, and nothing on this request can clear or re-default one,
            // so the state the baseline was created with stands — the ENABLED default the
            // model documents belongs to the create member alone (`ScheduleState` values,
            // source: the `ScheduleState` enum in the model). A report created DISABLED is
            // covered by `update_schedule_state_preservation`.
            label: "schedule config without a state keeps the stored state",
            request: json!({
                "scheduleConfig": {"scheduleExpression": "cron(0 6 1 1,4,7,10 ? *)"}
            }),
            expected: vec![(
                "scheduleConfig",
                Some(json!({
                    "scheduleExpression": "cron(0 6 1 1,4,7,10 ? *)",
                    "scheduleExpressionTimeZone": "UTC",
                    "state": "ENABLED"
                })),
            )],
        },
        UpdateCase {
            // The replacement names a widget of the OTHER dashboard, which the report
            // still does not point at — the same mismatch, reached from the other side,
            // so the derived health reports it too.
            label: "widget ids replaced with another dashboard's widget",
            request: json!({"widgetIds": [other_widget]}),
            expected: vec![
                ("widgetIds", Some(json!([other_widget]))),
                (
                    "healthStatus",
                    Some(json!({
                        "statusCode": "UNHEALTHY",
                        "statusReasons": ["WIDGET_ID_NOT_FOUND"]
                    })),
                ),
            ],
        },
        UpdateCase {
            // `WidgetIdList` admits the empty list (len ..1), and an empty list present is
            // not the same wire value as the member being absent.
            label: "widget ids replaced with the empty list",
            request: json!({"widgetIds": []}),
            expected: vec![("widgetIds", Some(json!([])))],
        },
        UpdateCase {
            label: "clearWidgetIds unsets the member",
            request: json!({"clearWidgetIds": true}),
            expected: vec![("widgetIds", None)],
        },
        UpdateCase {
            label: "date range override replaced with a relative range",
            request: json!({
                "widgetDateRangeOverride": {
                    "startTime": {"type": "RELATIVE", "value": "-P3M"},
                    "endTime": {"type": "RELATIVE", "value": "P0D"}
                }
            }),
            expected: vec![(
                "widgetDateRangeOverride",
                Some(json!({
                    "startTime": {"type": "RELATIVE", "value": "-P3M"},
                    "endTime": {"type": "RELATIVE", "value": "P0D"}
                })),
            )],
        },
        UpdateCase {
            label: "clearWidgetDateRangeOverride unsets the member",
            request: json!({"clearWidgetDateRangeOverride": true}),
            expected: vec![("widgetDateRangeOverride", None)],
        },
        UpdateCase {
            label: "both clear flags at once unset both members",
            request: json!({"clearWidgetIds": true, "clearWidgetDateRangeOverride": true}),
            expected: vec![("widgetIds", None), ("widgetDateRangeOverride", None)],
        },
        UpdateCase {
            label: "every updatable member at once",
            request: json!({
                "name": "Everything Changed",
                "description": "All of it.",
                "dashboardArn": other_dashboard,
                "scheduledReportExecutionRoleArn": OTHER_EXECUTION_ROLE_ARN,
                "scheduleConfig": {
                    "scheduleExpression": "cron(0 7 ? * MON *)",
                    "scheduleExpressionTimeZone": "Europe/Berlin",
                    "state": "DISABLED"
                },
                "widgetIds": [other_widget],
                "clearWidgetDateRangeOverride": true
            }),
            expected: vec![
                ("name", Some(json!("Everything Changed"))),
                ("description", Some(json!("All of it."))),
                ("dashboardArn", Some(json!(other_dashboard))),
                (
                    "scheduledReportExecutionRoleArn",
                    Some(json!(OTHER_EXECUTION_ROLE_ARN)),
                ),
                (
                    "scheduleConfig",
                    Some(json!({
                        "scheduleExpression": "cron(0 7 ? * MON *)",
                        "scheduleExpressionTimeZone": "Europe/Berlin",
                        "state": "DISABLED"
                    })),
                ),
                ("widgetIds", Some(json!([other_widget]))),
                ("widgetDateRangeOverride", None),
            ],
        },
    ]
}

/// Create a baseline report on `dashboard_arn` and return `(arn, the stored record)`.
async fn baseline_report(
    router: &MultiServiceRouter,
    dashboard_arn: &str,
    widget_id: &str,
    name: &str,
    label: &str,
) -> (String, Value) {
    let created = ok_call(
        router,
        "CreateScheduledReport",
        &baseline_request(dashboard_arn, widget_id, name),
    )
    .await;
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: CreateScheduledReport returned no arn: {created}"))
        .to_string();
    let stored = ok_call(router, "GetScheduledReport", &json!({"arn": arn})).await;
    let record = stored
        .get("scheduledReport")
        .unwrap_or_else(|| panic!("case {label}: no scheduledReport in {stored}"))
        .clone();
    (arn, record)
}

/// The stored record of `arn`, as `GetScheduledReport` answers it.
async fn stored_report(router: &MultiServiceRouter, arn: &str, label: &str) -> Value {
    let fetched = ok_call(router, "GetScheduledReport", &json!({"arn": arn})).await;
    fetched
        .get("scheduledReport")
        .unwrap_or_else(|| panic!("case {label}: no scheduledReport in {fetched}"))
        .clone()
}

/// A row's expected value for `member`, completed with the `schedulePeriod` the baseline
/// record carries where the row expects a `scheduleConfig`.
///
/// No row above submits a `schedulePeriod`, and a submitted config that names none leaves
/// the stored period alone — the period the create resolved from the defaults its two
/// bounds document. So the expectation inherits it rather than restating it: what the
/// bounds resolve to is the rule `tests/all/schedule_period_defaults.rs` pins, and this
/// suite is about which members an update moves.
fn with_stored_period(member: &str, expected: &Value, before: &Value) -> Value {
    let mut expected = expected.clone();
    if member != "scheduleConfig" {
        return expected;
    }
    let stored = before
        .get("scheduleConfig")
        .and_then(|config| config.get("schedulePeriod"))
        .cloned()
        .unwrap_or_else(|| {
            panic!("the baseline record's scheduleConfig carries a resolved schedulePeriod: {before}")
        });
    expected
        .as_object_mut()
        .expect("a scheduleConfig expectation is a JSON object")
        .insert("schedulePeriod".to_string(), stored);
    expected
}

/// Each row's update replaces exactly the members it carries, unsets exactly the members
/// its `clear…` flags name, leaves every other member of the stored record untouched, and
/// moves `updatedAt` to the instant of the call without moving `createdAt`.
#[tokio::test]
async fn an_update_changes_exactly_the_members_the_request_carries() {
    let router = router();
    let (dashboard, widget) = source_dashboard(&router, "UpdateSource").await;
    let (other_dashboard, other_widget) = source_dashboard(&router, "UpdateOtherSource").await;

    for (index, case) in cases(&other_dashboard, &other_widget)
        .into_iter()
        .enumerate()
    {
        let label = case.label;
        let (arn, before) = baseline_report(
            &router,
            &dashboard,
            &widget,
            &format!("Baseline Report-{index}"),
            label,
        )
        .await;

        // The clock has to have moved between the create and the update for `updatedAt`
        // to be observably the update's own instant rather than the create's.
        tokio::time::sleep(Duration::from_millis(5)).await;

        let mut request = case.request.clone();
        request["arn"] = json!(arn);
        let answered = ok_call(&router, "UpdateScheduledReport", &request).await;
        assert_eq!(
            answered.get("arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {label}: the update answers the arn it updated, got {answered}"
        );

        let after = stored_report(&router, &arn, label).await;

        // What the record must read back as: the record before the call, with exactly the
        // row's changes applied.
        let mut expected = before.clone();
        let members = expected
            .as_object_mut()
            .unwrap_or_else(|| panic!("case {label}: the stored record is not an object: {before}"));
        for (member, value) in &case.expected {
            match value {
                Some(value) => {
                    members.insert((*member).to_string(), with_stored_period(member, value, &before));
                }
                None => {
                    members.remove(*member);
                }
            }
        }

        // `updatedAt` is the one member every update moves, asserted on its own; the
        // create's instant is the floor it has to have moved past.
        let created_before = timestamp(&before, "createdAt", label);
        let updated_before = timestamp(&before, "updatedAt", label);
        let updated_after = timestamp(&after, "updatedAt", label);
        assert!(
            updated_after > updated_before,
            "case {label}: expected updatedAt to move past {updated_before}, got {updated_after}"
        );
        assert_eq!(
            timestamp(&after, "createdAt", label),
            created_before,
            "case {label}: createdAt must not move, got {after}"
        );
        members.insert("updatedAt".to_string(), after["updatedAt"].clone());

        assert_eq!(
            after, expected,
            "case {label}: expected the stored record to read back as {expected}, got {after}"
        );

        // An update is not a tagging operation, so the tags the create stored are
        // untouched by it.
        let tags = ok_call(
            &router,
            "ListTagsForResource",
            &json!({"resourceArn": arn}),
        )
        .await;
        assert_eq!(
            tags["resourceTags"],
            json!([{"key": "team", "value": "finops"}]),
            "case {label}: an update must change no tag, got {tags}"
        );
    }
}

/// Two updates compose: the second one's absent members keep what the first one wrote, so
/// the stored record is the accumulation of the parameters each call included.
#[tokio::test]
async fn successive_updates_each_change_only_their_own_members() {
    let router = router();
    let (dashboard, widget) = source_dashboard(&router, "ComposeSource").await;
    let label = "successive updates";
    let (arn, before) =
        baseline_report(&router, &dashboard, &widget, "Composed Report", label).await;

    ok_call(
        &router,
        "UpdateScheduledReport",
        &json!({"arn": arn, "name": "Composed Once"}),
    )
    .await;
    ok_call(
        &router,
        "UpdateScheduledReport",
        &json!({"arn": arn, "clearWidgetIds": true}),
    )
    .await;
    ok_call(
        &router,
        "UpdateScheduledReport",
        &json!({"arn": arn, "description": "Composed twice."}),
    )
    .await;

    let after = stored_report(&router, &arn, label).await;
    assert_eq!(
        after.get("name").and_then(Value::as_str),
        Some("Composed Once"),
        "case {label}: the name the first update wrote survives the later ones: {after}"
    );
    assert!(
        after.get("widgetIds").is_none(),
        "case {label}: the member the second update cleared stays cleared: {after}"
    );
    assert_eq!(
        after.get("description").and_then(Value::as_str),
        Some("Composed twice."),
        "case {label}: the description the third update wrote is stored: {after}"
    );
    assert_eq!(
        after.get("scheduleConfig"),
        Some(&json!({
            "scheduleExpression": "cron(0 8 1 * ? *)",
            "scheduleExpressionTimeZone": "UTC",
            // No update named the schedule, so the period the create resolved stands too.
            "schedulePeriod": before["scheduleConfig"]["schedulePeriod"],
            "state": "ENABLED"
        })),
        "case {label}: no update named the schedule, so the created one stands: {after}"
    );
}

/// An update naming a scheduled report this service does not store is refused with the
/// service's not-found answer and creates nothing.
#[tokio::test]
async fn updating_a_report_that_is_not_stored_is_not_found() {
    let router = router();
    let (dashboard, widget) = source_dashboard(&router, "MissingReportSource").await;
    // One real report exists, so a miss is a miss beside a live record.
    let (live, live_record) = baseline_report(
        &router,
        &dashboard,
        &widget,
        "The Only Report",
        "the live report",
    )
    .await;

    for arn in [
        "arn:aws:bcm-dashboards::123456789012:scheduled-report/never-created",
        "arn:aws-cn:bcm-dashboards::000000000000:scheduled-report/ffffffffffffffffffffffffffffffff",
        "arn:aws:bcm-dashboards::999999999999:scheduled-report/*",
    ] {
        let (status, body) = call(
            &router,
            "UpdateScheduledReport",
            &json!({"arn": arn, "name": "Back From The Dead"}),
        )
        .await;
        assert_not_found(arn, arn, status, &body);
    }

    assert_eq!(
        stored_report(&router, &live, "the live report").await,
        live_record,
        "the refused updates must leave the live report exactly as it was"
    );
}

/// A request that both clears a member and supplies a new value for it cannot hold
/// together, and a request that re-points the report at a dashboard this service does not
/// store names a missing resource: each is refused and leaves the stored record untouched
/// — including the members the refused request got right.
#[tokio::test]
async fn an_update_that_is_refused_changes_nothing() {
    let router = router();
    let (dashboard, widget) = source_dashboard(&router, "RefusedSource").await;
    let label = "refused updates";
    let (arn, before) =
        baseline_report(&router, &dashboard, &widget, "Untouched Report", label).await;

    // Contradictory member combinations, each paired with a member the request got right
    // so a partial write would be visible.
    for (case, request) in [
        (
            "clearWidgetIds with widgetIds",
            json!({
                "arn": arn,
                "name": "Should Not Stick",
                "widgetIds": [widget],
                "clearWidgetIds": true
            }),
        ),
        (
            "clearWidgetDateRangeOverride with widgetDateRangeOverride",
            json!({
                "arn": arn,
                "name": "Should Not Stick",
                "widgetDateRangeOverride": {
                    "startTime": {"type": "RELATIVE", "value": "-P1M"},
                    "endTime": {"type": "RELATIVE", "value": "P0D"}
                },
                "clearWidgetDateRangeOverride": true
            }),
        ),
    ] {
        let (status, body) = call(&router, "UpdateScheduledReport", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: a contradictory request is refused with 400, got {status}: {body}"
        );
        let code = error_code(&body);
        assert!(
            code.ends_with("ValidationException"),
            "case {case}: expected ValidationException, got {code:?}: {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case}: expected a message naming the members, got {body}"
        );
        assert_eq!(
            stored_report(&router, &arn, case).await,
            before,
            "case {case}: a refused update must leave the record exactly as it was"
        );
    }

    // A dashboard no store holds: the service's not-found answer, naming that ARN.
    let (status, body) = call(
        &router,
        "UpdateScheduledReport",
        &json!({
            "arn": arn,
            "name": "Should Not Stick",
            "dashboardArn": MISSING_DASHBOARD_ARN
        }),
    )
    .await;
    assert_not_found(
        "a dashboard that is not stored",
        MISSING_DASHBOARD_ARN,
        status,
        &body,
    );
    assert_eq!(
        stored_report(&router, &arn, label).await,
        before,
        "case {label}: an update naming a missing dashboard must change nothing"
    );
}
