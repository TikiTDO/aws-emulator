// SPDX-License-Identifier: CC0-1.0

//! How this service states a NUMERIC value outside its member's declared range, over the
//! wire.
//!
//! The rule: a boundary refusal of a value outside a member's modelled `@range` names the
//! member and the bound that member's values have to respect, and states no value at all —
//! `Value at '<member path>' failed to satisfy constraint: Member must have value
//! {less,greater} than or equal to <bound>`. A bound is a property of the member rather than
//! of one request, so the sentence reads the same whatever number tripped it, at whichever
//! end of the range and on whichever member. Everything else about the refusal is the
//! framework's and unchanged: HTTP 400, `ValidationException`, the
//! `N validation error(s) detected: ` envelope, and the member path including a list index.
//!
//! The suite is a TABLE over every `@range`-constrained member of this service's surface —
//! `maxResults` on both listings (range 1..100) and a widget's `width` (2..6) and `height`
//! (4..10) on both dashboard writes — crossed with each end of the range: one past the
//! maximum and one past the minimum, each stated at a top-level member and at one nested
//! under a list element. Each range is also exercised ON each published bound, which must
//! still be served. Sources for the catalogue and the bounds: `smithy_query(query="closure",
//! operation="ListDashboards,ListScheduledReports,CreateDashboard")` — `MaxResults` range
//! 1..100, `WidgetWidth` 2..6, `WidgetHeight` 4..10; they are the only `@range` members the
//! model declares.
//!
//! Two neighbouring tests pin the SCOPE of the rule: another constraint class still states
//! the value it was given, and a request that breaks a range AND a length states both
//! clauses in their own voices under one envelope.
//!
//! Every call goes through the real router, so what these tests assert is what an SDK client
//! sees.

use serde_json::{json, Value};

use super::dashboard_lifecycle::{call, error_code, graph_config, ok_call, router};
use super::scheduled_report_lifecycle::{source_dashboard, EXECUTION_ROLE_ARN};

