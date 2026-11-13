// SPDX-License-Identifier: CC0-1.0

//! Every resource ARN this service mints has ONE shape, over the wire.
//!
//! The rule both create operations implement: a minted ARN is
//! `arn:aws:bcm-dashboards::<12-digit account>:<resource type>/<id>`, where the id is 32
//! lowercase hex characters taken from that resource type's own monotonic counter — so
//! two ARNs of different resource types differ only in their resource-type segment and
//! their id, and two ARNs of one type are never equal. The same id rendering is what a
//! minted `WidgetId` carries.
//!
//! The table below drives that one path through `CreateDashboard` and
//! `CreateScheduledReport`, interleaved, with rows differing in resource type, in widget
//! count, in which optional members they carry and in whether they present a client
//! token: the shape assertions are made of the parsed ARN, not of a recorded value.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{graph_config, ok_call, router};
use super::scheduled_report_lifecycle::{source_dashboard, EXECUTION_ROLE_ARN};

/// The resource-type segment a dashboard ARN carries. Source: `DashboardArn`'s pattern
/// `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$`
/// (`smithy_query(query="operation",operation="CreateDashboard")`).
const DASHBOARD: &str = "dashboard";

/// The resource-type segment a scheduled-report ARN carries. Source:
/// `ScheduledReportArn`'s pattern
/// `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:scheduled-report/(\*|[-a-z0-9]+)$`
/// (`smithy_query(query="operation",operation="CreateScheduledReport")`).
const SCHEDULED_REPORT: &str = "scheduled-report";

/// The id width every minted resource id has, which is also the width `WidgetId`'s
/// `^[0-9a-f]{32}$` pattern fixes (source: `smithy_query(query="shape",shape="Widget")`).
const ID_WIDTH: usize = 32;

/// A minted ARN, split into the part every ARN this service mints shares (everything up
/// to and including the account segment), the resource-type segment and the id.
#[derive(Debug, PartialEq, Eq)]
struct MintedArn {
    /// `arn:aws<partition suffix>:bcm-dashboards::<account>` — identical for every ARN
    /// one store mints, whatever the resource type.
    account_prefix: String,
    resource_type: String,
    id: String,
}

/// Split `arn` the way the modelled patterns are written, or explain what is missing.
fn parse_minted_arn(arn: &str, case: &str) -> MintedArn {
    let segments: Vec<&str> = arn.split(':').collect();
    assert_eq!(
        segments.len(),
        6,
        "case {case:?}: an ARN has six colon-separated segments, got {arn:?}"
    );
    assert_eq!(
        segments[0], "arn",
        "case {case:?}: expected the `arn` prefix, got {arn:?}"
    );
    assert!(
        segments[1] == "aws"
            || segments[1]
                .strip_prefix("aws")
                .is_some_and(|rest| rest.starts_with('-')),
        "case {case:?}: expected an `aws[-a-z0-9]*` partition, got {:?}",
        segments[1]
    );
    assert_eq!(
        segments[2], "bcm-dashboards",
        "case {case:?}: expected the service's arnNamespace, got {arn:?}"
    );
    assert_eq!(
        segments[3], "",
        "case {case:?}: this service's ARN patterns carry an empty region, got {:?}",
        segments[3]
    );
    assert!(
        segments[4].len() == 12 && segments[4].chars().all(|c| c.is_ascii_digit()),
        "case {case:?}: expected a 12-digit account, got {:?}",
        segments[4]
    );
    let (resource_type, id) = segments[5].split_once('/').unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a `<type>/<id>` resource segment, got {:?}",
            segments[5]
        )
    });
    MintedArn {
        account_prefix: segments[..5].join(":"),
        resource_type: resource_type.to_string(),
        id: id.to_string(),
    }
}

/// The id of a minted resource, as a number, after checking its rendering: exactly
/// [`ID_WIDTH`] lowercase hex characters.
fn minted_id_value(id: &str, case: &str) -> u128 {
    assert_eq!(
        id.len(),
        ID_WIDTH,
        "case {case:?}: expected a {ID_WIDTH}-character id, got {id:?}"
    );
    assert!(
        id.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "case {case:?}: expected lowercase hex, got {id:?}"
    );
    u128::from_str_radix(id, 16)
        .unwrap_or_else(|e| panic!("case {case:?}: {id:?} is not hex ({e})"))
}

/// One row of the table: a create to drive, and the resource type its minted ARN must
/// carry.
struct MintCase {
    label: &'static str,
    resource_type: &'static str,
    /// The create request body, with `{dashboard}` standing for the source dashboard's
    /// ARN in the scheduled-report rows (filled in by [`request_for`]).
    build: fn(&str) -> Value,
}

