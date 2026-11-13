// SPDX-License-Identifier: CC0-1.0

//! `GetOrganizationStatistics`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handler are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of organizations rather than one call:
//!
//! 1. Every count is a count of STORED records: `totalAccountsCount` is the
//!    administrator's account plus one per member record, `memberAccountsCount` the
//!    members still associated with GuardDuty, `activeAccountsCount` those of them whose
//!    monitoring has not been stopped, and `enabledAccountsCount` the accounts that have
//!    GuardDuty enabled — those monitored members plus the administrator's own account
//!    while its detector's `Status` is `ENABLED`.
//! 2. `countByFeature` carries one entry per `OrgFeature` member, in the catalog order
//!    the service reports features in, and each entry counts the ACTIVE member accounts
//!    holding that feature's switch at `ENABLED` — the same switches
//!    `GetMemberDetectors` reports, which is what every expectation here is derived
//!    from. A nested `additionalConfiguration` counts the accounts that hold both that
//!    configuration's switch and its feature's at `ENABLED`.
//! 3. The operation names no detector, so the organization it reports is the one the
//!    Region the request was signed for serves; an endpoint holding no detector reports
//!    an organization of nothing rather than failing.
//! 4. `updatedAt` is the latest instant a record these counts read last changed: it
//!    moves on every write that can move the answer, and never on a read.
//! 5. It REPORTS: repeated calls agree, and nothing a later caller sees changes.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::detector_lifecycle::{call_in, new_router, router_with_engine, Router};
use crate::invitation_listing::enabled_detector_in;
use crate::member_relationship_scope::monitor_member_in;

/// The Region most rows use.
const REGION: &str = "us-east-1";

/// Every `OrgFeature` member, in the order this service reports features in: the
/// entries `countByFeature` must carry. Read from the crate's own model-derived catalog
/// rather than written out here, so a feature the model gains cannot be missed.
const ORG_FEATURES: &[&str] =
    service_com_amazonaws_guardduty_guarddutyapiservice::state::ORG_FEATURES;

/// The status a switch that is on carries.
const ENABLED: &str = service_com_amazonaws_guardduty_guarddutyapiservice::state::ENABLED;

/// Member accounts to record: 12 decimal digits each, the form `AccountId` takes
/// (`len 12..12` — `smithy_query(shape="AccountId")`). A row uses a prefix of this list,
/// so the suites differ along the SIZE of the membership as well as its shape.
const ORG_MEMBERS: [(&str, &str); 5] = [
    ("111122223333", "member-one@example.com"),
    ("222233334444", "first.last@sub.example-corp.com"),
    ("333344445555", "m+tag@example.io"),
    ("444455556666", "member-four@example.com"),
    ("555566667777", "member.five@example.net"),
];

// ---------------------------------------------------------------------------
// Rule 1: the counts are the membership the store holds.
// ---------------------------------------------------------------------------

/// One row of the membership table: how many accounts were recorded, which of them
/// were left merely RECORDED (never invited, so never associated) or disassociated,
/// and whether the administrator's own detector is left enabled. The index sets are
/// disjoint, so each member ends in exactly one state.
struct Membership {
    case: &'static str,
    members: usize,
    recorded_only: &'static [usize],
    disassociated: &'static [usize],
    administrator_enabled: bool,
}

