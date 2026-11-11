// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DEPENDENCY between a domain's indexes and the packages
//! whose files their analysis settings resolve: `CreateIndex` and `UpdateIndex`
//! resolving a package reference before the schema is stored, `DissociatePackages`
//! failing the removal of a package an index still uses, `DeleteIndex` lifting that
//! block, and the span a change to a domain's associations takes serializing a second
//! writer.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. Every call path and every store read comes from the
//! sibling suites — [`super::domain_lifecycle`], [`super::package_associations`],
//! [`super::package_dissociation`], [`super::index_registry`],
//! [`super::index_schema_updates`] and [`super::index_deletion`] — so this suite holds
//! no second copy of any of them; `DeleteIndex`'s own rules, including its refusals, are
//! the last of those suites'.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a package whose files an index of the domain resolves cannot be taken off it: the
//!   call is accepted, the association rests in its removal status and the clock then
//!   leaves it in `DISSOCIATION_FAILED` with the service's in-use error details, the
//!   association stays present and usable, and deleting the dependent index makes the
//!   IDENTICAL call remove it — over a table of the ways a schema can name a package's
//!   file (four path settings, the package's directory and a file inside it, the
//!   analysis section at two depths);
//! * a schema naming a file no package in place on the domain provides is refused with
//!   the modelled `ValidationException` and persists nothing, over the same table and
//!   over the three ways a package can fail to be in place (dissociated, never
//!   associated, still being associated), and the identical call is served once the
//!   package is associated;
//! * a path that names no package's content is NOT such a reference, over the rows that
//!   look like one and are not;
//! * a second associate or dissociate for the domain is refused with the modelled
//!   `ConflictException` for the WHOLE span the first change takes, over a table of
//!   delays within it — three taken from the span and one absolute, so the case fails if
//!   the span is shortened — and one past it, and the pair settles at exactly one
//!   association.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    package_reference_path, DOMAIN_PACKAGE_STATUS_ACTIVE, DOMAIN_PACKAGE_STATUS_ASSOCIATING,
    DOMAIN_PACKAGE_STATUS_DISSOCIATING, DOMAIN_PACKAGE_STATUS_DISSOCIATION_FAILED,
    PACKAGE_ASSOCIATION_CHANGE, PACKAGE_IN_USE, PACKAGE_TYPE_TXT_DICTIONARY,
};

use super::domain_lifecycle::{
    create_baseline, error_code, string_member, Answer, Emulator, RECORDED_NOT_FOUND_STATUS,
    SETTLE_WAIT,
};
use super::index_deletion::deleted_index;
use super::index_registry::{create as create_index, created_index, get as get_index};
use super::index_schema_updates::{read_schema, update as update_index, updated_index};
use super::package_associations::{create_package, member, stored_status};
use super::package_dissociation::{associate as associate_many, dissociate_many, listed_status, packages_of_domain};

/// `AssociatePackages` for the one package a case is about, through the wire path the
/// sibling suite drives.
async fn associate(emulator: &Emulator, domain: &str, package_id: &str) -> Answer {
    associate_many(emulator, domain, &[package_id.to_string()]).await
}

/// `DissociatePackages` for the one package a case is about, through the wire path the
/// sibling suite drives.
async fn dissociate(emulator: &Emulator, domain: &str, package_id: &str) -> Answer {
    dissociate_many(
        emulator,
        json!({ "DomainName": domain, "PackageList": [package_id] }),
    )
    .await
}

/// The `DomainPackageDetails` `ListPackagesForDomain` reports for `package_id`, or
/// `None` where the listing no longer reports that package at all — read through the
/// sibling suite's listing helper, so there is one copy of that read path.
pub(crate) async fn listed(
    emulator: &Emulator,
    domain: &str,
    package_id: &str,
    case: &str,
) -> Option<Value> {
    packages_of_domain(emulator, domain, case)
        .await
        .into_iter()
        .find(|details| details["PackageID"].as_str() == Some(package_id))
}

/// The relative path on the cluster nodes the service reports an association with
/// `package_id` at, read through the store the handlers write.
///
/// `ListPackagesForDomain` reports it as `ReferencePath` for a package that IS
/// associated; a case that needs the path of one that is NOT takes it from the same
/// derivation the handlers report, rather than writing the path form out by hand.
fn reference_path(emulator: &Emulator, package_id: &str, case: &str) -> String {
    let store = emulator.store();
    let guard = store.lock();
    let package = guard
        .packages
        .get(package_id)
        .unwrap_or_else(|| panic!("case {case}: expected the store to hold package {package_id}"));
    package_reference_path(package)
        .unwrap_or_else(|| panic!("case {case}: expected a reference path for {package_id}"))
}

