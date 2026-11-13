// SPDX-License-Identifier: CC0-1.0

//! The `State` view every enumerated IAM Identity Center instance carries, over accounts
//! and page sizes drawn by a GENERATOR rather than written out by hand, on the
//! `awsJson1_1` wire.
//!
//! THE RULE: the instance projection this service serves carries, beside each instance's
//! `Status`, a `State` structure whose single `Name` member is that same instance's
//! lifecycle value, read from the instance being served — so the member is on every entry
//! of every page of an enumeration and on the `Instance` envelope a single-instance read
//! repeats, and it never reports another instance's lifecycle.
//!
//! The sibling suites pin this rule over hand-written tables: `instance_state_view.rs`
//! that the member is served and mirrors the status through the lifecycle,
//! `listed_instance_state_source.rs` the record it is read from,
//! `listed_instance_state_under_any_name.rs` the `Name` value space,
//! `listed_instance_state_every_entry.rs` the cardinality of the enumeration and
//! `listed_instance_state_across_read_edge_cases.rs` the read paths. This suite is the
//! PROPERTY form of the same rule: the quantity it checks is an invariant of a served
//! entry ("`State.Name` is that entry's own `Status`"), so the inputs the rule generalises
//! over — how many instances the account holds, which lifecycle value each one is in, what
//! each one is named, what kind of instance it is, which Region it is enabled in and how
//! the listing is cut into pages — are DRAWN, bounded by what the model admits, instead of
//! enumerated. A single row of a table can be satisfied by a projection keyed on that
//! row's shape; a drawn population cannot.
//!
//! The draws' bounds are the model's own:
//!
//! * the lifecycle values are the model-generated `ops::types::INSTANCE_STATUS_VALUES`
//!   (`InstanceStatus`: `ACTIVE | CREATE_FAILED | CREATE_IN_PROGRESS |
//!   DELETE_IN_PROGRESS`), so nothing here is a hand-written catalog;
//! * the page size is inside `MaxResults`' published range
//!   (`smithy_query(query="operation",operation="ListInstances")`: `range 1..100`), plus
//!   the request that names none;
//! * a drawn `Name` is a word of `NameType`'s `^[\w+=,.@-]+$`, and an absent one, since
//!   the member is optional and no part of an instance's identity;
//! * the Regions are spellings of `RegionName`'s `^([a-z]+-){2,3}\d$`.
//!
//! The case count is bounded and each population is drawn from a FIXED seed that every
//! assertion message carries, so the suite draws the same populations on every run, a
//! failure names the draw that produced it and re-running the suite reproduces it;
//! `SSOADMIN_DRAWN_ACCOUNT_SEED` points the draw at one seed of the caller's choosing for
//! an exploratory run. Nothing is written outside the engine's own store.
//!
//! An account enables ONE instance through this API and these populations need many, so
//! the records are seeded through the same `state::Instance` constructor the handlers use;
//! everything asserted then goes over `MultiServiceRouter::dispatch`. The harness is the
//! one `ssoadmin_permission_sets.rs` owns and the rule assertion the one
//! `instance_state_view.rs` owns.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::SystemTime;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::Value;
use service_com_amazonaws_ssoadmin_swbexternalservice::ops::types::INSTANCE_STATUS_VALUES;
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_state_view::{assert_state_mirrors_status, listed_pages};
use crate::listed_instance_projection_matches_describe::assert_one_projection;
use crate::ssoadmin_permission_sets::{drawn_seeds, router_over, with_store};

/// The seeds the populations are drawn from — one population per seed, so the case count
/// is bounded and every run draws the SAME sixteen populations: a failure is reproduced by
/// re-running the suite, and the seed it names locates the population that produced it.
/// `SSOADMIN_DRAWN_ACCOUNT_SEED` replaces them with one seed of the caller's choosing, for
/// an exploratory run that walks further into the space than the fixed set does.
const SEEDS: [u64; 16] = [
    0x5eed_0000_0000_0001,
    0x5eed_0000_0000_0002,
    0x5eed_0000_0000_0003,
    0x5eed_0000_0000_0005,
    0x5eed_0000_0000_0008,
    0x5eed_0000_0000_000d,
    0x5eed_0000_0000_0015,
    0x5eed_0000_0000_0022,
    0x0bad_c0de_dead_0001,
    0x0bad_c0de_dead_0010,
    0x0bad_c0de_dead_0100,
    0x0bad_c0de_dead_1000,
    0xfeed_face_0000_0001,
    0xfeed_face_0000_0002,
    0xfeed_face_0000_0004,
    0xfeed_face_0000_0008,
];

