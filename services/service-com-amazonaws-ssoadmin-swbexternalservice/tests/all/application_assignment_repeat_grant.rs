// SPDX-License-Identifier: CC0-1.0

//! The IDEMPOTENCE of a direct application-assignment grant, over populations of tuples
//! drawn by a GENERATOR rather than written out by hand, on the `awsJson1_1` wire.
//!
//! THE RULE: a direct application assignment is identified by the tuple (application,
//! principal type, principal id), so the grant CONVERGES on that one assignment — every
//! grant of a tuple answers HTTP 200 with the modelled EMPTY body, whether it is the first
//! or the fifth, and the application ends up holding exactly one assignment per DISTINCT
//! tuple granted on it, whatever order the grants arrived in.
//!
//! `application_assignment_conflict.rs` pins that rule over a hand-written table. This
//! suite is its PROPERTY form: what it checks is an INVARIANT of a grant sequence (the set
//! of assignments an application holds is the SET of tuples granted on it, regardless of
//! multiplicity and order), so the inputs the rule generalises over — how many applications
//! the grants are spread across, how many distinct tuples each one gets, each tuple's
//! `PrincipalType` and the form and value of its `PrincipalId`, how often each tuple is
//! re-granted, and the ORDER the whole sequence is sent in — are DRAWN instead of
//! enumerated. A single row of a table can be satisfied by a branch keyed on that row's
//! shape; a drawn sequence cannot.
//!
//! The draws' bounds are the model's own
//! (`smithy_query(query="operation", operation="CreateApplicationAssignment")`):
//!
//! * the principal types are the model-generated `ops::types::PRINCIPAL_TYPE_VALUES`
//!   (`PrincipalType`: `GROUP | USER`), so nothing here is a hand-written catalog;
//! * a drawn `PrincipalId` is one of the forms its pattern
//!   `^([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}$`
//!   admits — a bare GUID in lower or upper hex, and the prefixed form, which is the
//!   member's 47-character maximum.
//!
//! The case count is bounded and each population is drawn from a FIXED seed that every
//! assertion message carries, so the suite draws the same populations on every run, a
//! failure names the draw that produced it and re-running the suite reproduces it;
//! `SSOADMIN_DRAWN_GRANT_SEED` points the draw at one seed of the caller's choosing for an
//! exploratory run. Nothing is written outside the engine's own store.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints and
//! the `awsJson1_1` envelope are exercised rather than bypassed. The harness is the one
//! `ssoadmin_permission_sets.rs` owns, and the answer and state assertions are the ones
//! `application_assignment_conflict.rs` owns.

use std::collections::BTreeSet;
use std::sync::Arc;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use service_com_amazonaws_ssoadmin_swbexternalservice::ops::types::PRINCIPAL_TYPE_VALUES;

use crate::application_assignment_conflict::{assert_granted, held};
use crate::application_assignments::{minted_application, Application};
use crate::ssoadmin_permission_sets::{
    create_application_assignment as grant, describe_application_assignment, drawn_seeds,
    router_over, seeded_instance_arn, with_store,
};

/// The seeds the populations are drawn from — one population per seed, so the case count is
/// bounded and every run draws the SAME eight populations: a failure is reproduced by
/// re-running the suite, and the seed it names locates the population that produced it.
/// [`SEED_OVERRIDE`] replaces them with one seed of the caller's choosing, for an
/// exploratory run that walks further into the space than the fixed set does.
const SEEDS: [u64; 8] = [
    0x9_1a7e_0000_0001,
    0x9_1a7e_0000_0002,
    0x9_1a7e_0000_0003,
    0x9_1a7e_0000_0005,
    0x9_1a7e_0000_0008,
    0x9_1a7e_0000_000d,
    0x9_1a7e_0000_0015,
    0x9_1a7e_0000_0022,
];

/// The environment variable an exploratory run names ONE seed in, in decimal.
const SEED_OVERRIDE: &str = "SSOADMIN_DRAWN_GRANT_SEED";

/// The applications one drawn population spreads its grants over.
const MAX_APPLICATIONS: usize = 3;

/// The distinct tuples one drawn application is granted.
const MAX_TUPLES_PER_APPLICATION: usize = 5;

/// How often one drawn tuple is granted: at least once, and up to a reconcile loop that
/// keeps re-asserting it.
const MAX_GRANTS_PER_TUPLE: usize = 4;

/// The seeds this run draws its populations from: the fixed set, or the single seed
/// [`SEED_OVERRIDE`] names — read by the shared reader every drawn suite of this crate
/// uses.
fn seeds() -> Vec<u64> {
    drawn_seeds(SEED_OVERRIDE, &SEEDS)
}

/// One drawn principal: a `PrincipalType` of the model's enum and a `PrincipalId` in one of
/// the forms its pattern admits.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Principal {
    principal_type: &'static str,
    principal_id: String,
}

impl Principal {
    /// The principal as the rest of the assignment's identity, once the application is
    /// fixed — the spelling `held` reports.
    fn identity(&self) -> String {
        format!("{}#{}", self.principal_type, self.principal_id)
    }
}

