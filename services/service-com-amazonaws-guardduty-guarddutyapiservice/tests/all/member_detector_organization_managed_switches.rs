// SPDX-License-Identifier: CC0-1.0

//! `UpdateMemberDetectors` under an organization configuration: what an
//! administrator's stored auto-enable settings do to a per-account write of the
//! same switches, read back through `GetMemberDetectors`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded call:
//! a switch the organization auto-enables for any account — a feature, or one of a
//! feature's additional configurations, whose `OrgFeatureStatus` is anything other
//! than `NONE` — is the ORGANIZATION's to set, so a per-account write leaves it
//! auto-enabled whatever the request asked for, while a switch auto-enabled for no
//! account is one "the administrator must manage ... for each account individually"
//! (`smithy_query(query="doc",shape="OrganizationFeatureConfiguration",member="AutoEnable")`)
//! and takes exactly what the request said.
//!
//! So a member's `GetMemberDetectors` configuration agrees with what
//! `DescribeOrganizationConfiguration` reports for the same organization after a
//! member write too — the agreement `CreateMembers` maintains for the accounts it
//! records, over the same stored switches.
//!
//! The rows span the whole `OrgFeature` catalog against the whole
//! `OrgFeatureStatus` catalog, both `FeatureStatus` values a request can ask for,
//! both views a request can name a switch through (the `Features` list and the
//! deprecated `DataSources` one) and the `OrgFeatureAdditionalConfiguration`
//! catalog.

use serde_json::{json, Value};

use crate::detector_lifecycle::{enabled_detector, router_with_engine, Router};
use crate::member_association::{unprocessed_ids, DEFAULT_REGION};
use crate::member_detector_configuration::update_member_detectors;
// The organization-side table, bodies and projections live in the sibling suite that
// asserts the same agreement through `CreateMembers`, so the two cannot drift on what
// an auto-enablement is or on what a member starts with.
use crate::member_organization_auto_enable::{
    account_id, default_status, feature_instant, feature_status, member_configuration,
    organization_body, organization_body_with_additional, record_member, ADDITIONAL_CONFIGURATIONS,
    DISABLED, EMAIL_FORMS, ENABLED, OPT_IN_FEATURES, ORG_FEATURES, ORG_FEATURE_STATUSES,
};
use crate::organization_configuration::{describe_in, update_ok_in};

/// The `DataSources` bodies a request can name a member's switches through, each
/// paired with the feature that governs the switch it writes: the members of
/// `DataSourceConfigurations` and its nested shapes
/// (`smithy_query(shape="DataSourceConfigurations")`), whose boolean says ENABLED
/// when true.
///
/// `enable` is substituted per row, so each body is written twice — once asking for
/// the switch on and once off.
const DATA_SOURCE_VIEWS: [(&str, &str); 3] = [
    ("S3_DATA_EVENTS", "s3Logs"),
    ("EKS_AUDIT_LOGS", "kubernetes"),
    ("EBS_MALWARE_PROTECTION", "malwareProtection"),
];

/// The `DataSources` body that writes one data source's switch to `enable`.
fn data_sources_body(node: &str, enable: bool) -> Value {
    match node {
        "s3Logs" => json!({"s3Logs": {"enable": enable}}),
        "kubernetes" => json!({"kubernetes": {"auditLogs": {"enable": enable}}}),
        _ => json!({
            "malwareProtection": {"scanEc2InstanceWithFindings": {"ebsVolumes": enable}}
        }),
    }
}

/// `UpdateMemberDetectors` for one account, asserting it processed the account.
async fn update_ok(
    router: &Router,
    detector_id: &str,
    account_id: &str,
    body: Value,
    case: &str,
) {
    let (status, response) =
        update_member_detectors(router, detector_id, &[account_id], body).await;
    assert_eq!(
        status, 200,
        "case {case}: update-member-detectors expected 200, got {status} {response:?}"
    );
    assert!(
        unprocessed_ids(&response).is_empty(),
        "case {case}: expected the account processed, got {response:?}"
    );
}

