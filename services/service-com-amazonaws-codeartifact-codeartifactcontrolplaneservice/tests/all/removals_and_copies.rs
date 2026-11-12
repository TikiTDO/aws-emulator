// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for the CodeArtifact removals and the copy, each read back
//! through the operation a client verifies it with: `UntagResource`,
//! `DeletePackageGroup`, `DeletePackageVersions` and `CopyPackageVersions`.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees — status, error code and body — never a handler's return
//! value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! example:
//!
//! 1. An accepted removal is COMMITTED: what it removed is gone from the store, so the
//!    read a client verifies it with no longer reports it, and the identifier it freed can
//!    be used again.
//!    * `UntagResource` deletes the named keys, and `ListTagsForResource` then reports the
//!      remaining set — on every taggable kind, and with the keys it did not name keeping
//!      their values and their order.
//!    * `DeletePackageGroup` removes the group, so `DescribePackageGroup` for its pattern
//!      is refused `ResourceNotFoundException` (404) and the pattern can be created again;
//!      the delete itself is `@idempotent`, so repeating it answers 200 describing nothing.
//!    * `DeletePackageVersions` removes the named versions, so `DescribePackageVersion` is
//!      refused for each; the PACKAGE survives even when its last version goes, because
//!      deleting every version of a package is not deleting the package (that is
//!      `DeletePackage`), so `DescribePackage` still answers for it.
//! 2. A removal removes ONLY what it names: the tags, groups, versions and packages the
//!    request did not name read back exactly as before.
//! 3. A per-version answer reports each named version in exactly one of the two modelled
//!    maps: `successfulVersions` with the revision it had and the status the operation
//!    documents, or `failedVersions` with the `PackageVersionErrorCode` for why — and a
//!    version reported as failed is left exactly as it was.
//! 4. `CopyPackageVersions` materializes the copied versions in the DESTINATION: a later
//!    `DescribePackageVersion` against the destination answers the same description the
//!    source answers, provenance included, while the source keeps the version.
//! 5. A removal or copy that cannot be served names the resource it could not reach
//!    (404), and a request that cannot hold together is refused (400) — and neither
//!    changes anything.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use sha2::Digest as _;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{auth_for, call, create_domain, emulator, Answer, ACCOUNT, REGION};
use crate::package_group_hierarchy::{create_package_group, describe_package_group, ROOT};
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::state::MAX_VERSIONS_PER_COPY_REQUEST;

/// The `PackageVersionStatus` a published version carries, and the one every copy of it
/// carries too. From the model: `PackageVersionStatus` = [Published, Unfinished, Unlisted,
/// Archived, Disposed, Deleted].
const PUBLISHED: &str = "Published";

/// The status `DeletePackageVersions` reports for a version it deleted ("The status of
/// every successful version will be Deleted" —
/// `DeletePackageVersionsResult$successfulVersions`).
const DELETED: &str = "Deleted";

/// Percent-encode one query value the way an SDK does, so an ARN's colons and slashes
/// survive the server's decode (the same maintained encoder the tagging suite uses).
fn encode_query(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// A modelled `TagList` body member from key/value pairs, in the order given.
fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": value }))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// The requests this suite sends. Each is the modelled route of one operation,
// in one place, so a row differs only in the values it carries.
// ---------------------------------------------------------------------------

/// `CreateRepository` in `domain`, carrying create-time `tags`.
async fn create_repository(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    tags: &[(&str, &str)],
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/repository?domain={domain}&repository={repository}"),
        Some(json!({ "tags": tag_list(tags) })),
    )
    .await
}

/// `TagResource`: the batch travels in the body, the ARN in the query.
async fn tag(router: &MultiServiceRouter, arn: &str, batch: &[(&str, &str)]) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/tag?resourceArn={}", encode_query(arn)),
        Some(json!({ "tags": tag_list(batch) })),
    )
    .await
}

/// `UntagResource`: the keys travel in the body, the ARN in the query.
async fn untag(router: &MultiServiceRouter, arn: &str, keys: &[&str]) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/untag?resourceArn={}", encode_query(arn)),
        Some(json!({ "tagKeys": keys })),
    )
    .await
}

/// The tags `arn` holds, read back through `ListTagsForResource` IN ORDER — a tag's
/// position is part of what an untag must not disturb.
async fn held_tags(router: &MultiServiceRouter, arn: &str, case: &str) -> Vec<(String, String)> {
    let answer = call(
        router,
        "POST",
        &format!("/v1/tags?resourceArn={}", encode_query(arn)),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (200, String::new()),
        "case {case:?}: ListTagsForResource must succeed, got {answer:?}"
    );
    answer.body["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags list, got {answer:?}"))
        .iter()
        .map(|entry| {
            (
                entry["key"].as_str().unwrap_or_default().to_string(),
                entry["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// `DeletePackageGroup`: both the domain and the pattern are `httpQuery`-bound.
async fn delete_package_group(
    router: &MultiServiceRouter,
    domain: &str,
    pattern: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/package-group?domain={domain}&package-group={pattern}"),
        None,
    )
    .await
}

/// The namespace every [`Pkg`] carries unless a row names another.
///
/// A generic package version's identity is `<namespace>/<package>/<version>`, so
/// `PublishPackageVersion` requires `namespace` of the `generic` format even though the
/// model marks the member optional for the formats that derive it.
const PKG_NAMESPACE: &str = "team-core";

/// One generic package, addressed the way every package-scoped operation addresses it: by
/// the repository it is in and its own `(format, namespace, name)` triple.
#[derive(Debug, Clone)]
struct Pkg {
    domain: String,
    repository: String,
    package: String,
    namespace: Option<String>,
}

impl Pkg {
    fn new(domain: &str, repository: &str, package: &str) -> Self {
        Self {
            domain: domain.to_string(),
            repository: repository.to_string(),
            package: package.to_string(),
            namespace: Some(PKG_NAMESPACE.to_string()),
        }
    }

    /// The same package, in another repository of the same domain.
    fn in_repository(&self, repository: &str) -> Self {
        Self {
            repository: repository.to_string(),
            ..self.clone()
        }
    }

    fn namespaced(mut self, namespace: &str) -> Self {
        self.namespace = Some(namespace.to_string());
        self
    }

    /// The query members every package-scoped route shares.
    fn query(&self) -> String {
        let mut query = format!(
            "domain={}&repository={}&format=generic&package={}",
            self.domain, self.repository, self.package
        );
        if let Some(namespace) = &self.namespace {
            query.push_str(&format!("&namespace={namespace}"));
        }
        query
    }

    /// The package coordinates a copy carries, with the two repositories named instead of
    /// one.
    fn copy_query(&self, destination: &str) -> String {
        let mut query = format!(
            "domain={}&source-repository={}&destination-repository={destination}&format=generic&package={}",
            self.domain, self.repository, self.package
        );
        if let Some(namespace) = &self.namespace {
            query.push_str(&format!("&namespace={namespace}"));
        }
        query
    }
}

/// `PublishPackageVersion`: the asset bytes are the HTTP payload and the declared digest
/// rides the `x-amz-content-sha256` header the model binds `assetSHA256` to, so this one
/// request cannot go through the suite's JSON [`call`].
async fn publish(
    router: &MultiServiceRouter,
    pkg: &Pkg,
    version: &str,
    asset: &str,
    content: &[u8],
) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "/v1/package/version/publish?{}&version={version}&asset={asset}",
            pkg.query()
        ))
        .header("host", format!("codeartifact.{REGION}.amazonaws.com"))
        .header("authorization", auth_for(REGION))
        .header("content-type", "application/octet-stream")
        .header("x-amz-content-sha256", hex::encode(sha2::Sha256::digest(content)))
        .body(bytes::Bytes::copy_from_slice(content))
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| json!({ "unparsed": String::from_utf8_lossy(&raw) }))
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// `DescribePackage` for the package a [`Pkg`] addresses.
async fn describe_package(router: &MultiServiceRouter, pkg: &Pkg) -> Answer {
    call(router, "GET", &format!("/v1/package?{}", pkg.query()), None).await
}

