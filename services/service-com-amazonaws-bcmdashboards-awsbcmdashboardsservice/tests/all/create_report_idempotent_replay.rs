// SPDX-License-Identifier: CC0-1.0

//! The idempotent REPLAY a `CreateScheduledReport` `clientToken` buys, end to end over the
//! wire.
//!
//! The rule under test: a create's `clientToken` is modelled `@idempotencyToken` — "A
//! unique, case-sensitive identifier that you provide to ensure the idempotency of the
//! request" (`smithy_query(query="operation",operation="CreateScheduledReport")`) — so one
//! request presented under one token is ONE logical create however many times it arrives:
//! every repeat is answered the first call's own answer, HTTP 200 with the ARN that call
//! minted, and creates nothing. That is the case the token exists for: a client (or an SDK
//! transport retry) whose first response was lost resends the request and gets the ARN that
//! was in fact created.
//!
//! The table spans what the rule generalises over — the REQUEST being retried: which
//! optional members it carries (none beyond the required ones, each one, all of them), the
//! form of its schedule (a cron and a rate expression, a period and none), its widget list
//! (absent, empty, one widget), its tags (absent, empty, several) and the token spelling
//! itself at both published length bounds of `ClientToken` (length 1..128, pattern
//! `^[\u0021-\u007E]+$`, source: `smithy_query(query="shape",shape="ClientToken")`). The
//! recorded case, a fully populated create retried once, is one row of it.
//!
//! How a later call that DIFFERS from the create holding the token is answered is the
//! sibling rule, tabled in `tests/all/create_report_token_spent.rs`.

use serde_json::{json, Value};

use super::dashboard_lifecycle::{call, ok_call, router};
use super::scheduled_report_lifecycle::{acceptable_period, source_dashboard, EXECUTION_ROLE_ARN};

/// How many times each row presents its request: the first call creates, and every call
/// after it is a repeat — more than one, so a replay that only worked once is caught.
const PRESENTATIONS: usize = 3;

/// One row: a token, and the `CreateScheduledReport` request presented under it.
struct ReplayCase {
    label: &'static str,
    token: String,
    /// Builds the request from the dashboard to report on and one of its widget ids.
    request: fn(&str, &str) -> Value,
}

