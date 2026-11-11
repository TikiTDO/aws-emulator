// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a domain's package listing answers when the account
//! holds no domain under the name the request carries.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. This suite adds no second copy of anything
//! the package suites already have: the harness comes from [`super::domain_lifecycle`],
//! the package helpers from [`super::package_associations`], the `DeleteDomain` request
//! from [`super::index_registry`], and the listing and associate request helpers and
//! the published page-size bound from [`super::package_listing_page_bounds`].
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * listing a domain's packages is a report over that domain's set of links, so an
//!   empty set is an empty page with no token however it came to be empty — a name
//!   nothing created, a name created and deleted, and a domain that exists holding no
//!   package — at every page size the model publishes;
//! * refusing a name no domain exists under belongs to the calls that CHANGE a
//!   domain's links: over the same table of names, the associate that names one
//!   package and the one that names a batch are both refused with 400
//!   `ValidationException` stating that the domain does not exist, while the listing of
//!   each name is served;
//! * the served listing is a report: it leaves a later caller seeing exactly what it
//!   would have seen, both for the name it could not resolve and for a domain that
//!   holds links.

use serde_json::Value;

use super::domain_lifecycle::{
    create_baseline, error_code, Emulator, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};
use super::index_registry::delete_domain;
use super::package_associations::available_packages;
use super::package_listing_page_bounds::{
    associate_many, associate_one, details, list_for_domain, page_query, PUBLISHED_MAX_RESULTS,
};

/// The names every table below carries, each one `DomainName` admits
/// (`@length(min: 3, max: 28)` with `@pattern("^[a-z][a-z0-9\-]+$")`, from
/// `smithy_query(query="operation",operation="ListPackagesForDomain")`) that this
/// account holds no domain under: the shortest the length allows, the longest, one
/// spanning the digits and hyphens the pattern allows, and one mixing them.
///
/// Reached by a sibling suite as `super::package_listing_for_missing_domain::MISSING_NAMES`,
/// so the crate holds ONE table of these names.
pub(crate) const MISSING_NAMES: [(&str, &str); 4] = [
    ("the shortest name the length allows", "abc"),
    (
        "the longest name the length allows",
        "a23456789012345678901234567",
    ),
    ("a name carrying digits and hyphens", "missing-os-91"),
    ("a hyphenated name ending in a letter", "no-such-domain-x"),
];

/// A name created and then deleted, which is a missing name a client is most likely to
/// carry: the resource was there and is not any more.
const DELETED_NAME: &str = "missing-gone-domain";

/// Every name the tables carry: the syntactically valid ones nothing created, plus the
/// one this call creates and deletes. Reached by a sibling suite on the same rule as
/// [`MISSING_NAMES`].
pub(crate) async fn missing_names(emulator: &Emulator) -> Vec<(String, String)> {
    let mut names: Vec<(String, String)> = MISSING_NAMES
        .iter()
        .map(|(case, name)| ((*case).to_owned(), (*name).to_owned()))
        .collect();

    create_baseline(emulator, DELETED_NAME).await;
    let released = delete_domain(emulator, DELETED_NAME).await;
    assert_eq!(
        released.status, 200,
        "case DeleteDomain({DELETED_NAME:?}): expected 200, got {} with {:?}",
        released.status, released.body
    );
    names.push((
        "a name created and then deleted".to_owned(),
        DELETED_NAME.to_owned(),
    ));
    names
}

/// The page sizes every listing row is asked at: none at all (the modelled default),
/// the smallest page a request can ask for, one in between, and the published maximum.
const PAGE_SIZES: [i32; 4] = [0, 1, 7, PUBLISHED_MAX_RESULTS];

#[tokio::test]
async fn a_domains_package_listing_is_the_empty_set_for_every_name_no_domain_holds() {
    let emulator = Emulator::new();
    let names = missing_names(&emulator).await;

    for (case, name) in &names {
        for page_size in PAGE_SIZES {
            let case = format!("ListPackagesForDomain({name:?}, {case}, maxResults={page_size})");
            let answer = list_for_domain(&emulator, name, &page_query(page_size, None)).await;
            let reported = details(&answer, &case);
            assert!(
                reported.is_empty(),
                "case {case}: expected an empty DomainPackageDetailsList, got {reported:?}"
            );
            assert_eq!(
                answer.body["NextToken"].as_str(),
                None,
                "case {case}: expected no token past the end of an empty set, got {:?}",
                answer.body
            );
        }
    }

    // A domain that EXISTS holding no package reports the same empty page, so the rule
    // is about the SET being empty rather than about the name resolving.
    let empty = "missing-empty-domain";
    create_baseline(&emulator, empty).await;
    for page_size in PAGE_SIZES {
        let case = format!("ListPackagesForDomain({empty:?}, maxResults={page_size})");
        let answer = list_for_domain(&emulator, empty, &page_query(page_size, None)).await;
        let reported = details(&answer, &case);
        assert!(
            reported.is_empty(),
            "case {case}: expected an empty DomainPackageDetailsList, got {reported:?}"
        );
    }

    // And a domain holding a link still reports it, so an empty page is the set and not
    // this operation answering nothing at all.
    let ids = available_packages(&emulator).await;
    let holder = "missing-holder-domain";
    create_baseline(&emulator, holder).await;
    let linked = associate_one(&emulator, holder, &ids[0]).await;
    assert_eq!(
        linked.status, 200,
        "case AssociatePackage({holder:?}): expected 200, got {} with {:?}",
        linked.status, linked.body
    );
    let reported = details(
        &list_for_domain(&emulator, holder, "").await,
        "ListPackagesForDomain(a domain holding one package)",
    );
    assert_eq!(
        reported.len(),
        1,
        "case ListPackagesForDomain(a domain holding one package): expected the one \
         association made, got {reported:?}"
    );
}