/// `DescribePackageVersion` for one version of the package a [`Pkg`] addresses.
async fn describe_version(router: &MultiServiceRouter, pkg: &Pkg, version: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/package/version?{}&version={version}", pkg.query()),
        None,
    )
    .await
}

/// `DeletePackageVersions`: the version list and the optional `expectedStatus` travel in
/// the body.
async fn delete_versions(
    router: &MultiServiceRouter,
    pkg: &Pkg,
    versions: &[&str],
    expected_status: Option<&str>,
) -> Answer {
    let mut body = json!({ "versions": versions });
    if let Some(expected) = expected_status {
        body["expectedStatus"] = json!(expected);
    }
    call(
        router,
        "POST",
        &format!("/v1/package/versions/delete?{}", pkg.query()),
        Some(body),
    )
    .await
}

/// `CopyPackageVersions` from the repository a [`Pkg`] names into `destination`, with the
/// caller's own body (which is what names the versions).
async fn copy_versions(
    router: &MultiServiceRouter,
    pkg: &Pkg,
    destination: &str,
    body: Value,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/package/versions/copy?{}", pkg.copy_query(destination)),
        Some(body),
    )
    .await
}

// ---------------------------------------------------------------------------
// Reading a per-version answer.
// ---------------------------------------------------------------------------