/// Switch EKS Runtime Monitoring off for one account, which a row whose request
/// ENABLES Runtime Monitoring has to do first: a member starts with the EKS switch
/// on, and "you can add only one of these two features"
/// (`smithy_query(query="doc",shape="UpdateMemberDetectors")`), so a request that
/// would leave both on is refused before it writes anything.
async fn clear_eks_runtime_monitoring(
    router: &Router,
    detector_id: &str,
    account: &str,
    case: &str,
) {
    update_ok(
        router,
        detector_id,
        account,
        json!({"features": [{"name": "EKS_RUNTIME_MONITORING", "status": DISABLED}]}),
        case,
    )
    .await;
}

/// One feature's `autoEnable` as `DescribeOrganizationConfiguration` reports it.
fn reported_auto_enable(organization: &Value, feature: &str, case: &str) -> String {
    organization["features"]
        .as_array()
        .and_then(|features| {
            features
                .iter()
                .find(|entry| entry["name"].as_str() == Some(feature))
        })
        .and_then(|entry| entry["autoEnable"].as_str())
        .unwrap_or_else(|| panic!("case {case}: expected {feature} reported, got {organization:?}"))
        .to_string()
}

/// One of a feature's reported additional-configuration entries, whole — its status
/// and the instant it carries — or `None` when the feature carries no such
/// configuration at all.
fn additional_entry(
    configuration: &Value,
    feature: &str,
    additional: &str,
    case: &str,
) -> Option<Value> {
    crate::member_organization_auto_enable::feature_entry(configuration, feature, case)
        ["additionalConfiguration"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["name"].as_str() == Some(additional))
        .cloned()
}

