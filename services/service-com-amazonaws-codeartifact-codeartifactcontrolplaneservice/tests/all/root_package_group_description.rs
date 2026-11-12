// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for the `description` of the implicit ROOT package group.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees.
//!
//! The rule under test: a package group the SERVICE creates is created with the metadata
//! the service assigns it, so the root group `/*` every domain is created with carries the
//! fixed description "Default root package group" — the same text in every domain, with no
//! part of the request, the account or the clock in it — while a group a CALLER creates
//! carries only the metadata that caller sent. `description` is a member of both
//! `PackageGroupDescription` and `PackageGroupSummary`
//! (`smithy_query(query="shape", shape="PackageGroupSummary")`), so EVERY projection of the
//! root group's record reports it.
//!
//! Asserted over a table rather than one recorded example: the rows differ along the domain
//! (its name at the modelled extremes), how many groups the domain holds and at what depths,
//! whether those groups carry descriptions of their own, which projection reads the root
//! group (a single read, a whole listing, a prefix-narrowed listing, a page of a paginated
//! listing, an association fallback) and how the root entry is ADDRESSED in a listing (by
//! pattern and by ARN). It is stored state, not a constant in the projection: an
//! `UpdatePackageGroup` of the root group replaces the description everywhere, and only in
//! the domain updated.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_domain, emulator, package_group_arn, Answer};
use crate::package_group_hierarchy::{create_package_group, describe_package_group, ROOT};

/// The description CodeArtifact itself assigns to the root package group of every domain.
const ROOT_DESCRIPTION: &str = "Default root package group";

/// `ListPackageGroups`: every input member is `httpQuery`-bound on
/// `POST /v1/package-groups`.
async fn list_package_groups(
    router: &MultiServiceRouter,
    domain: &str,
    prefix: Option<&str>,
    max_results: Option<u32>,
    next_token: Option<&str>,
) -> Answer {
    let mut uri = format!("/v1/package-groups?domain={domain}");
    if let Some(prefix) = prefix {
        uri.push_str(&format!("&prefix={prefix}"));
    }
    if let Some(max_results) = max_results {
        uri.push_str(&format!("&max-results={max_results}"));
    }
    if let Some(next_token) = next_token {
        uri.push_str(&format!("&next-token={next_token}"));
    }
    call(router, "POST", &uri, None).await
}

/// `GetAssociatedPackageGroup`: every input is `httpQuery`-bound. A package no caller-created
/// pattern covers resolves to the root group, which is how a read of the root group's record
/// is reached without naming it.
async fn get_associated_package_group(
    router: &MultiServiceRouter,
    domain: &str,
    format: &str,
    package: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/get-associated-package-group?domain={domain}&format={format}&package={package}"),
        None,
    )
    .await
}

/// `UpdatePackageGroup`: the domain is `httpQuery`-bound, the pattern and the metadata travel
/// in the BODY.
async fn update_package_group(
    router: &MultiServiceRouter,
    domain: &str,
    pattern: &str,
    description: &str,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/v1/package-group?domain={domain}"),
        Some(json!({ "packageGroup": pattern, "description": description })),
    )
    .await
}

/// The `packageGroups` of a successful listing.
fn listed<'a>(label: &str, answer: &'a Answer) -> &'a Vec<Value> {
    answer
        .body
        .get("packageGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {label:?}: no packageGroups in {answer:?}"))
}

/// The listing entry whose `pattern` is `pattern`.
fn entry_by_pattern<'a>(label: &str, answer: &'a Answer, pattern: &str) -> &'a Value {
    listed(label, answer)
        .iter()
        .find(|group| group.get("pattern").and_then(Value::as_str) == Some(pattern))
        .unwrap_or_else(|| panic!("case {label:?}: no entry for {pattern} in {answer:?}"))
}

/// The listing entry whose `arn` is `arn` — the root group as the recorded lifecycle client
/// addresses it, by the ARN carrying its percent-encoded pattern.
fn entry_by_arn<'a>(label: &str, answer: &'a Answer, arn: &str) -> &'a Value {
    listed(label, answer)
        .iter()
        .find(|group| group.get("arn").and_then(Value::as_str) == Some(arn))
        .unwrap_or_else(|| panic!("case {label:?}: no entry with arn {arn} in {answer:?}"))
}

/// The `description` of a package-group object, or `None` when the member is absent.
fn description(group: &Value) -> Option<&str> {
    group.get("description").and_then(Value::as_str)
}

