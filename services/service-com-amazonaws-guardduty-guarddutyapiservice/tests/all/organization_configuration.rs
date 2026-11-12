// SPDX-License-Identifier: CC0-1.0

//! The organization configuration of a delegated administrator's detector, end to
//! end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! serialization and the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `DescribeOrganizationConfiguration` reports what an
//!    `UpdateOrganizationConfiguration` stored, and only that: a value the update did
//!    not name keeps what it had, and a detector no update has touched reports that
//!    nothing is enabled automatically.
//! 2. The auto-enablement of GuardDuty itself is ONE stored value, written through
//!    `autoEnableOrganizationMembers` or through the deprecated boolean `autoEnable`
//!    and read back through both — the boolean reading true for any member other than
//!    `NONE`. Each feature switch is one stored value in the same way, written through
//!    `features` or through the deprecated `dataSources` view and read back through
//!    both.
//! 3. `features` reports every `OrgFeature` member exactly once, in the service's
//!    canonical order, and paging it with any `maxResults` yields each feature exactly
//!    once in that order, the last page carrying no `nextToken`.
//! 4. A request the operation documents as unacceptable is refused with HTTP 400 under
//!    the `InvalidInputException` class and writes NOTHING: a detector id the account
//!    does not hold, an auto-enablement named twice or not at all, a feature or
//!    auto-enablement value the model does not carry, and the two Runtime Monitoring
//!    features enabled together.
//! 5. A read changes nothing a later read sees.
//! 6. The report is about the account selected as the GuardDuty delegated
//!    administrator, so an organization that has none selected has no report: the read
//!    is refused with HTTP 400 and GuardDuty's sentence for that missing precondition,
//!    whatever made the standing absent.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};
// The designation this report is about, made through the operation that makes it.
use crate::organization_admin_accounts::{disable_admin, enable_admin, past_the_removal};
// The Region a case that does not name one calls through, defined once beside the
// member-call harness.
use crate::member_association::DEFAULT_REGION;

/// The error class every GuardDuty client rejection is named by on the wire.
const CLASS: &str = "InvalidInputException";

/// The modeled error SHAPE the class is enveloped in, which `x-amzn-errortype` names.
const ENVELOPE: &str = "BadRequestException";

/// Every `AutoEnableMembers` member (`smithy_query(shape="AutoEnableMembers")`),
/// paired with how the DEPRECATED boolean view of the same value reads back: the
/// boolean can only say "auto-enabled" or not, so both `NEW` and `ALL` read true.
const AUTO_ENABLE_MEMBERS: [(&str, bool); 3] = [("NEW", true), ("ALL", true), ("NONE", false)];

/// Every `OrgFeatureStatus` member (`smithy_query(shape="OrgFeatureStatus")`), paired
/// with the deprecated boolean view of it, for the same reason.
const ORG_FEATURE_STATUSES: [(&str, bool); 3] = [("ALL", true), ("NEW", true), ("NONE", false)];

/// Every `OrgFeature` member, in the order a read reports them
/// (`smithy_query(shape="OrgFeature")`).
///
/// Written out here rather than read from the crate, so the test PINS both the set
/// and its canonical order: a feature the model gains has to be added to this list
/// too, and until it is the ordering assertions below fail rather than following
/// whatever the handler reports.
const ORG_FEATURES: [&str; 8] = [
    "AI_PROTECTION",
    "EBS_MALWARE_PROTECTION",
    "EKS_AUDIT_LOGS",
    "EKS_RUNTIME_MONITORING",
    "LAMBDA_NETWORK_LOGS",
    "RDS_LOGIN_EVENTS",
    "RUNTIME_MONITORING",
    "S3_DATA_EVENTS",
];

/// The three features the deprecated `dataSources` view can name, each with the node
/// path it is read back at. The paths are the `jsonName`s of
/// `OrganizationDataSourceConfigurationsResult` and its nested result shapes
/// (`smithy_query(shape="OrganizationDataSourceConfigurationsResult")`).
const GOVERNED_NODES: [(&str, &[&str]); 3] = [
    ("S3_DATA_EVENTS", &["s3Logs"]),
    ("EKS_AUDIT_LOGS", &["kubernetes", "auditLogs"]),
    (
        "EBS_MALWARE_PROTECTION",
        &[
            "malwareProtection",
            "scanEc2InstanceWithFindings",
            "ebsVolumes",
        ],
    ),
];

/// An `OrgFeatureAdditionalConfiguration` member the cited issue does not name
/// (`smithy_query(shape="OrgFeatureAdditionalConfiguration")`).
const ADDITIONAL_CONFIGURATION: &str = "EC2_AGENT_MANAGEMENT";

/// Designate the account this emulator serves as the GuardDuty delegated administrator
/// `region`'s endpoint holds.
///
/// `DescribeOrganizationConfiguration` "returns information about the account selected
/// as the delegated administrator for GuardDuty"
/// (`smithy_query(query="doc",shape="DescribeOrganizationConfiguration")`), so an
/// organization with no account selected has no such report and the call is refused
/// (asserted by [`the_report_needs_a_delegated_administrator_to_be_about`]). That
/// standing is the PRECONDITION of every read below, so it is established here —
/// re-designating the account a Region already holds is the same designation, which is
/// why this is safe before every read.
pub(crate) async fn delegated_administrator_in(router: &Router, region: &str) {
    let (status, got) = enable_admin(router, region, ::engine::emulator_account_id()).await;
    assert_eq!(
        status, 200,
        "designating the delegated administrator of {region}: expected 200, got {status} {got:?}"
    );
}