/// The `message` of an error body.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The envelope a single-clause refusal carries.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause a `@range` violation states: the member path the value sat at and the bound
/// its values have to respect, and nothing else.
fn range_clause(path: &str, requirement: &str) -> String {
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// The requirement a value above `max` broke, as the framework words that class.
fn at_most(max: i64) -> String {
    format!("Member must have value less than or equal to {max}")
}

/// The requirement a value below `min` broke, as the framework words that class.
fn at_least(min: i64) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// A well-formed dashboard ARN, so a row that breaks a widget's range on an update breaks
/// only that. It names no stored dashboard: the boundary refusal is answered before any
/// handler looks one up.
const WELL_FORMED_DASHBOARD_ARN: &str = "arn:aws:bcm-dashboards::000000000000:dashboard/no-such";

/// One widget, with `extra` merged over it — the member whose range a row breaks or
/// respects.
fn widget(title: &str, extra: Value) -> Value {
    let mut widget = json!({
        "title": title,
        "configs": [graph_config("UnblendedCost", "LINE", "MONTHLY")]
    });
    for (member, value) in extra.as_object().expect("the overrides are an object") {
        widget[member] = value.clone();
    }
    widget
}

/// One row of the table: a request that breaks exactly one member's `@range`, and the
/// refusal it must be answered with.
struct RangeCase {
    label: &'static str,
    operation: &'static str,
    request: Value,
    /// The dotted member path the clause states.
    path: &'static str,
    /// The bound clause, built from the member's modelled `@range`.
    requirement: String,
    /// The number the request submitted there, which the clause must NOT carry.
    submitted: i64,
}

fn range_cases() -> Vec<RangeCase> {
    vec![
        RangeCase {
            label: "a dashboard page size one past the maximum",
            operation: "ListDashboards",
            request: json!({"maxResults": 101}),
            path: "maxResults",
            requirement: at_most(100),
            submitted: 101,
        },
        RangeCase {
            label: "a dashboard page size far past the maximum",
            operation: "ListDashboards",
            request: json!({"maxResults": 4096}),
            path: "maxResults",
            requirement: at_most(100),
            submitted: 4096,
        },
        RangeCase {
            label: "a dashboard page size one past the minimum",
            operation: "ListDashboards",
            request: json!({"maxResults": 0}),
            path: "maxResults",
            requirement: at_least(1),
            submitted: 0,
        },
        RangeCase {
            label: "a negative dashboard page size",
            operation: "ListDashboards",
            request: json!({"maxResults": -7}),
            path: "maxResults",
            requirement: at_least(1),
            submitted: -7,
        },
        RangeCase {
            label: "a report page size one past the maximum",
            operation: "ListScheduledReports",
            request: json!({"maxResults": 101}),
            path: "maxResults",
            requirement: at_most(100),
            submitted: 101,
        },
        RangeCase {
            label: "a report page size one past the minimum",
            operation: "ListScheduledReports",
            request: json!({"maxResults": 0}),
            path: "maxResults",
            requirement: at_least(1),
            submitted: 0,
        },
        RangeCase {
            label: "a widget width one past the maximum, on a create",
            operation: "CreateDashboard",
            request: json!({
                "name": "Width Above Max",
                "widgets": [widget("Spend", json!({"width": 7}))]
            }),
            path: "widgets.1.member.width",
            requirement: at_most(6),
            submitted: 7,
        },
        RangeCase {
            label: "a widget width one past the minimum, on a create",
            operation: "CreateDashboard",
            request: json!({
                "name": "Width Below Min",
                "widgets": [widget("Spend", json!({"width": 1}))]
            }),
            path: "widgets.1.member.width",
            requirement: at_least(2),
            submitted: 1,
        },
        RangeCase {
            label: "a widget height one past the maximum, on the second widget of a create",
            operation: "CreateDashboard",
            request: json!({
                "name": "Height Above Max",
                "widgets": [
                    widget("Spend", json!({})),
                    widget("Coverage", json!({"height": 11}))
                ]
            }),
            path: "widgets.2.member.height",
            requirement: at_most(10),
            submitted: 11,
        },
        RangeCase {
            label: "a widget height one past the minimum, on a create",
            operation: "CreateDashboard",
            request: json!({
                "name": "Height Below Min",
                "widgets": [widget("Spend", json!({"height": 3}))]
            }),
            path: "widgets.1.member.height",
            requirement: at_least(4),
            submitted: 3,
        },
        RangeCase {
            label: "a widget width one past the maximum, on an update",
            operation: "UpdateDashboard",
            request: json!({
                "arn": WELL_FORMED_DASHBOARD_ARN,
                "name": "Updated Dashboard",
                "widgets": [widget("Spend", json!({"width": 7}))]
            }),
            path: "widgets.1.member.width",
            requirement: at_most(6),
            submitted: 7,
        },
        RangeCase {
            label: "a widget height one past the minimum, on an update",
            operation: "UpdateDashboard",
            request: json!({
                "arn": WELL_FORMED_DASHBOARD_ARN,
                "name": "Updated Dashboard",
                "widgets": [widget("Spend", json!({"height": 3}))]
            }),
            path: "widgets.1.member.height",
            requirement: at_least(4),
            submitted: 3,
        },
    ]
}

#[tokio::test]
async fn a_range_refusal_names_the_member_and_the_bound_and_no_value() {
    let router = router();

    for case in range_cases() {
        let RangeCase {
            label,
            operation,
            request,
            path,
            requirement,
            submitted,
        } = case;

        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 from {operation}, got {status}: {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {label:?}: expected ValidationException from {operation}, got {:?}: {body}",
            error_code(&body)
        );

        let message = error_message(&body);
        assert_eq!(
            message,
            one_error(&range_clause(path, &requirement)),
            "case {label:?}: expected the clause to name the member and the bound, \
             got {message:?}"
        );
        assert!(
            !message.contains(&format!("'{submitted}'")),
            "case {label:?}: expected no submitted value in the clause, got {message:?}"
        );
    }
}

#[tokio::test]
async fn every_published_bound_is_still_served() {
    let router = router();

    // Each range ON its published bounds: the refusals above are about the value
    // submitted, not about the member. Sources as in the module header — `MaxResults`
    // 1..100, `WidgetWidth` 2..6, `WidgetHeight` 4..10.
    let widget_bounds = [
        ("width at its minimum", "width", 2),
        ("width at its maximum", "width", 6),
        ("height at its minimum", "height", 4),
        ("height at its maximum", "height", 10),
    ];
    for (label, member, bound) in widget_bounds {
        let created = ok_call(
            &router,
            "CreateDashboard",
            &json!({
                "name": format!("Bound {member} {bound}"),
                "widgets": [widget("Spend", json!({member: bound}))]
            }),
        )
        .await;
        let arn = created["arn"].as_str().unwrap_or_default();
        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_eq!(
            fetched["widgets"][0][member].as_i64(),
            Some(bound),
            "case {label}: the stored {member} must be the supplied bound: {fetched}"
        );
    }

    // The four dashboards above are what the page sizes below are read against, beside
    // whatever the account already held (this service's AWS-managed catalog lives in the
    // same store), so the total is counted rather than assumed: a page at the minimum
    // holds one dashboard and hands back a cursor, and a page at the maximum holds every
    // stored dashboard — the four created here among them — and hands back none.
    let stored = {
        let mut seen: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"maxResults": 100});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = ok_call(&router, "ListDashboards", &request).await;
            seen.extend(
                page["dashboards"]
                    .as_array()
                    .unwrap_or_else(|| panic!("expected a page of dashboards: {page}"))
                    .iter()
                    .cloned(),
            );
            match page.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break seen.len(),
            }
        }
    };
    assert!(
        stored >= widget_bounds.len(),
        "the {} dashboards created above must be among the {stored} stored",
        widget_bounds.len()
    );

    for (max_results, expected, cursor) in [(1, 1, true), (100, stored.min(100), stored > 100)] {
        let page = ok_call(&router, "ListDashboards", &json!({"maxResults": max_results})).await;
        let items = page["dashboards"]
            .as_array()
            .unwrap_or_else(|| panic!("case maxResults {max_results}: expected a page: {page}"));
        assert_eq!(
            items.len(),
            expected,
            "case maxResults {max_results}: expected {expected} of the {stored} stored \
             dashboards, got {page}"
        );
        assert_eq!(
            page.get("nextToken").is_some(),
            cursor,
            "case maxResults {max_results}: expected a cursor={cursor}, got {page}"
        );
    }

    // The other listing takes the same bounds, read the same way: two reports are stored
    // first, so a page at the minimum holds one of them and a cursor, and a page at the
    // maximum holds both and none.
    let (dashboard_arn, _) = source_dashboard(&router, "RangeBoundSource").await;
    let reports = ["Range Bound One", "Range Bound Two"];
    for name in reports {
        ok_call(
            &router,
            "CreateScheduledReport",
            &json!({
                "scheduledReport": {
                    "name": name,
                    "dashboardArn": dashboard_arn,
                    "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
                }
            }),
        )
        .await;
    }
    for (max_results, expected, cursor) in [(1, 1, true), (100, reports.len(), false)] {
        let page =
            ok_call(&router, "ListScheduledReports", &json!({"maxResults": max_results})).await;
        let items = page["scheduledReports"]
            .as_array()
            .unwrap_or_else(|| panic!("case maxResults {max_results}: expected a page: {page}"));
        assert_eq!(
            items.len(),
            expected,
            "case maxResults {max_results}: expected {expected} of the {} stored reports, \
             got {page}",
            reports.len()
        );
        assert_eq!(
            page.get("nextToken").is_some(),
            cursor,
            "case maxResults {max_results}: expected a cursor={cursor}, got {page}"
        );
    }
}

