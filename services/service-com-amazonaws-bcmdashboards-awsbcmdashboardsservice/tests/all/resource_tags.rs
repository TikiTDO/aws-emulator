// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` over the wire: the tags the named resource is persisted with.
//!
//! The rule under test is that `ListTagsForResource` reports exactly the tags stored on
//! the resource its `resourceArn` names — for every tag set the model admits and for
//! either resource type that ARN can name (a dashboard or a scheduled report) — answers
//! `ResourceNotFoundException` (HTTP 400, as `tests/all/resource_not_found.rs` pins) when
//! this service holds no resource under that ARN,
//! and changes nothing a later caller can observe. The tag sets below are a TABLE
//! spanning that class: they differ in count (none, one, several, the list maximum), in
//! key and value length (each modelled bound), in the characters the values carry, and
//! in the order the keys arrive.
//!
//! Every call goes through the real router, so what these tests assert is what an SDK
//! client sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{call, error_code, ok_call, router};

/// A well-formed `ServiceRoleArn` for a scheduled report's execution role (pattern
/// `^arn:aws[-a-z0-9]*:iam::[0-9]{12}:role/[a-zA-Z0-9+=,.@_/-]+$`, source:
/// `smithy_query(query="shape", shape="ScheduledReportInput")`).
const EXECUTION_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/BCMDashboardsReportRole";

/// The `(key, value)` pairs a `ListTagsForResource` answer reports, in the order it
/// reported them.
fn reported_tags(body: &Value) -> Vec<(String, String)> {
    body.get("resourceTags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListTagsForResource must answer a resourceTags list: {body}"))
        .iter()
        .map(|tag| {
            (
                tag["key"].as_str().unwrap_or("<no key>").to_string(),
                tag["value"].as_str().unwrap_or("<no value>").to_string(),
            )
        })
        .collect()
}

/// `(key, value)` pairs rendered as the modelled `ResourceTagList` wire shape.
fn tags_json(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    )
}

/// Borrowed literals as owned `(key, value)` pairs.
fn pairs(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Create a dashboard carrying `tags` and return the ARN it was stored under. When
/// `supply_member` is false the `resourceTags` member is left off the request entirely,
/// which is the other way a caller asks for an untagged resource.
async fn create_tagged(
    router: &MultiServiceRouter,
    name: &str,
    tags: &[(String, String)],
    supply_member: bool,
) -> String {
    let mut request = json!({"name": name, "widgets": []});
    if supply_member {
        request["resourceTags"] = tags_json(tags);
    }
    let created = ok_call(router, "CreateDashboard", &request).await;
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateDashboard must answer an arn: {created}"))
        .to_string()
}

/// Create a scheduled report on `dashboard_arn` carrying `tags` and return the ARN it
/// was stored under. As with a dashboard, `supply_member` false leaves the
/// `resourceTags` member off the request entirely.
async fn create_tagged_report(
    router: &MultiServiceRouter,
    name: &str,
    dashboard_arn: &str,
    tags: &[(String, String)],
    supply_member: bool,
) -> String {
    let mut request = json!({
        "scheduledReport": {
            "name": name,
            "dashboardArn": dashboard_arn,
            "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
            "scheduleConfig": {"scheduleExpression": "cron(0 12 L * ? *)"}
        }
    });
    if supply_member {
        request["resourceTags"] = tags_json(tags);
    }
    let created = ok_call(router, "CreateScheduledReport", &request).await;
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateScheduledReport must answer an arn: {created}"))
        .to_string()
}

/// How many dashboards of the ACCOUNT'S OWN this router holds, used to show a report
/// stored nothing.
///
/// A fresh store already holds the AWS-managed catalog this service seeds, whose entries
/// carry `type: AWS_MANAGED`; a dashboard a caller created carries `CUSTOM`, so counting
/// those counts what a create or a read of this suite's own dashboards changed.
async fn dashboard_count(router: &MultiServiceRouter) -> usize {
    let page = ok_call(router, "ListDashboards", &json!({"maxResults": 100})).await;
    page["dashboards"].as_array().map_or(0, |dashboards| {
        dashboards
            .iter()
            .filter(|item| item["type"].as_str() == Some("CUSTOM"))
            .count()
    })
}

/// One row of the tag table.
struct TagCase {
    label: &'static str,
    /// Whether the create request carries a `resourceTags` member at all.
    supply_member: bool,
    tags: Vec<(String, String)>,
}

/// The tag sets this suite spans. Bounds come from the model:
/// `ResourceTag.key` length 1..128, `ResourceTag.value` length 0..256, both patterned
/// `^[\S\s]*$` (any characters at all), and `ResourceTagList` length 0..200 — source:
/// `smithy_query(query="shape", shape="ResourceTag,ResourceTagList")` on this service's
/// model.
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            label: "no resourceTags member at all",
            supply_member: false,
            tags: Vec::new(),
        },
        TagCase {
            label: "an empty tag list",
            supply_member: true,
            tags: Vec::new(),
        },
        TagCase {
            label: "one tag",
            supply_member: true,
            tags: pairs(&[("team", "finops")]),
        },
        TagCase {
            label: "keys arriving in descending order",
            supply_member: true,
            tags: pairs(&[("zone", "c"), ("region", "eu-central-1"), ("app", "billing")]),
        },
        TagCase {
            label: "key at the 1-character minimum, value at the 0-character minimum",
            supply_member: true,
            tags: pairs(&[("k", "")]),
        },
        TagCase {
            label: "key at the 128-character maximum, value at the 256-character maximum",
            supply_member: true,
            tags: vec![("k".repeat(128), "v".repeat(256))],
        },
        TagCase {
            label: "values carrying whitespace, punctuation and non-ASCII characters",
            supply_member: true,
            tags: pairs(&[
                ("cost center", "Zahlungsabläufe / 50% \"prod\""),
                ("note", "line one\nline two\tend"),
                ("owner", "équipe-coût"),
            ]),
        },
        TagCase {
            label: "the 200-tag maximum of ResourceTagList",
            supply_member: true,
            tags: (0..200)
                .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
                .collect(),
        },
    ]
}

