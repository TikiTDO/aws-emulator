// SPDX-License-Identifier: CC0-1.0

//! What a write that submits a scheduled report's configuration is held to, end to end
//! over the wire.
//!
//! The rules under test, each stated by the member that carries the value and applied by
//! every write that submits it — `CreateScheduledReport` and `UpdateScheduledReport` alike:
//!
//! * a `scheduleConfig.scheduleExpression` is a cron expression in the form its member
//!   documents — "This value must be a cron expression consisting of six fields separated
//!   by white spaces: `cron(minutes hours day_of_month month day_of_week year)`"
//!   (`smithy_query(query="shape",shape="ScheduleConfig")`) — and anything else is refused
//!   with the modelled `ValidationException` (400), since the `GenericString` behind the
//!   member constrains nothing at the boundary;
//! * a submitted `widgetIds` entry identifies a widget OF the dashboard the report renders
//!   — the member is "the list of widget identifiers to include in the scheduled report"
//!   (`smithy_query(query="shape",shape="ScheduledReportInput")`) — and an id that
//!   identifies no widget of it is refused the same way. Only the ids a REQUEST submits are
//!   held to this: a widget that goes away AFTER the write is reported as the report's
//!   derived `WIDGET_ID_NOT_FOUND` health, which the last test below pins;
//! * a `widgetDateRangeOverride` defines a time period, so its `endTime` may not come
//!   before its `startTime` (`smithy_query(query="shape",shape="DateTimeRange")`).
//!
//! Each assertion spans the class rather than one request: the expression table carries
//! eight well-formed cron forms (numbers, lists, ranges, names, steps, the last day of a
//! month, the last weekday of a month, and a run of blanks between two fields) against
//! twelve values that are not that form (rate expressions, five, seven, one and no fields,
//! the fields without the wrapper, prose, the empty string, the wrapper in capitals, an
//! unclosed wrapper and trailing text); the widget table spans an absent member, the empty
//! list, the dashboard's own widget, an id no widget carries and another dashboard's
//! widget; and the range table spans an ordered, an equal-bounds and a relative period
//! against an inverted one. A refusal is asserted to create and change NOTHING, and to
//! leave the request's `clientToken` free.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{call, ok_call, router};
use super::scheduled_report_lifecycle::{error_code, source_dashboard, EXECUTION_ROLE_ARN};

/// A `WidgetId` in the form the model requires — length 32..32, pattern `^[0-9a-f]{32}$`
/// (`smithy_query(query="shape",shape="WidgetId")`) — that no widget of any dashboard these
/// stores hold carries, so a request naming it is well formed and still names nothing.
const UNKNOWN_WIDGET_ID: &str = "ffffffffffffffffffffffffffffffff";

/// A cron expression in the documented form, for the rows that are about something else.
const WELL_FORMED_EXPRESSION: &str = "cron(0 8 1 * ? *)";

/// Schedule expressions in the form `ScheduleConfig$scheduleExpression` documents —
/// `cron(...)` around six fields separated by white space — spanning the shapes a field of
/// the AWS cron dialect takes: plain numbers, a list, a range, names, a step, the last day
/// of the month and the last given weekday of the month.
///
/// The last row is the separator itself: the member words it as "six fields separated by
/// white spaces", so a run of blanks separates two fields rather than naming an empty one.
const ACCEPTED_EXPRESSIONS: &[(&str, &str)] = &[
    ("numbers with a ? day of the week", "cron(0 8 1 * ? *)"),
    ("a list of months", "cron(0 6 1 1,4,7,10 ? *)"),
    ("a named day of the week", "cron(0 7 ? * MON *)"),
    ("a range of named days", "cron(0 18 ? * MON-FRI *)"),
    ("a step in the minutes field", "cron(0/15 * * * ? *)"),
    ("the last day of the month", "cron(0 12 L * ? *)"),
    ("the last Friday of the month", "cron(15 10 ? * 6L *)"),
    ("several blanks between two fields", "cron(0  8  1 * ? *)"),
];

