// SPDX-License-Identifier: CC0-1.0

//! `CreateMembers` under an organization configuration: what the administrator's
//! stored auto-enable settings do to the member accounts a request names, read back
//! through `GetMemberDetectors`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. Recording a member account applies the organization's auto-enable settings to
//!    every account the request names, new or existing: a feature the organization
//!    auto-enables for any account — any `OrgFeatureStatus` other than `NONE` — is
//!    `ENABLED` on the member, and a feature auto-enabled for none of them keeps
//!    whatever the member's configuration already said, because that one is managed
//!    per account. Asserted for every `OrgFeature` against every `OrgFeatureStatus`.
//! 2. An auto-enabled feature brings the additional configurations the organization
//!    auto-enables with it, and only those.
//! 3. So a member's configuration AGREES with what
//!    `DescribeOrganizationConfiguration` reports for the same organization: every
//!    feature reported auto-enabled is enabled for the member, whatever mixture of
//!    statuses the organization holds, and whichever member of the batch is read.
//! 4. Applying the settings is idempotent and touches only the CONFIGURATION: a
//!    second identical `CreateMembers` reports the same statuses and the same
//!    per-feature instants, and the member's relationship — its
//!    `relationshipStatus`, its email and its `updatedAt` — is where it was.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, new_router, Router};
use crate::member_association::{
    create_members, get_members, invite_members, member_of, DEFAULT_REGION,
};
use crate::organization_configuration::{describe_in, update_ok_in};

/// Every `OrgFeature` member, in model order
/// (`smithy_query(shape="OrgFeature")`) — the features an organization
/// configuration carries an auto-enablement for, which is also the set a member
/// request can configure.
///
/// Written out here rather than read from the crate, so the test PINS the catalog:
/// a feature the model gains has to be added here too.
pub(crate) const ORG_FEATURES: [&str; 8] = [
    "S3_DATA_EVENTS",
    "EKS_AUDIT_LOGS",
    "EBS_MALWARE_PROTECTION",
    "RDS_LOGIN_EVENTS",
    "LAMBDA_NETWORK_LOGS",
    "EKS_RUNTIME_MONITORING",
    "RUNTIME_MONITORING",
    "AI_PROTECTION",
];

/// Every `OrgFeatureStatus` member (`smithy_query(shape="OrgFeatureStatus")`), each
/// paired with whether it is an auto-enablement: `NEW` auto-enables the feature for
/// an account joining the organization and `ALL` for all of them, while `NONE`
/// auto-enables it for no account at all
/// (`smithy_query(query="doc",shape="OrganizationFeatureConfiguration",member="AutoEnable")`).
pub(crate) const ORG_FEATURE_STATUSES: [(&str, bool); 3] =
    [("NEW", true), ("NONE", false), ("ALL", true)];

/// The features a member account starts DISABLED, before any organization setting or
/// `UpdateMemberDetectors` moves them; every other reported feature starts
/// `ENABLED`. These are the rows that DISCRIMINATE: an auto-enablement that lands
/// changes what the member reports only for a feature whose default is off. Runtime
/// Monitoring is the one exception `CreateDetector` documents to "all the optional
/// features are enabled by default".
pub(crate) const OPT_IN_FEATURES: [&str; 1] = ["RUNTIME_MONITORING"];

/// Every `OrgFeatureAdditionalConfiguration` member
/// (`smithy_query(shape="OrgFeatureAdditionalConfiguration")`), each paired with a
/// feature to carry it. The pairs span the whole catalog, including `RDS_DATA_RISK`,
/// which the cited issue does not name.
pub(crate) const ADDITIONAL_CONFIGURATIONS: [(&str, &str); 4] = [
    ("RUNTIME_MONITORING", "EKS_ADDON_MANAGEMENT"),
    ("RUNTIME_MONITORING", "ECS_FARGATE_AGENT_MANAGEMENT"),
    ("EKS_RUNTIME_MONITORING", "EC2_AGENT_MANAGEMENT"),
    ("RDS_LOGIN_EVENTS", "RDS_DATA_RISK"),
];