/// For every tag set the model admits, `ListTagsForResource` reports exactly the tags
/// the resource was stored with — and reports them again unchanged on a second read.
#[tokio::test]
async fn reported_tags_are_the_tags_the_resource_was_stored_with() {
    for case in tag_cases() {
        let router = router();
        let arn = create_tagged(&router, "Tagged Dashboard", &case.tags, case.supply_member).await;

        let listed = ok_call(&router, "ListTagsForResource", &json!({"resourceArn": arn})).await;
        let reported = reported_tags(&listed);
        assert_eq!(
            reported, case.tags,
            "case {}: expected the stored tags {:?}, got {:?}",
            case.label, case.tags, reported
        );

        // A report must not change what a later caller sees: the same read answers the
        // same tags, and the store still holds exactly the one dashboard.
        let again = ok_call(&router, "ListTagsForResource", &json!({"resourceArn": arn})).await;
        let reported_again = reported_tags(&again);
        assert_eq!(
            reported_again, reported,
            "case {}: a second read must report the same tags {:?}, got {:?}",
            case.label, reported, reported_again
        );
        let count = dashboard_count(&router).await;
        assert_eq!(
            count, 1,
            "case {}: listing tags must store nothing, got {count} dashboards",
            case.label
        );
    }
}

/// A resource that was created with a known tag set: what a read of it must report.
struct StoredResource {
    label: &'static str,
    arn: String,
    tags: Vec<(String, String)>,
}

/// Tags are per resource: each ARN answers its own stored tags, never another
/// resource's and never the tags of the most recent create.
#[tokio::test]
async fn tags_are_reported_per_resource() {
    let router = router();

    let mut stored: Vec<StoredResource> = Vec::new();
    for (index, case) in tag_cases().into_iter().enumerate() {
        // A distinct name per row: a dashboard's name is unique within the account, and
        // these rows share one router.
        let name = format!("Shared Router Dashboard {index}");
        let arn = create_tagged(&router, &name, &case.tags, case.supply_member).await;
        stored.push(StoredResource {
            label: case.label,
            arn,
            tags: case.tags,
        });
    }

    for resource in &stored {
        let listed = ok_call(
            &router,
            "ListTagsForResource",
            &json!({"resourceArn": resource.arn}),
        )
        .await;
        let reported = reported_tags(&listed);
        assert_eq!(
            reported, resource.tags,
            "case {}: expected that resource's own tags {:?}, got {reported:?}",
            resource.label, resource.tags
        );
    }
}