/// Values that are not that form, spanning the ways a request can miss it: another AWS
/// schedule dialect, the wrong number of fields, the fields without the wrapper, prose, no
/// text at all, the wrapper misspelled, a wrapper left open and text past its close.
const REFUSED_EXPRESSIONS: &[(&str, &str)] = &[
    ("a rate expression in days", "rate(7 days)"),
    ("a rate expression in minutes", "rate(30 minutes)"),
    ("five fields", "cron(0 8 1 * ?)"),
    ("seven fields", "cron(0 8 1 * ? * *)"),
    ("one field", "cron(*)"),
    ("no field at all", "cron()"),
    ("the six fields without the wrapper", "0 8 1 * ? *"),
    ("prose", "not a schedule"),
    ("the empty string", ""),
    ("the wrapper in capitals", "CRON(0 8 1 * ? *)"),
    ("a wrapper left open", "cron(0 8 1 * ? *"),
    ("text past the wrapper", "cron(0 8 1 * ? *) every month"),
];

/// A `CreateScheduledReport` request for a report named `name` on `dashboard_arn`, carrying
/// `schedule_expression` and `widget_ids` (`None` leaves the member out).
fn create_request(
    name: &str,
    dashboard_arn: &str,
    schedule_expression: &str,
    widget_ids: Option<&[&str]>,
) -> Value {
    let mut report = json!({
        "name": name,
        "dashboardArn": dashboard_arn,
        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
        "scheduleConfig": {"scheduleExpression": schedule_expression}
    });
    if let Some(ids) = widget_ids {
        report["widgetIds"] = json!(ids);
    }
    json!({"scheduledReport": report})
}

/// The stored record `GetScheduledReport` answers for `arn`.
async fn stored(router: &MultiServiceRouter, arn: &str) -> Value {
    let fetched = ok_call(router, "GetScheduledReport", &json!({"arn": arn})).await;
    fetched
        .get("scheduledReport")
        .cloned()
        .unwrap_or_else(|| panic!("no scheduledReport in {fetched}"))
}

/// How many scheduled reports the account stores, so a refusal can be shown to have
/// created none.
async fn stored_report_count(router: &MultiServiceRouter) -> usize {
    let listed = ok_call(router, "ListScheduledReports", &json!({})).await;
    listed["scheduledReports"]
        .as_array()
        .unwrap_or_else(|| panic!("ListScheduledReports answered no list: {listed}"))
        .len()
}

/// Assert that `body` is this service's `ValidationException` at `status`, and that its
/// message names `member` and quotes every one of `quoted`.
fn assert_validation(
    case: &str,
    status: u16,
    body: &Value,
    member: &str,
    quoted: &[&str],
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}: {body}"
    );
    assert!(
        error_code(body).ends_with("ValidationException"),
        "case {case:?}: expected a ValidationException, got {body}"
    );
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message: {body}"));
    assert!(
        message.contains(member),
        "case {case:?}: expected the refusal to name {member:?}, got {message:?}"
    );
    for value in quoted {
        assert!(
            message.contains(value),
            "case {case:?}: expected the refusal to quote {value:?}, got {message:?}"
        );
    }
}

/// A create whose schedule expression is in the documented cron form stores it verbatim,
/// and one whose expression is anything else is refused with `ValidationException` and
/// creates no report at all.
#[tokio::test]
async fn create_holds_the_schedule_expression_to_the_documented_cron_form() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "ExpressionSource").await;

    for (index, (label, expression)) in ACCEPTED_EXPRESSIONS.iter().enumerate() {
        let name = format!("Accepted Schedule {index}");
        let created = ok_call(
            &router,
            "CreateScheduledReport",
            &create_request(&name, &dashboard_arn, expression, None),
        )
        .await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the create answered no arn: {created}"));
        let record = stored(&router, arn).await;
        assert_eq!(
            record["scheduleConfig"]
                .get("scheduleExpression")
                .and_then(Value::as_str),
            Some(*expression),
            "case {label:?}: the stored schedule must carry the submitted expression, \
             got {record}"
        );
    }

    for (index, (label, expression)) in REFUSED_EXPRESSIONS.iter().enumerate() {
        let before = stored_report_count(&router).await;
        let name = format!("Refused Schedule {index}");
        let (status, body) = call(
            &router,
            "CreateScheduledReport",
            &create_request(&name, &dashboard_arn, expression, None),
        )
        .await;
        assert_validation(
            label,
            status,
            &body,
            "scheduledReport.scheduleConfig.scheduleExpression",
            &["cron(minutes hours day_of_month month day_of_week year)"],
        );
        assert_eq!(
            stored_report_count(&router).await,
            before,
            "case {label:?}: a refused create must store no report"
        );
    }
}