/// `DescribeOrganizationConfiguration` through `region`'s endpoint, asserting it
/// succeeded. The region-less [`describe`] is this with [`DEFAULT_REGION`], so a
/// suite that spans two administrators and one that stays with a single detector
/// read an organization configuration through ONE helper.
pub(crate) async fn describe_in(router: &Router, region: &str, detector_id: &str) -> Value {
    delegated_administrator_in(router, region).await;
    let (status, got) = call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector_id}/admin"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "describe {detector_id}: expected 200, got {status} {got:?}"
    );
    got
}

/// `DescribeOrganizationConfiguration`, asserting it succeeded.
async fn describe(router: &Router, detector_id: &str) -> Value {
    describe_in(router, DEFAULT_REGION, detector_id).await
}

/// `DescribeOrganizationConfiguration` with a page window.
async fn describe_page(
    router: &Router,
    detector_id: &str,
    max_results: Option<usize>,
    next_token: Option<&str>,
) -> (u16, Value) {
    delegated_administrator_in(router, DEFAULT_REGION).await;
    let mut query = Vec::new();
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/admin{suffix}"),
        None,
    )
    .await
}

/// `UpdateOrganizationConfiguration` with `body`, through `region`'s endpoint.
pub(crate) async fn update_in(
    router: &Router,
    region: &str,
    detector_id: &str,
    body: Value,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/admin"),
        Some(body),
    )
    .await
}

/// `UpdateOrganizationConfiguration` with `body`.
async fn update(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    update_in(router, DEFAULT_REGION, detector_id, body).await
}

/// `UpdateOrganizationConfiguration` through `region`'s endpoint, asserting it
/// stored what it was given.
pub(crate) async fn update_ok_in(router: &Router, region: &str, detector_id: &str, body: Value) {
    let (status, got) = update_in(router, region, detector_id, body.clone()).await;
    assert_eq!(
        status, 200,
        "update {body}: expected 200, got {status} {got:?}"
    );
}

/// `UpdateOrganizationConfiguration`, asserting it succeeded.
async fn update_ok(router: &Router, detector_id: &str, body: Value) {
    update_ok_in(router, DEFAULT_REGION, detector_id, body).await;
}

/// One reported feature entry.
fn feature<'a>(got: &'a Value, name: &str) -> &'a Value {
    got["features"]
        .as_array()
        .and_then(|features| {
            features
                .iter()
                .find(|entry| entry["name"].as_str() == Some(name))
        })
        .unwrap_or_else(|| panic!("feature {name} is not reported at all: got {got:?}"))
}

/// The reported auto-enablement of one feature.
fn feature_status(got: &Value, name: &str) -> String {
    feature(got, name)["autoEnable"]
        .as_str()
        .unwrap_or_else(|| panic!("feature {name} carries no autoEnable: got {got:?}"))
        .to_string()
}

/// Walk a `dataSources` node path and read the boolean it carries.
fn node_flag(got: &Value, path: &[&str], case: &str) -> bool {
    let mut cursor = &got["dataSources"];
    for segment in path {
        cursor = &cursor[*segment];
    }
    cursor["autoEnable"].as_bool().unwrap_or_else(|| {
        panic!(
            "{case}: dataSources.{} carries no autoEnable boolean, got {got:?}",
            path.join(".")
        )
    })
}

/// Assert a response is a 400 the HANDLER refused, under the service's own error
/// class, and nothing else.
fn assert_rejected(status: u16, got: &Value, case: &str) {
    assert_refused(status, got, case, CLASS);
}

/// Assert a response is a 400 the BOUNDARY refused — a value outside a constraint
/// the model itself declares, which is named by the modelled error shape.
fn assert_boundary_refusal(status: u16, got: &Value, case: &str) {
    assert_refused(status, got, case, ENVELOPE);
}

/// Assert a 400 named by `class` in the body, enveloped in the modelled error shape.
fn assert_refused(status: u16, got: &Value, case: &str, class: &str) {
    assert_eq!(status, 400, "{case}: expected 400, got {status} {got:?}");
    assert_eq!(
        error_code(got),
        class,
        "{case}: expected the {class} class, got {got:?}"
    );
    assert_eq!(
        error_shape(got),
        ENVELOPE,
        "{case}: expected the {ENVELOPE} envelope, got {got:?}"
    );
    assert!(
        got["message"].as_str().is_some_and(|m| !m.is_empty()),
        "{case}: a rejection must say why, got {got:?}"
    );
}