/// The environment variable an exploratory run names ONE seed in, in decimal.
const SEED_OVERRIDE: &str = "SSOADMIN_DRAWN_ACCOUNT_SEED";

/// The seeds this run draws its populations from: the fixed set, or the single seed
/// [`SEED_OVERRIDE`] names — read by the shared reader every drawn suite of this crate
/// uses.
fn seeds() -> Vec<u64> {
    drawn_seeds(SEED_OVERRIDE, &SEEDS)
}

/// The largest account a population is drawn for: the published maximum of the output list
/// (`smithy_query(query="shape",shape="com.amazonaws.ssoadmin#InstanceList")`: `list of
/// InstanceMetadata`, `length min 0 max 10`), and one past it, so a listing at and over the
/// bound is drawn too.
const MAX_HELD_INSTANCES: usize = 11;

/// The Regions a drawn instance is enabled in — spellings of the model's `RegionName`
/// (`^([a-z]+-){2,3}\d$`) in both its two- and three-prefix forms, across partitions.
const REGIONS: [&str; 5] = [
    "us-east-2",
    "eu-west-3",
    "ap-southeast-4",
    "sa-east-1",
    "us-gov-west-1",
];

/// The characters a drawn `Name` is built from: `NameType`'s `^[\w+=,.@-]+$`.
const NAME_CHARACTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_+=,.@-";

/// One drawn population: the account's instances, the Region its calls are signed for and
/// the page size the enumeration asks for.
#[derive(Debug, Clone)]
struct Population {
    /// The Region the instances are enabled in and the calls are made in.
    region: &'static str,
    /// Each instance's `InstanceArn` and the lifecycle value its record holds.
    expected: BTreeMap<String, String>,
    /// The `MaxResults` the enumeration asks for, or `None` for the request that names
    /// none.
    max_results: Option<i64>,
}

/// A `Name` for a drawn instance: absent, or a word of `NameType`'s character class — so
/// nothing about the view can be keyed on a name the service chose, and an instance with no
/// name is served the same view as one with any.
fn drawn_name(rng: &mut StdRng) -> Option<String> {
    if rng.random_ratio(1, 4) {
        return None;
    }
    // `NameType` is `length 0..255`; a drawn name stays a short word of that class.
    let length = rng.random_range(1..=16usize);
    Some(
        (0..length)
            .map(|_| NAME_CHARACTERS[rng.random_range(0..NAME_CHARACTERS.len())] as char)
            .collect(),
    )
}

/// Draw one population and stand a router over the account holding it.
///
/// Nothing is left owed on any record (no activation and no removal instant), so the status
/// a draw seeded IS the status every read of that record reports and no transition the clock
/// owes can move it out from under the enumeration.
fn drawn_account(rng: &mut StdRng) -> (MultiServiceRouter, Population) {
    let region = REGIONS[rng.random_range(0..REGIONS.len())];
    let held = rng.random_range(0..=MAX_HELD_INSTANCES);
    let max_results = if rng.random_ratio(1, 4) {
        None
    } else {
        // `MaxResults` is `range(min: 1, max: 100)`; small sizes are drawn more often, so a
        // population is usually cut across several pages.
        Some(match rng.random_range(0..4u8) {
            0 => 1,
            1 => rng.random_range(1..=3),
            2 => rng.random_range(1..=10),
            _ => rng.random_range(1..=100),
        })
    };

    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let now = SystemTime::now();
    let expected = with_store(&engine, |store| {
        store.instances.clear();
        // The account's ambient instance has been enabled once and is gone, so no later
        // request enables one beside the records drawn here.
        store.ambient_instance_enabled = true;
        let mut expected = BTreeMap::new();
        let mut drawn_ids: BTreeSet<String> = BTreeSet::new();
        for _ in 0..held {
            let status = INSTANCE_STATUS_VALUES[rng.random_range(0..INSTANCE_STATUS_VALUES.len())];
            // `ssoins-` + 16 of `[a-zA-Z0-9-.]`, as the model's `InstanceArn` pattern
            // requires. Drawn until it is one this population does not hold yet, so a
            // repeated draw costs another draw rather than failing the suite.
            let instance_id = loop {
                let candidate = format!("ssoins-{:016x}", rng.random::<u64>());
                if drawn_ids.insert(candidate.clone()) {
                    break candidate;
                }
            };
            let mut instance = state::Instance::active(
                &instance_id,
                // `d-` + 10 hex digits, per `InstanceIdentityStoreArn`.
                &format!("d-{:010x}", rng.random::<u64>() & 0x00ff_ffff_ffff),
                if rng.random_ratio(1, 2) {
                    state::InstanceType::Account
                } else {
                    state::InstanceType::Organization
                },
                drawn_name(rng),
                aws_smithy_types::DateTime::from(now),
                region,
            );
            instance.status = status.to_string();
            instance.active_due = None;
            instance.deleted_due = None;
            expected.insert(instance.instance_arn.clone(), status.to_string());
            store
                .instances
                .insert(instance.instance_arn.clone(), instance);
        }
        expected
    });

    (
        router_over(engine),
        Population {
            region,
            expected,
            max_results,
        },
    )
}