/// The update path holds a submitted expression to the same form: an accepted one replaces
/// the stored expression, a refused one leaves the stored record exactly as it was.
#[tokio::test]
async fn an_update_holds_the_schedule_expression_to_the_documented_cron_form() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "ExpressionUpdateSource").await;
    let created = ok_call(
        &router,
        "CreateScheduledReport",
        &create_request(
            "Updated Schedule",
            &dashboard_arn,
            WELL_FORMED_EXPRESSION,
            None,
        ),
    )
    .await;
    let arn = created["arn"]
        .as_str()
        .expect("the create answers an arn")
        .to_string();

    for (label, expression) in REFUSED_EXPRESSIONS {
        let before = stored(&router, &arn).await;
        let (status, body) = call(
            &router,
            "UpdateScheduledReport",
            &json!({"arn": arn, "scheduleConfig": {"scheduleExpression": expression}}),
        )
        .await;
        assert_validation(
            label,
            status,
            &body,
            "scheduleConfig.scheduleExpression",
            &["cron(minutes hours day_of_month month day_of_week year)"],
        );
        assert_eq!(
            stored(&router, &arn).await,
            before,
            "case {label:?}: a refused update must leave the stored record alone"
        );
    }

    for (label, expression) in ACCEPTED_EXPRESSIONS {
        ok_call(
            &router,
            "UpdateScheduledReport",
            &json!({"arn": arn, "scheduleConfig": {"scheduleExpression": expression}}),
        )
        .await;
        let record = stored(&router, &arn).await;
        assert_eq!(
            record["scheduleConfig"]
                .get("scheduleExpression")
                .and_then(Value::as_str),
            Some(*expression),
            "case {label:?}: the update must store the submitted expression, got {record}"
        );
    }
}

/// Which `widgetIds` a create row submits.
#[derive(Debug, Clone, Copy)]
enum Ids {
    /// The member is left out of the request.
    Absent,
    /// An empty list, which names no widget.
    Empty,
    /// The widget the dashboard the report is created on really holds.
    OwnWidget,
    /// A well-formed id no widget of any stored dashboard carries.
    Unknown,
    /// A widget of a SECOND stored dashboard, which this report does not render.
    OtherDashboardsWidget,
}

/// A create naming a widget of its dashboard — or no widget at all — is served and stores
/// what it submitted; one naming an id that identifies no widget of that dashboard is
/// refused with `ValidationException` naming the member, the dashboard and the id, and
/// creates no report.
#[tokio::test]
async fn create_requires_every_widget_id_to_name_a_widget_of_the_dashboard() {
    let router = router();
    let (dashboard_arn, widget_id) = source_dashboard(&router, "WidgetIdSource").await;
    let (_, other_widget_id) = source_dashboard(&router, "WidgetIdOtherSource").await;

    let rows: [(&str, Ids, bool); 5] = [
        ("widgetIds left out", Ids::Absent, true),
        ("the empty list", Ids::Empty, true),
        ("the dashboard's own widget", Ids::OwnWidget, true),
        ("an id no widget carries", Ids::Unknown, false),
        (
            "another dashboard's widget",
            Ids::OtherDashboardsWidget,
            false,
        ),
    ];

    for (index, (label, ids, accepted)) in rows.into_iter().enumerate() {
        let submitted: Option<Vec<&str>> = match ids {
            Ids::Absent => None,
            Ids::Empty => Some(Vec::new()),
            Ids::OwnWidget => Some(vec![widget_id.as_str()]),
            Ids::Unknown => Some(vec![UNKNOWN_WIDGET_ID]),
            Ids::OtherDashboardsWidget => Some(vec![other_widget_id.as_str()]),
        };
        let name = format!("Widget Case {index}");
        let request = create_request(
            &name,
            &dashboard_arn,
            WELL_FORMED_EXPRESSION,
            submitted.as_deref(),
        );
        let before = stored_report_count(&router).await;
        let (status, body) = call(&router, "CreateScheduledReport", &request).await;

        if !accepted {
            let offending = submitted
                .as_ref()
                .and_then(|ids| ids.first().copied())
                .unwrap_or_default();
            assert_validation(
                label,
                status,
                &body,
                "scheduledReport.widgetIds",
                &[offending, dashboard_arn.as_str()],
            );
            assert_eq!(
                stored_report_count(&router).await,
                before,
                "case {label:?}: a refused create must store no report"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status}: {body}"
        );
        let arn = body["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the create answered no arn: {body}"));
        let record = stored(&router, arn).await;
        assert_eq!(
            record.get("widgetIds").cloned(),
            submitted.as_ref().map(|ids| json!(ids)),
            "case {label:?}: the stored record must carry the submitted widgetIds, got {record}"
        );
    }
}