/// `FeatureStatus` on the member side (`smithy_query(shape="FeatureStatus")`).
pub(crate) const ENABLED: &str = "ENABLED";
pub(crate) const DISABLED: &str = "DISABLED";

/// The `RelationshipStatus` a member the administrator has invited carries, read
/// from the service so the test cannot drift from it: the invitation is what
/// associates the account, and an associated member is the one whose detector this
/// suite reads.
const INVITED: &str =
    service_com_amazonaws_guardduty_guarddutyapiservice::state::RELATIONSHIP_INVITED;

/// Email forms the rows cycle, so a case varies along the account details as well as
/// along the configuration. Each is in a domain that can receive mail, so a member of
/// this suite is left [`INVITED`] by the send that invites it.
pub(crate) const EMAIL_FORMS: [&str; 3] = [
    "member@example-corp.com",
    "first.last@sub.example-corp.com",
    "m+tag@example.io",
];

/// A 12-digit account id of `case`'s own, clear of the administrator's account id.
pub(crate) fn account_id(case: usize) -> String {
    format!("8{case:011}")
}

/// What a member's feature reports before any organization setting reaches it.
pub(crate) fn default_status(feature: &str) -> &'static str {
    if OPT_IN_FEATURES.contains(&feature) {
        DISABLED
    } else {
        ENABLED
    }
}

/// An `UpdateOrganizationConfiguration` body naming each `(feature, autoEnable)`
/// row, with the auto-enablement of GuardDuty itself the operation requires beside
/// it: "you must provide a value for either `autoEnableOrganizationMembers` or
/// `autoEnable`" (`smithy_query(query="doc",shape="UpdateOrganizationConfiguration")`).
pub(crate) fn organization_body(rows: &[(&str, &str)]) -> Value {
    let features: Vec<Value> = rows
        .iter()
        .map(|(name, auto_enable)| json!({"name": name, "autoEnable": auto_enable}))
        .collect();
    json!({"autoEnableOrganizationMembers": "ALL", "features": features})
}

/// The same, for one feature carrying one additional configuration.
pub(crate) fn organization_body_with_additional(
    feature: &str,
    auto_enable: &str,
    additional: &str,
    additional_auto_enable: &str,
) -> Value {
    json!({
        "autoEnableOrganizationMembers": "ALL",
        "features": [{
            "name": feature,
            "autoEnable": auto_enable,
            "additionalConfiguration": [
                {"name": additional, "autoEnable": additional_auto_enable}
            ],
        }],
    })
}

/// `CreateMembers` for one account followed by `InviteMembers` for it, asserting
/// each call processed the account.
///
/// The invitation is what associates a recorded account with its administrator, and
/// an associated member is the one whose detector the administrator reads and
/// configures — so a suite that reads a member's configuration records the account
/// this way.
pub(crate) async fn record_member(
    router: &Router,
    detector_id: &str,
    account_id: &str,
    email: &str,
    case: &str,
) {
    let (status, body) = create_members(router, detector_id, &[(account_id, email)]).await;
    assert_eq!(
        status, 200,
        "case {case}: create-members expected 200, got {status} {body:?}"
    );
    assert!(
        body["unprocessedAccounts"]
            .as_array()
            .is_some_and(|unprocessed| unprocessed.is_empty()),
        "case {case}: expected the account processed, got {body:?}"
    );
    let (status, body) = invite_members(router, detector_id, &[account_id]).await;
    assert_eq!(
        status, 200,
        "case {case}: invite-members expected 200, got {status} {body:?}"
    );
    assert!(
        body["unprocessedAccounts"]
            .as_array()
            .is_some_and(|unprocessed| unprocessed.is_empty()),
        "case {case}: expected the account invited, got {body:?}"
    );
}