/// The versions a `successfulVersions` map reports, with the status reported for each.
fn successful(answer: &Answer) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = answer
        .body
        .get("successfulVersions")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(version, info)| {
                    assert!(
                        info.get("revision").and_then(Value::as_str).is_some(),
                        "a successful version must report the revision it had, got {info:?}"
                    );
                    (
                        version.clone(),
                        info.get("status")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    rows.sort();
    rows
}

/// The versions a `failedVersions` map reports, with the error code reported for each.
fn failed(answer: &Answer) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = answer
        .body
        .get("failedVersions")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(version, error)| {
                    (
                        version.clone(),
                        error
                            .get("errorCode")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    rows.sort();
    rows
}

/// `(version, status)` rows from `&str` literals, sorted the way [`successful`] reports.
fn rows(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = pairs
        .iter()
        .map(|(version, value)| ((*version).to_string(), (*value).to_string()))
        .collect();
    rows.sort();
    rows
}

/// Content of `size` bytes that is not text, so a row that mangled a payload cannot pass
/// by accident.
fn binary_content(size: usize, seed: u8) -> Vec<u8> {
    (0..size)
        .map(|index| (index as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

// ---------------------------------------------------------------------------
// RULES 1 and 2 for tags: UntagResource deletes the keys it names, and only those.
// ---------------------------------------------------------------------------

/// One untag row: what the resource holds, the keys the request names, and what must be
/// left — in ORDER, because the keys a request does not name keep their positions.
struct UntagCase {
    label: &'static str,
    held: Vec<(&'static str, &'static str)>,
    remove: Vec<&'static str>,
    left: Vec<(&'static str, &'static str)>,
}

/// RULE 1 and 2 on the tag surface, over every taggable kind this store holds.
///
/// Rows differ along what the rule generalises over: how many tags are held (1, 2, 3), how
/// many keys the request names (0, 1, 2, every one), whether a named key is held at all,
/// WHICH held position is removed (first, middle, last) and the form of a key (ASCII,
/// multi-byte, a key that is a prefix of another). Every row is sent TWICE: an untag is
/// idempotent, so the second call must leave the same set.
#[tokio::test]
async fn untag_resource_deletes_the_named_keys_on_every_taggable_kind() {
    let cases = vec![
        UntagCase {
            label: "the only key held",
            held: vec![("solo", "1")],
            remove: vec!["solo"],
            left: vec![],
        },
        UntagCase {
            label: "the first of three",
            held: vec![("alpha", "1"), ("beta", "2"), ("gamma", "3")],
            remove: vec!["alpha"],
            left: vec![("beta", "2"), ("gamma", "3")],
        },
        UntagCase {
            label: "the middle of three",
            held: vec![("alpha", "1"), ("beta", "2"), ("gamma", "3")],
            remove: vec!["beta"],
            left: vec![("alpha", "1"), ("gamma", "3")],
        },
        UntagCase {
            label: "the last of three",
            held: vec![("alpha", "1"), ("beta", "2"), ("gamma", "3")],
            remove: vec!["gamma"],
            left: vec![("alpha", "1"), ("beta", "2")],
        },
        UntagCase {
            label: "two of three",
            held: vec![("alpha", "1"), ("beta", "2"), ("gamma", "3")],
            remove: vec!["gamma", "alpha"],
            left: vec![("beta", "2")],
        },
        UntagCase {
            label: "every key held",
            held: vec![("alpha", "1"), ("beta", "2")],
            remove: vec!["alpha", "beta"],
            left: vec![],
        },
        UntagCase {
            label: "a key that is not held removes nothing",
            held: vec![("alpha", "1")],
            remove: vec!["absent"],
            left: vec![("alpha", "1")],
        },
        UntagCase {
            label: "an empty key list removes nothing",
            held: vec![("alpha", "1"), ("beta", "2")],
            remove: vec![],
            left: vec![("alpha", "1"), ("beta", "2")],
        },
        UntagCase {
            label: "a key is matched whole, not as a prefix",
            held: vec![("env", "prod"), ("environment", "staging")],
            remove: vec!["env"],
            left: vec![("environment", "staging")],
        },
        UntagCase {
            label: "a multi-byte key",
            held: vec![("ключ", "значение"), ("ascii", "1")],
            remove: vec!["ключ"],
            left: vec![("ascii", "1")],
        },
        UntagCase {
            label: "a key whose value is empty",
            held: vec![("empty", ""), ("kept", "2")],
            remove: vec!["empty"],
            left: vec![("kept", "2")],
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        // One domain, one repository and one package group per row, each holding the row's
        // tags: the rule is about a resource's tag set, and this store keeps one per
        // record whatever kind the record is.
        let router = emulator();
        let domain = format!("untag-{index}");
        let created = call(
            &router,
            "POST",
            &format!("/v1/domain?domain={domain}"),
            Some(json!({ "tags": tag_list(&case.held) })),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateDomain must succeed, got {created:?}",
            case.label
        );
        let repository = create_repository(&router, &domain, "repo", &case.held).await;
        assert_eq!(
            repository.status, 200,
            "case {:?}: CreateRepository must succeed, got {repository:?}",
            case.label
        );
        let group = call(
            &router,
            "POST",
            &format!("/v1/package-group?domain={domain}"),
            Some(json!({ "packageGroup": "/npm/*", "tags": tag_list(&case.held) })),
        )
        .await;
        assert_eq!(
            group.status, 200,
            "case {:?}: CreatePackageGroup must succeed, got {group:?}",
            case.label
        );

        let kinds = [
            (
                "domain",
                format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:domain/{domain}"),
            ),
            (
                "repository",
                format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:repository/{domain}/repo"),
            ),
            (
                "package group",
                format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/{domain}/npm/*"),
            ),
        ];

        for (kind, arn) in &kinds {
            let label = format!("{} / {kind}", case.label);
            let expected: Vec<(String, String)> = case
                .left
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();

            // Sent twice: the first call removes, the second must find nothing left to
            // remove and answer the same success.
            for attempt in 1..=2 {
                let answer = untag(&router, arn, &case.remove).await;
                assert_eq!(
                    (answer.status, answer.code()),
                    (200, String::new()),
                    "case {label:?} attempt {attempt}: UntagResource must succeed, got {answer:?}"
                );
                let got = held_tags(&router, arn, &label).await;
                assert_eq!(
                    got, expected,
                    "case {label:?} attempt {attempt}: expected the tags {expected:?} to be left, got {got:?}"
                );
            }
        }

        // RULE 2 across resources: untagging one record never touched another's set. Each
        // kind above was untagged to the same expected set, so re-reading them all after
        // every call proves no write leaked between records.
        for (kind, arn) in &kinds {
            let got = held_tags(&router, arn, kind).await;
            let expected: Vec<(String, String)> = case
                .left
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            assert_eq!(
                got, expected,
                "case {:?} / {kind}: expected {expected:?} after every row's untags, got {got:?}",
                case.label
            );
        }

        // A key removed can be added again: the tag set is a store, not a tombstone.
        if let Some((key, _)) = case.held.first() {
            let (_, domain_arn) = &kinds[0];
            let retagged = tag(&router, domain_arn, &[(key, "re-added")]).await;
            assert_eq!(
                retagged.status, 200,
                "case {:?}: TagResource after an untag must succeed, got {retagged:?}",
                case.label
            );
            let got = held_tags(&router, domain_arn, case.label).await;
            assert!(
                got.iter()
                    .any(|(held, value)| held == key && value == "re-added"),
                "case {:?}: expected {key:?} to be re-addable, got {got:?}",
                case.label
            );
        }
    }
}

/// RULE 5 for the untag path: the ARN names one resource of this service, and a request
/// naming none changes nothing.
#[tokio::test]
async fn untagging_a_resource_this_service_does_not_hold_is_refused() {
    let router = emulator();
    let created = call(
        &router,
        "POST",
        "/v1/domain?domain=untag-refusals",
        Some(json!({ "tags": tag_list(&[("keep", "1")]) })),
    )
    .await;
    assert_eq!(created.status, 200, "CreateDomain failed: {created:?}");
    let domain_arn = format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:domain/untag-refusals");

    // (case, resourceArn, status, error code)
    let refusals = [
        (
            "an ARN of another service",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:domain/untag-refusals"),
            400,
            "ValidationException",
        ),
        (
            "a value that is not an ARN at all",
            "untag-refusals".to_string(),
            400,
            "ValidationException",
        ),
        (
            "a well-formed ARN of a kind this store holds no record of",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:package/untag-refusals/repo/generic//pkg"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a domain that is not held",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:domain/absent"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a repository that is not held",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:repository/untag-refusals/absent"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a package group that is not held",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/untag-refusals/npm/*"),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, arn, status, code) in &refusals {
        let answer = untag(&router, arn, &["keep"]).await;
        assert_eq!(
            (answer.status, answer.code()),
            (*status, (*code).to_string()),
            "case {case:?}: UntagResource on {arn:?} expected {status} {code}, got {answer:?}"
        );
    }

    // Not one refused call removed the tag the held domain carries.
    let got = held_tags(&router, &domain_arn, "after the refusals").await;
    assert_eq!(
        got,
        vec![("keep".to_string(), "1".to_string())],
        "a refused untag must leave the held tags alone, got {got:?}"
    );
}

// ---------------------------------------------------------------------------
// RULES 1, 2 and 5 for package groups.
// ---------------------------------------------------------------------------

/// RULE 1: a deleted package group is unreadable, and its pattern is free again.
///
/// Rows differ along what the rule generalises over: the depth of the pattern (one
/// component, two, three), whether it is a prefix pattern or an exact one, and whether the
/// group has a child below it.
#[tokio::test]
async fn a_deleted_package_group_is_no_longer_readable() {
    // (case, pattern, the child created under it — if any)
    let cases: [(&str, &str, Option<&str>); 5] = [
        ("one component", "/npm", None),
        ("a prefix pattern", "/npm/*", None),
        ("a scoped prefix pattern", "/npm/scope/*", None),
        ("an exact pattern", "/maven/com.example/lib$", None),
        ("a group with a child below it", "/pypi/*", Some("/pypi/pkg/*")),
    ];

    for (case, pattern, child) in cases {
        let router = emulator();
        let created = create_domain(&router, "group-removal", None).await;
        assert_eq!(created.status, 200, "case {case:?}: CreateDomain {created:?}");

        let group = create_package_group(&router, "group-removal", pattern, None, None).await;
        assert_eq!(
            group.status, 200,
            "case {case:?}: CreatePackageGroup {pattern} must succeed, got {group:?}"
        );
        let arn = group.group()["arn"].as_str().unwrap_or_default().to_string();

        if let Some(child) = child {
            let created = create_package_group(&router, "group-removal", child, None, None).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreatePackageGroup {child} must succeed, got {created:?}"
            );
            assert_eq!(
                created.group()["parent"]["pattern"].as_str(),
                Some(pattern),
                "case {case:?}: {child} must start out below {pattern}, got {created:?}"
            );
        }

        // The answer describes what was removed, read off the record before it went.
        let deleted = delete_package_group(&router, "group-removal", pattern).await;
        assert_eq!(
            (deleted.status, deleted.code()),
            (200, String::new()),
            "case {case:?}: DeletePackageGroup must succeed, got {deleted:?}"
        );
        assert_eq!(
            deleted.group()["pattern"].as_str(),
            Some(pattern),
            "case {case:?}: the answer must describe the deleted group, got {deleted:?}"
        );
        assert_eq!(
            deleted.group()["arn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: the answer must carry the deleted group's ARN, got {deleted:?}"
        );

        // RULE 1: the read a client verifies the delete with no longer reports it.
        let described = describe_package_group(&router, "group-removal", pattern).await;
        assert_eq!(
            (described.status, described.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: DescribePackageGroup after the delete must be 404, got {described:?}"
        );
        assert!(
            described.message().contains(pattern),
            "case {case:?}: the refusal must name the pattern, got {described:?}"
        );

        // A second delete of the same pattern has its goal already met: the delete is
        // idempotent, so it answers 200 and describes nothing.
        let again = delete_package_group(&router, "group-removal", pattern).await;
        assert_eq!(
            (again.status, again.code(), again.body.clone()),
            (200, String::new(), serde_json::json!({})),
            "case {case:?}: a second DeletePackageGroup must be an empty 200, got {again:?}"
        );

        // RULE 2: the root group, and any child, are still there — "deleting a package
        // group does not delete packages or package versions", and a child reparents onto
        // the deleted group's own parent.
        let root = describe_package_group(&router, "group-removal", ROOT).await;
        assert_eq!(
            root.status, 200,
            "case {case:?}: the root group must survive the delete, got {root:?}"
        );
        if let Some(child) = child {
            let described = describe_package_group(&router, "group-removal", child).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: the child must survive its parent's delete, got {described:?}"
            );
            assert_eq!(
                described.group()["parent"]["pattern"].as_str(),
                Some(ROOT),
                "case {case:?}: the child must reparent onto the root, got {described:?}"
            );
        }

        // The identifier the delete freed can be used again.
        let recreated = create_package_group(&router, "group-removal", pattern, None, None).await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: the pattern must be creatable again, got {recreated:?}"
        );
        let described = describe_package_group(&router, "group-removal", pattern).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the recreated group must read back, got {described:?}"
        );
    }
}

/// RULE 5 for `DeletePackageGroup`: what it cannot delete, what it refuses to, and what it
/// answers for a group that is not there at all.
#[tokio::test]
async fn package_group_delete_refusals_change_nothing() {
    let router = emulator();
    let created = create_domain(&router, "group-refusals", None).await;
    assert_eq!(created.status, 200, "CreateDomain failed: {created:?}");
    let group = create_package_group(&router, "group-refusals", "/npm/*", None, None).await;
    assert_eq!(group.status, 200, "CreatePackageGroup failed: {group:?}");

    // (case, domain, pattern, status, error code)
    let refusals = [
        (
            "the root group, which the domain is created with",
            "group-refusals",
            ROOT,
            400,
            "ValidationException",
        ),
        (
            "a domain that is not held",
            "absent-domain",
            "/npm/*",
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, domain, pattern, status, code) in &refusals {
        let answer = delete_package_group(&router, domain, pattern).await;
        assert_eq!(
            (answer.status, answer.code()),
            (*status, (*code).to_string()),
            "case {case:?}: DeletePackageGroup expected {status} {code}, got {answer:?}"
        );
    }

    // A pattern the domain holds no group under is not a refusal at all: the delete is
    // `@idempotent` and its `packageGroup` output member is optional, so there is nothing
    // to remove and nothing to describe. Rows differ along HOW the pattern misses: one the
    // domain never held, and one that is not even a path from the domain root.
    for pattern in ["/pypi/*", "npm"] {
        let answer = delete_package_group(&router, "group-refusals", pattern).await;
        assert_eq!(
            (answer.status, answer.code(), answer.body.clone()),
            (200, String::new(), serde_json::json!({})),
            "case {pattern:?}: deleting a group that is not there must be an empty 200, got \
             {answer:?}"
        );
    }

    // Neither the root group nor the one group this domain holds was removed.
    for pattern in [ROOT, "/npm/*"] {
        let described = describe_package_group(&router, "group-refusals", pattern).await;
        assert_eq!(
            described.status, 200,
            "a refused delete must leave {pattern} readable, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULES 1, 2 and 3 for package versions.
// ---------------------------------------------------------------------------

/// A domain with two repositories, each the destination of its own suite cases.
async fn emulator_with_repositories(domain: &str, repositories: &[&str]) -> MultiServiceRouter {
    let router = emulator();
    let created = create_domain(&router, domain, None).await;
    assert_eq!(created.status, 200, "CreateDomain {domain}: {created:?}");
    for repository in repositories {
        let created = create_repository(&router, domain, repository, &[]).await;
        assert_eq!(
            created.status, 200,
            "CreateRepository {repository}: {created:?}"
        );
    }
    router
}

/// One `DeletePackageVersions` row: the versions published, the versions the request
/// names, the `expectedStatus` it pins, and the two answer maps it must report.
struct DeleteCase {
    label: &'static str,
    published: Vec<&'static str>,
    delete: Vec<&'static str>,
    expected_status: Option<&'static str>,
    successful: Vec<(&'static str, &'static str)>,
    failed: Vec<(&'static str, &'static str)>,
}

/// RULES 1, 2 and 3: a delete removes the versions it names, reports each named version in
/// exactly one map, and leaves the rest readable — and the PACKAGE survives, however many
/// of its versions went.
///
/// Rows differ along what the rules generalise over: how many versions the package holds
/// (1, 3), how many the request names (1, 2, all, one that is not held), whether a named
/// version exists, whether an `expectedStatus` is pinned and whether it matches, and
/// whether the package has a namespace.
#[tokio::test]
async fn deleting_package_versions_commits_and_keeps_the_package() {
    let cases = vec![
        DeleteCase {
            label: "the only version",
            published: vec!["1.0.0"],
            delete: vec!["1.0.0"],
            expected_status: None,
            successful: vec![("1.0.0", DELETED)],
            failed: vec![],
        },
        DeleteCase {
            label: "one of three",
            published: vec!["1.0.0", "2.0.0", "3.0.0"],
            delete: vec!["2.0.0"],
            expected_status: None,
            successful: vec![("2.0.0", DELETED)],
            failed: vec![],
        },
        DeleteCase {
            label: "two of three, in one call",
            published: vec!["1.0.0", "2.0.0", "3.0.0"],
            delete: vec!["3.0.0", "1.0.0"],
            expected_status: None,
            successful: vec![("1.0.0", DELETED), ("3.0.0", DELETED)],
            failed: vec![],
        },
        DeleteCase {
            label: "every version held",
            published: vec!["1.0.0", "2.0.0"],
            delete: vec!["1.0.0", "2.0.0"],
            expected_status: None,
            successful: vec![("1.0.0", DELETED), ("2.0.0", DELETED)],
            failed: vec![],
        },
        DeleteCase {
            label: "a version the package does not hold",
            published: vec!["1.0.0"],
            delete: vec!["9.9.9"],
            expected_status: None,
            successful: vec![],
            failed: vec![("9.9.9", "NOT_FOUND")],
        },
        DeleteCase {
            label: "one held and one absent, in one call",
            published: vec!["1.0.0", "2.0.0"],
            delete: vec!["1.0.0", "9.9.9"],
            expected_status: None,
            successful: vec![("1.0.0", DELETED)],
            failed: vec![("9.9.9", "NOT_FOUND")],
        },
        DeleteCase {
            label: "an expectedStatus that matches",
            published: vec!["1.0.0", "2.0.0"],
            delete: vec!["1.0.0"],
            expected_status: Some(PUBLISHED),
            successful: vec![("1.0.0", DELETED)],
            failed: vec![],
        },
        DeleteCase {
            label: "an expectedStatus that does not match",
            published: vec!["1.0.0", "2.0.0"],
            delete: vec!["1.0.0", "2.0.0"],
            expected_status: Some("Archived"),
            successful: vec![],
            failed: vec![("1.0.0", "MISMATCHED_STATUS"), ("2.0.0", "MISMATCHED_STATUS")],
        },
        DeleteCase {
            label: "an empty version list names nothing",
            published: vec!["1.0.0"],
            delete: vec![],
            expected_status: None,
            successful: vec![],
            failed: vec![],
        },
        DeleteCase {
            label: "a repeated version carries one outcome",
            published: vec!["1.0.0", "2.0.0"],
            delete: vec!["1.0.0", "1.0.0"],
            expected_status: None,
            successful: vec![("1.0.0", DELETED)],
            failed: vec![],
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let router = emulator_with_repositories("version-removal", &["repo"]).await;
        // Every other row carries a DIFFERENT namespace: a package is addressed by its
        // whole triple, so the rule has to hold under more than one of them.
        let pkg = if index % 2 == 0 {
            Pkg::new("version-removal", "repo", "pkg")
        } else {
            Pkg::new("version-removal", "repo", "pkg").namespaced("space")
        };

        for (seed, version) in case.published.iter().enumerate() {
            let published = publish(
                &router,
                &pkg,
                version,
                "payload.bin",
                &binary_content(64 + seed, seed as u8),
            )
            .await;
            assert_eq!(
                published.status, 200,
                "case {:?}: publishing {version} must succeed, got {published:?}",
                case.label
            );
        }

        let answer = delete_versions(&router, &pkg, &case.delete, case.expected_status).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {:?}: DeletePackageVersions must answer 200, got {answer:?}",
            case.label
        );
        assert_eq!(
            successful(&answer),
            rows(&case.successful),
            "case {:?}: unexpected successfulVersions in {answer:?}",
            case.label
        );
        assert_eq!(
            failed(&answer),
            rows(&case.failed),
            "case {:?}: unexpected failedVersions in {answer:?}",
            case.label
        );

        // RULE 1: every version reported successful is gone. RULE 2 and 3: every other
        // published version — including one reported as a failure — is still readable.
        let removed: BTreeSet<String> = successful(&answer)
            .into_iter()
            .map(|(version, _)| version)
            .collect();
        for version in &case.published {
            let described = describe_version(&router, &pkg, version).await;
            if removed.contains(*version) {
                assert_eq!(
                    (described.status, described.code()),
                    (404, "ResourceNotFoundException".to_string()),
                    "case {:?}: {version} was deleted, so DescribePackageVersion must be 404, got {described:?}",
                    case.label
                );
            } else {
                assert_eq!(
                    described.status, 200,
                    "case {:?}: {version} was not deleted, so it must still read back, got {described:?}",
                    case.label
                );
                assert_eq!(
                    described.body["packageVersion"]["status"].as_str(),
                    Some(PUBLISHED),
                    "case {:?}: {version} must keep its status, got {described:?}",
                    case.label
                );
            }
        }

        // RULE 1: the package itself is NOT what a version delete removes — it keeps
        // answering whether or not it has versions left (`DeletePackage` is the operation
        // that removes a package).
        let versions_left = case.published.len() - removed.len();
        let described = describe_package(&router, &pkg).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: the package holds {versions_left} version(s) and must still read back, got {described:?}",
            case.label
        );

        // The domain's derived assetSizeBytes follows the assets that are left.
        let domain = call(&router, "GET", "/v1/domain?domain=version-removal", None).await;
        let reported = domain.body["domain"]["assetSizeBytes"].as_i64().unwrap_or(0);
        let expected: i64 = case
            .published
            .iter()
            .enumerate()
            .filter(|(_, version)| !removed.contains(**version))
            .map(|(seed, _)| 64i64 + seed as i64)
            .sum();
        assert_eq!(
            reported, expected,
            "case {:?}: expected assetSizeBytes {expected}, got {reported} in {domain:?}",
            case.label
        );
    }
}

/// RULE 1 for a package that holds no versions at all: a package
/// `PutPackageOriginConfiguration` created ("When called on a package that does not exist,
/// a package is created in the repository with no versions and the requested restrictions
/// are set on the package") keeps both its record and its restrictions across a delete
/// that removed nothing.
#[tokio::test]
async fn a_delete_that_removed_nothing_changes_nothing() {
    let router = emulator_with_repositories("prune-bounds", &["repo"]).await;
    let pkg = Pkg::new("prune-bounds", "repo", "configured-only");

    let configured = call(
        &router,
        "POST",
        &format!("/v1/package?{}", pkg.query()),
        Some(json!({ "restrictions": { "publish": "BLOCK", "upstream": "ALLOW" } })),
    )
    .await;
    assert_eq!(
        configured.status, 200,
        "PutPackageOriginConfiguration failed: {configured:?}"
    );

    let answer = delete_versions(&router, &pkg, &["1.0.0"], None).await;
    assert_eq!(
        (answer.status, answer.code()),
        (200, String::new()),
        "DeletePackageVersions must answer 200, got {answer:?}"
    );
    assert_eq!(
        failed(&answer),
        rows(&[("1.0.0", "NOT_FOUND")]),
        "a version the package never held must be NOT_FOUND, got {answer:?}"
    );

    let described = describe_package(&router, &pkg).await;
    assert_eq!(
        described.status, 200,
        "the configured package must survive a delete that removed nothing, got {described:?}"
    );
    assert_eq!(
        described.body["package"]["originConfiguration"]["restrictions"]["publish"].as_str(),
        Some("BLOCK"),
        "the configured restrictions must survive too, got {described:?}"
    );
}

/// RULE 5 for `DeletePackageVersions`: a resource it cannot reach is a 404 naming that
/// resource, not a per-version failure.
#[tokio::test]
async fn deleting_versions_of_a_resource_that_is_not_held_is_not_found() {
    let router = emulator_with_repositories("delete-refusals", &["repo"]).await;
    let pkg = Pkg::new("delete-refusals", "repo", "pkg");
    let published = publish(&router, &pkg, "1.0.0", "payload.bin", &binary_content(16, 7)).await;
    assert_eq!(published.status, 200, "publish failed: {published:?}");

    // (case, the package the request addresses, the resource the refusal must name)
    let refusals = [
        (
            "a domain that is not held",
            Pkg::new("absent-domain", "repo", "pkg"),
            "domain",
        ),
        (
            "a repository that is not held",
            Pkg::new("delete-refusals", "absent", "pkg"),
            "repository",
        ),
        (
            "a package that is not held",
            Pkg::new("delete-refusals", "repo", "absent"),
            "package",
        ),
        (
            "the right package name under another namespace",
            Pkg::new("delete-refusals", "repo", "pkg").namespaced("other"),
            "package",
        ),
    ];

    for (case, addressed, resource_type) in &refusals {
        let answer = delete_versions(&router, addressed, &["1.0.0"], None).await;
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: expected 404 ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.body["resourceType"].as_str(),
            Some(*resource_type),
            "case {case:?}: the refusal must name the resource kind it could not reach, got {answer:?}"
        );
    }

    // The version every refusal failed to address is still there.
    let described = describe_version(&router, &pkg, "1.0.0").await;
    assert_eq!(
        described.status, 200,
        "a refused delete must leave the version readable, got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// RULES 3, 4 and 5 for CopyPackageVersions.
// ---------------------------------------------------------------------------

/// One copy row: the versions published into the source, the versions the request names,
/// whether it names them by revision, and the package's namespace.
struct CopyCase {
    label: &'static str,
    published: &'static [&'static str],
    copied: &'static [&'static str],
    by_revision: bool,
    namespace: Option<&'static str>,
}

/// RULE 4: a copy materializes the versions in the destination, provenance included, and
/// leaves the source holding them.
///
/// Rows differ along what the rule generalises over: how many versions are copied (1, 2,
/// every one the package holds), whether the versions are named by `versions` or by
/// `versionRevisions`, and which namespace the package carries.
#[tokio::test]
async fn copied_versions_read_back_from_the_destination_repository() {
    let cases = [
        CopyCase {
            label: "one of one",
            published: &["1.0.0"],
            copied: &["1.0.0"],
            by_revision: false,
            namespace: Some(PKG_NAMESPACE),
        },
        CopyCase {
            label: "one of three",
            published: &["1.0.0", "2.0.0", "3.0.0"],
            copied: &["2.0.0"],
            by_revision: false,
            namespace: Some(PKG_NAMESPACE),
        },
        CopyCase {
            label: "two of three",
            published: &["1.0.0", "2.0.0", "3.0.0"],
            copied: &["1.0.0", "3.0.0"],
            by_revision: false,
            namespace: Some(PKG_NAMESPACE),
        },
        CopyCase {
            label: "every version held",
            published: &["1.0.0", "2.0.0"],
            copied: &["1.0.0", "2.0.0"],
            by_revision: false,
            namespace: Some(PKG_NAMESPACE),
        },
        CopyCase {
            label: "named by revision",
            published: &["1.0.0", "2.0.0"],
            copied: &["1.0.0"],
            by_revision: true,
            namespace: Some(PKG_NAMESPACE),
        },
        CopyCase {
            label: "another namespace",
            published: &["1.0.0", "2.0.0"],
            copied: &["2.0.0"],
            by_revision: false,
            namespace: Some("space"),
        },
    ];

    for CopyCase {
        label: case,
        published,
        copied,
        by_revision,
        namespace,
    } in cases
    {
        let router = emulator_with_repositories("copy-domain", &["source", "release"]).await;
        let source = match namespace {
            Some(namespace) => Pkg::new("copy-domain", "source", "pkg").namespaced(namespace),
            None => Pkg::new("copy-domain", "source", "pkg"),
        };
        let destination = source.in_repository("release");

        for (seed, version) in published.iter().enumerate() {
            let answer = publish(
                &router,
                &source,
                version,
                "payload.bin",
                &binary_content(32 + seed, seed as u8),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: publishing {version} must succeed, got {answer:?}"
            );
        }

        // What the source answers for each version BEFORE the copy: the copy has to make
        // the destination answer exactly this.
        let mut before = Vec::new();
        for version in copied {
            let described = describe_version(&router, &source, version).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: the source must describe {version}, got {described:?}"
            );
            before.push((*version, described.body["packageVersion"].clone()));
        }

        let body = if by_revision {
            let revisions: serde_json::Map<String, Value> = before
                .iter()
                .map(|(version, description)| {
                    (
                        (*version).to_string(),
                        description["revision"].clone(),
                    )
                })
                .collect();
            json!({ "versionRevisions": Value::Object(revisions) })
        } else {
            json!({ "versions": copied })
        };

        let answer = copy_versions(&router, &source, "release", body).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {case:?}: CopyPackageVersions must answer 200, got {answer:?}"
        );
        let expected: Vec<(String, String)> = copied
            .iter()
            .map(|version| ((*version).to_string(), PUBLISHED.to_string()))
            .collect();
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            successful(&answer),
            expected,
            "case {case:?}: unexpected successfulVersions in {answer:?}"
        );
        assert_eq!(
            failed(&answer),
            Vec::<(String, String)>::new(),
            "case {case:?}: nothing must fail, got {answer:?}"
        );

        for (version, source_description) in &before {
            // RULE 4: the destination answers the same description the source does —
            // format, version, status, revision and the origin provenance.
            let described = describe_version(&router, &destination, version).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: the destination must describe the copied {version}, got {described:?}"
            );
            assert_eq!(
                &described.body["packageVersion"], source_description,
                "case {case:?}: the copied {version} must describe as the source does, got {described:?}"
            );
            let origin = &described.body["packageVersion"]["origin"];
            assert_eq!(
                origin["originType"].as_str(),
                Some("INTERNAL"),
                "case {case:?}: a version published into the domain stays INTERNAL, got {described:?}"
            );
            assert_eq!(
                origin["domainEntryPoint"]["repositoryName"].as_str(),
                Some("source"),
                "case {case:?}: the entry point must keep naming the repository {version} was published to, got {described:?}"
            );

            // RULE 2: the source still holds it — a copy is not a move.
            let still = describe_version(&router, &source, version).await;
            assert_eq!(
                still.status, 200,
                "case {case:?}: the source must keep {version}, got {still:?}"
            );
        }

        // RULE 2: only the named versions were materialized.
        for version in published {
            if copied.contains(version) {
                continue;
            }
            let described = describe_version(&router, &destination, version).await;
            assert_eq!(
                (described.status, described.code()),
                (404, "ResourceNotFoundException".to_string()),
                "case {case:?}: {version} was not copied, so the destination must not hold it, got {described:?}"
            );
        }

        // The destination now holds the package itself, with the same identity.
        let described = describe_package(&router, &destination).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the destination must hold the package, got {described:?}"
        );
        assert_eq!(
            described.body["package"]["name"].as_str(),
            Some("pkg"),
            "case {case:?}: the copied package keeps its name, got {described:?}"
        );
        assert_eq!(
            described.body["package"]["namespace"].as_str(),
            namespace,
            "case {case:?}: the copied package keeps its namespace, got {described:?}"
        );
    }
}

/// RULES 3 and 5 for `CopyPackageVersions`: every per-version refusal, and every refusal
/// of the call itself — none of which may materialize anything.
#[tokio::test]
async fn copy_refusals_report_the_version_code_and_materialize_nothing() {
    let router = emulator_with_repositories("copy-refusals", &["source", "release"]).await;
    let source = Pkg::new("copy-refusals", "source", "pkg");
    let destination = source.in_repository("release");
    for (seed, version) in ["1.0.0", "2.0.0"].iter().enumerate() {
        let answer = publish(
            &router,
            &source,
            version,
            "payload.bin",
            &binary_content(48 + seed, seed as u8),
        )
        .await;
        assert_eq!(answer.status, 200, "publishing {version}: {answer:?}");
    }

    // A version the source does not hold is NOT_FOUND, and a named one beside it still
    // copies: the two answers are per version, not per call.
    let answer = copy_versions(
        &router,
        &source,
        "release",
        json!({ "versions": ["1.0.0", "9.9.9"] }),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (200, String::new()),
        "a partially satisfiable copy must answer 200, got {answer:?}"
    );
    assert_eq!(
        successful(&answer),
        rows(&[("1.0.0", PUBLISHED)]),
        "the held version must copy, got {answer:?}"
    );
    assert_eq!(
        failed(&answer),
        rows(&[("9.9.9", "NOT_FOUND")]),
        "the absent version must be NOT_FOUND, got {answer:?}"
    );

    // Copying it again without `allowOverwrite` is ALREADY_EXISTS, and the version the
    // destination holds is untouched.
    let held = describe_version(&router, &destination, "1.0.0").await;
    let answer = copy_versions(
        &router,
        &source,
        "release",
        json!({ "versions": ["1.0.0"] }),
    )
    .await;
    assert_eq!(
        failed(&answer),
        rows(&[("1.0.0", "ALREADY_EXISTS")]),
        "a second copy must be ALREADY_EXISTS, got {answer:?}"
    );
    assert_eq!(
        successful(&answer),
        Vec::<(String, String)>::new(),
        "a refused copy reports no success, got {answer:?}"
    );
    let after = describe_version(&router, &destination, "1.0.0").await;
    assert_eq!(
        after.body["packageVersion"], held.body["packageVersion"],
        "a refused copy must leave the held version alone, got {after:?}"
    );

    // With `allowOverwrite` the same copy is accepted.
    let answer = copy_versions(
        &router,
        &source,
        "release",
        json!({ "versions": ["1.0.0"], "allowOverwrite": true }),
    )
    .await;
    assert_eq!(
        successful(&answer),
        rows(&[("1.0.0", PUBLISHED)]),
        "allowOverwrite must accept the copy, got {answer:?}"
    );

    // A pinned revision that is not the one the source holds is MISMATCHED_REVISION, and
    // nothing is written for it.
    let answer = copy_versions(
        &router,
        &source,
        "release",
        json!({ "versionRevisions": { "2.0.0": "not-the-held-revision" } }),
    )
    .await;
    assert_eq!(
        failed(&answer),
        rows(&[("2.0.0", "MISMATCHED_REVISION")]),
        "a pinned revision that does not match must be MISMATCHED_REVISION, got {answer:?}"
    );
    let described = describe_version(&router, &destination, "2.0.0").await;
    assert_eq!(
        described.status, 404,
        "a mismatched revision must materialize nothing, got {described:?}"
    );

    // Origin control on the destination package refuses a new version: NOT_ALLOWED.
    let configured = call(
        &router,
        "POST",
        &format!("/v1/package?{}", destination.query()),
        Some(json!({ "restrictions": { "publish": "BLOCK", "upstream": "ALLOW" } })),
    )
    .await;
    assert_eq!(
        configured.status, 200,
        "PutPackageOriginConfiguration failed: {configured:?}"
    );
    let answer = copy_versions(
        &router,
        &source,
        "release",
        json!({ "versions": ["2.0.0"] }),
    )
    .await;
    assert_eq!(
        failed(&answer),
        rows(&[("2.0.0", "NOT_ALLOWED")]),
        "a blocked destination package must be NOT_ALLOWED, got {answer:?}"
    );
    let described = describe_version(&router, &destination, "2.0.0").await;
    assert_eq!(
        described.status, 404,
        "a blocked copy must materialize nothing, got {described:?}"
    );

    // RULE 5: the call itself is refused when it cannot hold together, or when it names a
    // resource this caller does not hold.
    //
    // (case, the package addressed, the destination repository, the body, status, code)
    let refusals: [(&str, Pkg, &str, Value, u16, &str); 6] = [
        (
            "both versions and versionRevisions",
            source.clone(),
            "release",
            json!({ "versions": ["1.0.0"], "versionRevisions": { "1.0.0": "r" } }),
            400,
            "ValidationException",
        ),
        (
            "neither versions nor versionRevisions",
            source.clone(),
            "release",
            json!({}),
            400,
            "ValidationException",
        ),
        (
            "a domain that is not held",
            Pkg::new("absent-domain", "source", "pkg"),
            "release",
            json!({ "versions": ["1.0.0"] }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a source repository that is not held",
            Pkg::new("copy-refusals", "absent", "pkg"),
            "release",
            json!({ "versions": ["1.0.0"] }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a destination repository that is not held",
            source.clone(),
            "absent",
            json!({ "versions": ["1.0.0"] }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a package the source repository does not hold",
            Pkg::new("copy-refusals", "source", "absent"),
            "release",
            json!({ "versions": ["1.0.0"] }),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, addressed, destination_repository, body, status, code) in refusals {
        let answer = copy_versions(&router, &addressed, destination_repository, body).await;
        assert_eq!(
            (answer.status, answer.code()),
            (status, code.to_string()),
            "case {case:?}: expected {status} {code}, got {answer:?}"
        );
    }

    // Nothing above moved what the source holds.
    for version in ["1.0.0", "2.0.0"] {
        let described = describe_version(&router, &source, version).await;
        assert_eq!(
            described.status, 200,
            "the source must still hold {version}, got {described:?}"
        );
    }
}

/// A copy that materializes nothing leaves the destination without the package record at
/// all: the record is created to hold a version, so a call that wrote none creates none.
#[tokio::test]
async fn a_copy_that_writes_no_version_creates_no_package() {
    let router = emulator_with_repositories("copy-empty", &["source", "release"]).await;
    let source = Pkg::new("copy-empty", "source", "pkg");
    let destination = source.in_repository("release");
    let published = publish(&router, &source, "1.0.0", "payload.bin", &binary_content(8, 1)).await;
    assert_eq!(published.status, 200, "publish failed: {published:?}");

    for (case, versions) in [
        ("an empty version list", json!({ "versions": [] })),
        (
            "only versions the source does not hold",
            json!({ "versions": ["9.9.9"] }),
        ),
    ] {
        let answer = copy_versions(&router, &source, "release", versions).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {case:?}: the copy must answer 200, got {answer:?}"
        );
        assert_eq!(
            successful(&answer),
            Vec::<(String, String)>::new(),
            "case {case:?}: nothing must be reported copied, got {answer:?}"
        );
        let described = describe_package(&router, &destination).await;
        assert_eq!(
            (described.status, described.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: the destination must hold no package, got {described:?}"
        );
    }
}

/// The number of versions ONE copy may name is bounded, whichever member names them.
///
/// `versions` is modelled `@length(0..100)`, so the boundary validator caps it; the
/// `versionRevisions` map names the same set and carries no modelled length, so the
/// handler caps it at [`MAX_VERSIONS_PER_COPY_REQUEST`] — the same figure. Tested AT the
/// bound and one past it, in both forms, with nothing materialized by a refusal.
#[tokio::test]
async fn the_number_of_versions_one_copy_names_is_bounded() {
    let router = emulator_with_repositories("copy-bounds", &["source", "release"]).await;
    let source = Pkg::new("copy-bounds", "source", "pkg");
    let destination = source.in_repository("release");
    let published = publish(&router, &source, "1.0.0", "payload.bin", &binary_content(8, 3)).await;
    assert_eq!(published.status, 200, "publish failed: {published:?}");

    /// `n` distinct version strings, none of which the source holds — the bound is on the
    /// COUNT the request names, not on what the store holds.
    fn names(n: usize) -> Vec<String> {
        (0..n).map(|index| format!("9.0.{index}")).collect()
    }

    // (case, how many versions the request names, whether it is accepted)
    let rows = [
        ("none", 0, true),
        ("one", 1, true),
        ("the bound itself", MAX_VERSIONS_PER_COPY_REQUEST, true),
        ("one past the bound", MAX_VERSIONS_PER_COPY_REQUEST + 1, false),
    ];

    for (case, count, accepted) in rows {
        let versions = names(count);
        let revisions: serde_json::Map<String, Value> = versions
            .iter()
            .map(|version| (version.clone(), json!("some-revision")))
            .collect();

        for (form, body) in [
            ("versions", json!({ "versions": versions })),
            (
                "versionRevisions",
                json!({ "versionRevisions": Value::Object(revisions) }),
            ),
        ] {
            let answer = copy_versions(&router, &source, "release", body).await;
            if accepted {
                assert_eq!(
                    (answer.status, answer.code()),
                    (200, String::new()),
                    "case {case:?} by {form}: {count} versions are inside the bound, got {answer:?}"
                );
                assert_eq!(
                    failed(&answer).len(),
                    count,
                    "case {case:?} by {form}: every named version must be reported, got {answer:?}"
                );
            } else {
                assert_eq!(
                    (answer.status, answer.code()),
                    (400, "ValidationException".to_string()),
                    "case {case:?} by {form}: {count} versions are past the bound, got {answer:?}"
                );
            }
        }
    }

    // No row materialized anything: every version they named is one the source never held.
    let described = describe_package(&router, &destination).await;
    assert_eq!(
        (described.status, described.code()),
        (404, "ResourceNotFoundException".to_string()),
        "the destination must hold no package, got {described:?}"
    );
}