/// The update path holds the ids its REQUEST submits to the dashboard the report renders
/// once the call is applied, and leaves the ids already stored alone: a request that only
/// re-points the report is served, and the mismatch it leaves behind is reported as the
/// report's derived health instead.
#[tokio::test]
async fn an_update_requires_submitted_widget_ids_to_name_a_widget_of_the_dashboard() {
    let router = router();
    let (dashboard_arn, widget_id) = source_dashboard(&router, "WidgetUpdateSource").await;
    let (other_dashboard, other_widget) =
        source_dashboard(&router, "WidgetUpdateOtherSource").await;
    let created = ok_call(
        &router,
        "CreateScheduledReport",
        &create_request(
            "Widget Update Report",
            &dashboard_arn,
            WELL_FORMED_EXPRESSION,
            Some(&[widget_id.as_str()]),
        ),
    )
    .await;
    let arn = created["arn"]
        .as_str()
        .expect("the create answers an arn")
        .to_string();

    // An id that identifies no widget of the dashboard the report renders, submitted on its
    // own, is refused and changes nothing — whether it belongs to another dashboard or to
    // none at all.
    for (label, submitted) in [
        ("an id no widget carries", UNKNOWN_WIDGET_ID),
        ("another dashboard's widget", other_widget.as_str()),
    ] {
        let before = stored(&router, &arn).await;
        let (status, body) = call(
            &router,
            "UpdateScheduledReport",
            &json!({"arn": arn, "widgetIds": [submitted]}),
        )
        .await;
        assert_validation(
            label,
            status,
            &body,
            "widgetIds",
            &[submitted, dashboard_arn.as_str()],
        );
        assert_eq!(
            stored(&router, &arn).await,
            before,
            "case {label:?}: a refused update must leave the stored record alone"
        );
    }

    // The widget of the dashboard this very request points the report at is served, and the
    // ids it carries are what the record reads back as.
    ok_call(
        &router,
        "UpdateScheduledReport",
        &json!({"arn": arn, "dashboardArn": other_dashboard, "widgetIds": [other_widget]}),
    )
    .await;
    let record = stored(&router, &arn).await;
    assert_eq!(
        record.get("widgetIds").cloned(),
        Some(json!([other_widget])),
        "the re-pointed report must carry the widget it was re-pointed with, got {record}"
    );
    assert_eq!(
        record["healthStatus"].get("statusCode").and_then(Value::as_str),
        Some("HEALTHY"),
        "a report whose configuration resolves reads back HEALTHY, got {record}"
    );

    // A request that re-points the report and says nothing about its widget ids submits no
    // id, so no id is held to the new dashboard: the call is served, and the stored
    // configuration that no longer resolves is reported as the report's health
    // (`WIDGET_ID_NOT_FOUND`) rather than refused.
    ok_call(
        &router,
        "UpdateScheduledReport",
        &json!({"arn": arn, "dashboardArn": dashboard_arn}),
    )
    .await;
    let record = stored(&router, &arn).await;
    assert_eq!(
        record.get("widgetIds").cloned(),
        Some(json!([other_widget])),
        "an update naming no widgetIds keeps the stored ones, got {record}"
    );
    assert_eq!(
        record["healthStatus"].get("statusCode").and_then(Value::as_str),
        Some("UNHEALTHY"),
        "the report whose stored widget is no widget of its dashboard is UNHEALTHY, \
         got {record}"
    );
    assert_eq!(
        record["healthStatus"].get("statusReasons").cloned(),
        Some(json!(["WIDGET_ID_NOT_FOUND"])),
        "the unhealthy report names the widget it cannot resolve, got {record}"
    );
}