/// A scheduled report is the other resource type a `ResourceArn` can name, and its tags
/// are reported the same way: from the stored record, per resource, for every tag set in
/// the table.
#[tokio::test]
async fn reported_tags_of_a_scheduled_report_are_the_tags_it_was_stored_with() {
    let router = router();
    let dashboard = create_tagged(
        &router,
        "Reported Dashboard",
        &pairs(&[("owner", "the dashboard, not the report")]),
        true,
    )
    .await;

    let mut stored: Vec<StoredResource> = Vec::new();
    for (index, case) in tag_cases().into_iter().enumerate() {
        // `ScheduledReportName` is 2..50 characters matching
        // `^(?!.* {2})[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]$` (source:
        // `smithy_query(query="shape", shape="ScheduledReportInput")`), so each row
        // gets a distinct name within that shape.
        let name = format!("Tagged Report {index}");
        let arn =
            create_tagged_report(&router, &name, &dashboard, &case.tags, case.supply_member).await;
        stored.push(StoredResource {
            label: case.label,
            arn,
            tags: case.tags,
        });
    }

    for resource in &stored {
        let listed = ok_call(
            &router,
            "ListTagsForResource",
            &json!({"resourceArn": resource.arn}),
        )
        .await;
        let reported = reported_tags(&listed);
        assert_eq!(
            reported, resource.tags,
            "case {}: expected the report's stored tags {:?}, got {reported:?}",
            resource.label, resource.tags
        );
    }

    // The dashboard the reports hang off keeps its own tags: a resource's tags are
    // never another resource's.
    let listed = ok_call(
        &router,
        "ListTagsForResource",
        &json!({"resourceArn": dashboard}),
    )
    .await;
    let reported = reported_tags(&listed);
    let dashboard_tags = pairs(&[("owner", "the dashboard, not the report")]);
    assert_eq!(
        reported, dashboard_tags,
        "case source dashboard: expected {dashboard_tags:?}, got {reported:?}"
    );
}

/// A well-formed ARN this service holds no resource under is a not-found, whatever
/// resource type it names — and the refused read stores nothing.
#[tokio::test]
async fn an_arn_naming_no_resource_is_not_found() {
    let router = router();

    // A dashboard that was created and then deleted: its tags went with the record.
    let deleted = create_tagged(
        &router,
        "Doomed Dashboard",
        &pairs(&[("team", "finops")]),
        true,
    )
    .await;
    ok_call(&router, "DeleteDashboard", &json!({"arn": deleted})).await;

    let before = dashboard_count(&router).await;

    // Each row satisfies `ResourceArn`'s pattern yet names no stored resource. The
    // `scheduled-report` and `*` forms are what that pattern admits beside a dashboard
    // id — source: the `resourceArn` member reported by
    // `smithy_query(query="operation", operation="ListTagsForResource")`.
    let cases: Vec<(&str, String)> = vec![
        (
            "a dashboard id never created",
            "arn:aws:bcm-dashboards::000000000000:dashboard/does-not-exist".to_string(),
        ),
        (
            "the wildcard dashboard id",
            "arn:aws:bcm-dashboards::000000000000:dashboard/*".to_string(),
        ),
        (
            "a scheduled report",
            "arn:aws:bcm-dashboards::000000000000:scheduled-report/does-not-exist".to_string(),
        ),
        (
            "a partition other than aws",
            "arn:aws-cn:bcm-dashboards::000000000000:dashboard/does-not-exist".to_string(),
        ),
        ("a deleted dashboard", deleted.clone()),
    ];

    for (label, arn) in &cases {
        let (status, body) = call(&router, "ListTagsForResource", &json!({"resourceArn": arn})).await;
        assert_eq!(
            status, 400,
            "case {label}: {arn} names no resource, expected 400, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {label}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );
    }

    let after = dashboard_count(&router).await;
    assert_eq!(
        after, before,
        "a refused read must store nothing: {before} dashboards before, {after} after"
    );
}

/// An ARN that is not a `ResourceArn` at all is refused as invalid input (400), not as
/// a not-found: the modelled constraints are checked before the handler runs.
#[tokio::test]
async fn an_arn_outside_the_modelled_pattern_is_refused() {
    let router = router();

    // Each row breaks `ResourceArn`'s pattern or its 20..2048 length in a different way.
    let cases: Vec<(&str, String)> = vec![
        ("not an arn at all", "not-an-arn".to_string()),
        (
            "shorter than the 20-character minimum",
            "arn:aws:bcm-dash".to_string(),
        ),
        (
            "another service",
            "arn:aws:bcm-dataexports::000000000000:dashboard/abc".to_string(),
        ),
        (
            "a resource type the pattern does not admit",
            "arn:aws:bcm-dashboards::000000000000:widget/abc".to_string(),
        ),
        (
            "an account id that is not 12 digits",
            "arn:aws:bcm-dashboards::1234:dashboard/abc".to_string(),
        ),
        (
            "an uppercase resource id",
            "arn:aws:bcm-dashboards::000000000000:dashboard/ABC".to_string(),
        ),
        (
            "a region segment, which this service's arns do not carry",
            "arn:aws:bcm-dashboards:us-east-1:000000000000:dashboard/abc".to_string(),
        ),
        (
            "longer than the 2048-character maximum",
            format!(
                "arn:aws:bcm-dashboards::000000000000:dashboard/{}",
                "a".repeat(2048)
            ),
        ),
    ];

    for (label, arn) in &cases {
        let (status, body) = call(&router, "ListTagsForResource", &json!({"resourceArn": arn})).await;
        assert_eq!(
            status, 400,
            "case {label}: {arn} is not a ResourceArn, expected 400, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {label}: expected ValidationException, got {}",
            error_code(&body)
        );
    }
}