/// A domain and one AVAILABLE `TXT-DICTIONARY` package, with no association between
/// them yet. Answers the `PackageID` the create minted.
pub(crate) async fn domain_and_package(emulator: &Emulator, domain: &str, package: &str) -> String {
    let package_id = create_package(emulator, package, PACKAGE_TYPE_TXT_DICTIONARY).await;
    create_baseline(emulator, domain).await;
    // The copy span `CreatePackage` leaves the package in `COPYING` for, and the
    // domain's own provisioning span, both fall due inside this one wait.
    tokio::time::sleep(SETTLE_WAIT).await;
    package_id
}

/// Associate `package_id` with `domain` and wait for the association to be in place,
/// answering the `ReferencePath` the call reported for it.
pub(crate) async fn associated(
    emulator: &Emulator,
    domain: &str,
    package_id: &str,
    case: &str,
) -> String {
    let answer = associate(emulator, domain, package_id).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected AssociatePackages to be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    let details = &answer.body["DomainPackageDetailsList"][0];
    let path = member(details, "ReferencePath", case);
    tokio::time::sleep(SETTLE_WAIT).await;
    let status = listed_status(emulator, domain, package_id, case).await;
    assert_eq!(
        status.as_deref(),
        Some(DOMAIN_PACKAGE_STATUS_ACTIVE),
        "case {case}: expected the association to be in place before the case runs, got \
         {status:?}"
    );
    path
}

/// One way an index's analysis settings name a file a custom package provides: which
/// path setting carries it, and how the schema addresses it relative to the package's
/// reference path.
///
/// The settings are the ones OpenSearch's analysis components load their content from,
/// all of the `<setting>_path` form the service's `ReferencePath` documents ("the
/// relative path of the package on the OpenSearch Service cluster nodes. This is
/// `synonym_path` when the package is for synonym files"), so the rows differ along
/// the setting's name, the component it sits in, how deep the analysis section is and
/// whether the path names the package's directory or a file inside it — not along one
/// recorded setting.
pub(crate) struct UseRow {
    pub(crate) case: &'static str,
    pub(crate) index: &'static str,
    /// The sentence the service answers when the file this row's path setting names
    /// cannot be read, pinned per row as the engine words it — the setting the schema
    /// carried, followed by the suffix the engine appends to whatever setting it was
    /// handed. It names that setting, so the row's setting is stated here rather than a
    /// second time beside it.
    pub(crate) message: &'static str,
    pub(crate) schema: fn(&str) -> Value,
}

/// The table every case about a resolved package reference runs over.
pub(crate) fn use_rows() -> Vec<UseRow> {
    vec![
        UseRow {
            case: "a synonym filter naming the package's directory",
            index: "synonyms-index",
            message: "IOException while reading synonyms_path_path: file not readable",
            schema: |path| {
                json!({
                    "settings": {
                        "index": { "number_of_shards": 1, "number_of_replicas": 0 },
                        "analysis": {
                            "filter": { "syn_filter": { "type": "synonym", "synonyms_path": path } },
                            "analyzer": {
                                "syn_analyzer": {
                                    "type": "custom",
                                    "tokenizer": "standard",
                                    "filter": ["lowercase", "syn_filter"],
                                },
                            },
                        },
                    },
                })
            },
        },
        UseRow {
            case: "a stop filter naming a file inside the package",
            index: "stopwords-index",
            message: "IOException while reading stopwords_path_path: file not readable",
            schema: |path| {
                json!({
                    "settings": {
                        "analysis": {
                            "filter": {
                                "stop_filter": {
                                    "type": "stop",
                                    "stopwords_path": format!("{path}/stopwords.txt"),
                                },
                            },
                        },
                    },
                })
            },
        },
        UseRow {
            case: "a character filter under settings.index.analysis",
            index: "mappings-index",
            message: "IOException while reading mappings_path_path: file not readable",
            schema: |path| {
                json!({
                    "settings": {
                        "index": {
                            "analysis": {
                                "char_filter": {
                                    "map_filter": {
                                        "type": "mapping",
                                        "mappings_path": format!("{path}/mappings.txt"),
                                    },
                                },
                            },
                        },
                    },
                })
            },
        },
        UseRow {
            case: "a tokenizer dictionary naming the package's directory",
            index: "tokenizer-index",
            message: "IOException while reading user_dictionary_path_path: file not readable",
            schema: |path| {
                json!({
                    "settings": {
                        "analysis": {
                            "tokenizer": {
                                "dict_tokenizer": {
                                    "type": "kuromoji_tokenizer",
                                    "user_dictionary_path": path,
                                },
                            },
                        },
                    },
                })
            },
        },
    ]
}