fn cases() -> Vec<ReplayCase> {
    vec![
        ReplayCase {
            label: "the required members only",
            token: "sdk-retry-token-0001".to_string(),
            request: |dashboard, _widget| {
                json!({
                    "scheduledReport": {
                        "name": "Minimal Rollup",
                        "dashboardArn": dashboard,
                        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                        "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
                    }
                })
            },
        },
        ReplayCase {
            label: "a schedule carrying a period, a time zone and a state",
            token: "sdk-retry-token-0002".to_string(),
            request: |dashboard, _widget| {
                json!({
                    "scheduledReport": {
                        "name": "Scheduled Rollup",
                        "dashboardArn": dashboard,
                        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                        "scheduleConfig": {
                            "scheduleExpression": "rate(7 days)",
                            "scheduleExpressionTimeZone": "Europe/Berlin",
                            // Built relative to the call: each bound of a submitted period
                            // is held to a rule measured from the request that carries it.
                            "schedulePeriod": acceptable_period(),
                            // `ScheduleState` values, source: the `ScheduleState` enum in
                            // the model.
                            "state": "DISABLED"
                        }
                    }
                })
            },
        },
        ReplayCase {
            label: "a description and the empty widget list",
            token: "!".to_string(), // `ClientToken`'s shortest admitted value.
            request: |dashboard, _widget| {
                json!({
                    "scheduledReport": {
                        "name": "Described Rollup",
                        "dashboardArn": dashboard,
                        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                        "scheduleConfig": {"scheduleExpression": "cron(0 9 1 * ? *)"},
                        "description": "Spend, by service.",
                        "widgetIds": []
                    }
                })
            },
        },
        ReplayCase {
            label: "one widget, a date-range override and no tags",
            token: "t".repeat(128), // The published upper length bound, 128.
            request: |dashboard, widget| {
                json!({
                    "scheduledReport": {
                        "name": "Windowed Rollup",
                        "dashboardArn": dashboard,
                        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                        "scheduleConfig": {"scheduleExpression": "cron(0 7 ? * MON *)"},
                        "widgetIds": [widget],
                        "widgetDateRangeOverride": {
                            "startTime": {"type": "RELATIVE", "value": "-P1M"},
                            "endTime": {"type": "RELATIVE", "value": "P0D"}
                        }
                    }
                })
            },
        },
        ReplayCase {
            label: "the empty tag list",
            token: "~!#$%&'()*+,-./:;<=>?@[\\]^_`{|}\"".to_string(),
            request: |dashboard, _widget| {
                json!({
                    "resourceTags": [],
                    "scheduledReport": {
                        "name": "Untagged Rollup",
                        "dashboardArn": dashboard,
                        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                        "scheduleConfig": {"scheduleExpression": "cron(0 6 1 * ? *)"}
                    }
                })
            },
        },
        ReplayCase {
            label: "every optional member at once, several tags",
            token: "arn:aws:bcm-dashboards::123456789012:scheduled-report/not-a-report".to_string(),
            request: |dashboard, widget| {
                json!({
                    "resourceTags": [
                        {"key": "team", "value": "finops"},
                        {"key": "env", "value": "prod"},
                        {"key": "empty-value-is-legal", "value": ""}
                    ],
                    "scheduledReport": {
                        "name": "Everything Rollup",
                        "dashboardArn": dashboard,
                        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                        "scheduleConfig": {
                            "scheduleExpression": "cron(0 8 1 * ? *)",
                            "scheduleExpressionTimeZone": "UTC",
                            "schedulePeriod": {"startTime": acceptable_period()["startTime"]},
                            "state": "ENABLED"
                        },
                        "description": "All of it.",
                        "widgetIds": [widget],
                        "widgetDateRangeOverride": {
                            "startTime": {"type": "ABSOLUTE", "value": "2025-01-01"},
                            "endTime": {"type": "ABSOLUTE", "value": "2025-07-01"}
                        }
                    }
                })
            },
        },
    ]
}