/// A `features` list putting every `OrgFeature` at `status`, paired with the status
/// each feature then holds.
///
/// EKS Runtime Monitoring is left at `NONE` whenever `status` is an enablement:
/// "specifying both EKS Runtime Monitoring and Runtime Monitoring will cause an
/// error" (`smithy_query(query="doc",shape="UpdateOrganizationConfiguration")`), so
/// a list enabling the whole set at once is not a request the service accepts.
fn every_feature_at(status: &str) -> (Vec<Value>, Vec<(&'static str, String)>) {
    let held = |name: &str| {
        if name == "EKS_RUNTIME_MONITORING" && status != "NONE" {
            "NONE".to_string()
        } else {
            status.to_string()
        }
    };
    (
        ORG_FEATURES
            .iter()
            .map(|name| json!({"name": name, "autoEnable": held(name)}))
            .collect(),
        ORG_FEATURES.iter().map(|name| (*name, held(name))).collect(),
    )
}

// ---------------------------------------------------------------------------
// Rule 1.
// ---------------------------------------------------------------------------

/// A detector no update has touched reports that nothing is enabled automatically,
/// and reports the whole feature set while saying so.
#[tokio::test]
async fn an_untouched_detector_reports_nothing_auto_enabled() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let got = describe(&router, &detector_id).await;

    assert_eq!(
        got["autoEnableOrganizationMembers"],
        json!("NONE"),
        "a fresh detector: expected NONE, got {got:?}"
    );
    assert_eq!(
        got["autoEnable"],
        json!(false),
        "a fresh detector: the deprecated view of NONE is false, got {got:?}"
    );
    assert_eq!(
        got["memberAccountLimitReached"],
        json!(false),
        "a fresh detector administers nobody, so its member quota is not reached, got {got:?}"
    );
    for name in ORG_FEATURES {
        assert_eq!(
            feature_status(&got, name),
            "NONE",
            "a fresh detector: feature {name} expected NONE, got {got:?}"
        );
        assert!(
            feature(&got, name).get("additionalConfiguration").is_none(),
            "a fresh detector: feature {name} has no additional configuration set, so none is \
             reported, got {got:?}"
        );
    }
    for (_, path) in GOVERNED_NODES {
        assert!(
            !node_flag(&got, path, "a fresh detector"),
            "a fresh detector: dataSources.{} expected false, got {got:?}",
            path.join(".")
        );
    }
}

/// Every `(AutoEnableMembers, OrgFeatureStatus)` pair an update can store is what a
/// later describe reports.
#[tokio::test]
async fn an_update_is_what_a_later_describe_reports() {
    for (members, _) in AUTO_ENABLE_MEMBERS {
        for (status, _) in ORG_FEATURE_STATUSES {
            let case = format!("members {members}, every feature {status}");
            let router = new_router();
            let detector_id = enabled_detector(&router).await;
            let (features, expected) = every_feature_at(status);
            update_ok(
                &router,
                &detector_id,
                json!({
                    "autoEnableOrganizationMembers": members,
                    "features": features,
                }),
            )
            .await;

            let got = describe(&router, &detector_id).await;
            assert_eq!(
                got["autoEnableOrganizationMembers"],
                json!(members),
                "{case}: expected {members}, got {got:?}"
            );
            for (name, expected) in expected {
                assert_eq!(
                    feature_status(&got, name),
                    expected,
                    "{case}: feature {name} expected {expected}, got {got:?}"
                );
            }
        }
    }
}

/// A switch the update did not name keeps the value it already had: the update is a
/// partial write, not a replacement of the whole configuration.
#[tokio::test]
async fn an_unnamed_switch_keeps_its_value() {
    // Each row names ONE feature, so every other feature must be left where it was.
    for name in ORG_FEATURES {
        let case = format!("update naming only {name}");
        let router = new_router();
        let detector_id = enabled_detector(&router).await;

        // Everything to ALL first, so a switch left alone is observably different
        // from the NONE a fresh detector starts at.
        let (features, baseline) = every_feature_at("ALL");
        update_ok(
            &router,
            &detector_id,
            json!({"autoEnableOrganizationMembers": "ALL", "features": features}),
        )
        .await;
        update_ok(
            &router,
            &detector_id,
            json!({
                "autoEnableOrganizationMembers": "ALL",
                "features": [{"name": name, "autoEnable": "NONE"}],
            }),
        )
        .await;

        let got = describe(&router, &detector_id).await;
        for (other, was) in baseline {
            let expected = if other == name { "NONE" } else { was.as_str() };
            assert_eq!(
                feature_status(&got, other),
                expected,
                "{case}: feature {other} expected {expected}, got {got:?}"
            );
        }
    }
}