/// A change to the dependent index that names no path at all, so what it asserts is
/// that the index is still usable while the association is in its failed state.
fn plain_change() -> Value {
    json!({ "mappings": { "properties": { "title": { "type": "text" } } } })
}

#[tokio::test]
async fn a_package_an_index_resolves_stays_on_the_domain_until_the_index_is_gone() {
    let emulator = Emulator::new();

    for (position, row) in use_rows().into_iter().enumerate() {
        let domain = format!("in-use-{position}");
        let package_id =
            domain_and_package(&emulator, &domain, &format!("in-use-package-{position}")).await;
        let path = associated(&emulator, &domain, &package_id, row.case).await;
        let schema = (row.schema)(&path);
        created_index(&emulator, &domain, row.index, &schema, row.case).await;

        // The call is accepted SYNCHRONOUSLY and reports the association on its way off
        // the domain: whether the removal can happen is not this answer's to give.
        let answer = dissociate(&emulator, &domain, &package_id).await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected DissociatePackages to be accepted, got {} with {:?}",
            row.case, answer.status, answer.body
        );
        let details = &answer.body["DomainPackageDetailsList"][0];
        let status = member(details, "DomainPackageStatus", row.case);
        assert_eq!(
            status, DOMAIN_PACKAGE_STATUS_DISSOCIATING,
            "case {}: expected the association in its removal status, got {status:?}",
            row.case
        );
        assert!(
            details["ErrorDetails"].is_null(),
            "case {}: the removal has not failed yet, got {:?}",
            row.case,
            details["ErrorDetails"]
        );

        // Nothing below asks for the outcome: the clock owes it, and it is the FAILURE,
        // because an index of the domain still reads a file the package provides.
        tokio::time::sleep(SETTLE_WAIT).await;
        let entry = listed(&emulator, &domain, &package_id, row.case)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {}: expected the association to survive the failed removal",
                    row.case
                )
            });
        let status = member(&entry, "DomainPackageStatus", row.case);
        assert_eq!(
            status, DOMAIN_PACKAGE_STATUS_DISSOCIATION_FAILED,
            "case {}: expected the failed-removal status, got {status:?}",
            row.case
        );
        let message = member(&entry["ErrorDetails"], "ErrorMessage", row.case);
        assert_eq!(
            message, PACKAGE_IN_USE,
            "case {}: expected the service's in-use sentence, got {message:?}",
            row.case
        );
        let error_type = member(&entry["ErrorDetails"], "ErrorType", row.case);
        assert!(
            !error_type.is_empty(),
            "case {}: expected an ErrorType beside the message, got {error_type:?}",
            row.case
        );

        // The package is still on the domain, so the index that reads it still works.
        updated_index(&emulator, &domain, row.index, &plain_change(), row.case).await;

        // Removing the ONE dependency lifts the block, and the identical call then
        // takes the package off.
        deleted_index(&emulator, &domain, row.index, row.case).await;
        let read = get_index(&emulator, &domain, row.index).await;
        assert_eq!(
            read.status, RECORDED_NOT_FOUND_STATUS,
            "case {}: expected the deleted index to be gone, got {} with {:?}",
            row.case, read.status, read.body
        );
        assert_eq!(
            string_member(&read.body, "message", row.case),
            format!("Resource index {} does not exist", row.index),
            "case {}: expected the service's missing-index sentence, got {:?}",
            row.case,
            read.body
        );

        let again = dissociate(&emulator, &domain, &package_id).await;
        assert_eq!(
            again.status, 200,
            "case {}: expected the identical DissociatePackages to be accepted, got {} with {:?}",
            row.case, again.status, again.body
        );
        tokio::time::sleep(SETTLE_WAIT).await;
        let gone = listed_status(&emulator, &domain, &package_id, row.case).await;
        assert_eq!(
            gone, None,
            "case {}: expected the association to be gone once nothing used it, got {gone:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn only_the_package_an_index_resolves_is_kept_on_the_domain() {
    let emulator = Emulator::new();
    let domain = "in-use-one-of-two";
    let used = domain_and_package(&emulator, domain, "used-package").await;
    let unused = create_package(&emulator, "unused-package", PACKAGE_TYPE_TXT_DICTIONARY).await;
    tokio::time::sleep(SETTLE_WAIT).await;

    let path = associated(&emulator, domain, &used, "two packages").await;
    created_index(
        &emulator,
        domain,
        "one-dependency",
        &(use_rows()[0].schema)(&path),
        "two packages",
    )
    .await;
    // The second association is made after the first has landed, so neither call races
    // the other.
    associated(&emulator, domain, &unused, "two packages").await;

    // The package nothing reads comes off; the one an index reads does not.
    for (case, package_id, expected) in [
        ("the unused package", unused.as_str(), None),
        (
            "the used package",
            used.as_str(),
            Some(DOMAIN_PACKAGE_STATUS_DISSOCIATION_FAILED),
        ),
    ] {
        let answer = dissociate(&emulator, domain, package_id).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected DissociatePackages to be accepted, got {} with {:?}",
            answer.status, answer.body
        );
        tokio::time::sleep(SETTLE_WAIT).await;
        let status = listed_status(&emulator, domain, package_id, case).await;
        assert_eq!(
            status.as_deref(),
            expected,
            "case {case}: expected {expected:?} after the removal fell due, got {status:?}"
        );
    }
}