/// The ARNs of the scheduled reports this account stores, in the listing's order.
async fn stored_arns(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<String> {
    let listed = ok_call(router, "ListScheduledReports", &json!({"maxResults": 100})).await;
    listed["scheduledReports"]
        .as_array()
        .unwrap_or_else(|| panic!("ListScheduledReports answers a list: {listed}"))
        .iter()
        .filter_map(|summary| summary.get("arn").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// Every row: the first call creates the report, and each repeat of that same request under
/// the same token is answered 200 with the ARN the first call minted — with no second
/// report created, no stored record changed and the tags untouched.
#[tokio::test]
async fn a_repeat_of_a_create_under_its_client_token_is_answered_the_first_arn() {
    let router = router();
    let (dashboard, widget) = source_dashboard(&router, "ReplaySource").await;

    let mut minted: Vec<String> = Vec::new();
    for case in cases() {
        let label = case.label;
        let mut request = (case.request)(&dashboard, &widget);
        request["clientToken"] = json!(case.token);

        let created = ok_call(&router, "CreateScheduledReport", &request).await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: the create returns an arn: {created}"))
            .to_string();
        minted.push(arn.clone());
        let stored = ok_call(&router, "GetScheduledReport", &json!({"arn": arn})).await;
        let tags = ok_call(&router, "ListTagsForResource", &json!({"resourceArn": arn})).await;

        for attempt in 2..=PRESENTATIONS {
            let (status, body) = call(&router, "CreateScheduledReport", &request).await;
            assert_eq!(
                status, 200,
                "case {label} (presentation {attempt}): a repeat under the same clientToken \
                 is answered 200, got {status}: {body}"
            );
            assert_eq!(
                body["arn"].as_str(),
                Some(arn.as_str()),
                "case {label} (presentation {attempt}): expected the first call's arn \
                 {arn:?}, got {body}"
            );
            assert_eq!(
                ok_call(&router, "GetScheduledReport", &json!({"arn": arn})).await,
                stored,
                "case {label} (presentation {attempt}): the replay must change nothing \
                 about the stored report"
            );
            assert_eq!(
                ok_call(&router, "ListTagsForResource", &json!({"resourceArn": arn})).await,
                tags,
                "case {label} (presentation {attempt}): the replay must change no tag"
            );
        }

        // One report per row, whatever a row presented: the replays created nothing.
        let arns = stored_arns(&router).await;
        let mut expected = minted.clone();
        expected.sort();
        let mut actual = arns.clone();
        actual.sort();
        assert_eq!(
            actual, expected,
            "case {label}: the rows so far must have created exactly one report each, got {arns:?}"
        );
    }
}

/// The replay is the first call's answer, not the latest call's: it stands even once the
/// store has moved on around it — other reports created, the replayed report updated — and
/// it is still that one report the token names.
#[tokio::test]
async fn the_replay_stands_while_the_store_moves_on_around_it() {
    let router = router();
    let (dashboard, widget) = source_dashboard(&router, "ReplayMovingSource").await;
    let label = "the store moves on around the replay";
    let token = "sdk-retry-token-moving";

    let request = json!({
        "clientToken": token,
        "resourceTags": [{"key": "team", "value": "finops"}],
        "scheduledReport": {
            "name": "Moving Rollup",
            "dashboardArn": dashboard,
            "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
            "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"},
            "widgetIds": [widget]
        }
    });
    let created = ok_call(&router, "CreateScheduledReport", &request).await;
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: the create returns an arn: {created}"))
        .to_string();

    // Another report, under another token, and an update of the replayed one.
    let mut other = request.clone();
    other["clientToken"] = json!("sdk-retry-token-other");
    other["scheduledReport"]["name"] = json!("Other Rollup");
    let other_arn = ok_call(&router, "CreateScheduledReport", &other).await["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: the second create returns an arn"))
        .to_string();
    ok_call(
        &router,
        "UpdateScheduledReport",
        &json!({"arn": arn, "description": "Updated after the create."}),
    )
    .await;
    let after_update = ok_call(&router, "GetScheduledReport", &json!({"arn": arn})).await;

    let (status, body) = call(&router, "CreateScheduledReport", &request).await;
    assert_eq!(
        status, 200,
        "case {label}: the repeat is still answered 200, got {status}: {body}"
    );
    assert_eq!(
        body["arn"].as_str(),
        Some(arn.as_str()),
        "case {label}: the repeat still answers the arn its create minted, got {body}"
    );
    assert_eq!(
        ok_call(&router, "GetScheduledReport", &json!({"arn": arn})).await,
        after_update,
        "case {label}: the replay must not undo the update it followed"
    );
    let mut arns = stored_arns(&router).await;
    arns.sort();
    let mut expected = vec![arn, other_arn];
    expected.sort();
    assert_eq!(
        arns, expected,
        "case {label}: the two tokens must have created exactly two reports"
    );
}

/// Without a `clientToken` there is no key for a repeat to be a repeat OF, so two identical
/// requests are two creates — the baseline the replay above is measured against.
#[tokio::test]
async fn without_a_client_token_an_identical_request_creates_a_second_report() {
    let router = router();
    let (dashboard, widget) = source_dashboard(&router, "NoTokenReplaySource").await;
    let label = "no clientToken";

    let request = json!({
        "scheduledReport": {
            "name": "Unkeyed Rollup",
            "dashboardArn": dashboard,
            "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
            "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"},
            "widgetIds": [widget]
        }
    });
    let first = ok_call(&router, "CreateScheduledReport", &request).await["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: the first create returns an arn"))
        .to_string();
    let second = ok_call(&router, "CreateScheduledReport", &request).await["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: the second create returns an arn"))
        .to_string();
    assert_ne!(
        first, second,
        "case {label}: each create mints its own report, got {first} twice"
    );
    let mut arns = stored_arns(&router).await;
    arns.sort();
    let mut expected = vec![first, second];
    expected.sort();
    assert_eq!(
        arns, expected,
        "case {label}: both creates must be stored, got {arns:?}"
    );
}