/// The configuration `GetMemberDetectors` reports for `account_id`.
pub(crate) async fn member_configuration(
    router: &Router,
    detector_id: &str,
    account_id: &str,
    case: &str,
) -> Value {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/member/detector/get"),
        Some(json!({"accountIds": [account_id]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: get-member-detectors expected 200, got {status} {body:?}"
    );
    member_of(&body, account_id)
        .unwrap_or_else(|| panic!("case {case}: expected a member configuration, got {body:?}"))
}

/// One reported `features` entry of a member configuration.
pub(crate) fn feature_entry<'a>(configuration: &'a Value, name: &str, case: &str) -> &'a Value {
    configuration["features"]
        .as_array()
        .and_then(|features| {
            features
                .iter()
                .find(|entry| entry["name"].as_str() == Some(name))
        })
        .unwrap_or_else(|| panic!("case {case}: {name} is not reported at all: {configuration:?}"))
}

/// The status a member's feature reports.
pub(crate) fn feature_status(configuration: &Value, name: &str, case: &str) -> String {
    let entry = feature_entry(configuration, name, case);
    entry["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a status for {name}, got {entry:?}"))
        .to_string()
}

/// The instant a member's feature reports (`UpdatedAt` is a `Timestamp`, so the wire
/// carries epoch seconds).
pub(crate) fn feature_instant(configuration: &Value, name: &str, case: &str) -> f64 {
    let entry = feature_entry(configuration, name, case);
    entry["updatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected an updatedAt for {name}, got {entry:?}"))
}

/// The status a member's feature reports for one of its additional configurations,
/// or `None` when the feature carries no such configuration at all.
pub(crate) fn additional_status(
    configuration: &Value,
    feature: &str,
    additional: &str,
    case: &str,
) -> Option<String> {
    feature_entry(configuration, feature, case)["additionalConfiguration"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["name"].as_str() == Some(additional))
        .map(|entry| entry["status"].as_str().unwrap_or_default().to_string())
}