// ---------------------------------------------------------------------------
// The feature switch: every `OrgFeature` against every `OrgFeatureStatus`, written
// both ways a request can ask for.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_member_write_cannot_move_a_feature_the_organization_auto_enables() {
    for (case_index, feature) in ORG_FEATURES.into_iter().enumerate() {
        for (status_index, (auto_enable, enables)) in ORG_FEATURE_STATUSES.into_iter().enumerate() {
            for (requested_index, requested) in [DISABLED, ENABLED].into_iter().enumerate() {
                // One organization per row: the two Runtime Monitoring features
                // cannot be auto-enabled together, so a row must not inherit the
                // previous row's settings.
                let (router, engine) = router_with_engine();
                let detector_id = enabled_detector(&router).await;
                let case =
                    format!("{feature} auto-enabled {auto_enable}, member write {requested}");

                update_ok_in(
                    &router,
                    DEFAULT_REGION,
                    &detector_id,
                    organization_body(&[(feature, auto_enable)]),
                )
                .await;

                let account = account_id(
                    case_index * ORG_FEATURE_STATUSES.len() * 2 + status_index * 2
                        + requested_index,
                );
                let email = EMAIL_FORMS[requested_index % EMAIL_FORMS.len()];
                record_member(&engine, &router, &detector_id, &account, email, &case).await;

                if feature == "RUNTIME_MONITORING" && requested == ENABLED {
                    clear_eks_runtime_monitoring(&router, &detector_id, &account, &case).await;
                }

                let before = member_configuration(&router, &detector_id, &account, &case).await;
                let was = feature_status(&before, feature, &case);
                let was_at = feature_instant(&before, feature, &case);

                update_ok(
                    &router,
                    &detector_id,
                    &account,
                    json!({"features": [{"name": feature, "status": requested}]}),
                    &case,
                )
                .await;

                // The organization owns the switch, so the request moves nothing;
                // otherwise the switch is managed per account and takes what the
                // request said.
                let expected = if enables { ENABLED } else { requested };
                let after = member_configuration(&router, &detector_id, &account, &case).await;
                let got = feature_status(&after, feature, &case);
                assert_eq!(
                    got, expected,
                    "case {case}: expected {feature} {expected}, got {got} in {after:?}"
                );

                // The agreement, read off the organization itself rather than off
                // the row: whatever the member write asked for, a feature the
                // organization reports auto-enabled is enabled for the account.
                let organization = describe_in(&router, DEFAULT_REGION, &detector_id).await;
                let reported = reported_auto_enable(&organization, feature, &case);
                if reported != "NONE" {
                    assert_eq!(
                        got, ENABLED,
                        "case {case}: {feature} is auto-enabled {reported}, so expected \
                         {ENABLED}, got {got} in {after:?}"
                    );
                }

                // The instant says when the switch last CHANGED: a write the
                // organization overrides changes nothing, so it does not move.
                let now_at = feature_instant(&after, feature, &case);
                if got == was {
                    assert!(
                        (now_at - was_at).abs() < 0.001,
                        "case {case}: {feature} stayed {got}, so expected its instant to stay \
                         at {was_at}, got {now_at}"
                    );
                } else {
                    assert!(
                        now_at > was_at,
                        "case {case}: {feature} moved {was} -> {got}, so expected its instant \
                         to move past {was_at}, got {now_at}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_member_write_brings_a_feature_the_organization_auto_enables_into_agreement() {
    // The opt-in features are the discriminating ones: a member records them
    // DISABLED, so an account recorded BEFORE the organization was configured
    // disagrees with the organization read until a write reaches it.
    for (case_index, feature) in OPT_IN_FEATURES.into_iter().enumerate() {
        for (status_index, auto_enable) in ["NEW", "ALL"].into_iter().enumerate() {
            for (requested_index, requested) in [DISABLED, ENABLED].into_iter().enumerate() {
                let (router, engine) = router_with_engine();
                let detector_id = enabled_detector(&router).await;
                let case = format!(
                    "{feature} auto-enabled {auto_enable} after the member was recorded, \
                     member write {requested}"
                );

                let account = account_id(
                    400 + case_index * 4 + status_index * 2 + requested_index,
                );
                let email = EMAIL_FORMS[case_index % EMAIL_FORMS.len()];
                record_member(&engine, &router, &detector_id, &account, email, &case).await;
                let before = member_configuration(&router, &detector_id, &account, &case).await;
                let was_at = feature_instant(&before, feature, &case);
                assert_eq!(
                    feature_status(&before, feature, &case),
                    DISABLED,
                    "case {case}: expected {feature} to start {DISABLED}, got {before:?}"
                );

                update_ok_in(
                    &router,
                    DEFAULT_REGION,
                    &detector_id,
                    organization_body(&[(feature, auto_enable)]),
                )
                .await;

                if feature == "RUNTIME_MONITORING" && requested == ENABLED {
                    clear_eks_runtime_monitoring(&router, &detector_id, &account, &case).await;
                }
                update_ok(
                    &router,
                    &detector_id,
                    &account,
                    json!({"features": [{"name": feature, "status": requested}]}),
                    &case,
                )
                .await;

                let after = member_configuration(&router, &detector_id, &account, &case).await;
                let got = feature_status(&after, feature, &case);
                assert_eq!(
                    got, ENABLED,
                    "case {case}: the organization auto-enables {feature}, so expected \
                     {ENABLED} after the member write, got {got} in {after:?}"
                );
                let now_at = feature_instant(&after, feature, &case);
                assert!(
                    now_at > was_at,
                    "case {case}: {feature} moved {DISABLED} -> {got}, so expected its instant \
                     to move past {was_at}, got {now_at}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The additional-configuration switch, which carries an auto-enablement of its own.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_member_write_cannot_move_an_additional_configuration_the_organization_auto_enables() {
    for (case_index, (feature, additional)) in ADDITIONAL_CONFIGURATIONS.into_iter().enumerate() {
        for (status_index, (auto_enable, enables)) in ORG_FEATURE_STATUSES.into_iter().enumerate() {
            let (router, engine) = router_with_engine();
            let detector_id = enabled_detector(&router).await;
            let case = format!("{feature}/{additional} auto-enabled {auto_enable}");

            // The feature itself is auto-enabled for every account, so what varies
            // along the rows is the additional configuration's own auto-enablement:
            // one the organization auto-enables is its to set, one it auto-enables
            // for nobody is the administrator's to set per account.
            update_ok_in(
                &router,
                DEFAULT_REGION,
                &detector_id,
                organization_body_with_additional(feature, "ALL", additional, auto_enable),
            )
            .await;

            let account = account_id(500 + case_index * ORG_FEATURE_STATUSES.len() + status_index);
            let email = EMAIL_FORMS[status_index % EMAIL_FORMS.len()];
            record_member(&engine, &router, &detector_id, &account, email, &case).await;
            if feature == "RUNTIME_MONITORING" {
                clear_eks_runtime_monitoring(&router, &detector_id, &account, &case).await;
            }
            let before = member_configuration(&router, &detector_id, &account, &case).await;
            let was = additional_entry(&before, feature, additional, &case);

            update_ok(
                &router,
                &detector_id,
                &account,
                json!({"features": [{
                    "name": feature,
                    "status": ENABLED,
                    "additionalConfiguration": [{"name": additional, "status": DISABLED}],
                }]}),
                &case,
            )
            .await;

            let after = member_configuration(&router, &detector_id, &account, &case).await;
            let got = additional_entry(&after, feature, additional, &case);
            if enables {
                // The organization auto-enables it, so the write moved nothing at
                // all — not the status, and not the instant beside it.
                assert_eq!(
                    got.as_ref().and_then(|entry| entry["status"].as_str()),
                    Some(ENABLED),
                    "case {case}: expected {additional} {ENABLED}, got {after:?}"
                );
                assert_eq!(
                    got, was,
                    "case {case}: expected {additional} untouched, got {got:?} after {was:?}"
                );
            } else {
                assert_eq!(
                    got.as_ref().and_then(|entry| entry["status"].as_str()),
                    Some(DISABLED),
                    "case {case}: {additional} is auto-enabled for no account, so expected the \
                     member write to leave it {DISABLED}, got {after:?}"
                );
            }

            // The feature that carries it is auto-enabled for every account either
            // way.
            let feature_got = feature_status(&after, feature, &case);
            assert_eq!(
                feature_got, ENABLED,
                "case {case}: expected the auto-enabled {feature} {ENABLED}, got {feature_got} \
                 in {after:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The deprecated `DataSources` view writes the same stored switches, so the rule
// cannot depend on which view a request names one through.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_deprecated_data_sources_view_obeys_the_organizations_auto_enablement_too() {
    for (case_index, (feature, node)) in DATA_SOURCE_VIEWS.into_iter().enumerate() {
        for (status_index, (auto_enable, enables)) in ORG_FEATURE_STATUSES.into_iter().enumerate() {
            for (enable_index, enable) in [false, true].into_iter().enumerate() {
                let (router, engine) = router_with_engine();
                let detector_id = enabled_detector(&router).await;
                let case = format!(
                    "{feature} auto-enabled {auto_enable}, dataSources.{node} enable {enable}"
                );

                update_ok_in(
                    &router,
                    DEFAULT_REGION,
                    &detector_id,
                    organization_body(&[(feature, auto_enable)]),
                )
                .await;

                let account = account_id(
                    600 + case_index * ORG_FEATURE_STATUSES.len() * 2 + status_index * 2
                        + enable_index,
                );
                let email = EMAIL_FORMS[enable_index % EMAIL_FORMS.len()];
                record_member(&engine, &router, &detector_id, &account, email, &case).await;

                update_ok(
                    &router,
                    &detector_id,
                    &account,
                    json!({"dataSources": data_sources_body(node, enable)}),
                    &case,
                )
                .await;

                // A data source the organization auto-enables stays on; one it
                // auto-enables for nobody takes the boolean the view carried.
                let expected = if enables || enable { ENABLED } else { DISABLED };
                let after = member_configuration(&router, &detector_id, &account, &case).await;
                let got = feature_status(&after, feature, &case);
                assert_eq!(
                    got, expected,
                    "case {case}: expected {feature} {expected}, got {got} in {after:?}"
                );

                // Every other feature is where a member starts: one view's node
                // writes the switch it names and no other.
                for other in ORG_FEATURES.into_iter().filter(|name| *name != feature) {
                    let other_got = feature_status(&after, other, &case);
                    assert_eq!(
                        other_got,
                        default_status(other),
                        "case {case}: expected {other} left at its default, got {other_got} in \
                         {after:?}"
                    );
                }
            }
        }
    }
}