/// A `features` entry that names a feature but states no `autoEnable` states
/// nothing to change about it: `AutoEnable` is an optional member
/// (`smithy_query(shape="OrganizationFeatureConfiguration")`), so the stored
/// auto-enablement stays, exactly as it does for a feature the list leaves out.
///
/// The rows span the stored value the entry must not disturb, and the second half of
/// each row proves such an entry can still carry an additional configuration.
#[tokio::test]
async fn a_feature_entry_without_an_auto_enable_keeps_the_stored_value() {
    for (stored, _) in ORG_FEATURE_STATUSES {
        let case = format!("S3_DATA_EVENTS stored at {stored}, then named with no autoEnable");
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        update_ok(
            &router,
            &detector_id,
            json!({
                "autoEnableOrganizationMembers": "NEW",
                "features": [{"name": "S3_DATA_EVENTS", "autoEnable": stored}],
            }),
        )
        .await;
        update_ok(
            &router,
            &detector_id,
            json!({
                "autoEnableOrganizationMembers": "NEW",
                "features": [{
                    "name": "S3_DATA_EVENTS",
                    "additionalConfiguration": [
                        {"name": ADDITIONAL_CONFIGURATION, "autoEnable": "NEW"},
                    ],
                }],
            }),
        )
        .await;

        let got = describe(&router, &detector_id).await;
        assert_eq!(
            feature_status(&got, "S3_DATA_EVENTS"),
            stored,
            "{case}: expected the stored {stored}, got {got:?}"
        );
        assert_eq!(
            feature(&got, "S3_DATA_EVENTS")["additionalConfiguration"],
            json!([{"name": ADDITIONAL_CONFIGURATION, "autoEnable": "NEW"}]),
            "{case}: such an entry still carries its additional configuration, got {got:?}"
        );
    }
}