#[tokio::test]
async fn the_counts_are_the_membership_the_store_holds() {
    // The rows differ along the COUNT of members and along the STATE each member is
    // left in — recorded, invited (which is what associates it), no longer associated
    // — plus whether the administrator's own GuardDuty is on.
    let rows = [
        Membership {
            case: "an organization with no member account at all",
            members: 0,
            recorded_only: &[],
            disassociated: &[],
            administrator_enabled: true,
        },
        Membership {
            case: "one invited member",
            members: 1,
            recorded_only: &[],
            disassociated: &[],
            administrator_enabled: true,
        },
        Membership {
            case: "three invited members",
            members: 3,
            recorded_only: &[],
            disassociated: &[],
            administrator_enabled: true,
        },
        Membership {
            case: "two of three members only recorded",
            members: 3,
            recorded_only: &[0, 1],
            disassociated: &[],
            administrator_enabled: true,
        },
        Membership {
            case: "one member only recorded",
            members: 3,
            recorded_only: &[2],
            disassociated: &[],
            administrator_enabled: true,
        },
        Membership {
            case: "one member disassociated",
            members: 3,
            recorded_only: &[],
            disassociated: &[1],
            administrator_enabled: true,
        },
        Membership {
            case: "a membership in every state at once",
            members: 5,
            recorded_only: &[0, 4],
            disassociated: &[3],
            administrator_enabled: true,
        },
        Membership {
            case: "the administrator's own detector disabled",
            members: 2,
            recorded_only: &[],
            disassociated: &[],
            administrator_enabled: false,
        },
        Membership {
            case: "a disabled administrator over an unassociated member",
            members: 2,
            recorded_only: &[1],
            disassociated: &[],
            administrator_enabled: false,
        },
    ];

    for row in rows {
        let router = new_router();
        let detector_id = enabled_detector_in(&router, REGION).await;
        let recorded = &ORG_MEMBERS[..row.members];
        if !recorded.is_empty() {
            create_members(&router, REGION, &detector_id, recorded, row.case).await;
        }
        // Every member the row does not leave merely recorded is INVITED, which is
        // what associates the account with the administrator; a disassociated one is
        // invited first, because only an association can be ended.
        let associated_indexes: Vec<usize> = (0..row.members)
            .filter(|index| !row.recorded_only.contains(index))
            .collect();
        for (action, indexes) in [
            ("invite", associated_indexes.as_slice()),
            ("disassociate", row.disassociated),
        ] {
            if indexes.is_empty() {
                continue;
            }
            let accounts = account_ids(indexes);
            member_action(&router, REGION, &detector_id, action, &accounts, row.case).await;
        }
        if !row.administrator_enabled {
            disable_detector(&router, REGION, &detector_id, row.case).await;
        }

        // The administrator's account is one of the organization's accounts; a member
        // record is retained across a disassociation, so it still counts in the total.
        let total = row.members as i64 + 1;
        let associated =
            (row.members - row.recorded_only.len() - row.disassociated.len()) as i64;
        let active = associated;
        let enabled = active + i64::from(row.administrator_enabled);

        let statistics = organization_statistics(&router, REGION, row.case).await;
        assert_eq!(
            counts(&statistics, row.case),
            [total, associated, active, enabled],
            "case {:?}: expected [total, member, active, enabled] = {:?}, got {statistics:?}",
            row.case,
            [total, associated, active, enabled]
        );

        // No feature can be enabled on more accounts than there are active accounts to
        // enable it on.
        for entry in feature_entries(&statistics, row.case) {
            let name = entry["name"].as_str().unwrap_or_default().to_string();
            let counted = entry["enabledAccountsCount"].as_i64().unwrap_or_else(|| {
                panic!("case {:?}: expected a count for {name}, got {entry:?}", row.case)
            });
            assert!(
                counted <= active,
                "case {:?}: {name} is enabled on {counted} accounts, more than the {active} active ones",
                row.case
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: countByFeature counts the active members' own feature switches.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn count_by_feature_counts_the_active_members_holding_each_feature_enabled() {
    let (router, engine) = router_with_engine();
    let detector_id = enabled_detector_in(&router, REGION).await;
    let recorded = &ORG_MEMBERS[..3];
    create_members(&router, REGION, &detector_id, recorded, "three recorded members").await;
    let all: Vec<&str> = recorded.iter().map(|(id, _)| *id).collect();
    // The invitation associates each account, which is what makes it one of the
    // organization's counted members; the account's own answer to it is what makes
    // GuardDuty maintain its detector, which is the configuration this suite reads
    // the counts against (the `member_relationship_scope` seam).
    member_action(
        &router,
        REGION,
        &detector_id,
        "invite",
        &all,
        "three recorded members",
    )
    .await;
    for account_id in &all {
        monitor_member_in(&engine, REGION, &detector_id, account_id);
    }

    // The rows differ along the SWITCH they move and along how many members they move it
    // on: a feature that defaults enabled, one that defaults disabled, a feature the
    // report's own catalog carries that no issue cited, and an additional configuration
    // both under an enabled feature and under a disabled one.
    let rows: [(&str, &[&str], Value); 6] = [
        (
            "the members as CreateMembers recorded them",
            &[],
            Value::Null,
        ),
        (
            "a feature that defaults ENABLED switched off on one member",
            &all[..1],
            json!({"features": [{"name": "S3_DATA_EVENTS", "status": "DISABLED"}]}),
        ),
        (
            "a feature that defaults DISABLED switched on for two members",
            &all[..2],
            // Runtime Monitoring already covers Amazon EKS, so enabling it switches
            // EKS Runtime Monitoring off in the same call: the two cannot both end up
            // enabled on one account.
            json!({"features": [{"name": "RUNTIME_MONITORING", "status": "ENABLED"},
                                {"name": "EKS_RUNTIME_MONITORING", "status": "DISABLED"}]}),
        ),
        (
            "a feature switched off on every member",
            &all[..],
            json!({"features": [{"name": "LAMBDA_NETWORK_LOGS", "status": "DISABLED"}]}),
        ),
        (
            "an additional configuration under an enabled feature",
            &all[..2],
            json!({"features": [{
                "name": "RUNTIME_MONITORING",
                "status": "ENABLED",
                "additionalConfiguration": [{"name": "EKS_ADDON_MANAGEMENT", "status": "ENABLED"}]
            }]}),
        ),
        (
            "the same configuration under a feature left DISABLED",
            &all[2..],
            json!({"features": [{
                "name": "RUNTIME_MONITORING",
                "status": "DISABLED",
                "additionalConfiguration": [{"name": "EKS_ADDON_MANAGEMENT", "status": "ENABLED"}]
            }]}),
        ),
    ];

    for (case, accounts, configuration) in rows {
        if !accounts.is_empty() {
            update_member_detectors(
                &router,
                REGION,
                &detector_id,
                accounts,
                configuration,
                case,
            )
            .await;
        }
        assert_count_by_feature(&router, REGION, &detector_id, &all, case).await;
    }

    // An account that is no longer ASSOCIATED leaves every count: disassociating one
    // member takes it out, and so does disassociating another.
    member_action(
        &router,
        REGION,
        &detector_id,
        "disassociate",
        &all[..1],
        "disassociated",
    )
    .await;
    assert_count_by_feature(
        &router,
        REGION,
        &detector_id,
        &all[1..],
        "after one member was disassociated",
    )
    .await;
    member_action(
        &router,
        REGION,
        &detector_id,
        "disassociate",
        &all[1..2],
        "disassociated again",
    )
    .await;
    assert_count_by_feature(
        &router,
        REGION,
        &detector_id,
        &all[2..],
        "after a second member was disassociated",
    )
    .await;
}

#[tokio::test]
async fn a_feature_every_member_holds_enabled_is_counted_on_every_member() {
    // The counts are not all-zero placeholders: a feature `CreateMembers` leaves enabled
    // is counted on every active member, for each size of membership.
    for size in 1..=ORG_MEMBERS.len() {
        let case = format!("{size} recorded member(s)");
        let (router, engine) = router_with_engine();
        let detector_id = enabled_detector_in(&router, REGION).await;
        let recorded = &ORG_MEMBERS[..size];
        create_members(&router, REGION, &detector_id, recorded, &case).await;
        let accounts: Vec<&str> = recorded.iter().map(|(id, _)| *id).collect();
        member_action(&router, REGION, &detector_id, "invite", &accounts, &case).await;
        for account_id in &accounts {
            monitor_member_in(&engine, REGION, &detector_id, account_id);
        }

        let statistics = organization_statistics(&router, REGION, &case).await;
        // Read the feature to expect from the members themselves rather than naming one:
        // whichever features a recorded member holds enabled must be counted on all of
        // them.
        let switches = member_switches(&router, REGION, &detector_id, &accounts, &case).await;
        let enabled_everywhere: Vec<String> = ORG_FEATURES
            .iter()
            .copied()
            .filter(|feature| {
                switches
                    .values()
                    .all(|features| features.get(*feature).is_some_and(|(s, _)| s == ENABLED))
            })
            .map(str::to_string)
            .collect();
        assert!(
            !enabled_everywhere.is_empty(),
            "case {case:?}: expected a recorded member to hold at least one feature enabled, got {switches:?}"
        );
        for feature in &enabled_everywhere {
            let counted = reported_feature_count(&statistics, feature, &case);
            assert_eq!(
                counted, size as i64,
                "case {case:?}: {feature} is enabled on every one of the {size} members, got {counted}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the organization reported is the one the request's Region serves.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_organization_reported_is_the_one_the_requests_region_serves() {
    let router = new_router();
    // One organization per Region, each with a membership of its own.
    let rows: [(&str, usize); 3] = [("us-east-1", 3), ("eu-west-1", 1), ("ap-southeast-2", 0)];
    let mut detectors: BTreeMap<&str, String> = BTreeMap::new();
    for (region, size) in rows {
        let detector_id = enabled_detector_in(&router, region).await;
        if size > 0 {
            create_members(&router, region, &detector_id, &ORG_MEMBERS[..size], region).await;
            let accounts: Vec<&str> = ORG_MEMBERS[..size].iter().map(|(id, _)| *id).collect();
            member_action(&router, region, &detector_id, "invite", &accounts, region).await;
        }
        detectors.insert(region, detector_id);
    }

    for (region, size) in rows {
        let case = format!("the {region} organization");
        let statistics = organization_statistics(&router, region, &case).await;
        let size = size as i64;
        assert_eq!(
            counts(&statistics, &case),
            [size + 1, size, size, size + 1],
            "case {case:?}: expected only this Region's membership, got {statistics:?}"
        );
    }

    // An endpoint holding no detector reports an organization of nothing — every count
    // zero, one entry per feature at zero, and no instant at all — rather than failing:
    // the operation names no detector, so there is nothing for it to refuse.
    let case = "a Region holding no detector";
    let details = organization_details(&router, "us-west-2", case).await;
    assert!(
        details.get("updatedAt").is_none_or(Value::is_null),
        "case {case:?}: expected no instant for an organization with no record, got {details:?}"
    );
    let statistics = details["organizationStatistics"].clone();
    assert_eq!(
        counts(&statistics, case),
        [0, 0, 0, 0],
        "case {case:?}: expected every count at zero, got {statistics:?}"
    );
    for entry in feature_entries(&statistics, case) {
        assert_eq!(
            entry["enabledAccountsCount"].as_i64(),
            Some(0),
            "case {case:?}: expected a zero count, got {entry:?}"
        );
        assert!(
            entry.get("additionalConfiguration").is_none_or(Value::is_null),
            "case {case:?}: expected no additional configuration, got {entry:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4 and 5: the instant moves with the records, and the report is a read.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn updated_at_is_the_latest_instant_a_counted_record_changed() {
    let router = new_router();

    let case = "before any detector exists";
    let details = organization_details(&router, REGION, case).await;
    assert!(
        details.get("updatedAt").is_none_or(Value::is_null),
        "case {case:?}: nothing has been recorded, so nothing can have been updated: got {details:?}"
    );

    let detector_id = enabled_detector_in(&router, REGION).await;
    let mut previous = instant(&router, REGION, "after the detector was created").await;
    let accounts: Vec<&str> = ORG_MEMBERS[..2].iter().map(|(id, _)| *id).collect();

    // Every write that can move the answer moves the instant; a read never does.
    create_members(&router, REGION, &detector_id, &ORG_MEMBERS[..2], "create").await;
    previous = moved_on(&router, "CreateMembers", previous).await;
    let read_again = instant(&router, REGION, "a repeated read").await;
    assert_eq!(
        read_again, previous,
        "case \"a repeated read\": reading must not move the instant, got {read_again} after {previous}"
    );

    member_action(&router, REGION, &detector_id, "invite", &accounts, "invite").await;
    previous = moved_on(&router, "InviteMembers", previous).await;

    update_member_detectors(
        &router,
        REGION,
        &detector_id,
        &accounts[..1],
        json!({"features": [{"name": "S3_DATA_EVENTS", "status": "DISABLED"}]}),
        "configure",
    )
    .await;
    previous = moved_on(&router, "UpdateMemberDetectors", previous).await;

    member_action(
        &router,
        REGION,
        &detector_id,
        "disassociate",
        &accounts[1..],
        "disassociate",
    )
    .await;
    moved_on(&router, "DisassociateMembers", previous).await;
}

#[tokio::test]
async fn the_report_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_engine();
    let detector_id = enabled_detector_in(&router, REGION).await;
    let recorded = &ORG_MEMBERS[..3];
    create_members(&router, REGION, &detector_id, recorded, "three members").await;
    let accounts: Vec<&str> = recorded.iter().map(|(id, _)| *id).collect();
    member_action(
        &router,
        REGION,
        &detector_id,
        "invite",
        &accounts,
        "three members",
    )
    .await;
    for account_id in &accounts {
        monitor_member_in(&engine, REGION, &detector_id, account_id);
    }

    let before = organization_details(&router, REGION, "the first report").await;
    let members_before = list_members(&router, REGION, &detector_id, "before").await;
    let switches_before =
        member_switches(&router, REGION, &detector_id, &accounts, "before").await;

    for round in 0..3 {
        let case = format!("report round {round}");
        let again = organization_details(&router, REGION, &case).await;
        assert_eq!(
            again, before,
            "case {case:?}: repeated reports must agree, got {again:?} after {before:?}"
        );
    }

    let members_after = list_members(&router, REGION, &detector_id, "after").await;
    assert_eq!(
        members_after, members_before,
        "the report must leave the membership exactly as it was"
    );
    let switches_after = member_switches(&router, REGION, &detector_id, &accounts, "after").await;
    assert_eq!(
        switches_after, switches_before,
        "the report must leave every member's configuration exactly as it was"
    );
}

// ---------------------------------------------------------------------------
// The call and the helpers.
// ---------------------------------------------------------------------------

/// One `GetOrganizationStatistics` call — `GET /organization/statistics`
/// (`smithy_query(operation="GetOrganizationStatistics")`) — returning the
/// `organizationDetails` it reported and failing the test if it did not succeed or
/// reported no details at all.
async fn organization_details(router: &Router, region: &str, case: &str) -> Value {
    let (status, body) = call_in(router, region, "GET", "/organization/statistics", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-organization-statistics expected 200, got {status} {body:?}"
    );
    body.get("organizationDetails").cloned().unwrap_or_else(|| {
        panic!("case {case:?}: expected an organizationDetails document, got {body:?}")
    })
}

/// The `organizationStatistics` the report carries.
async fn organization_statistics(router: &Router, region: &str, case: &str) -> Value {
    let details = organization_details(router, region, case).await;
    details
        .get("organizationStatistics")
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected organizationStatistics, got {details:?}")
        })
}

/// The instant the report carries, which every row compares against the previous one.
async fn instant(router: &Router, region: &str, case: &str) -> f64 {
    let details = organization_details(router, region, case).await;
    details["updatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt instant, got {details:?}"))
}

/// Assert the instant moved past `previous` after the write named by `case`, and return
/// the new one.
async fn moved_on(router: &Router, case: &str, previous: f64) -> f64 {
    let now = instant(router, REGION, case).await;
    assert!(
        now > previous,
        "case {case:?}: a write that moves the counts must move the instant, got {now} after {previous}"
    );
    now
}

/// The four counts, in one array so a table row compares them all at once.
fn counts(statistics: &Value, case: &str) -> [i64; 4] {
    [
        counted(statistics, "totalAccountsCount", case),
        counted(statistics, "memberAccountsCount", case),
        counted(statistics, "activeAccountsCount", case),
        counted(statistics, "enabledAccountsCount", case),
    ]
}

/// One count the statistics report, failing the test when it is absent or not a number.
fn counted(statistics: &Value, member: &str, case: &str) -> i64 {
    statistics[member].as_i64().unwrap_or_else(|| {
        panic!("case {case:?}: expected an integer {member}, got {statistics:?}")
    })
}

/// The `countByFeature` entries the statistics report.
fn feature_entries(statistics: &Value, case: &str) -> Vec<Value> {
    statistics["countByFeature"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a countByFeature list, got {statistics:?}"))
}

/// The count `countByFeature` reports for one feature.
fn reported_feature_count(statistics: &Value, feature: &str, case: &str) -> i64 {
    feature_entries(statistics, case)
        .into_iter()
        .find(|entry| entry["name"].as_str() == Some(feature))
        .and_then(|entry| entry["enabledAccountsCount"].as_i64())
        .unwrap_or_else(|| panic!("case {case:?}: expected a count for {feature}, got {statistics:?}"))
}

/// Assert the whole `countByFeature` list against what `GetMemberDetectors` reports for
/// the accounts that are ACTIVE: the catalog and its order, every per-feature count, and
/// every nested additional-configuration count.
async fn assert_count_by_feature(
    router: &Router,
    region: &str,
    detector_id: &str,
    active: &[&str],
    case: &str,
) {
    let switches = member_switches(router, region, detector_id, active, case).await;
    let statistics = organization_statistics(router, region, case).await;

    assert_eq!(
        counted(&statistics, "activeAccountsCount", case),
        active.len() as i64,
        "case {case:?}: expected {} active accounts, got {statistics:?}",
        active.len()
    );

    let entries = feature_entries(&statistics, case);
    let reported: Vec<String> = entries
        .iter()
        .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported,
        ORG_FEATURES
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<String>>(),
        "case {case:?}: expected one entry per OrgFeature member in catalog order, got {reported:?}"
    );

    for entry in &entries {
        let feature = entry["name"].as_str().unwrap_or_default().to_string();
        let expected = switches
            .values()
            .filter(|features| {
                features
                    .get(&feature)
                    .is_some_and(|(status, _)| status == ENABLED)
            })
            .count() as i64;
        let counted = entry["enabledAccountsCount"].as_i64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a count for {feature}, got {entry:?}")
        });
        assert_eq!(
            counted, expected,
            "case {case:?}: {feature} is enabled on {expected} active member(s) per GetMemberDetectors, got {counted}"
        );

        // The nested counts: one entry per additional configuration any active member
        // stores under this feature, counting the members that hold both switches on.
        let mut expected_additional: BTreeMap<String, i64> = BTreeMap::new();
        for features in switches.values() {
            let Some((status, additional)) = features.get(&feature) else {
                continue;
            };
            for (name, nested) in additional {
                let counted = expected_additional.entry(name.clone()).or_insert(0);
                if status == ENABLED && nested == ENABLED {
                    *counted += 1;
                }
            }
        }
        let reported_additional: BTreeMap<String, i64> = entry["additionalConfiguration"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|configuration| {
                (
                    configuration["name"].as_str().unwrap_or_default().to_string(),
                    configuration["enabledAccountsCount"].as_i64().unwrap_or(-1),
                )
            })
            .collect();
        assert_eq!(
            reported_additional, expected_additional,
            "case {case:?}: {feature}'s additional configurations expected {expected_additional:?}, got {entry:?}"
        );
    }
}

/// One member's feature switches: per feature, its status and the status of each
/// additional configuration stored under it.
type FeatureSwitches = BTreeMap<String, (String, BTreeMap<String, String>)>;

/// What `GetMemberDetectors` reports for `account_ids`, reduced to the switches these
/// statistics count and keyed by account id — so every expectation here is derived from
/// an independent read of the same stored configuration.
async fn member_switches(
    router: &Router,
    region: &str,
    detector_id: &str,
    account_ids: &[&str],
    case: &str,
) -> BTreeMap<String, FeatureSwitches> {
    if account_ids.is_empty() {
        return BTreeMap::new();
    }
    let (status, body) = call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/member/detector/get"),
        Some(json!({"accountIds": account_ids})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-member-detectors expected 200, got {status}"
    );
    let mut switches: BTreeMap<String, FeatureSwitches> = BTreeMap::new();
    // `MemberDataSourceConfigurations` goes on the wire as `members` — its `jsonName`
    // (`smithy_query(operation="GetMemberDetectors")`).
    for configuration in body["members"].as_array().into_iter().flatten() {
        let account = configuration["accountId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let mut features = FeatureSwitches::new();
        for entry in configuration["features"].as_array().into_iter().flatten() {
            let additional = entry["additionalConfiguration"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|nested| {
                    (
                        nested["name"].as_str().unwrap_or_default().to_string(),
                        nested["status"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            features.insert(
                entry["name"].as_str().unwrap_or_default().to_string(),
                (
                    entry["status"].as_str().unwrap_or_default().to_string(),
                    additional,
                ),
            );
        }
        switches.insert(account, features);
    }
    assert_eq!(
        switches.len(),
        account_ids.len(),
        "case {case:?}: expected one configuration per account, got {switches:?}"
    );
    switches
}

/// `CreateMembers` through `region`'s endpoint, asserting every account was recorded.
async fn create_members(
    router: &Router,
    region: &str,
    detector_id: &str,
    accounts: &[(&str, &str)],
    case: &str,
) {
    let details: Vec<Value> = accounts
        .iter()
        .map(|(account_id, email)| json!({"accountId": account_id, "email": email}))
        .collect();
    let (status, body) = call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/member"),
        Some(json!({"accountDetails": details})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: create-members expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["unprocessedAccounts"],
        json!([]),
        "case {case:?}: expected every account recorded, got {body:?}"
    );
}

/// One of the member operations that takes a plain `accountIds` list: `invite` or
/// `disassociate`.
async fn member_action(
    router: &Router,
    region: &str,
    detector_id: &str,
    action: &str,
    account_ids: &[&str],
    case: &str,
) {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/member/{action}"),
        Some(json!({"accountIds": account_ids})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: member {action} expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["unprocessedAccounts"],
        json!([]),
        "case {case:?}: expected every account processed by {action}, got {body:?}"
    );
}

/// `UpdateMemberDetectors` with whatever configuration members `configuration` names.
async fn update_member_detectors(
    router: &Router,
    region: &str,
    detector_id: &str,
    account_ids: &[&str],
    configuration: Value,
    case: &str,
) {
    let mut body = json!({"accountIds": account_ids});
    for (key, value) in configuration.as_object().into_iter().flatten() {
        body[key.as_str()] = value.clone();
    }
    let (status, answered) = call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/member/detector/update"),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: update-member-detectors expected 200, got {status} {answered:?}"
    );
    assert_eq!(
        answered["unprocessedAccounts"],
        json!([]),
        "case {case:?}: expected every account configured, got {answered:?}"
    );
}

/// `UpdateDetector`, switching GuardDuty off for the administrator's own account.
async fn disable_detector(router: &Router, region: &str, detector_id: &str, case: &str) {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}"),
        Some(json!({"enable": false})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: update-detector expected 200, got {status} {body:?}"
    );
}

/// `ListMembers` over every member, associated or not: the read a report must leave
/// untouched.
async fn list_members(router: &Router, region: &str, detector_id: &str, case: &str) -> Value {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector_id}/member?onlyAssociated=false"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-members expected 200, got {status} {body:?}"
    );
    body
}

/// The account ids the indexes name, in index order.
fn account_ids(indexes: &[usize]) -> Vec<&'static str> {
    indexes.iter().map(|index| ORG_MEMBERS[*index].0).collect()
}