#[tokio::test]
async fn another_constraint_class_still_states_the_value_it_was_given() {
    let router = router();

    // The scope of the rule: only the `@range` class drops its value half. `nextToken`
    // targets `NextPageToken`, length 1..8192, so the empty token breaks the length minimum
    // and is stated with the (empty) value it submitted.
    let (status, body) = call(&router, "ListDashboards", &json!({"nextToken": ""})).await;
    assert_eq!(
        status, 400,
        "case an empty nextToken: expected HTTP 400, got {status}: {body}"
    );
    let message = error_message(&body);
    assert_eq!(
        message,
        one_error(
            "Value '' at 'nextToken' failed to satisfy constraint: \
             Member must have length greater than or equal to 1"
        ),
        "case an empty nextToken: expected the length clause to keep its value half, \
         got {message:?}"
    );
}

#[tokio::test]
async fn a_request_breaking_a_range_and_a_length_states_each_in_its_own_voice() {
    let router = router();

    // One request, two classes: the page size is past its maximum and the cursor is under
    // its length minimum. Both clauses are stated under one envelope, the range one with no
    // value and the length one with the value it was given.
    let (status, body) = call(
        &router,
        "ListScheduledReports",
        &json!({"maxResults": 101, "nextToken": ""}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a bad page size and a bad cursor: expected HTTP 400, got {status}: {body}"
    );
    let message = error_message(&body);
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case a bad page size and a bad cursor: expected the plural envelope over two \
         clauses, got {message:?}"
    );
    for clause in [
        range_clause("maxResults", &at_most(100)),
        "Value '' at 'nextToken' failed to satisfy constraint: \
         Member must have length greater than or equal to 1"
            .to_string(),
    ] {
        assert!(
            message.contains(&clause),
            "case a bad page size and a bad cursor: expected {clause:?} among the clauses, \
             got {message:?}"
        );
    }
}
