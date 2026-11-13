// SPDX-License-Identifier: CC0-1.0

//! Which Regions an account discovers is ONE computation, and every step of a discovery
//! pass answers from it.
//!
//! The rule under test: an account discovers exactly the `SourceRegions` its settings name
//! while `LinuxSubscriptionsDiscovery` is `Enabled`, and no Region at all while it is not,
//! so the Regions a pass plans to read, the Regions it is allowed to hop to, the Regions its
//! writes may land in and the Regions whose records it keeps when it ends are the SAME
//! Regions. A pass whose two ends disagreed
//! about one Region would show it: a Region admitted by the hop and write gate but absent
//! from the plan's set would have its records dropped by the pruning step that ends the pass
//! (the instances running there would be missing), and one kept by that pruning step but
//! refused by the gate would hold records no hop ever refreshes (instances would be reported
//! for a Region the settings no longer discover).
//!
//! Each row therefore observes both ends of one pass: what the report answers and what the
//! store holds after it, and it is driven through EACH of the two operations that
//! materialize a pass (`ListLinuxSubscriptionInstances` and `ListLinuxSubscriptions`), so
//! neither reaches the Region rule by a path of its own.
//!
//! The rows span what the rule generalises over: how many of the Regions the account runs
//! instances in are named, which of them, Regions the account runs nothing in named beside
//! and instead of those, and the switch itself not being `Enabled` with every Region left
//! named beside it. The rows run in sequence against one fleet, so each row's settings also
//! NARROW or WIDEN the previous row's Region set: a Region that stopped being discovered has
//! to lose its records, and one that started being discovered has to gain them.
//!
//! The instances are EC2's resources, so the fleet, the AMI and the two services on one
//! engine come from `tests/all/discovery_from_ec2.rs` — the suite that drives discovery
//! against EC2 itself; this one only varies the Region set the pass is decided by.

use std::collections::BTreeSet;

use serde_json::json;

use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState;

use crate::discovery_from_ec2::{
    reported_instance_ids, reported_subscriptions, Fleet, DISABLED, ENABLED, RHEL,
};
use crate::harness::{Harness, Reply, REGION};

/// The Regions the fleet runs one instance in, so a row can name a subset of them, all of
/// them, or none. `REGION` is the Region every call is signed for.
const FLEET_REGIONS: &[&str] = &["eu-west-1", "us-east-1", REGION];

/// Regions the fleet runs nothing in, for the rows whose `SourceRegions` point past the
/// fleet: a named Region a pass finds no instance in is discovered just the same, and it is
/// what the account discovers that decides, not where it happens to run something.
const EMPTY_REGION: &str = "ca-central-1";
const SECOND_EMPTY_REGION: &str = "ap-south-1";

/// One account configuration a pass is decided by.
#[derive(Debug, Clone, Copy)]
struct Row {
    /// What this row is about, named in every assertion it drives.
    case: &'static str,
    /// The `LinuxSubscriptionsDiscovery` the settings carry
    /// (`smithy_query(query="shape",shape="LinuxSubscriptionsDiscovery")`: Enabled |
    /// Disabled).
    discovery: &'static str,
    /// The `SourceRegions` beside that switch. The member is `uniqueItems` and holds 1 to
    /// 100 Regions
    /// (`smithy_query(query="shape",shape="LinuxSubscriptionsDiscoverySettings")`), so no
    /// row names a Region twice, nor none at all.
    source_regions: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "enabled for every Region the account runs an instance in",
        discovery: ENABLED,
        source_regions: FLEET_REGIONS,
    },
    Row {
        case: "narrowed to one of those Regions",
        discovery: ENABLED,
        source_regions: &[REGION],
    },
    Row {
        case: "widened to two of them, named in the reverse of the order a pass walks",
        discovery: ENABLED,
        source_regions: &["us-east-1", "eu-west-1"],
    },
    Row {
        case: "every Region the account runs an instance in, beside two it runs nothing in",
        discovery: ENABLED,
        source_regions: &[
            EMPTY_REGION,
            SECOND_EMPTY_REGION,
            "eu-west-1",
            "us-east-1",
            REGION,
        ],
    },
    Row {
        case: "a Region the account runs nothing in, beside one it does",
        discovery: ENABLED,
        source_regions: &[EMPTY_REGION, REGION],
    },
    Row {
        case: "only Regions the account runs nothing in",
        discovery: ENABLED,
        source_regions: &[EMPTY_REGION, SECOND_EMPTY_REGION],
    },
    Row {
        case: "every Region the account runs an instance in, discovered again",
        discovery: ENABLED,
        source_regions: FLEET_REGIONS,
    },
    Row {
        case: "not enabled, with every one of those Regions still named",
        discovery: DISABLED,
        source_regions: FLEET_REGIONS,
    },
];

/// Which report materializes the pass: both operations that run one, so the Region rule is
/// driven through each of them.
#[derive(Debug, Clone, Copy)]
enum Report {
    Instances,
    Subscriptions,
}

const REPORTS: &[Report] = &[Report::Instances, Report::Subscriptions];

impl Report {
    fn label(self) -> &'static str {
        match self {
            Self::Instances => "ListLinuxSubscriptionInstances",
            Self::Subscriptions => "ListLinuxSubscriptions",
        }
    }

    /// Run the report, which runs the pass, and answer its reply.
    async fn run(self, fleet: &Fleet) -> Reply {
        match self {
            Self::Instances => fleet.instances(&json!({})).await,
            Self::Subscriptions => fleet.subscriptions(&json!({})).await,
        }
    }
}