/// Collect every entry of a listing walked at a page size of `max_results`, so the root group
/// is read out of a PAGE rather than a single complete answer.
async fn walk(
    router: &MultiServiceRouter,
    domain: &str,
    max_results: u32,
) -> Vec<Value> {
    let label = format!("walk {domain} at {max_results}");
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let answer = list_package_groups(router, domain, None, Some(max_results), token.as_deref())
            .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: page {pages} expected 200, got {answer:?}"
        );
        collected.extend(listed(&label, &answer).iter().cloned());
        pages += 1;
        match answer
            .body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            Some(next) => token = Some(next),
            None => break,
        }
        assert!(
            pages < 100,
            "case {label:?}: the walk did not terminate after {pages} pages"
        );
    }
    collected
}

/// One group a caller creates for a row of the table, with the description it is created
/// with (`None` is "no description member sent").
struct Held {
    pattern: &'static str,
    description: Option<&'static str>,
}

/// One row of the table: a domain, the groups a caller creates in it, and a package of a
/// format none of those groups covers (so its association falls back to the root group).
struct DomainCase {
    label: &'static str,
    domain: &'static str,
    held: &'static [Held],
    /// A `(format, package)` no pattern in `held` covers.
    unmatched: (&'static str, &'static str),
}

/// The table. Rows differ along the domain NAME (the modelled `DomainName` extremes, length
/// 2..50), how many groups the domain holds (none, one, several), the DEPTH of those groups,
/// and whether they carry descriptions of their own.
const DOMAIN_CASES: &[DomainCase] = &[
    DomainCase {
        label: "a fresh domain holds only the root group",
        domain: "rootdesc-fresh",
        held: &[],
        unmatched: ("npm", "left-pad"),
    },
    DomainCase {
        label: "the shortest domain name the model allows",
        domain: "ab",
        held: &[],
        unmatched: ("cargo", "serde"),
    },
    DomainCase {
        label: "one caller-created group, itself undescribed",
        domain: "rootdesc-one",
        held: &[Held {
            pattern: "/npm/*",
            description: None,
        }],
        unmatched: ("pypi", "requests"),
    },
    DomainCase {
        label: "several groups at several depths, some described by their creator",
        domain: "rootdesc-populated",
        held: &[
            Held {
                pattern: "/npm/*",
                description: Some("every npm package"),
            },
            Held {
                pattern: "/npm/@scope/*",
                description: None,
            },
            Held {
                pattern: "/npm/@scope/package$",
                description: Some(""),
            },
            Held {
                pattern: "/maven/com.example/*",
                description: Some("the example artifacts"),
            },
            Held {
                pattern: "/pypi/requests$",
                description: Some("one python package"),
            },
        ],
        unmatched: ("cargo", "serde"),
    },
];