/// A feature's additional configurations are stored and reported as a request set
/// them, and only then.
#[tokio::test]
async fn a_features_additional_configuration_round_trips() {
    for (status, _) in ORG_FEATURE_STATUSES {
        let case = format!("{ADDITIONAL_CONFIGURATION} at {status}");
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        update_ok(
            &router,
            &detector_id,
            json!({
                "autoEnableOrganizationMembers": "NEW",
                "features": [{
                    "name": "RUNTIME_MONITORING",
                    "autoEnable": "ALL",
                    "additionalConfiguration": [
                        {"name": ADDITIONAL_CONFIGURATION, "autoEnable": status},
                    ],
                }],
            }),
        )
        .await;

        let got = describe(&router, &detector_id).await;
        let entry = feature(&got, "RUNTIME_MONITORING");
        assert_eq!(
            entry["additionalConfiguration"],
            json!([{"name": ADDITIONAL_CONFIGURATION, "autoEnable": status}]),
            "{case}: expected the additional configuration the update stored, got {got:?}"
        );
        assert!(
            feature(&got, "S3_DATA_EVENTS")
                .get("additionalConfiguration")
                .is_none(),
            "{case}: a feature the update set no additional configuration on reports none, got \
             {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2.
// ---------------------------------------------------------------------------

/// The deprecated boolean `autoEnable` and `autoEnableOrganizationMembers` are one
/// stored value: written through either, read back through both.
#[tokio::test]
async fn the_two_auto_enable_views_are_one_stored_value() {
    // Written through the current member.
    for (members, flag) in AUTO_ENABLE_MEMBERS {
        let case = format!("autoEnableOrganizationMembers {members}");
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        update_ok(
            &router,
            &detector_id,
            json!({"autoEnableOrganizationMembers": members}),
        )
        .await;
        let got = describe(&router, &detector_id).await;
        assert_eq!(
            got["autoEnableOrganizationMembers"],
            json!(members),
            "{case}: expected {members}, got {got:?}"
        );
        assert_eq!(
            got["autoEnable"],
            json!(flag),
            "{case}: the deprecated view of {members} is {flag}, got {got:?}"
        );
    }

    // Written through the deprecated boolean: it can only name the auto-enablement
    // of the accounts that JOIN, so true is NEW and false is NONE.
    for (flag, members) in [(true, "NEW"), (false, "NONE")] {
        let case = format!("autoEnable {flag}");
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        update_ok(&router, &detector_id, json!({"autoEnable": flag})).await;
        let got = describe(&router, &detector_id).await;
        assert_eq!(
            got["autoEnableOrganizationMembers"],
            json!(members),
            "{case}: expected {members}, got {got:?}"
        );
        assert_eq!(
            got["autoEnable"],
            json!(flag),
            "{case}: expected {flag} back, got {got:?}"
        );
    }
}

/// Each feature the deprecated `dataSources` view can name is one stored switch:
/// written through `features` or through `dataSources`, read back through both.
#[tokio::test]
async fn a_data_source_node_and_its_feature_are_one_switch() {
    // Written through `features`, over the whole value space of the switch.
    for (name, path) in GOVERNED_NODES {
        for (status, flag) in ORG_FEATURE_STATUSES {
            let case = format!("feature {name} at {status} -> dataSources.{}", path.join("."));
            let router = new_router();
            let detector_id = enabled_detector(&router).await;
            update_ok(
                &router,
                &detector_id,
                json!({
                    "autoEnableOrganizationMembers": "NEW",
                    "features": [{"name": name, "autoEnable": status}],
                }),
            )
            .await;
            let got = describe(&router, &detector_id).await;
            assert_eq!(
                feature_status(&got, name),
                status,
                "{case}: expected {status}, got {got:?}"
            );
            assert_eq!(
                node_flag(&got, path, &case),
                flag,
                "{case}: the deprecated view of {status} is {flag}, got {got:?}"
            );
        }
    }

    // Written through the deprecated `dataSources` input, which carries a boolean,
    // so true is NEW and false is NONE.
    let data_sources: [(&str, &[&str], Value, Value); 3] = [
        (
            "S3_DATA_EVENTS",
            &["s3Logs"],
            json!({"s3Logs": {"autoEnable": true}}),
            json!({"s3Logs": {"autoEnable": false}}),
        ),
        (
            "EKS_AUDIT_LOGS",
            &["kubernetes", "auditLogs"],
            json!({"kubernetes": {"auditLogs": {"autoEnable": true}}}),
            json!({"kubernetes": {"auditLogs": {"autoEnable": false}}}),
        ),
        (
            "EBS_MALWARE_PROTECTION",
            &[
                "malwareProtection",
                "scanEc2InstanceWithFindings",
                "ebsVolumes",
            ],
            json!({"malwareProtection": {"scanEc2InstanceWithFindings": {"ebsVolumes": {"autoEnable": true}}}}),
            json!({"malwareProtection": {"scanEc2InstanceWithFindings": {"ebsVolumes": {"autoEnable": false}}}}),
        ),
    ];
    for (name, path, enabling, disabling) in data_sources {
        for (body, expected, flag) in [(enabling, "NEW", true), (disabling, "NONE", false)] {
            let case = format!("dataSources {body} -> {name}");
            let router = new_router();
            let detector_id = enabled_detector(&router).await;
            update_ok(
                &router,
                &detector_id,
                json!({"autoEnable": true, "dataSources": body}),
            )
            .await;
            let got = describe(&router, &detector_id).await;
            assert_eq!(
                feature_status(&got, name),
                expected,
                "{case}: expected {expected}, got {got:?}"
            );
            assert_eq!(
                node_flag(&got, path, &case),
                flag,
                "{case}: expected {flag}, got {got:?}"
            );
        }
    }
}

/// An explicit `features` entry wins over the deprecated `dataSources` node for the
/// same switch: one request cannot leave a switch in two states.
#[tokio::test]
async fn an_explicit_feature_entry_wins_over_the_deprecated_view() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    update_ok(
        &router,
        &detector_id,
        json!({
            "autoEnable": true,
            "dataSources": {"s3Logs": {"autoEnable": true}},
            "features": [{"name": "S3_DATA_EVENTS", "autoEnable": "NONE"}],
        }),
    )
    .await;
    let got = describe(&router, &detector_id).await;
    assert_eq!(
        feature_status(&got, "S3_DATA_EVENTS"),
        "NONE",
        "features must win over dataSources for one switch, got {got:?}"
    );
    assert!(
        !node_flag(&got, &["s3Logs"], "features over dataSources"),
        "both views read one switch, so dataSources must report false too, got {got:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3.
// ---------------------------------------------------------------------------

/// `features` is the whole `OrgFeature` set, once each, in canonical order, whatever
/// an update named.
#[tokio::test]
async fn features_are_reported_over_the_whole_set_in_canonical_order() {
    let updates: [Value; 4] = [
        json!({"autoEnableOrganizationMembers": "NONE"}),
        json!({"autoEnable": true}),
        json!({"autoEnableOrganizationMembers": "ALL",
               "features": [{"name": "S3_DATA_EVENTS", "autoEnable": "NEW"}]}),
        json!({"autoEnable": false, "dataSources": {"s3Logs": {"autoEnable": true}}}),
    ];

    for body in updates {
        let case = format!("update {body}");
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        update_ok(&router, &detector_id, body.clone()).await;
        let got = describe(&router, &detector_id).await;
        let reported: Vec<String> = got["features"]
            .as_array()
            .unwrap_or_else(|| panic!("{case}: expected a features list, got {got:?}"))
            .iter()
            .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported,
            ORG_FEATURES.map(String::from).to_vec(),
            "{case}: features must be the whole OrgFeature set, once each, in canonical order, \
             got {got:?}"
        );
        for name in ORG_FEATURES {
            assert!(
                matches!(
                    feature_status(&got, name).as_str(),
                    "ALL" | "NEW" | "NONE"
                ),
                "{case}: feature {name} must report an OrgFeatureStatus, got {got:?}"
            );
        }
    }
}

/// Paging the feature list with any `maxResults` yields each feature exactly once, in
/// the canonical order, and the last page carries no token.
#[tokio::test]
async fn paging_the_feature_list_yields_each_feature_exactly_once() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    update_ok(
        &router,
        &detector_id,
        json!({"autoEnableOrganizationMembers": "NEW"}),
    )
    .await;

    // Every page size from one feature at a time to past the whole set, plus the
    // modelled maximum (`MaxResults` is `range 1..50`).
    for page_size in [1usize, 2, 3, 7, 8, 9, 50] {
        let case = format!("maxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let (status, got) =
                describe_page(&router, &detector_id, Some(page_size), token.as_deref()).await;
            assert_eq!(status, 200, "{case}: expected 200, got {status} {got:?}");
            let page = got["features"]
                .as_array()
                .unwrap_or_else(|| panic!("{case}: expected a features list, got {got:?}"))
                .clone();
            assert!(
                !page.is_empty() && page.len() <= page_size,
                "{case}: a page carries between one and {page_size} features, got {} ({got:?})",
                page.len()
            );
            // The members that are not the paginated list are reported on every page.
            assert_eq!(
                got["autoEnableOrganizationMembers"],
                json!("NEW"),
                "{case}: every page reports the organization's auto-enablement, got {got:?}"
            );
            walked.extend(
                page.iter()
                    .map(|entry| entry["name"].as_str().unwrap_or_default().to_string()),
            );
            pages += 1;
            assert!(
                pages <= ORG_FEATURES.len() + 1,
                "{case}: paging must terminate, walked {walked:?}"
            );
            match got["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked,
            ORG_FEATURES.map(String::from).to_vec(),
            "{case}: paging must yield the whole set once each, in order"
        );
    }
}

/// A `nextToken` this service never issued is refused, not answered with a page.
#[tokio::test]
async fn a_token_the_service_never_issued_is_refused() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    // Rows differ along FORM (not an offset at all) and along RANGE (an offset past
    // the end of a list this service only ever issues tokens inside).
    for token in ["not-a-token", "-1", "8", "99", "1.5"] {
        let case = format!("nextToken {token:?}");
        let (status, got) = describe_page(&router, &detector_id, None, Some(token)).await;
        assert_rejected(status, &got, &case);
    }
}

// ---------------------------------------------------------------------------
// Rule 4.
// ---------------------------------------------------------------------------

/// A detector id this account/Region does not hold is refused by both operations,
/// never answered with a hollow 200 or a 501.
#[tokio::test]
async fn an_unheld_detector_id_is_refused_by_both_operations() {
    // (the id, why it cannot resolve)
    let ids: [(&str, &str); 3] = [
        (ABSENT_DETECTOR_ID, "well formed, never minted"),
        ("not-a-detector-id", "not of the minted form at all"),
        (
            "deadbeefdeadbeefdeadbeefdeadbeefdead",
            "36 hexadecimal characters",
        ),
    ];
    let router = new_router();
    // The describe half is about the DETECTOR id, so the standing that report needs is
    // established first — otherwise its refusal would be the delegated-administrator one
    // ([`the_report_needs_a_delegated_administrator_to_be_about`]) and the row would
    // assert nothing about the id.
    delegated_administrator_in(&router, DEFAULT_REGION).await;
    for (id, why) in ids {
        let (status, got) = call(&router, "GET", &format!("/detector/{id}/admin"), None).await;
        assert_rejected(status, &got, &format!("describe {why}"));
        assert_ne!(
            got["message"].as_str().unwrap_or_default(),
            service_com_amazonaws_guardduty_guarddutyapiservice::scaffolded::DELEGATED_ADMINISTRATOR_NOT_ENABLED_MESSAGE,
            "describe {why}: the refusal must be about the detector id, not the standing, \
             got {got:?}"
        );
        let (status, got) =
            update(&router, id, json!({"autoEnableOrganizationMembers": "NEW"})).await;
        assert_rejected(status, &got, &format!("update {why}"));
    }
}

/// A request that names the auto-enablement twice, or not at all, is refused and
/// writes nothing.
#[tokio::test]
async fn the_auto_enable_views_are_mutually_exclusive_and_one_is_required() {
    let bodies: [(Value, &str); 4] = [
        (json!({}), "neither view"),
        (
            json!({"features": [{"name": "S3_DATA_EVENTS", "autoEnable": "ALL"}]}),
            "features but neither view",
        ),
        (
            json!({"autoEnable": true, "autoEnableOrganizationMembers": "NEW"}),
            "both views, agreeing",
        ),
        (
            json!({"autoEnable": false, "autoEnableOrganizationMembers": "ALL"}),
            "both views, disagreeing",
        ),
    ];

    for (body, case) in bodies {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let before = describe(&router, &detector_id).await;
        let (status, got) = update(&router, &detector_id, body.clone()).await;
        assert_rejected(status, &got, case);
        let after = describe(&router, &detector_id).await;
        assert_eq!(after, before, "{case}: a refused update must write nothing");
    }
}

/// A `features` list the model cannot carry, or the operation documents as
/// contradictory, is refused and writes nothing.
///
/// A value outside an enum the model declares is refused at the BOUNDARY, under the
/// modelled error shape; the rule that spans two entries is the operation's own, so
/// the handler refuses it under the service's error class.
#[tokio::test]
async fn an_unacceptable_features_list_is_refused_and_writes_nothing() {
    // (the features body, why, whether the model itself refuses the value)
    let bodies: [(Value, &str, bool); 6] = [
        (
            json!({"features": [{"name": "NOT_A_FEATURE", "autoEnable": "ALL"}]}),
            "a name no OrgFeature carries",
            true,
        ),
        (
            json!({"features": [{"name": "CLOUD_TRAIL", "autoEnable": "ALL"}]}),
            "a DetectorFeature that is not an OrgFeature",
            true,
        ),
        (
            json!({"features": [{"name": "S3_DATA_EVENTS", "autoEnable": "ENABLED"}]}),
            "a FeatureStatus where an OrgFeatureStatus belongs",
            true,
        ),
        (
            json!({"features": [{"name": "RUNTIME_MONITORING", "autoEnable": "ALL",
                                 "additionalConfiguration": [{"name": "NOPE", "autoEnable": "ALL"}]}]}),
            "an additional configuration the model does not carry",
            true,
        ),
        (
            json!({"features": [{"name": "RUNTIME_MONITORING", "autoEnable": "ALL",
                                 "additionalConfiguration": [
                                     {"name": ADDITIONAL_CONFIGURATION, "autoEnable": "ENABLED"}]}]}),
            "an additional configuration at a status the model does not carry",
            true,
        ),
        (
            json!({"features": [{"name": "RUNTIME_MONITORING", "autoEnable": "ALL"},
                                {"name": "EKS_RUNTIME_MONITORING", "autoEnable": "NEW"}]}),
            "both Runtime Monitoring features enabled together",
            false,
        ),
    ];

    for (features, case, modelled) in bodies {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let before = describe(&router, &detector_id).await;
        let mut body = features.clone();
        body["autoEnableOrganizationMembers"] = json!("NEW");
        let (status, got) = update(&router, &detector_id, body).await;
        if modelled {
            assert_boundary_refusal(status, &got, case);
        } else {
            assert_rejected(status, &got, case);
        }
        let after = describe(&router, &detector_id).await;
        assert_eq!(after, before, "{case}: a refused update must write nothing");
    }

    // Runtime Monitoring and EKS Runtime Monitoring are refused only when BOTH are
    // being enabled: either one alone, and both at NONE, are acceptable. So is a
    // list that NAMES one of them without a status, which leaves that switch at the
    // NONE a fresh detector holds.
    let acceptable: [Value; 4] = [
        json!([{"name": "RUNTIME_MONITORING", "autoEnable": "ALL"}]),
        json!([{"name": "EKS_RUNTIME_MONITORING", "autoEnable": "ALL"}]),
        json!([{"name": "RUNTIME_MONITORING", "autoEnable": "NONE"},
               {"name": "EKS_RUNTIME_MONITORING", "autoEnable": "NONE"}]),
        json!([{"name": "RUNTIME_MONITORING"},
               {"name": "EKS_RUNTIME_MONITORING", "autoEnable": "ALL"}]),
    ];
    for features in acceptable {
        let case = format!("features {features}");
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let (status, got) = update(
            &router,
            &detector_id,
            json!({"autoEnableOrganizationMembers": "NEW", "features": features}),
        )
        .await;
        assert_eq!(status, 200, "{case}: expected 200, got {status} {got:?}");
    }
}

/// The exclusion is judged on the auto-enablement each of the two features ENDS UP
/// with — the request's value where it states one, the STORED value otherwise — not
/// on what one request happens to mention.
///
/// The rows span where the other feature's enablement comes from: both from this
/// request, one from the store, and (the accepted row) a mention with no status over
/// a stored `NONE`.
#[tokio::test]
async fn the_runtime_monitoring_exclusion_is_judged_on_the_effective_status() {
    // (what an earlier update stored, what this update asks for, accepted?)
    let cases: [(Value, Value, bool); 4] = [
        (
            json!([{"name": "EKS_RUNTIME_MONITORING", "autoEnable": "ALL"}]),
            json!([{"name": "RUNTIME_MONITORING", "autoEnable": "ALL"}]),
            false,
        ),
        (
            json!([{"name": "RUNTIME_MONITORING", "autoEnable": "NEW"}]),
            json!([{"name": "EKS_RUNTIME_MONITORING", "autoEnable": "NEW"}]),
            false,
        ),
        // Switching the stored one OFF in the same request makes room for the other.
        (
            json!([{"name": "EKS_RUNTIME_MONITORING", "autoEnable": "ALL"}]),
            json!([{"name": "EKS_RUNTIME_MONITORING", "autoEnable": "NONE"},
                   {"name": "RUNTIME_MONITORING", "autoEnable": "ALL"}]),
            true,
        ),
        // A mention with no status leaves the stored NONE alone, so nothing overlaps.
        (
            json!([{"name": "EKS_RUNTIME_MONITORING", "autoEnable": "NONE"}]),
            json!([{"name": "EKS_RUNTIME_MONITORING"},
                   {"name": "RUNTIME_MONITORING", "autoEnable": "ALL"}]),
            true,
        ),
    ];

    for (stored, asked, accepted) in cases {
        let case = format!("stored {stored}, asked {asked}");
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        update_ok(
            &router,
            &detector_id,
            json!({"autoEnableOrganizationMembers": "NEW", "features": stored}),
        )
        .await;
        let before = describe(&router, &detector_id).await;
        let (status, got) = update(
            &router,
            &detector_id,
            json!({"autoEnableOrganizationMembers": "NEW", "features": asked}),
        )
        .await;
        if accepted {
            assert_eq!(status, 200, "{case}: expected 200, got {status} {got:?}");
            let after = describe(&router, &detector_id).await;
            assert_eq!(
                feature_status(&after, "RUNTIME_MONITORING"),
                "ALL",
                "{case}: the accepted update must store the enablement it asked for, got {after:?}"
            );
            assert_eq!(
                feature_status(&after, "EKS_RUNTIME_MONITORING"),
                "NONE",
                "{case}: and must leave the other feature off, got {after:?}"
            );
        } else {
            assert_rejected(status, &got, &case);
            let after = describe(&router, &detector_id).await;
            assert_eq!(after, before, "{case}: a refused update must write nothing");
        }
    }
}

/// An `autoEnableOrganizationMembers` value the enum does not carry is refused by
/// the model's own constraint, and writes nothing.
#[tokio::test]
async fn an_auto_enable_members_value_the_enum_does_not_carry_is_refused() {
    // Rows differ along FORM (a value no member names, a member mis-cased, a member
    // of the WRONG enum, two members at once) and include the empty string.
    for value in ["EVERYTHING", "new", "ENABLED", "ALL,NEW", ""] {
        let case = format!("autoEnableOrganizationMembers {value:?}");
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let before = describe(&router, &detector_id).await;
        let (status, got) = update(
            &router,
            &detector_id,
            json!({"autoEnableOrganizationMembers": value}),
        )
        .await;
        assert_boundary_refusal(status, &got, &case);
        let after = describe(&router, &detector_id).await;
        assert_eq!(after, before, "{case}: a refused update must write nothing");
    }
}

// ---------------------------------------------------------------------------
// Rule 5.
// ---------------------------------------------------------------------------

/// Reading the configuration changes nothing a later read sees.
#[tokio::test]
async fn a_read_changes_nothing_a_later_read_sees() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    update_ok(
        &router,
        &detector_id,
        json!({
            "autoEnableOrganizationMembers": "ALL",
            "features": [{"name": "S3_DATA_EVENTS", "autoEnable": "NEW"}],
        }),
    )
    .await;

    let first = describe(&router, &detector_id).await;
    for round in 1..=3 {
        let again = describe(&router, &detector_id).await;
        assert_eq!(
            again, first,
            "read {round}: a report must not change what a later caller sees"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 6.
// ---------------------------------------------------------------------------

/// The report is about the account selected as the GuardDuty delegated administrator,
/// so without one selected there is nothing to report and the read is refused.
///
/// The rows span every way the standing can be absent — never designated, designated
/// for ANOTHER account, designated through another Region's endpoint only, and a
/// designation whose removal the clock has already settled — and the refusal is the
/// same 400 with the same sentence for each, because the one fact it reports is that
/// the precondition is missing. Each row then designates this account and reads again,
/// so the test cannot pass against a handler that simply always refuses.
#[tokio::test]
async fn the_report_needs_a_delegated_administrator_to_be_about() {
    // What the delegated-administrator refusal states — the service's own sentence for
    // the missing precondition, read from the crate so the test tracks the handler's
    // wording rather than restating it.
    use service_com_amazonaws_guardduty_guarddutyapiservice::scaffolded::DELEGATED_ADMINISTRATOR_NOT_ENABLED_MESSAGE;

    // The account this emulator serves, which is the only one a designation makes the
    // delegated administrator of the organization it answers for.
    let served = ::engine::emulator_account_id();
    // An account id of the modelled form that is not the one this emulator serves.
    const ANOTHER_ACCOUNT: &str = "999999999000";
    // A second Region's endpoint, since a designation is held by the Region the
    // designating request was signed for and by no other.
    const ANOTHER_REGION: &str = "eu-west-1";

    // (the case, whether the row has to wait out a removal the clock owes)
    type Absent = (&'static str, bool);
    let rows: [Absent; 4] = [
        ("no designation at all", false),
        ("a designation for another account", false),
        ("a designation through another Region only", false),
        ("a designation whose removal has settled", true),
    ];

    for (case, waits) in rows {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        match case {
            "a designation for another account" => {
                let (status, got) = enable_admin(&router, DEFAULT_REGION, ANOTHER_ACCOUNT).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the setup designation expected 200, got {status} {got:?}"
                );
            }
            "a designation through another Region only" => {
                let (status, got) = enable_admin(&router, ANOTHER_REGION, served).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the setup designation expected 200, got {status} {got:?}"
                );
            }
            "a designation whose removal has settled" => {
                let (status, got) = enable_admin(&router, DEFAULT_REGION, served).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the setup designation expected 200, got {status} {got:?}"
                );
                let (status, got) = disable_admin(&router, DEFAULT_REGION, served).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the setup removal expected 200, got {status} {got:?}"
                );
            }
            _ => {}
        }
        if waits {
            tokio::time::sleep(past_the_removal()).await;
        }

        let (status, got) = call(
            &router,
            "GET",
            &format!("/detector/{detector_id}/admin"),
            None,
        )
        .await;
        assert_rejected(status, &got, case);
        assert_eq!(
            got["message"].as_str().unwrap_or_default(),
            DELEGATED_ADMINISTRATOR_NOT_ENABLED_MESSAGE,
            "case {case:?}: expected GuardDuty's sentence for a missing delegated \
             administrator, got {got:?}"
        );

        // The same read, once this account IS the organization's delegated
        // administrator: the refusal above is the missing standing and nothing else.
        if case == "a designation for another account" {
            // One organization has at most one delegated administrator, so the other
            // account's designation is removed before this one is made.
            let (status, got) = disable_admin(&router, DEFAULT_REGION, ANOTHER_ACCOUNT).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the setup removal expected 200, got {status} {got:?}"
            );
            tokio::time::sleep(past_the_removal()).await;
        }
        let reported = describe(&router, &detector_id).await;
        assert_eq!(
            reported["autoEnableOrganizationMembers"]
                .as_str()
                .unwrap_or_default(),
            "NONE",
            "case {case:?}: with the standing held the read must report the stored \
             configuration, got {reported:?}"
        );
    }
}