/// A `widgetDateRangeOverride` defines a time period: a create is served for a range that
/// runs forwards (and for one of no length, and for bounds resolved when the data is
/// queried), and refused for one whose end comes before its start.
#[tokio::test]
async fn create_requires_the_widget_date_range_override_to_run_forwards() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "RangeSource").await;

    // (label, the submitted range, whether the service stores it)
    let rows: [(&str, Value, bool); 5] = [
        (
            "an ordered absolute period",
            json!({
                "startTime": {"type": "ABSOLUTE", "value": "2025-01-01"},
                "endTime": {"type": "ABSOLUTE", "value": "2025-07-01"}
            }),
            true,
        ),
        (
            "an absolute period of no length",
            json!({
                "startTime": {"type": "ABSOLUTE", "value": "2025-01-01"},
                "endTime": {"type": "ABSOLUTE", "value": "2025-01-01"}
            }),
            true,
        ),
        (
            "a relative period",
            json!({
                "startTime": {"type": "RELATIVE", "value": "-P3M"},
                "endTime": {"type": "RELATIVE", "value": "P0D"}
            }),
            true,
        ),
        (
            "an inverted absolute period",
            json!({
                "startTime": {"type": "ABSOLUTE", "value": "2025-06-01"},
                "endTime": {"type": "ABSOLUTE", "value": "2025-01-01"}
            }),
            false,
        ),
        (
            "an inverted absolute period a day apart",
            json!({
                "startTime": {"type": "ABSOLUTE", "value": "2025-06-02"},
                "endTime": {"type": "ABSOLUTE", "value": "2025-06-01"}
            }),
            false,
        ),
    ];

    for (index, (label, range, accepted)) in rows.into_iter().enumerate() {
        let name = format!("Range Case {index}");
        let mut request = create_request(&name, &dashboard_arn, WELL_FORMED_EXPRESSION, None);
        request["scheduledReport"]["widgetDateRangeOverride"] = range.clone();
        let before = stored_report_count(&router).await;
        let (status, body) = call(&router, "CreateScheduledReport", &request).await;

        if !accepted {
            assert_validation(
                label,
                status,
                &body,
                "scheduledReport.widgetDateRangeOverride",
                &[],
            );
            assert_eq!(
                stored_report_count(&router).await,
                before,
                "case {label:?}: a refused create must store no report"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status}: {body}"
        );
        let arn = body["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the create answered no arn: {body}"));
        let record = stored(&router, arn).await;
        assert_eq!(
            record.get("widgetDateRangeOverride").cloned(),
            Some(range.clone()),
            "case {label:?}: the stored record must carry the submitted range, got {record}"
        );
    }
}

/// A create refused by one of these rules creates nothing, so the `clientToken` it
/// presented is still free: the request that corrects it is served under that same token.
#[tokio::test]
async fn a_refused_create_leaves_its_client_token_free() {
    let router = router();
    let (dashboard_arn, widget_id) = source_dashboard(&router, "TokenSource").await;
    let token = "config-validation-token-0001";

    // (label, the expression, the widget ids) — one refused request per rule.
    let refused: [(&str, &str, Option<Vec<&str>>); 2] = [
        ("a non-cron schedule expression", "rate(7 days)", None),
        (
            "a widget id that names nothing",
            WELL_FORMED_EXPRESSION,
            Some(vec![UNKNOWN_WIDGET_ID]),
        ),
    ];

    for (label, expression, ids) in refused {
        let mut request = create_request(
            "Token Retry Report",
            &dashboard_arn,
            expression,
            ids.as_deref(),
        );
        request["clientToken"] = json!(token);
        let (status, body) = call(&router, "CreateScheduledReport", &request).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected the refusal at 400, got {status}: {body}"
        );
    }

    // The corrected request, under the token the refused ones presented.
    let mut request = create_request(
        "Token Retry Report",
        &dashboard_arn,
        WELL_FORMED_EXPRESSION,
        Some(&[widget_id.as_str()]),
    );
    request["clientToken"] = json!(token);
    let created = ok_call(&router, "CreateScheduledReport", &request).await;
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("the corrected create answered no arn: {created}"));
    let record = stored(&router, arn).await;
    assert_eq!(
        record.get("widgetIds").cloned(),
        Some(json!([widget_id])),
        "the corrected create stores what it submitted, got {record}"
    );
}