/// Every entry `ListInstances` serves for this population, following each `NextToken` it
/// hands out so an entry is asserted on whichever page it arrives on.
///
/// The request walk itself is the shared one `instance_state_view.rs` owns, driven page by
/// page so this suite can also check each page against the size it asked for: the smallest
/// page the model admits is one instance and the enumeration ends on the page that returns
/// nothing, so the population's size plus a few is more requests than there can be pages.
async fn enumerate(case: &str, router: &MultiServiceRouter, population: &Population) -> Vec<Value> {
    let pages = listed_pages(
        case,
        router,
        population.region,
        population.max_results,
        population.expected.len() + 3,
    )
    .await;
    if let Some(max) = population.max_results {
        for (index, page) in pages.iter().enumerate() {
            assert!(
                page.len() as i64 <= max,
                "case {case:?}: page {index} serves at most the MaxResults asked for, got {} \
                 entries",
                page.len()
            );
        }
    }
    pages.concat()
}

/// THE RULE over drawn populations: however many instances the account holds, whatever
/// lifecycle value each one is in, whatever it is named and however the listing is cut into
/// pages, every entry served carries a `State` structure whose `Name` is that entry's own
/// `Status` — and the single-instance read of that entry reports the same one.
#[tokio::test]
async fn every_drawn_population_serves_each_entry_its_own_state_view() {
    for (case_index, seed) in seeds().into_iter().enumerate() {
        // One generator per population, seeded from that population's own seed, which every
        // assertion message carries: a failure names the draw that produced it and is
        // replayed by seeding the generator with it again.
        let mut rng = StdRng::seed_from_u64(seed);
        let (router, population) = drawn_account(&mut rng);
        let case = format!(
            "seed {seed} / case {case_index} / {} instance(s) in {} / MaxResults {:?}",
            population.expected.len(),
            population.region,
            population.max_results
        );

        let entries = enumerate(&case, &router, &population).await;
        assert_eq!(
            entries.len(),
            population.expected.len(),
            "case {case:?}: expected every held instance to be enumerated once, got {entries:?}"
        );

        let mut seen = BTreeMap::new();
        for (position, entry) in entries.iter().enumerate() {
            let entry_case = format!("{case} / entry {position}");
            assert_state_mirrors_status(&entry_case, entry);
            let instance_arn = entry["InstanceArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {entry_case:?}: the entry reports no InstanceArn: {entry}")
                })
                .to_string();
            let state_name = entry["State"]["Name"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {entry_case:?}: the entry reports no State.Name: {entry}")
                })
                .to_string();
            seen.insert(instance_arn, state_name);
        }
        assert_eq!(
            seen, population.expected,
            "case {case:?}: expected each instance's State.Name to be its own stored status, \
             got {seen:?}"
        );

        // One entry of the population, read back through the single-instance read — the
        // assertion `listed_instance_projection_matches_describe.rs` owns, so one site
        // decides what "the same projection" means — and the enumeration repeated: a read
        // changes nothing a later caller sees, so the second listing is the first one.
        if let Some(entry) = entries.first() {
            assert_one_projection(&case, &router, population.region, entry).await;
        }
        let again = enumerate(&case, &router, &population).await;
        assert_eq!(
            again, entries,
            "case {case:?}: the enumeration serves the same projection after a read, got \
             {again:?}"
        );
    }
}