/// The create request for `case`, pointed at `dashboard_arn` where the row needs a
/// dashboard to report on.
fn request_for(case: &MintCase, dashboard_arn: &str) -> Value {
    (case.build)(dashboard_arn)
}

fn cases() -> Vec<MintCase> {
    vec![
        MintCase {
            label: "dashboard, one widget, required members only",
            resource_type: DASHBOARD,
            build: |_| {
                json!({
                    "name": "Mint One",
                    "widgets": [{
                        "title": "Spend",
                        "configs": [graph_config("UnblendedCost", "LINE", "MONTHLY")]
                    }]
                })
            },
        },
        MintCase {
            label: "scheduled report, no client token",
            resource_type: SCHEDULED_REPORT,
            build: |dashboard_arn| {
                json!({
                    "scheduledReport": {
                        "name": "Mint Report One",
                        "dashboardArn": dashboard_arn,
                        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                        "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
                    }
                })
            },
        },
        MintCase {
            label: "dashboard, two widgets, description and tags",
            resource_type: DASHBOARD,
            build: |_| {
                json!({
                    "name": "Mint Two",
                    "description": "Two widgets.",
                    "widgets": [
                        {
                            "title": "Spend",
                            "configs": [graph_config("AmortizedCost", "BAR", "DAILY")]
                        },
                        {
                            "title": "Usage",
                            "width": 6,
                            "height": 9,
                            "configs": [graph_config("UsageQuantity", "STACK", "MONTHLY")]
                        }
                    ],
                    "resourceTags": [{"key": "team", "value": "finops"}]
                })
            },
        },
        MintCase {
            label: "scheduled report, client token and DISABLED schedule",
            resource_type: SCHEDULED_REPORT,
            build: |dashboard_arn| {
                json!({
                    "scheduledReport": {
                        "name": "Mint Report Two",
                        "description": "Paused.",
                        "dashboardArn": dashboard_arn,
                        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                        "scheduleConfig": {
                            "scheduleExpression": "cron(0 7 ? * MON *)",
                            "state": "DISABLED"
                        }
                    },
                    "clientToken": "token-mint-0001"
                })
            },
        },
        MintCase {
            label: "dashboard, one widget, long name",
            resource_type: DASHBOARD,
            build: |_| {
                json!({
                    "name": "Mint-Three_The Dashboard With A Longer Name",
                    "widgets": [{
                        "title": "Savings",
                        "configs": [graph_config("NetUnblendedCost", "LINE", "DAILY")]
                    }]
                })
            },
        },
        MintCase {
            label: "scheduled report, empty widget-id list",
            resource_type: SCHEDULED_REPORT,
            build: |dashboard_arn| {
                json!({
                    "scheduledReport": {
                        "name": "Mint Report Three",
                        "dashboardArn": dashboard_arn,
                        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                        "widgetIds": [],
                        "scheduleConfig": {
                            "scheduleExpression": "cron(0/15 * * * ? *)",
                            "state": "ENABLED"
                        }
                    },
                    "clientToken": "token-mint-0002"
                })
            },
        },
    ]
}

/// Read the created resource back, so the assertions are about the ARN the store keyed
/// the resource under and not only about the create response. Returns the stored ARN and,
/// for a dashboard, the ids the store minted for its widgets, in stored order.
async fn read_back(
    router: &MultiServiceRouter,
    case: &MintCase,
    arn: &str,
) -> (String, Vec<String>) {
    let label = case.label;
    if case.resource_type == DASHBOARD {
        let fetched = ok_call(router, "GetDashboard", &json!({"arn": arn})).await;
        let stored_arn = fetched["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: GetDashboard returned no arn: {fetched}"))
            .to_string();
        let widget_ids = fetched["widgets"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: GetDashboard returned no widgets: {fetched}"))
            .iter()
            .map(|widget| {
                widget["id"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {label:?}: a stored widget carries no minted id: {widget}")
                    })
                    .to_string()
            })
            .collect();
        (stored_arn, widget_ids)
    } else {
        let fetched = ok_call(router, "GetScheduledReport", &json!({"arn": arn})).await;
        let stored_arn = fetched["scheduledReport"]["arn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {label:?}: GetScheduledReport returned no arn: {fetched}")
            })
            .to_string();
        (stored_arn, Vec::new())
    }
}