#[tokio::test]
async fn an_index_created_while_the_removal_is_in_flight_still_keeps_the_package_on_the_domain() {
    let emulator = Emulator::new();
    let domain = "in-use-mid-removal";
    let package_id = domain_and_package(&emulator, domain, "mid-removal-package").await;
    let path = associated(&emulator, domain, &package_id, "mid removal").await;

    // The package is on the domain and nothing reads it yet, so the removal this call
    // starts is the one the clock would otherwise perform.
    let answer = dissociate(&emulator, domain, &package_id).await;
    assert_eq!(
        answer.status, 200,
        "case mid removal: expected DissociatePackages to be accepted, got {} with {:?}",
        answer.status, answer.body
    );

    // The content is still on the nodes while the removal is in flight, so an index
    // naming it is served — and by the time the removal falls due that index depends on
    // the package, so the removal FAILS and the association stays.
    created_index(
        &emulator,
        domain,
        "late-dependency",
        &(use_rows()[0].schema)(&path),
        "mid removal",
    )
    .await;
    tokio::time::sleep(SETTLE_WAIT).await;
    let entry = listed(&emulator, domain, &package_id, "mid removal")
        .await
        .expect("case mid removal: expected the association to survive the failed removal");
    let status = member(&entry, "DomainPackageStatus", "mid removal");
    assert_eq!(
        status, DOMAIN_PACKAGE_STATUS_DISSOCIATION_FAILED,
        "case mid removal: expected the failed-removal status, got {status:?}"
    );
    let message = member(&entry["ErrorDetails"], "ErrorMessage", "mid removal");
    assert_eq!(
        message, PACKAGE_IN_USE,
        "case mid removal: expected the service's in-use sentence, got {message:?}"
    );
    // The index the window admitted resolves a package that is still there.
    updated_index(&emulator, domain, "late-dependency", &plain_change(), "mid removal").await;
}

/// How a package can fail to be in place on the domain when a schema names one of its
/// files.
#[derive(Clone, Copy)]
enum NotInPlace {
    /// Associated and then dissociated, with the removal already fallen due.
    Dissociated,
    /// Never associated with this domain at all.
    NeverAssociated,
    /// Being associated right now, so its content is not on the nodes yet.
    StillAssociating,
}

