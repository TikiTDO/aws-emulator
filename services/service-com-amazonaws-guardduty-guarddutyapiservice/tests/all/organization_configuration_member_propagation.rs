// SPDX-License-Identifier: CC0-1.0

//! `UpdateOrganizationConfiguration` over the member accounts the administrator
//! ALREADY holds: what writing the organization's auto-enable settings does to the
//! stored configuration of the accounts recorded before the write, read back through
//! `GetMemberDetectors`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded call:
//! writing the organization's configuration applies every switch auto-enabled for
//! `ALL` of the organization's accounts — "all accounts in the organization have the
//! feature enabled automatically. This includes NEW accounts that join the
//! organization and accounts that may have been suspended or removed from the
//! organization in GuardDuty"
//! (`smithy_query(query="doc",shape="OrganizationFeatureConfiguration",member="AutoEnable")`)
//! — to the member accounts the administrator already holds, while a switch
//! auto-enabled `NEW` covers only the accounts recorded from then on, and no
//! auto-enablement ever switches a member's feature OFF: "when you update the
//! auto-enable setting from ALL or NEW to NONE, this action doesn't disable the
//! corresponding option for your existing accounts"
//! (`smithy_query(query="doc",shape="UpdateOrganizationConfigurationRequest",member="AutoEnableOrganizationMembers")`).
//!
//! So a member recorded BEFORE the organization was configured agrees with what
//! `DescribeOrganizationConfiguration` reports for every feature auto-enabled `ALL`,
//! with no second `CreateMembers` and no per-account write in between — the
//! agreement the sibling suites assert for the accounts `CreateMembers` records and
//! for a per-account write.
//!
//! The rows span the whole `OrgFeature` catalog against the whole `OrgFeatureStatus`
//! catalog and the whole `OrgFeatureAdditionalConfiguration` catalog, each read back
//! against the organization's own report.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::detector_lifecycle::{enabled_detector, new_router, Router};
use crate::member_association::{get_members, member_of, unprocessed_ids, DEFAULT_REGION};
use crate::member_detector_configuration::update_member_detectors;
// The organization-side table, bodies and projections live in the sibling suite that
// asserts the same agreement through `CreateMembers`, so the suites cannot drift on
// what an auto-enablement is or on what a member starts with.
use crate::member_organization_auto_enable::{
    account_id, additional_status, every_feature_status, feature_instant, feature_status,
    member_configuration, organization_body, organization_body_with_additional, record_member,
    ADDITIONAL_CONFIGURATIONS, DISABLED, EMAIL_FORMS, ENABLED, ORG_FEATURES, ORG_FEATURE_STATUSES,
};
use crate::organization_configuration::{describe_in, update_in, update_ok_in};

/// The one `OrgFeatureStatus` that covers the accounts already in the organization,
/// as opposed to the accounts that join later
/// (`smithy_query(query="doc",shape="OrganizationFeatureConfiguration",member="AutoEnable")`).
const COVERS_EXISTING_ACCOUNTS: &str = "ALL";

/// `EKS_RUNTIME_MONITORING`, which a member starts `ENABLED` and which "you can add
/// only one of" together with `RUNTIME_MONITORING`
/// (`smithy_query(query="doc",shape="UpdateMemberDetectors")`), so a row that will
/// have the organization auto-enable Runtime Monitoring clears it first.
const EKS_RUNTIME_MONITORING: &str = "EKS_RUNTIME_MONITORING";