/// Both create operations mint their ARN through one computation: the rows differ in
/// resource type, widget count and optional members, and every minted ARN comes out with
/// the same account prefix, a 32-hex-character id from its own type's counter, and
/// nothing varying but the resource-type segment.
#[tokio::test]
async fn every_minted_arn_has_the_one_shape() {
    let router = router();
    let (source_arn, source_widget_id) = source_dashboard(&router, "MintSource").await;

    // The source dashboard was minted by the same path, so it is row zero.
    let source = parse_minted_arn(&source_arn, "source dashboard");
    assert_eq!(
        source.resource_type, DASHBOARD,
        "source dashboard: expected the {DASHBOARD:?} resource type, got {source:?}"
    );
    let shared_prefix = source.account_prefix.clone();
    let mut last_id: Vec<(&str, u128)> = vec![(DASHBOARD, minted_id_value(&source.id, "source dashboard"))];
    let mut minted: Vec<String> = vec![source_arn.clone()];

    // Widget ids come off their own monotonic counter through the same id rendering: the
    // source dashboard's widget is the first of them.
    let mut widget_ids: Vec<String> = vec![source_widget_id.clone()];
    let mut last_widget_value = minted_id_value(&source_widget_id, "source widget");

    for case in cases() {
        let label = case.label;
        let operation = if case.resource_type == DASHBOARD {
            "CreateDashboard"
        } else {
            "CreateScheduledReport"
        };
        let created = ok_call(&router, operation, &request_for(&case, &source_arn)).await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: {operation} returned no arn: {created}"))
            .to_string();

        let parsed = parse_minted_arn(&arn, label);
        assert_eq!(
            parsed.account_prefix, shared_prefix,
            "case {label:?}: every minted ARN carries the same partition, service, empty region and account, got {arn:?}"
        );
        assert_eq!(
            parsed.resource_type, case.resource_type,
            "case {label:?}: expected the {:?} resource type, got {arn:?}",
            case.resource_type
        );
        let id_value = minted_id_value(&parsed.id, label);
        assert_eq!(
            format!("{}:{}/{}", parsed.account_prefix, parsed.resource_type, parsed.id),
            arn,
            "case {label:?}: the ARN must be exactly its parsed segments, got {arn:?}"
        );

        // Per-type monotonic counter: an id is strictly above the previous id of its own
        // resource type, and no ARN repeats.
        if let Some(previous) = last_id
            .iter()
            .rev()
            .find(|(resource_type, _)| *resource_type == case.resource_type)
            .map(|(_, value)| *value)
        {
            assert!(
                id_value > previous,
                "case {label:?}: expected an id above the previous {:?} id {previous}, got {id_value}",
                case.resource_type
            );
        }
        last_id.push((case.resource_type, id_value));
        assert!(
            !minted.contains(&arn),
            "case {label:?}: {arn:?} repeats an already minted ARN {minted:?}"
        );
        minted.push(arn.clone());

        let (stored, stored_widget_ids) = read_back(&router, &case, &arn).await;
        assert_eq!(
            stored, arn,
            "case {label:?}: the stored record must report the ARN it was minted under, got {stored:?}"
        );

        // The widget ids the dashboard reports are rendered by the same shared id path,
        // off their own counter: every one is fresh and above the last minted widget id.
        for (position, widget_id) in stored_widget_ids.iter().enumerate() {
            let widget_case = format!("{label} / widget {position}");
            let value = minted_id_value(widget_id, &widget_case);
            assert!(
                value > last_widget_value,
                "case {widget_case:?}: expected a widget id above the previous {last_widget_value}, got {value}"
            );
            assert!(
                !widget_ids.contains(widget_id),
                "case {widget_case:?}: {widget_id:?} repeats an already minted widget id {widget_ids:?}"
            );
            last_widget_value = value;
            widget_ids.push(widget_id.clone());
        }
    }

    assert!(
        widget_ids.len() > 3,
        "the table must mint several widget ids for the monotonic check to mean anything, got {widget_ids:?}"
    );

    // The two resource types really did share one computation: a dashboard ARN and a
    // scheduled-report ARN differ in nothing but the resource-type segment and the id.
    let dashboard_arn = minted
        .iter()
        .find(|arn| parse_minted_arn(arn, "cross-type dashboard").resource_type == DASHBOARD)
        .expect("the table mints at least one dashboard");
    let report_arn = minted
        .iter()
        .find(|arn| parse_minted_arn(arn, "cross-type report").resource_type == SCHEDULED_REPORT)
        .expect("the table mints at least one scheduled report");
    let dashboard = parse_minted_arn(dashboard_arn, "cross-type dashboard");
    let report = parse_minted_arn(report_arn, "cross-type report");
    assert_eq!(
        dashboard.account_prefix, report.account_prefix,
        "cross-type: the two resource types must share one ARN prefix, got {dashboard:?} and {report:?}"
    );
    assert_eq!(
        dashboard.id.len(),
        report.id.len(),
        "cross-type: the two resource types must render ids at one width, got {dashboard:?} and {report:?}"
    );
    assert_ne!(
        dashboard.resource_type, report.resource_type,
        "cross-type: the resource-type segment is what distinguishes them, got {dashboard:?}"
    );
}