/// The Regions `row`'s settings discover: the `SourceRegions` they name while the switch is
/// `Enabled`, and none at all while it is not.
fn discovered_regions(row: &Row) -> BTreeSet<String> {
    if row.discovery != ENABLED {
        return BTreeSet::new();
    }
    row.source_regions
        .iter()
        .map(|region| (*region).to_string())
        .collect()
}

/// The instances that must be discovered under `row`: the fleet's instances in the Regions
/// those settings discover, by id, sorted.
fn expected_ids(row: &Row, fleet_ids: &[(&str, String)]) -> Vec<String> {
    let discovered = discovered_regions(row);
    let mut ids: Vec<String> = fleet_ids
        .iter()
        .filter(|(region, _)| discovered.contains(*region))
        .map(|(_, instance_id)| instance_id.clone())
        .collect();
    ids.sort();
    ids
}

/// The discovered instances the store holds, by id, read through the same seam the handlers
/// use — the end of the pass the pruning step decides.
fn held_ids(h: &Harness) -> Vec<String> {
    let mut ids: Vec<String> = h
        .engine
        .state::<ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState>()
        .lock()
        .discovered_instances
        .keys()
        .cloned()
        .collect();
    ids.sort();
    ids
}

/// The Regions the store holds a discovered record for.
fn held_regions(h: &Harness) -> BTreeSet<String> {
    h.engine
        .state::<ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState>()
        .lock()
        .discovered_instances
        .values()
        .map(|record| record.region.clone())
        .collect()
}

/// What the reply reports, as the sorted instance ids or the sorted `(name, type, count)`
/// subscriptions, so the two reports are compared against the same expectation.
fn reported(report: Report, reply: &Reply, case: &str) -> Vec<String> {
    match report {
        Report::Instances => {
            let mut ids = reported_instance_ids(reply, case);
            ids.sort();
            ids
        }
        Report::Subscriptions => {
            let mut rows: Vec<String> = reported_subscriptions(reply, case)
                .into_iter()
                .map(|(name, subscription_type, count)| {
                    format!("{name}|{subscription_type}|{count}")
                })
                .collect();
            rows.sort();
            rows
        }
    }
}

/// What `report` must answer for the instances `expected_ids` names: their ids, or the one
/// subscription their AMI bills counting them — "the total amount of running instances using
/// this subscription" (`Subscription$InstanceCount`), every instance of this fleet being
/// running.
fn expected_report(report: Report, expected_ids: &[String]) -> Vec<String> {
    match report {
        Report::Instances => expected_ids.to_vec(),
        Report::Subscriptions => {
            if expected_ids.is_empty() {
                return Vec::new();
            }
            let (name, subscription_type) = RHEL
                .subscription
                .expect("the AMI this fleet launches from bills a Linux subscription");
            vec![format!("{name}|{subscription_type}|{}", expected_ids.len())]
        }
    }
}

#[tokio::test]
async fn one_region_set_decides_a_pass_from_its_plan_to_its_pruning() {
    let fleet = Fleet::new();
    let image_id = fleet.publish_image(RHEL);
    // One instance per Region, launched through EC2 itself, so what is discovered in a
    // Region is an instance its owner answers for.
    let mut fleet_ids: Vec<(&str, String)> = Vec::new();
    for region in FLEET_REGIONS {
        let ids = fleet.launch(region, &image_id, 1).await;
        fleet_ids.push((region, ids[0].clone()));
    }

    for row in ROWS {
        let discovered = discovered_regions(row);
        let expected = expected_ids(row, &fleet_ids);

        for report in REPORTS {
            let case = &format!("{}, through {}", row.case, report.label());
            // The settings are persisted over the wire, the way a client onboards the
            // account or changes what it discovers.
            fleet.set_discovery(row.discovery, row.source_regions).await;

            let reply = report.run(&fleet).await;
            let answered = reported(*report, &reply, case);
            let expected_answer = expected_report(*report, &expected);
            assert_eq!(
                answered, expected_answer,
                "case {case:?}: the report must answer from the Regions {discovered:?} those \
                 settings discover, expected {expected_answer:?}, got {answered:?}"
            );

            // The other end of the same pass: what it left in the store. Records for a
            // Region this account no longer discovers are gone, and every instance of a
            // Region it does discover is there — which is the two ends agreeing.
            let held = held_ids(&fleet.h);
            assert_eq!(
                held, expected,
                "case {case:?}: the pass must leave the store holding exactly the instances \
                 of {discovered:?}, expected {expected:?}, got {held:?}"
            );
            let regions = held_regions(&fleet.h);
            assert!(
                regions.is_subset(&discovered),
                "case {case:?}: no record may be kept for a Region no hop of a pass refreshes, \
                 the store holds records for {regions:?} against the discovered {discovered:?}"
            );

            // And a second report answers the same: the pass is decided by the settings, not
            // by how often it has run.
            let again = report.run(&fleet).await;
            let answered_again = reported(*report, &again, case);
            assert_eq!(
                answered_again, expected_answer,
                "case {case:?}: a second report of the same settings must answer the same, \
                 expected {expected_answer:?}, got {answered_again:?}"
            );
            let held_again = held_ids(&fleet.h);
            assert_eq!(
                held_again, expected,
                "case {case:?}: a second pass over the same settings must hold the same \
                 instances, expected {expected:?}, got {held_again:?}"
            );
        }
    }
}