/// Every feature's status in a member configuration, as `(name, status)` pairs in the
/// order reported.
pub(crate) fn every_feature_status(configuration: &Value, case: &str) -> Vec<(String, String)> {
    ORG_FEATURES
        .iter()
        .map(|name| {
            (
                (*name).to_string(),
                feature_status(configuration, name, case),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1: every `OrgFeature` against every `OrgFeatureStatus`, new and existing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_auto_enabled_feature_is_enabled_for_a_newly_recorded_member() {
    for (case_index, feature) in ORG_FEATURES.into_iter().enumerate() {
        for (status_index, (auto_enable, enables)) in ORG_FEATURE_STATUSES.into_iter().enumerate() {
            // One organization per case: the two Runtime Monitoring features cannot
            // be auto-enabled together, so a case must not inherit the previous
            // case's settings.
            let router = new_router();
            let detector_id = enabled_detector(&router).await;
            let case = format!("{feature} auto-enabled {auto_enable} for a new member");

            update_ok_in(
                &router,
                DEFAULT_REGION,
                &detector_id,
                organization_body(&[(feature, auto_enable)]),
            )
            .await;

            let account = account_id(case_index * ORG_FEATURE_STATUSES.len() + status_index);
            let email = EMAIL_FORMS[status_index % EMAIL_FORMS.len()];
            record_member(&router, &detector_id, &account, email, &case).await;

            let configuration = member_configuration(&router, &detector_id, &account, &case).await;
            let expected = if enables {
                ENABLED
            } else {
                default_status(feature)
            };
            assert_eq!(
                feature_status(&configuration, feature, &case),
                expected,
                "case {case}: expected {feature} {expected}, got {configuration:?}"
            );

            // The organization named ONE feature, so every other feature is where a
            // member starts: an auto-enablement reaches the switch it names and no
            // other.
            for other in ORG_FEATURES.into_iter().filter(|name| *name != feature) {
                assert_eq!(
                    feature_status(&configuration, other, &case),
                    default_status(other),
                    "case {case}: expected {other} left at its default, got {configuration:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_auto_enabled_feature_reaches_an_existing_member_too() {
    // The opt-in features are the discriminating ones: a member records them
    // DISABLED, so an auto-enablement that lands is visible.
    for (case_index, feature) in OPT_IN_FEATURES.into_iter().enumerate() {
        for (status_index, (auto_enable, enables)) in ORG_FEATURE_STATUSES.into_iter().enumerate() {
            let router = new_router();
            let detector_id = enabled_detector(&router).await;
            let case = format!("{feature} auto-enabled {auto_enable} for an existing member");

            // Recorded BEFORE the organization is configured, so the member exists
            // with the configuration a member starts in.
            let account = account_id(100 + case_index * ORG_FEATURE_STATUSES.len() + status_index);
            let email = EMAIL_FORMS[case_index % EMAIL_FORMS.len()];
            record_member(&router, &detector_id, &account, email, &case).await;
            let before = member_configuration(&router, &detector_id, &account, &case).await;
            assert_eq!(
                feature_status(&before, feature, &case),
                DISABLED,
                "case {case}: expected {feature} to start DISABLED, got {before:?}"
            );
            let (_, members_before) = get_members(&router, &detector_id, &[account.as_str()]).await;
            let relationship_before = member_of(&members_before, &account)
                .unwrap_or_else(|| panic!("case {case}: expected a member, got {members_before:?}"));

            update_ok_in(
                &router,
                DEFAULT_REGION,
                &detector_id,
                organization_body(&[(feature, auto_enable)]),
            )
            .await;

            // The same account again: "irrespective of the accounts being new or
            // existing members".
            record_member(&router, &detector_id, &account, email, &case).await;
            let after = member_configuration(&router, &detector_id, &account, &case).await;
            let expected = if enables { ENABLED } else { DISABLED };
            assert_eq!(
                feature_status(&after, feature, &case),
                expected,
                "case {case}: expected {feature} {expected} on the existing member, got {after:?}"
            );

            // Rule 4: the RELATIONSHIP is a different fact from the configuration, so
            // re-recording the same details leaves it exactly where it was.
            let (_, members_after) = get_members(&router, &detector_id, &[account.as_str()]).await;
            let relationship_after = member_of(&members_after, &account)
                .unwrap_or_else(|| panic!("case {case}: expected a member, got {members_after:?}"));
            assert_eq!(
                relationship_after, relationship_before,
                "case {case}: expected the relationship unchanged, got {relationship_after:?} after {relationship_before:?}"
            );
            assert_eq!(
                relationship_after["relationshipStatus"].as_str(),
                Some(INVITED),
                "case {case}: expected an invited member, got {relationship_after:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the additional configurations the organization auto-enables, and only
// those.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_auto_enabled_additional_configuration_lands_with_its_feature() {
    for (case_index, (feature, additional)) in ADDITIONAL_CONFIGURATIONS.into_iter().enumerate() {
        for (status_index, (auto_enable, enables)) in ORG_FEATURE_STATUSES.into_iter().enumerate() {
            let router = new_router();
            let detector_id = enabled_detector(&router).await;
            let case = format!("{feature}/{additional} auto-enabled {auto_enable}");

            // The feature itself is auto-enabled for every account, so what varies
            // along the rows is the additional configuration's own auto-enablement.
            update_ok_in(
                &router,
                DEFAULT_REGION,
                &detector_id,
                organization_body_with_additional(feature, "ALL", additional, auto_enable),
            )
            .await;

            let account =
                account_id(200 + case_index * ORG_FEATURE_STATUSES.len() + status_index);
            let email = EMAIL_FORMS[status_index % EMAIL_FORMS.len()];
            record_member(&router, &detector_id, &account, email, &case).await;

            let configuration = member_configuration(&router, &detector_id, &account, &case).await;
            assert_eq!(
                feature_status(&configuration, feature, &case),
                ENABLED,
                "case {case}: expected the auto-enabled feature ENABLED, got {configuration:?}"
            );
            let reported = additional_status(&configuration, feature, additional, &case);
            if enables {
                assert_eq!(
                    reported.as_deref(),
                    Some(ENABLED),
                    "case {case}: expected {additional} ENABLED, got {configuration:?}"
                );
            } else {
                assert!(
                    reported.is_none(),
                    "case {case}: expected {additional} not configured at all, got {reported:?}"
                );
            }

            // An additional configuration reaches the feature that carries it and no
            // other.
            for (other_feature, other) in ADDITIONAL_CONFIGURATIONS
                .into_iter()
                .filter(|(_, name)| *name != additional)
            {
                assert!(
                    additional_status(&configuration, other_feature, other, &case).is_none(),
                    "case {case}: expected {other_feature}/{other} not configured, got {configuration:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a member's configuration agrees with the organization's own read.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_member_configuration_agrees_with_the_organization_read() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    // A MIXTURE of statuses across the catalog, so the agreement is asserted over
    // features auto-enabled both ways and features auto-enabled for nobody. EKS
    // Runtime Monitoring stays at NONE: it and Runtime Monitoring cannot both be
    // auto-enabled.
    let rows: [(&str, &str); 8] = [
        ("S3_DATA_EVENTS", "NONE"),
        ("EKS_AUDIT_LOGS", "NEW"),
        ("EBS_MALWARE_PROTECTION", "ALL"),
        ("RDS_LOGIN_EVENTS", "ALL"),
        ("LAMBDA_NETWORK_LOGS", "NEW"),
        ("EKS_RUNTIME_MONITORING", "NONE"),
        ("RUNTIME_MONITORING", "NEW"),
        ("AI_PROTECTION", "ALL"),
    ];
    update_ok_in(
        &router,
        DEFAULT_REGION,
        &detector_id,
        organization_body(&rows),
    )
    .await;

    // What the organization reports is the source every assertion below reads, so
    // the two reads cannot disagree by construction of the test either.
    let organization = describe_in(&router, DEFAULT_REGION, &detector_id).await;
    let reported_auto_enable = |feature: &str| -> String {
        organization["features"]
            .as_array()
            .and_then(|features| {
                features
                    .iter()
                    .find(|entry| entry["name"].as_str() == Some(feature))
            })
            .and_then(|entry| entry["autoEnable"].as_str())
            .unwrap_or_else(|| panic!("expected {feature} reported, got {organization:?}"))
            .to_string()
    };

    // A batch, so the settings are asserted for every member of one call rather than
    // for a single account.
    let ids: Vec<String> = (0..EMAIL_FORMS.len()).map(|index| account_id(300 + index)).collect();
    let rows_to_create: Vec<(&str, &str)> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), EMAIL_FORMS[index]))
        .collect();
    let (status, body) = create_members(&router, &detector_id, &rows_to_create).await;
    assert_eq!(
        status, 200,
        "case a batch under an organization: expected 200, got {status} {body:?}"
    );
    // The invitation associates each account of the batch, which is what makes its
    // detector one the administrator reads.
    let invited: Vec<&str> = ids.iter().map(|id| id.as_str()).collect();
    let (status, body) = invite_members(&router, &detector_id, &invited).await;
    assert_eq!(
        status, 200,
        "case a batch under an organization: invite expected 200, got {status} {body:?}"
    );

    for (account, email) in &rows_to_create {
        let case = format!("{account} under a mixed organization");
        let configuration = member_configuration(&router, &detector_id, account, &case).await;
        for feature in ORG_FEATURES {
            let auto_enable = reported_auto_enable(feature);
            let expected = if auto_enable == "NONE" {
                default_status(feature)
            } else {
                ENABLED
            };
            assert_eq!(
                feature_status(&configuration, feature, &case),
                expected,
                "case {case}: {feature} is auto-enabled {auto_enable}, so expected {expected}, got {configuration:?}"
            );
        }

        // Rule 4: applying the settings again changes nothing — not a status, not an
        // instant.
        let before = every_feature_status(&configuration, &case);
        let instants: Vec<f64> = ORG_FEATURES
            .iter()
            .map(|feature| feature_instant(&configuration, feature, &case))
            .collect();
        record_member(&router, &detector_id, account, email, &case).await;
        let again = member_configuration(&router, &detector_id, account, &case).await;
        assert_eq!(
            every_feature_status(&again, &case),
            before,
            "case {case}: expected the same statuses on a repeated create, got {again:?}"
        );
        let instants_again: Vec<f64> = ORG_FEATURES
            .iter()
            .map(|feature| feature_instant(&again, feature, &case))
            .collect();
        assert_eq!(
            instants_again, instants,
            "case {case}: expected the per-feature instants not to move, got {instants_again:?} after {instants:?}"
        );
    }
}