#[tokio::test]
async fn the_listing_is_served_where_the_calls_that_change_the_links_are_refused() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;
    let names = missing_names(&emulator).await;

    for (case, name) in &names {
        // The read is served for this name.
        let listed = details(
            &list_for_domain(&emulator, name, "").await,
            &format!("ListPackagesForDomain({name:?}, {case})"),
        );
        assert!(
            listed.is_empty(),
            "case ListPackagesForDomain({name:?}, {case}): expected the empty set, got {listed:?}"
        );

        // And every call that would CHANGE this domain's links is refused, naming the
        // domain the request carried.
        for (call, answer) in [
            (
                "AssociatePackage",
                associate_one(&emulator, name, &ids[0]).await,
            ),
            (
                "AssociatePackages",
                associate_many(&emulator, name, &ids).await,
            ),
        ] {
            let case = format!("{call}({name:?}, {case})");
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (400, "ValidationException"),
                "case {case}: expected the name to be refused as a bad input value, got {} with \
                 {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.body["message"].as_str(),
                Some(format!("The domain {name} does not exist.").as_str()),
                "case {case}: expected the refusal to state that the domain does not exist, got \
                 {:?}",
                answer.body
            );
        }

        // The refused calls linked nothing, so the listing still reports the empty set.
        let listed = details(
            &list_for_domain(&emulator, name, "").await,
            &format!("ListPackagesForDomain({name:?}, after the refused calls)"),
        );
        assert!(
            listed.is_empty(),
            "case ListPackagesForDomain({name:?}, after the refused calls): expected the empty \
             set, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn listing_a_name_no_domain_holds_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;
    let names = missing_names(&emulator).await;

    let holder = "missing-untouched-domain";
    create_baseline(&emulator, holder).await;
    let linked = associate_one(&emulator, holder, &ids[0]).await;
    assert_eq!(
        linked.status, 200,
        "case AssociatePackage({holder:?}): expected 200, got {} with {:?}",
        linked.status, linked.body
    );
    // Past the span the clock owes the association, so the records the reads below
    // compare are the ones it settled on.
    tokio::time::sleep(SETTLE_WAIT).await;

    let before = details(
        &list_for_domain(&emulator, holder, "").await,
        "the first read of the domain holding a link",
    );

    // Reading each missing name, twice over and at several page sizes, resolves no
    // domain and creates none.
    for (case, name) in &names {
        for page_size in PAGE_SIZES {
            let answer = list_for_domain(&emulator, name, &page_query(page_size, None)).await;
            let reported = details(
                &answer,
                &format!("ListPackagesForDomain({name:?}, {case}, maxResults={page_size})"),
            );
            assert!(
                reported.is_empty(),
                "case ListPackagesForDomain({name:?}, {case}, maxResults={page_size}): expected \
                 the empty set, got {reported:?}"
            );
        }

        // The read minted nothing: `DescribeDomain` of the same name still finds no
        // domain, and so the associate that needs one is still refused.
        let described = emulator.describe(name).await;
        assert_eq!(
            described.status, RECORDED_NOT_FOUND_STATUS,
            "case DescribeDomain({name:?}, {case}): expected the listing to have created no \
             domain, got {} with {:?}",
            described.status, described.body
        );
        let refused = associate_one(&emulator, name, &ids[0]).await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case AssociatePackage({name:?}, {case}, after the listings): expected the name to \
             still be refused, got {} with {:?}",
            refused.status,
            refused.body
        );
    }

    let after: Vec<Value> = details(
        &list_for_domain(&emulator, holder, "").await,
        "the read after every missing-name listing",
    );
    assert_eq!(
        after, before,
        "case the read after every missing-name listing: expected the records the first read \
         reported, got {after:?}"
    );
}