/// Switch one feature OFF for one member account through `UpdateMemberDetectors`,
/// so an auto-enablement that later reaches the account is OBSERVABLE: most
/// features are enabled for a member from the start, and a switch that was already
/// on says nothing about which accounts the setting covered.
///
/// `EKS_RUNTIME_MONITORING` goes off with it, so no row leaves the member holding
/// both Runtime Monitoring features once an auto-enablement lands on the other one.
async fn switch_off(router: &Router, detector_id: &str, account: &str, feature: &str, case: &str) {
    let (status, response) = update_member_detectors(
        router,
        detector_id,
        &[account],
        json!({"features": [
            {"name": feature, "status": DISABLED},
            {"name": EKS_RUNTIME_MONITORING, "status": DISABLED},
        ]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: update-member-detectors expected 200, got {status} {response:?}"
    );
    assert!(
        unprocessed_ids(&response).is_empty(),
        "case {case}: expected the account processed, got {response:?}"
    );
}

/// An invited member account whose `feature` has been switched off, so the account is
/// one the administrator ALREADY holds when the organization is configured.
async fn held_member(
    router: &Router,
    detector_id: &str,
    index: usize,
    feature: &str,
    case: &str,
) -> String {
    let account = account_id(index);
    let email = EMAIL_FORMS[index % EMAIL_FORMS.len()];
    record_member(router, detector_id, &account, email, case).await;
    switch_off(router, detector_id, &account, feature, case).await;
    let configuration = member_configuration(router, detector_id, &account, case).await;
    assert_eq!(
        feature_status(&configuration, feature, case),
        DISABLED,
        "case {case}: expected {feature} switched off before the organization is \
         configured, got {configuration:?}"
    );
    account
}

/// One feature's `autoEnable` as `DescribeOrganizationConfiguration` reports it.
async fn reported_auto_enable(
    router: &Router,
    detector_id: &str,
    feature: &str,
    case: &str,
) -> String {
    let organization = describe_in(router, DEFAULT_REGION, detector_id).await;
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

/// Every feature's reported instant, in `ORG_FEATURES` order.
fn every_feature_instant(configuration: &Value, case: &str) -> Vec<f64> {
    ORG_FEATURES
        .iter()
        .map(|name| feature_instant(configuration, name, case))
        .collect()
}

// ---------------------------------------------------------------------------
// The feature switch: every `OrgFeature` against every `OrgFeatureStatus`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn configuring_the_organization_reaches_the_members_it_already_holds() {
    for (case_index, feature) in ORG_FEATURES.into_iter().enumerate() {
        for (status_index, (auto_enable, _)) in ORG_FEATURE_STATUSES.into_iter().enumerate() {
            // One organization per row: the two Runtime Monitoring features cannot
            // be auto-enabled together, so a row must not inherit the previous row's
            // settings.
            let router = new_router();
            let detector_id = enabled_detector(&router).await;
            let case = format!("{feature} auto-enabled {auto_enable} after the member was recorded");

            let index = case_index * ORG_FEATURE_STATUSES.len() + status_index;
            let account = held_member(&router, &detector_id, index, feature, &case).await;
            let before = member_configuration(&router, &detector_id, &account, &case).await;
            let statuses_before = every_feature_status(&before, &case);
            let instants_before = every_feature_instant(&before, &case);
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

            // The organization's own read is the source the expectation is taken
            // from, so the two reads cannot disagree by construction of the test
            // either.
            let reported = reported_auto_enable(&router, &detector_id, feature, &case).await;
            assert_eq!(
                reported, auto_enable,
                "case {case}: expected the organization to report {auto_enable}, got {reported}"
            );
            let covers_held_accounts = reported == COVERS_EXISTING_ACCOUNTS;
            let expected = if covers_held_accounts { ENABLED } else { DISABLED };

            let after = member_configuration(&router, &detector_id, &account, &case).await;
            let got = feature_status(&after, feature, &case);
            assert_eq!(
                got, expected,
                "case {case}: {feature} is auto-enabled {reported}, so expected {expected} on \
                 the member the administrator already holds, got {got} in {after:?}"
            );

            // The instant says when the switch last CHANGED: a setting that does not
            // cover the held accounts moves nothing at all.
            let statuses_after = every_feature_status(&after, &case);
            let instants_after = every_feature_instant(&after, &case);
            if covers_held_accounts {
                let moved = feature_instant(&after, feature, &case);
                let was = feature_instant(&before, feature, &case);
                assert!(
                    moved > was,
                    "case {case}: {feature} moved {DISABLED} -> {got}, so expected its instant \
                     to move past {was}, got {moved}"
                );
                // The write reaches the switch the request named and no other: every
                // other feature keeps the status and the instant it had.
                for (index, (name, status)) in statuses_before.iter().enumerate() {
                    if name == feature {
                        continue;
                    }
                    assert_eq!(
                        &statuses_after[index].1,
                        status,
                        "case {case}: expected {name} left at {status}, got {statuses_after:?}"
                    );
                    assert!(
                        (instants_after[index] - instants_before[index]).abs() < 0.001,
                        "case {case}: expected {name}'s instant to stay at {}, got {}",
                        instants_before[index],
                        instants_after[index]
                    );
                }
            } else {
                assert_eq!(
                    statuses_after, statuses_before,
                    "case {case}: {feature} is auto-enabled {reported}, which covers no account \
                     already held, so expected every status unchanged, got {statuses_after:?} \
                     after {statuses_before:?}"
                );
                assert_eq!(
                    instants_after, instants_before,
                    "case {case}: expected every instant unchanged, got {instants_after:?} after \
                     {instants_before:?}"
                );
            }

            // The RELATIONSHIP is a different fact from the configuration, so
            // configuring the organization leaves it exactly where it was.
            let (_, members_after) = get_members(&router, &detector_id, &[account.as_str()]).await;
            let relationship_after = member_of(&members_after, &account)
                .unwrap_or_else(|| panic!("case {case}: expected a member, got {members_after:?}"));
            assert_eq!(
                relationship_after, relationship_before,
                "case {case}: expected the relationship unchanged, got {relationship_after:?} \
                 after {relationship_before:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The additional-configuration switch, which carries an auto-enablement of its own.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_additional_configuration_reaches_a_held_member_only_when_it_covers_it() {
    for (case_index, (feature, additional)) in ADDITIONAL_CONFIGURATIONS.into_iter().enumerate() {
        for (status_index, (auto_enable, _)) in ORG_FEATURE_STATUSES.into_iter().enumerate() {
            let router = new_router();
            let detector_id = enabled_detector(&router).await;
            let case = format!(
                "{feature}/{additional} auto-enabled {auto_enable} after the member was recorded"
            );

            let index = 100 + case_index * ORG_FEATURE_STATUSES.len() + status_index;
            let account = held_member(&router, &detector_id, index, feature, &case).await;
            assert!(
                additional_status(
                    &member_configuration(&router, &detector_id, &account, &case).await,
                    feature,
                    additional,
                    &case,
                )
                .is_none(),
                "case {case}: expected {additional} not configured before the organization is"
            );

            // The feature itself is auto-enabled for every account, so what varies
            // along the rows is the additional configuration's own auto-enablement.
            update_ok_in(
                &router,
                DEFAULT_REGION,
                &detector_id,
                organization_body_with_additional(
                    feature,
                    COVERS_EXISTING_ACCOUNTS,
                    additional,
                    auto_enable,
                ),
            )
            .await;

            let after = member_configuration(&router, &detector_id, &account, &case).await;
            let feature_got = feature_status(&after, feature, &case);
            assert_eq!(
                feature_got, ENABLED,
                "case {case}: {feature} is auto-enabled {COVERS_EXISTING_ACCOUNTS}, so expected \
                 {ENABLED} on the member already held, got {feature_got} in {after:?}"
            );
            let got = additional_status(&after, feature, additional, &case);
            if auto_enable == COVERS_EXISTING_ACCOUNTS {
                assert_eq!(
                    got.as_deref(),
                    Some(ENABLED),
                    "case {case}: expected {additional} {ENABLED} on the member already held, \
                     got {got:?} in {after:?}"
                );
            } else {
                assert!(
                    got.is_none(),
                    "case {case}: {additional} is auto-enabled {auto_enable}, which covers no \
                     account already held, so expected it not configured, got {got:?}"
                );
            }

            // An auto-enablement reaches the feature that carries it and no other.
            for (other_feature, other) in ADDITIONAL_CONFIGURATIONS
                .into_iter()
                .filter(|(_, name)| *name != additional)
            {
                assert!(
                    additional_status(&after, other_feature, other, &case).is_none(),
                    "case {case}: expected {other_feature}/{other} not configured, got {after:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Nothing an organization write does switches a member's feature OFF.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lowering_the_auto_enablement_does_not_switch_a_held_member_off() {
    for (case_index, feature) in ORG_FEATURES.into_iter().enumerate() {
        for (status_index, lowered) in ["NEW", "NONE"].into_iter().enumerate() {
            let router = new_router();
            let detector_id = enabled_detector(&router).await;
            let case =
                format!("{feature} auto-enabled {COVERS_EXISTING_ACCOUNTS} then {lowered}");

            let index = 200 + case_index * 2 + status_index;
            let account = held_member(&router, &detector_id, index, feature, &case).await;

            update_ok_in(
                &router,
                DEFAULT_REGION,
                &detector_id,
                organization_body(&[(feature, COVERS_EXISTING_ACCOUNTS)]),
            )
            .await;
            let enabled = member_configuration(&router, &detector_id, &account, &case).await;
            assert_eq!(
                feature_status(&enabled, feature, &case),
                ENABLED,
                "case {case}: expected {feature} {ENABLED} on the member already held, got \
                 {enabled:?}"
            );
            let statuses = every_feature_status(&enabled, &case);
            let instants = every_feature_instant(&enabled, &case);

            update_ok_in(
                &router,
                DEFAULT_REGION,
                &detector_id,
                organization_body(&[(feature, lowered)]),
            )
            .await;

            let after = member_configuration(&router, &detector_id, &account, &case).await;
            assert_eq!(
                every_feature_status(&after, &case),
                statuses,
                "case {case}: lowering the auto-enablement to {lowered} must not disable the \
                 option for an account already held, got {after:?}"
            );
            assert_eq!(
                every_feature_instant(&after, &case),
                instants,
                "case {case}: expected every instant unchanged, got {after:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What the write stores: one record per catalog member, so the walk over the
// accounts already held is bounded by the model and not by the request.
// ---------------------------------------------------------------------------

/// Every `OrgFeatureAdditionalConfiguration` member
/// (`smithy_query(shape="OrgFeatureAdditionalConfiguration")`), which is the catalog
/// `OrganizationAdditionalConfiguration.Name` targets — read off the sibling suite's
/// table, where each member appears exactly once.
fn additional_configuration_catalog() -> Vec<&'static str> {
    ADDITIONAL_CONFIGURATIONS
        .into_iter()
        .map(|(_, additional)| additional)
        .collect()
}

/// One feature's reported `autoEnable`, as `DescribeOrganizationConfiguration` states
/// it, paired with its reported additional configurations keyed by name.
fn reported_feature(organization: &Value, feature: &str, case: &str) -> (String, Value) {
    let entry = organization["features"]
        .as_array()
        .and_then(|features| {
            features
                .iter()
                .find(|entry| entry["name"].as_str() == Some(feature))
        })
        .unwrap_or_else(|| panic!("case {case}: expected {feature} reported, got {organization:?}"));
    (
        entry["autoEnable"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected an autoEnable, got {entry:?}"))
            .to_string(),
        entry["additionalConfiguration"].clone(),
    )
}

#[tokio::test]
async fn the_stored_configuration_holds_one_record_per_catalog_member() {
    let case = "a request repeating every catalog member three times";
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let account = held_member(&router, &detector_id, 400, "RUNTIME_MONITORING", case).await;
    let before = member_configuration(&router, &detector_id, &account, case).await;

    // The auto-enablement each feature is to END UP with. Runtime Monitoring is the
    // one that covers the accounts already held and EKS Runtime Monitoring is left
    // off: "you can add only one of these two features".
    let features: Vec<(&str, &str)> = ORG_FEATURES
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            let auto_enable = match name {
                "EKS_RUNTIME_MONITORING" => "NONE",
                "RUNTIME_MONITORING" => COVERS_EXISTING_ACCOUNTS,
                _ => ["NEW", "NONE", COVERS_EXISTING_ACCOUNTS][index % 3],
            };
            (name, auto_enable)
        })
        .collect();
    // The same for each additional configuration of the feature that carries them.
    let additional: Vec<(&str, &str)> = additional_configuration_catalog()
        .into_iter()
        .enumerate()
        .map(|(index, name)| (name, ["NEW", "NONE", COVERS_EXISTING_ACCOUNTS][index % 3]))
        .collect();

    // Each catalog member is named three times over, so the request is far longer
    // than either catalog and every member but the last occurrence is a repeat. The
    // LAST entry naming a member is the one that lands, as it is for a feature's own
    // switch.
    let rounds = 3;
    let mut requested: Vec<Value> = Vec::new();
    for round in 0..rounds {
        let last_round = round + 1 == rounds;
        for (name, auto_enable) in &features {
            let stated = if last_round { *auto_enable } else { "NEW" };
            let mut entry = json!({"name": name, "autoEnable": stated});
            if *name == "RUNTIME_MONITORING" {
                entry["additionalConfiguration"] = Value::Array(
                    additional
                        .iter()
                        .map(|(additional_name, additional_auto_enable)| {
                            let stated = if last_round {
                                *additional_auto_enable
                            } else {
                                "NEW"
                            };
                            json!({"name": additional_name, "autoEnable": stated})
                        })
                        .collect(),
                );
            }
            requested.push(entry);
        }
    }
    assert!(
        requested.len() > ORG_FEATURES.len(),
        "case {case}: the request must carry more entries than the catalog has members"
    );

    let (status, got) = update_in(
        &router,
        DEFAULT_REGION,
        &detector_id,
        json!({"autoEnableOrganizationMembers": "ALL", "features": requested}),
    )
    .await;
    assert_eq!(status, 200, "case {case}: expected 200, got {status} {got:?}");

    // The stored configuration carries each `OrgFeature` once, at the auto-enablement
    // the LAST entry naming it asked for.
    let organization = describe_in(&router, DEFAULT_REGION, &detector_id).await;
    let reported = organization["features"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a features list, got {organization:?}"));
    assert_eq!(
        reported.len(),
        ORG_FEATURES.len(),
        "case {case}: expected one record per OrgFeature member, got {organization:?}"
    );
    for (feature, auto_enable) in &features {
        let (got_auto_enable, got_additional) = reported_feature(&organization, feature, case);
        assert_eq!(
            &got_auto_enable, auto_enable,
            "case {case}: expected {feature} stored at {auto_enable}, got {got_auto_enable}"
        );
        if *feature != "RUNTIME_MONITORING" {
            continue;
        }
        // And each `OrgFeatureAdditionalConfiguration` once, at its own last-named
        // auto-enablement — whatever order and however often the request named them.
        let stored: BTreeMap<String, String> = got_additional
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}: expected an additionalConfiguration list, got {got_additional:?}")
            })
            .iter()
            .map(|entry| {
                (
                    entry["name"].as_str().unwrap_or_default().to_string(),
                    entry["autoEnable"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let expected: BTreeMap<String, String> = additional
            .iter()
            .map(|(name, auto_enable)| ((*name).to_string(), (*auto_enable).to_string()))
            .collect();
        assert_eq!(
            stored, expected,
            "case {case}: expected one record per additional-configuration member at its \
             last-named value, got {stored:?}"
        );
    }

    // And the member the administrator already holds took exactly the switches that
    // cover it, from that one stored record per member.
    let after = member_configuration(&router, &detector_id, &account, case).await;
    for (feature, auto_enable) in &features {
        let got = feature_status(&after, feature, case);
        let expected = if *auto_enable == COVERS_EXISTING_ACCOUNTS {
            ENABLED.to_string()
        } else {
            feature_status(&before, feature, case)
        };
        assert_eq!(
            got, expected,
            "case {case}: {feature} is auto-enabled {auto_enable}, so expected {expected} on \
             the member already held, got {got} in {after:?}"
        );
    }
    for (name, auto_enable) in &additional {
        let got = additional_status(&after, "RUNTIME_MONITORING", name, case);
        if *auto_enable == COVERS_EXISTING_ACCOUNTS {
            assert_eq!(
                got.as_deref(),
                Some(ENABLED),
                "case {case}: expected {name} {ENABLED} on the member already held, got {got:?}"
            );
        } else {
            assert!(
                got.is_none(),
                "case {case}: {name} is auto-enabled {auto_enable}, which covers no account \
                 already held, so expected it not configured, got {got:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Applying the settings is idempotent, and reading a member changes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn applying_the_same_settings_again_moves_nothing() {
    for (status_index, (auto_enable, _)) in ORG_FEATURE_STATUSES.into_iter().enumerate() {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let feature = "RUNTIME_MONITORING";
        let case = format!("{feature} auto-enabled {auto_enable} twice");

        let account = held_member(&router, &detector_id, 300 + status_index, feature, &case).await;
        update_ok_in(
            &router,
            DEFAULT_REGION,
            &detector_id,
            organization_body(&[(feature, auto_enable)]),
        )
        .await;
        let once = member_configuration(&router, &detector_id, &account, &case).await;
        // A read reports what the store holds and changes nothing a later caller
        // sees, so the same read twice reports the same configuration.
        let again = member_configuration(&router, &detector_id, &account, &case).await;
        assert_eq!(
            again, once,
            "case {case}: expected a second read to report the same configuration, got {again:?} \
             after {once:?}"
        );

        update_ok_in(
            &router,
            DEFAULT_REGION,
            &detector_id,
            organization_body(&[(feature, auto_enable)]),
        )
        .await;
        let twice = member_configuration(&router, &detector_id, &account, &case).await;
        assert_eq!(
            twice, once,
            "case {case}: expected the same settings applied again to move nothing, got \
             {twice:?} after {once:?}"
        );
    }
}
