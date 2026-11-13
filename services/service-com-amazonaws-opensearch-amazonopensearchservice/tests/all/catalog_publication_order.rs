// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the instance-type catalog `ListInstanceTypeDetails` publishes
//! being PUBLISHED CONTENT: the listing of one engine release is a function of that
//! release alone, so element N of it names the same instance type on every call, in
//! every emulator, and whatever the account holds.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the listing helpers from
//! [`super::instance_type_catalog`], which owns this operation's suite; this one adds no
//! second copy of either, and states the rules that suite does not:
//!
//! * a release's listing is IDENTICAL — entry for entry, in order — across repeated
//!   reads, across two independently constructed emulators, and across a store that has
//!   gained a domain in between, and it names each instance type exactly once. A listing
//!   drawn from a collection whose iteration order belongs to the run rather than to the
//!   catalog fails this: two emulators hold two such collections, so their orders differ;
//! * the catalog GROWS with the release and keeps its order: the listing of a release
//!   that a later release succeeds appears inside the later listing as a SUBSEQUENCE, so
//!   a release that adds instance families never moves a type ahead of one it followed.
//!
//! Both rules are asserted over a TABLE of releases spanning both engine lines and each
//! generation prerequisite the catalog gates a family on, so neither rests on one
//! release.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::compatible_versions::rank;
use super::domain_lifecycle::Emulator;
use super::instance_type_catalog::{instance_types, list, whole_listing};

/// The releases every case below spans: the oldest Elasticsearch release, the releases
/// the catalog gates an instance family on and one either side of them, the engine-line
/// crossing, and a current OpenSearch release. Source of the set: the engine versions
/// `ListVersions` publishes, which [`super::version_catalog`] holds.
const RELEASES: [&str; 10] = [
    "Elasticsearch_1.5",
    "Elasticsearch_6.8",
    "Elasticsearch_7.8",
    "Elasticsearch_7.9",
    "OpenSearch_1.0",
    "OpenSearch_2.11",
    "OpenSearch_2.13",
    "OpenSearch_2.15",
    "OpenSearch_2.17",
    "OpenSearch_2.19",
];

/// A domain name of the modelled shape (`^[a-z][a-z0-9\-]+$`, 3..28), so the state the
/// second half of the first case reads the catalog against is state an account really
/// holds.
const TENANT: &str = "catalog-tenant";

#[tokio::test]
async fn a_release_publishes_one_listing_whatever_the_account_holds() {
    // Two emulators, each with its OWN engine and store: a listing that came from a
    // collection ordered by the run would differ between them.
    let first = Emulator::new();
    let second = Emulator::new();

    let mut baseline: Vec<(&str, Vec<Value>)> = Vec::new();
    for version in RELEASES {
        let published = whole_listing(&first, version, "").await;
        assert!(
            !published.is_empty(),
            "case {version}: expected the release's published catalog, got no entries"
        );

        let elsewhere = whole_listing(&second, version, "").await;
        assert_eq!(
            published,
            elsewhere,
            "case {version}: expected two emulators to publish one catalog, got {} and {} \
             entries",
            published.len(),
            elsewhere.len()
        );

        let again = whole_listing(&first, version, "").await;
        assert_eq!(
            published, again,
            "case {version}: expected a repeated read to publish the same listing, got {} \
             entries",
            again.len()
        );

        // Each instance type is published once, which is what makes "element N" name one
        // instance type at all.
        let types = instance_types(&published);
        let distinct: BTreeSet<&String> = types.iter().collect();
        assert_eq!(
            distinct.len(),
            types.len(),
            "case {version}: expected each instance type published once, got {} distinct of \
             {} entries",
            distinct.len(),
            types.len()
        );

        baseline.push((version, published));
    }

    // The catalog is the SERVICE's, so nothing the account does moves an element of it:
    // a created domain, and the filtered and zone-carrying views of the listing, leave
    // every release's listing exactly as it was.
    let created = first.create(json!({ "DomainName": TENANT })).await;
    assert_eq!(
        created.status, 200,
        "case {TENANT}: expected the domain the catalog is re-read against to be created, \
         got {} {:?}",
        created.status, created.body
    );
    for query in [
        "retrieveAZs=true",
        "instanceType=m5.large.search",
        &format!("domainName={TENANT}"),
    ] {
        let answer = list(&first, "OpenSearch_2.19", query).await;
        assert_eq!(
            answer.status, 200,
            "case {query}: expected the view to be answered, got {} {:?}",
            answer.status, answer.body
        );
    }

    for (version, published) in &baseline {
        let after = whole_listing(&first, version, "").await;
        assert_eq!(
            after,
            *published,
            "case {version}: expected the published catalog to be unmoved by the account's \
             own state, got {} entries",
            after.len()
        );
    }
}

#[tokio::test]
async fn a_later_release_publishes_the_earlier_listing_in_the_same_order() {
    let emulator = Emulator::new();

    let mut listings: Vec<(&str, Vec<String>)> = Vec::new();
    for version in RELEASES {
        let types = instance_types(&whole_listing(&emulator, version, "").await);
        listings.push((version, types));
    }

    // Every ordered pair of the table where the second release succeeds the first: the
    // later releases of an engine line, and — since the OpenSearch line continues the
    // Elasticsearch one — every OpenSearch release after every Elasticsearch one.
    let mut growing_pairs = 0_usize;
    for (earlier, earlier_types) in &listings {
        for (later, later_types) in &listings {
            if rank(later) < rank(earlier) {
                continue;
            }
            let case = format!("{earlier} -> {later}");

            // Walk the later listing once: each type of the earlier listing appears in
            // it after the types it followed there, so the earlier listing is the later
            // one with the types the earlier release did not offer taken out.
            let mut cursor = 0_usize;
            for instance_type in earlier_types {
                match later_types[cursor..]
                    .iter()
                    .position(|candidate| candidate == instance_type)
                {
                    Some(offset) => cursor += offset + 1,
                    None => panic!(
                        "case {case}: expected the later listing to publish {instance_type} \
                         after the types it follows, got {} of its {} entries before it",
                        cursor,
                        later_types.len()
                    ),
                }
            }

            if later_types.len() > earlier_types.len() {
                growing_pairs += 1;
            }
        }
    }

    // The table really spans growth, so the rule above is not asserted over pairs of
    // equal listings alone.
    assert!(
        growing_pairs >= RELEASES.len(),
        "case the release table: expected releases whose catalogs differ in size, got \
         {growing_pairs} such pairs"
    );
}