/// A drawn hex string of `digits` digits, in lower or upper case.
fn hex(rng: &mut StdRng, digits: usize, upper: bool) -> String {
    (0..digits)
        .map(|_| {
            let digit = rng.random_range(0..16u8);
            let rendered = char::from_digit(u32::from(digit), 16).unwrap_or('0');
            if upper {
                rendered.to_ascii_uppercase()
            } else {
                rendered
            }
        })
        .collect()
}

/// Draw one principal: a type from the model's enum, and an id in one of the three forms
/// the member's pattern admits (a bare GUID in lower hex, the same in upper hex, and the
/// `[0-9a-f]{10}-` prefixed form, which is the member's 47-character maximum).
fn drawn_principal(rng: &mut StdRng) -> Principal {
    let principal_type = PRINCIPAL_TYPE_VALUES[rng.random_range(0..PRINCIPAL_TYPE_VALUES.len())];
    let upper = rng.random_ratio(1, 3);
    let guid = format!(
        "{}-{}-{}-{}-{}",
        hex(rng, 8, upper),
        hex(rng, 4, upper),
        hex(rng, 4, upper),
        hex(rng, 4, upper),
        hex(rng, 12, upper)
    );
    let principal_id = if rng.random_ratio(1, 3) {
        // The prefix is lower hex only, per the pattern's `[0-9a-f]{10}-` branch.
        format!("{}-{guid}", hex(rng, 10, false))
    } else {
        guid
    };
    Principal {
        principal_type,
        principal_id,
    }
}

/// THE PROPERTY: over each drawn population, every grant is served with the modelled empty
/// 200 and each application ends up holding exactly the SET of tuples granted on it — one
/// row per distinct tuple, whatever the multiplicities and the order were.
#[tokio::test]
async fn a_drawn_grant_sequence_leaves_one_assignment_per_distinct_tuple() {
    for seed in seeds() {
        let case = format!("seed {seed:#x}");
        let mut rng = StdRng::seed_from_u64(seed);

        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(Arc::clone(&engine));
        let instance_arn = seeded_instance_arn(&router).await;

        // The drawn population: each application, the distinct principals granted on it,
        // and the grant sequence those multiplicities expand to.
        let applications = rng.random_range(1..=MAX_APPLICATIONS);
        let mut expected: Vec<(String, BTreeSet<String>)> = Vec::new();
        let mut sequence: Vec<(usize, Principal)> = Vec::new();
        for index in 0..applications {
            let application_arn = minted_application(
                &router,
                &instance_arn,
                &Application::new(
                    &format!("{case} app {index}"),
                    &format!("drawn-grants-{seed:x}-{index}"),
                ),
            )
            .await;
            let mut distinct: BTreeSet<String> = BTreeSet::new();
            for _ in 0..rng.random_range(1..=MAX_TUPLES_PER_APPLICATION) {
                let principal = drawn_principal(&mut rng);
                distinct.insert(principal.identity());
                for _ in 0..rng.random_range(1..=MAX_GRANTS_PER_TUPLE) {
                    sequence.push((index, principal.clone()));
                }
            }
            expected.push((application_arn, distinct));
        }

        // Shuffled, so the repeats of one tuple are interleaved with the other tuples'
        // rather than arriving back to back: the invariant is about the SET granted, not
        // about the order.
        for position in (1..sequence.len()).rev() {
            sequence.swap(position, rng.random_range(0..=position));
        }

        let mut sent = 0usize;
        for (index, principal) in &sequence {
            let application_arn = &expected[*index].0;
            let (status, body) = grant(
                &router,
                application_arn,
                principal.principal_type,
                &principal.principal_id,
            )
            .await;
            sent += 1;
            assert_granted(
                &case,
                &format!(
                    "grant #{sent} of {} on application {index}",
                    principal.identity()
                ),
                status,
                &body,
            );
        }

        // One row per distinct tuple, and every one of them readable.
        let mut total = 0usize;
        for (index, (application_arn, distinct)) in expected.iter().enumerate() {
            let found = held(&router, application_arn, &case).await;
            let wanted: Vec<String> = distinct.iter().cloned().collect();
            assert_eq!(
                found, wanted,
                "case {case:?}: application {index} must hold exactly its distinct drawn \
                 tuples {wanted:?}, got {found:?}"
            );
            total += distinct.len();

            for identity in distinct {
                let (principal_type, principal_id) = identity
                    .split_once('#')
                    .unwrap_or_else(|| panic!("case {case:?}: {identity:?} is a (type, id) key"));
                let (status, body) = describe_application_assignment(
                    &router,
                    application_arn,
                    principal_type,
                    principal_id,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: {identity:?} must describe on application {index}, got \
                     {status} {body}"
                );
            }
        }
        assert_eq!(
            with_store(&engine, |store| store.application_assignments.len()),
            total,
            "case {case:?}: {sent} grants of {total} distinct tuples must store {total} rows"
        );
    }
}