/// The rule, over the whole table and over every projection of the root group's record.
#[tokio::test]
async fn the_root_package_group_carries_the_service_assigned_description() {
    let router = emulator();

    for case in DOMAIN_CASES {
        let label = case.label;
        let domain = case.domain;
        let created_domain = create_domain(&router, domain, None).await;
        assert_eq!(
            created_domain.status, 200,
            "case {label:?}: domain setup expected 200, got {created_domain:?}"
        );
        for held in case.held {
            let created =
                create_package_group(&router, domain, held.pattern, None, held.description).await;
            assert_eq!(
                created.status, 200,
                "case {label:?}: setup {} expected 200, got {created:?}",
                held.pattern
            );
        }

        // Projection 1: a read of the root group by pattern.
        let described = describe_package_group(&router, domain, ROOT).await;
        assert_eq!(
            described.status, 200,
            "case {label:?}: the root group must be readable, got {described:?}"
        );
        assert_eq!(
            description(described.group()),
            Some(ROOT_DESCRIPTION),
            "case {label:?}: DescribePackageGroup of the root group expected description {ROOT_DESCRIPTION:?}, got {:?}",
            described.group()
        );

        // Projection 2: the whole listing, with the root entry addressed BOTH by its pattern
        // and by its ARN (the percent-encoded pattern), and projection 3: the same listing
        // narrowed by a prefix every pattern starts with.
        let root_arn = package_group_arn(domain, ROOT);
        for (projection, prefix) in [("whole listing", None), ("prefix-narrowed", Some("/"))] {
            let listing = list_package_groups(&router, domain, prefix, None, None).await;
            assert_eq!(
                listing.status, 200,
                "case {label:?}/{projection}: expected 200, got {listing:?}"
            );
            let by_pattern = entry_by_pattern(label, &listing, ROOT);
            assert_eq!(
                description(by_pattern),
                Some(ROOT_DESCRIPTION),
                "case {label:?}/{projection}: the root entry expected description {ROOT_DESCRIPTION:?}, got {by_pattern:?}"
            );
            let by_arn = entry_by_arn(label, &listing, &root_arn);
            assert_eq!(
                by_arn, by_pattern,
                "case {label:?}/{projection}: the entry at {root_arn} must be the root group's own entry"
            );

            // The service's description belongs to the ROOT group alone: a caller-created
            // group reports what its creator sent, and nothing when the creator sent none.
            for held in case.held {
                let entry = entry_by_pattern(label, &listing, held.pattern);
                assert_eq!(
                    description(entry),
                    held.description,
                    "case {label:?}/{projection}: {} expected the description its creator sent, got {entry:?}",
                    held.pattern
                );
            }
        }

        // Projection 4: a page of a paginated listing — one entry per page, so the root group
        // is read out of a page of its own rather than a complete answer.
        let walked = walk(&router, domain, 1).await;
        let paged = walked
            .iter()
            .find(|group| group.get("pattern").and_then(Value::as_str) == Some(ROOT))
            .unwrap_or_else(|| panic!("case {label:?}: no root entry in the paged walk {walked:?}"));
        assert_eq!(
            description(paged),
            Some(ROOT_DESCRIPTION),
            "case {label:?}: the paged root entry expected description {ROOT_DESCRIPTION:?}, got {paged:?}"
        );

        // Projection 5: the association of a package no caller-created pattern covers, which
        // resolves to the root group.
        let (format, package) = case.unmatched;
        let associated = get_associated_package_group(&router, domain, format, package).await;
        assert_eq!(
            associated.status, 200,
            "case {label:?}: expected 200 associating {format}/{package}, got {associated:?}"
        );
        assert_eq!(
            associated.group().get("pattern").and_then(Value::as_str),
            Some(ROOT),
            "case {label:?}: {format}/{package} must fall back to the root group, got {associated:?}"
        );
        assert_eq!(
            description(associated.group()),
            Some(ROOT_DESCRIPTION),
            "case {label:?}: the associated root group expected description {ROOT_DESCRIPTION:?}, got {:?}",
            associated.group()
        );
    }
}

/// The description is the root group's STORED record, not a constant the projection writes:
/// an `UpdatePackageGroup` of the root group replaces it in every projection, and only in the
/// domain that was updated.
#[tokio::test]
async fn updating_the_root_group_replaces_the_service_assigned_description() {
    let router = emulator();
    let updated_domain = "rootdesc-updated";
    let untouched_domain = "rootdesc-untouched";
    for domain in [updated_domain, untouched_domain] {
        assert_eq!(
            create_domain(&router, domain, None).await.status,
            200,
            "setup: domain {domain} must be created"
        );
    }

    // Rows differ along the replacement value: ordinary text, the modelled zero-length
    // description, the longest the model allows (`Description` is length 0..1000,
    // `smithy_query(query="shape", shape="Description")`), and the service's own text sent
    // back by a client that round-tripped a listing.
    let replacements = [
        ("ordinary text", "the whole domain, ours".to_string()),
        ("the modelled zero-length description", String::new()),
        ("the modelled maximum length", "d".repeat(1000)),
        ("the service's own text, round-tripped", ROOT_DESCRIPTION.to_string()),
    ];

    for (label, replacement) in replacements {
        let updated = update_package_group(&router, updated_domain, ROOT, &replacement).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected 200, got {updated:?}"
        );
        assert_eq!(
            description(updated.group()),
            Some(replacement.as_str()),
            "case {label:?}: the update must answer with the description it stored, got {:?}",
            updated.group()
        );

        let described = describe_package_group(&router, updated_domain, ROOT).await;
        assert_eq!(
            description(described.group()),
            Some(replacement.as_str()),
            "case {label:?}: a read of the root group must answer the stored description, got {:?}",
            described.group()
        );

        let listing = list_package_groups(&router, updated_domain, None, None, None).await;
        let entry = entry_by_pattern(label, &listing, ROOT);
        assert_eq!(
            description(entry),
            Some(replacement.as_str()),
            "case {label:?}: the listing must answer the stored description, got {entry:?}"
        );

        // Another domain's root group is its own record: untouched by this update.
        let other = describe_package_group(&router, untouched_domain, ROOT).await;
        assert_eq!(
            description(other.group()),
            Some(ROOT_DESCRIPTION),
            "case {label:?}: another domain's root group keeps the service's description, got {:?}",
            other.group()
        );
    }
}