#[tokio::test]
async fn a_schema_naming_a_file_no_package_in_place_provides_is_refused_and_served_once_it_is() {
    let emulator = Emulator::new();

    let rows = use_rows();
    let ways = [
        ("a dissociated package", NotInPlace::Dissociated),
        ("a package never associated", NotInPlace::NeverAssociated),
        ("a package still being associated", NotInPlace::StillAssociating),
    ];
    for (position, row) in rows.iter().enumerate() {
        // Each row runs on its own way of not being in place, and every way is covered
        // by the rows, so neither dimension is pinned to one value.
        let (way_case, way) = ways[position % ways.len()];
        let case = format!("{} / {way_case}", row.case);
        let domain = format!("unresolved-{position}");
        let package_id =
            domain_and_package(&emulator, &domain, &format!("unresolved-package-{position}")).await;

        let path = match way {
            NotInPlace::Dissociated => {
                let path = associated(&emulator, &domain, &package_id, &case).await;
                let answer = dissociate(&emulator, &domain, &package_id).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected DissociatePackages to be accepted, got {} with {:?}",
                    answer.status, answer.body
                );
                tokio::time::sleep(SETTLE_WAIT).await;
                let gone = listed_status(&emulator, &domain, &package_id, &case).await;
                assert_eq!(
                    gone, None,
                    "case {case}: expected the package off the domain before the create, got \
                     {gone:?}"
                );
                path
            }
            NotInPlace::NeverAssociated => reference_path(&emulator, &package_id, &case),
            NotInPlace::StillAssociating => {
                let answer = associate(&emulator, &domain, &package_id).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected AssociatePackages to be accepted, got {} with {:?}",
                    answer.status, answer.body
                );
                let details = &answer.body["DomainPackageDetailsList"][0];
                let status = member(details, "DomainPackageStatus", &case);
                assert_eq!(
                    status, DOMAIN_PACKAGE_STATUS_ASSOCIATING,
                    "case {case}: expected the association still being made, got {status:?}"
                );
                member(details, "ReferencePath", &case)
            }
        };

        let schema = (row.schema)(&path);
        let refused = create_index(
            &emulator,
            &domain,
            json!({ "IndexName": row.index, "IndexSchema": schema }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected 400 for a file no package provides, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            refused.body
        );
        assert_eq!(
            string_member(&refused.body, "message", &case),
            row.message,
            "case {case}: expected the engine's unreadable-file sentence, got {:?}",
            refused.body
        );

        // A refused create mints NOTHING: the name still names no index of the domain.
        let read = get_index(&emulator, &domain, row.index).await;
        assert_eq!(
            read.status, RECORDED_NOT_FOUND_STATUS,
            "case {case}: a refused create must mint nothing, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            string_member(&read.body, "message", &case),
            format!("Resource index {} does not exist", row.index),
            "case {case}: expected the service's missing-index sentence, got {:?}",
            read.body
        );

        // With the SAME package in place, the IDENTICAL create is served and the schema
        // reads back still naming the package.
        if matches!(way, NotInPlace::StillAssociating) {
            tokio::time::sleep(SETTLE_WAIT).await;
        } else {
            associated(&emulator, &domain, &package_id, &case).await;
        }
        created_index(&emulator, &domain, row.index, &schema, &case).await;
        let stored = read_schema(&emulator, &domain, row.index, &case).await;
        assert_eq!(
            stored, schema,
            "case {case}: expected the schema the create carried, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn an_update_naming_a_file_no_package_in_place_provides_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let domain = "unresolved-update";
    let package_id = domain_and_package(&emulator, domain, "unresolved-update-package").await;
    let path = reference_path(&emulator, &package_id, "update");
    let created = plain_change();
    created_index(&emulator, domain, "updated-index", &created, "update").await;

    let row = &use_rows()[0];
    let schema = (row.schema)(&path);
    let refused = update_index(&emulator, domain, "updated-index", &schema).await;
    assert_eq!(
        refused.status, 400,
        "case update: expected 400 for a file no package provides, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case update: expected ValidationException, got {:?}",
        refused.body
    );
    assert_eq!(
        string_member(&refused.body, "message", "update"),
        row.message,
        "case update: expected the engine's unreadable-file sentence, got {:?}",
        refused.body
    );
    let stored = read_schema(&emulator, domain, "updated-index", "update").await;
    assert_eq!(
        stored, created,
        "case update: a refused update must change nothing, got {stored:?}"
    );

    // The identical update lands once the package is in place.
    associated(&emulator, domain, &package_id, "update").await;
    updated_index(&emulator, domain, "updated-index", &schema, "update").await;
}

#[tokio::test]
async fn a_path_that_names_no_packages_content_is_not_a_package_reference() {
    let emulator = Emulator::new();
    let domain = "not-a-reference";
    let package_id = domain_and_package(&emulator, domain, "not-a-reference-package").await;
    let path = reference_path(&emulator, &package_id, "not a reference");

    // Each row LOOKS like a package reference and is not one, so no association is
    // needed for it to resolve: a path outside the directory packages are mounted
    // under, that directory with no package named, and a path setting outside the
    // analysis settings entirely.
    let rows = [
        (
            "a path outside the package directory",
            "own-synonyms",
            json!({
                "settings": {
                    "analysis": {
                        "filter": {
                            "syn": { "type": "synonym", "synonyms_path": "custom/synonyms.txt" },
                        },
                    },
                },
            }),
        ),
        (
            "the package directory with no package named",
            "bare-directory",
            json!({
                "settings": {
                    "analysis": {
                        "filter": { "syn": { "type": "synonym", "synonyms_path": "analyzers/" } },
                    },
                },
            }),
        ),
        (
            "a path member outside the analysis settings",
            "mapped-path",
            json!({
                "mappings": {
                    "properties": { "source_path": { "type": "keyword" } },
                },
                "settings": { "index": { "lifecycle_path": path.clone() } },
            }),
        ),
    ];
    for (case, index, schema) in rows {
        created_index(&emulator, domain, index, &schema, case).await;
        let stored = read_schema(&emulator, domain, index, case).await;
        assert_eq!(
            stored, schema,
            "case {case}: expected the schema the create carried, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn a_second_change_to_the_domains_associations_is_refused_for_the_whole_span_the_first_takes()
{
    let emulator = Emulator::new();

    // The delay between the call that starts a change and the second writer: points
    // inside the span the change takes, and one past it. A writer released by a barrier,
    // by a second client's first request, or by a poll pays one of these. Three are
    // taken FROM the span, so the margin left for the round trips scales with it; the
    // absolute one pins the span's LENGTH, so shortening it back fails this case.
    let delays = [
        ("at once", Duration::from_millis(0), true),
        ("a third of the way in", PACKAGE_ASSOCIATION_CHANGE / 3, true),
        ("half way in", PACKAGE_ASSOCIATION_CHANGE / 2, true),
        ("600 ms in", Duration::from_millis(600), true),
        ("past the span", SETTLE_WAIT, false),
    ];
    for (position, (when, delay, refused)) in delays.into_iter().enumerate() {
        // Both second writers the service serializes: another associate of the same
        // pair, and the removal of it.
        for (form, associates) in [("associate", true), ("dissociate", false)] {
            let case = format!("{form} {when}");
            let domain = format!("race-{position}-{form}");
            let package_id = domain_and_package(
                &emulator,
                &domain,
                &format!("race-package-{position}-{form}"),
            )
            .await;
            associated(&emulator, &domain, &package_id, &case).await;

            // The change this call starts has not landed when the second writer arrives.
            let first = associate(&emulator, &domain, &package_id).await;
            assert_eq!(
                first.status, 200,
                "case {case}: expected the first call to be accepted, got {} with {:?}",
                first.status, first.body
            );
            tokio::time::sleep(delay).await;
            let second = if associates {
                associate(&emulator, &domain, &package_id).await
            } else {
                dissociate(&emulator, &domain, &package_id).await
            };

            if refused {
                assert_eq!(
                    second.status, 409,
                    "case {case}: expected 409 while the first change is in flight, got {} with \
                     {:?}",
                    second.status, second.body
                );
                assert_eq!(
                    error_code(&second),
                    "ConflictException",
                    "case {case}: expected ConflictException, got {:?}",
                    second.body
                );
            } else {
                assert_eq!(
                    second.status, 200,
                    "case {case}: expected the second call to be served once the first had \
                     landed, got {} with {:?}",
                    second.status, second.body
                );
            }

            // Whatever happened, the pair settles at exactly ONE record — in place when
            // no removal was served, and gone when one was.
            tokio::time::sleep(SETTLE_WAIT).await;
            let removed = !refused && !associates;
            let expected = if removed {
                None
            } else {
                Some(DOMAIN_PACKAGE_STATUS_ACTIVE)
            };
            let status = stored_status(&emulator, &domain, &package_id);
            assert_eq!(
                status.as_deref(),
                expected,
                "case {case}: expected {expected:?} once everything had landed, got {status:?}"
            );

            // And a solo removal afterwards both succeeds and takes the package off.
            if !removed {
                let solo = dissociate(&emulator, &domain, &package_id).await;
                assert_eq!(
                    solo.status, 200,
                    "case {case}: expected the solo removal to be served, got {} with {:?}",
                    solo.status, solo.body
                );
                tokio::time::sleep(SETTLE_WAIT).await;
                let gone = listed_status(&emulator, &domain, &package_id, &case).await;
                assert_eq!(
                    gone, None,
                    "case {case}: expected the solo removal to take the package off, got {gone:?}"
                );
            }
        }
    }
}
