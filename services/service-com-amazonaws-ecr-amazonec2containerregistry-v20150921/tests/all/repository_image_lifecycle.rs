// SPDX-License-Identifier: CC0-1.0

//! The ECR repository and image lifecycle, end to end over the wire.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes. The
//! rule under test is the registry's own: a repository and an image are whatever the
//! create or the push stored, addressed by name and by content digest, and every
//! reporting operation computes its answer from those records.
//!
//! `manifest_digest` and `given_repository` are `pub(crate)` so a sibling suite under
//! `tests/all/` reuses them beside the shared wire harness in `tests/all/wire.rs`.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness lives in `tests/all/wire.rs`, shared with every other suite.
use crate::wire::{
    announced_registry_endpoint, at, call, emulator, given_layer, ok, REGION, REGISTRY_ID,
};

/// The values `ImageTagMutability` accepts, from the model's enum. Spelled out here so a
/// setting the implementation silently ignores is visible as an unexercised value.
const IMAGE_TAG_MUTABILITY: [&str; 4] = [
    "MUTABLE",
    "IMMUTABLE",
    "MUTABLE_WITH_EXCLUSION",
    "IMMUTABLE_WITH_EXCLUSION",
];

/// An image identifier as the listing reports it: the digest, and the tag when one
/// points at it.
type ImagePair = (String, Option<String>);

/// One row of the push table: the case name, the manifest, the tag the push carries, the
/// media type it sends, the media type the stored image is expected to report, and the
/// `imageSizeInBytes` the stored image is expected to report (`None` where the manifest
/// states no layer content at all).
type PushCase<'a> = (
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<i64>,
);

/// One row of the listing-filter table: the case name, the `filter` member, and the
/// pairs the listing must report for it.
type FilterCase<'a> = (&'a str, Value, BTreeSet<ImagePair>);

/// One row of the scanning-configuration table: the case name, the repository name, the
/// `scanOnPush` the create carried (`None` when it named no scanning configuration), the
/// `scanOnPush` values put in order, and whether the put names `registryId`.
type ScanningCase<'a> = (&'a str, &'a str, Option<bool>, Vec<bool>, bool);

/// The digest a registry addresses a manifest by: `sha256:<hex>` over the manifest
/// bytes. Computed here independently of the crate's own helper.
pub(crate) fn manifest_digest(manifest: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{}", hex::encode(Sha256::digest(manifest.as_bytes())))
}

/// The `(imageDigest, imageTag)` pairs `ListImages` reported, as a set.
fn listed_pairs(body: &Value) -> BTreeSet<ImagePair> {
    body["imageIds"]
        .as_array()
        .unwrap_or_else(|| panic!("ListImages must return an imageIds list: {body:?}"))
        .iter()
        .map(|id| {
            (
                id["imageDigest"].as_str().unwrap_or_default().to_string(),
                id["imageTag"].as_str().map(ToString::to_string),
            )
        })
        .collect()
}

/// Create a repository that the case under test only needs to exist.
pub(crate) async fn given_repository(
    router: &MultiServiceRouter,
    name: &str,
    request: Value,
) -> Value {
    let mut request = request;
    request["repositoryName"] = json!(name);
    ok(router, name, "CreateRepository", &request).await
}

/// `CreateRepository` persists what it was given and computes the repository's identity
/// once; `DescribeRepositories` reports that record unchanged.
///
/// The rows differ along what the rule generalises over — the shape and length of the
/// name, which optional settings are present, which mutability setting and how many
/// exclusion filters, which encryption type, and how many tags.
#[tokio::test]
async fn a_created_repository_is_described_from_what_was_stored() {
    let cases = vec![
        ("only the required member", json!({"repositoryName": "my-app"})),
        (
            "the shortest name the model admits",
            json!({"repositoryName": "ab"}),
        ),
        (
            "a namespaced name",
            json!({"repositoryName": "project-a/nginx-web-app"}),
        ),
        (
            "a name using every separator the pattern allows",
            json!({"repositoryName": "team.a_b__c--d/inner-1"}),
        ),
        (
            "immutable tags, scanning on push and tags",
            json!({
                "repositoryName": "immutable-app",
                "imageTagMutability": "IMMUTABLE",
                "imageScanningConfiguration": {"scanOnPush": true},
                "tags": [{"Key": "env", "Value": "test"}],
            }),
        ),
        (
            "mutable except two excluded tag patterns",
            json!({
                "repositoryName": "partly-immutable",
                "imageTagMutability": "MUTABLE_WITH_EXCLUSION",
                "imageTagMutabilityExclusionFilters": [
                    {"filterType": "WILDCARD", "filter": "release-*"},
                    {"filterType": "WILDCARD", "filter": "prod"},
                ],
                "tags": [
                    {"Key": "env", "Value": "prod"},
                    {"Key": "team", "Value": "platform"},
                    {"Key": "cost-centre", "Value": "42"},
                ],
            }),
        ),
        (
            "KMS encryption with a key, and the registry named explicitly",
            json!({
                "repositoryName": "kms-app",
                "registryId": REGISTRY_ID,
                "encryptionConfiguration": {
                    "encryptionType": "KMS",
                    "kmsKey": "arn:aws:kms:eu-west-1:000000000000:key/abcd",
                },
                "imageScanningConfiguration": {"scanOnPush": false},
            }),
        ),
    ];

    let router = emulator();
    let mut created_names = Vec::new();

    for (case, request) in cases {
        let name = request["repositoryName"].as_str().unwrap().to_string();
        let created = ok(&router, case, "CreateRepository", &request).await;
        let repository = at(&created, "/repository");

        assert_eq!(
            at(&repository, "/repositoryName"),
            json!(name),
            "case {case:?}: the repository must report the name it was created with, got {:?}",
            at(&repository, "/repositoryName")
        );
        assert_eq!(
            at(&repository, "/registryId"),
            json!(REGISTRY_ID),
            "case {case:?}: expected registryId {REGISTRY_ID}, got {:?}",
            at(&repository, "/registryId")
        );
        assert_eq!(
            at(&repository, "/repositoryArn"),
            json!(format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:repository/{name}")),
            "case {case:?}: the ARN names the signed region, the registry and the name, got {:?}",
            at(&repository, "/repositoryArn")
        );
        assert_eq!(
            at(&repository, "/repositoryUri"),
            json!(format!("{}/{name}", announced_registry_endpoint())),
            "case {case:?}: expected the repository announced at the endpoint this \
             emulator serves the registry at, got {:?}",
            at(&repository, "/repositoryUri")
        );
        assert!(
            at(&repository, "/createdAt").as_f64().is_some_and(|t| t > 0.0),
            "case {case:?}: createdAt must be the instant of creation, got {:?}",
            at(&repository, "/createdAt")
        );
        assert_eq!(
            at(&repository, "/imageTagMutability"),
            request
                .get("imageTagMutability")
                .cloned()
                .unwrap_or(json!("MUTABLE")),
            "case {case:?}: an unspecified imageTagMutability defaults to MUTABLE, got {:?}",
            at(&repository, "/imageTagMutability")
        );
        assert_eq!(
            at(&repository, "/encryptionConfiguration"),
            request
                .get("encryptionConfiguration")
                .cloned()
                .unwrap_or(json!({"encryptionType": "AES256"})),
            "case {case:?}: an unspecified encryptionConfiguration defaults to AES256, got {:?}",
            at(&repository, "/encryptionConfiguration")
        );
        assert_eq!(
            at(&repository, "/imageScanningConfiguration"),
            request
                .get("imageScanningConfiguration")
                .cloned()
                // `scanOnPush` is modelled `@default(false)`, so a create that names no
                // scanning configuration still reports one, with scanning off.
                .unwrap_or(json!({"scanOnPush": false})),
            "case {case:?}: imageScanningConfiguration is reported as it was stored, and \
             defaults to scanOnPush false, got {:?}",
            at(&repository, "/imageScanningConfiguration")
        );
        assert_eq!(
            at(&repository, "/imageTagMutabilityExclusionFilters"),
            request
                .get("imageTagMutabilityExclusionFilters")
                .cloned()
                .unwrap_or(Value::Null),
            "case {case:?}: the exclusion filters are reported as they were stored, got {:?}",
            at(&repository, "/imageTagMutabilityExclusionFilters")
        );

        // The read reports the stored record, member for member — not a repository
        // re-synthesised from the describe request.
        let described = ok(
            &router,
            case,
            "DescribeRepositories",
            &json!({"repositoryNames": [name.clone()]}),
        )
        .await;
        assert_eq!(
            at(&described, "/repositories"),
            json!([repository]),
            "case {case:?}: DescribeRepositories must report exactly what was stored, got {:?}",
            at(&described, "/repositories")
        );

        created_names.push(name);
    }

    // And a describe naming no repository reports every one of them, in name order.
    let all = ok(&router, "all repositories", "DescribeRepositories", &json!({})).await;
    let reported: Vec<String> = all["repositories"]
        .as_array()
        .expect("a repositories list")
        .iter()
        .map(|repo| repo["repositoryName"].as_str().unwrap_or_default().to_string())
        .collect();
    created_names.sort();
    assert_eq!(
        reported, created_names,
        "a describe naming no repository reports every stored one in name order"
    );
}

/// `PutImageScanningConfiguration` replaces the named repository's image scanning
/// configuration with the one the request carries, reports the setting it stored, and
/// leaves it there for `DescribeRepositories` to read back. No other repository moves.
///
/// The rows differ along the setting the repository was created with (absent, off, on),
/// the sequence of values put — a switch, a toggle and back, and the same value twice —
/// whether the request names this registry, and the shape and length of the name.
#[tokio::test]
async fn image_scanning_configuration_is_replaced_by_what_was_put() {
    // case, repository name, the create's `imageScanningConfiguration.scanOnPush`
    // (`None` for a create that names no scanning configuration at all), the
    // `scanOnPush` values put in order, and whether the put names `registryId`.
    let cases: Vec<ScanningCase<'_>> = vec![
        (
            "created without one, switched on",
            "scanned-app",
            None,
            vec![true],
            false,
        ),
        (
            "created without one, switched off",
            "unscanned-app",
            None,
            vec![false],
            false,
        ),
        (
            "created off, toggled on and back off",
            "toggling-app",
            Some(false),
            vec![true, false],
            false,
        ),
        (
            "created on, toggled off and back on, namespaced name",
            "project-a/toggling-app",
            Some(true),
            vec![false, true],
            false,
        ),
        (
            "the same value put twice, shortest name the model admits",
            "ab",
            Some(false),
            vec![true, true],
            false,
        ),
        (
            "naming this registry explicitly",
            "explicit-registry-app",
            None,
            vec![true],
            true,
        ),
    ];

    let router = emulator();
    // A repository nothing in this test puts a configuration on: its own setting must
    // stay exactly as its create left it.
    given_repository(
        &router,
        "untouched-app",
        json!({"imageScanningConfiguration": {"scanOnPush": true}}),
    )
    .await;

    for (case, name, created_with, puts, names_registry) in cases {
        let mut create = json!({});
        if let Some(scan_on_push) = created_with {
            create["imageScanningConfiguration"] = json!({"scanOnPush": scan_on_push});
        }
        given_repository(&router, name, create).await;

        for scan_on_push in puts {
            let expected = json!({"scanOnPush": scan_on_push});
            let mut request = json!({
                "repositoryName": name,
                "imageScanningConfiguration": expected.clone(),
            });
            if names_registry {
                request["registryId"] = json!(REGISTRY_ID);
            }

            let put = ok(
                &router,
                case,
                "PutImageScanningConfiguration",
                &request,
            )
            .await;
            assert_eq!(
                at(&put, "/imageScanningConfiguration"),
                expected,
                "case {case:?}: the put reports the setting it stored, got {:?}",
                at(&put, "/imageScanningConfiguration")
            );
            assert_eq!(
                at(&put, "/repositoryName"),
                json!(name),
                "case {case:?}: the put reports the repository it updated, got {:?}",
                at(&put, "/repositoryName")
            );
            assert_eq!(
                at(&put, "/registryId"),
                json!(REGISTRY_ID),
                "case {case:?}: the put reports this registry, got {:?}",
                at(&put, "/registryId")
            );

            let described = ok(
                &router,
                case,
                "DescribeRepositories",
                &json!({"repositoryNames": [name]}),
            )
            .await;
            assert_eq!(
                at(&described, "/repositories/0/imageScanningConfiguration"),
                expected,
                "case {case:?}: the stored repository reports the setting that was put, got {:?}",
                at(&described, "/repositories/0/imageScanningConfiguration")
            );
            // The update touches that one setting and nothing else about the record.
            assert_eq!(
                at(&described, "/repositories/0/repositoryArn"),
                json!(format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:repository/{name}")),
                "case {case:?}: the repository keeps the identity its create computed, got {:?}",
                at(&described, "/repositories/0/repositoryArn")
            );
        }
    }

    let untouched = ok(
        &router,
        "untouched-app",
        "DescribeRepositories",
        &json!({"repositoryNames": ["untouched-app"]}),
    )
    .await;
    assert_eq!(
        at(&untouched, "/repositories/0/imageScanningConfiguration"),
        json!({"scanOnPush": true}),
        "a put on one repository leaves every other repository's setting alone, got {:?}",
        at(&untouched, "/repositories/0/imageScanningConfiguration")
    );
}

/// A name already taken in the registry is refused, whatever its shape.
#[tokio::test]
async fn a_second_create_of_a_name_already_taken_is_refused() {
    let cases = [
        ("a plain name", "my-app"),
        ("a namespaced name", "project-a/nginx-web-app"),
        ("the shortest name", "ab"),
    ];

    let router = emulator();
    for (case, name) in cases {
        let request = json!({"repositoryName": name});
        ok(&router, case, "CreateRepository", &request).await;

        let again = call(&router, "CreateRepository", &request).await;
        assert_eq!(
            again.status, 400,
            "case {case:?}: expected HTTP 400 on the second create, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "RepositoryAlreadyExistsException",
            "case {case:?}: expected RepositoryAlreadyExistsException, got {:?}",
            again.body
        );
    }
}

/// `PutImage` stores the manifest under the digest of its own bytes, and the reporting
/// operations read that record back: the digest, every tag on it, its size, its push
/// instant and its media type.
///
/// The rows differ along manifest content and length, whether the push names a tag,
/// whether it asserts the digest, and whether the media type is given or carried by the
/// manifest itself.
#[tokio::test]
async fn a_pushed_manifest_is_stored_and_reported_under_its_own_digest() {
    let router = emulator();
    given_repository(&router, "pushes", json!({})).await;
    // The blob the docker v2 manifest below names as its config: in the repository
    // first, because a manifest cannot be pushed before every blob it references is
    // there (`PutImage`'s `LayersNotFoundException`).
    let config_digest = given_layer(&router, "seed", "pushes", b"pushes config blob").await;
    let v2_manifest = format!(
        "{{\"schemaVersion\":2,\
         \"mediaType\":\"application/vnd.docker.distribution.manifest.v2+json\",\
         \"config\":{{\"size\":7023,\"digest\":\"{config_digest}\"}}}}"
    );
    let oci_manifest = "{\"schemaVersion\":2,\
         \"mediaType\":\"application/vnd.oci.image.manifest.v1+json\",\
         \"layers\":[]}";
    let long_manifest = format!("{{\"schemaVersion\":2,\"history\":\"{}\"}}", "x".repeat(4096));

    let cases: Vec<PushCase<'_>> = vec![
        // case, manifest, tag, media type sent, media type expected back, size expected
        (
            "a docker v2 manifest, tagged, media type declared by the manifest",
            v2_manifest.as_str(),
            Some("v1"),
            None,
            Some("application/vnd.docker.distribution.manifest.v2+json"),
            // The manifest references one blob, its configuration, and declares 7023
            // bytes for it; it names no `layers`, so that is the whole of the image's
            // content (and nothing to do with the manifest document's own length).
            Some(7023),
        ),
        (
            "an OCI manifest, tagged, media type sent explicitly",
            oci_manifest,
            Some("oci"),
            Some("application/vnd.oci.image.manifest.v1+json"),
            Some("application/vnd.oci.image.manifest.v1+json"),
            // An EMPTY `layers` array is a statement: no layer content, so zero bytes.
            Some(0),
        ),
        (
            // A `config` descriptor naming no digest references no blob.
            "an untagged push",
            "{\"schemaVersion\":2,\"config\":{\"size\":1}}",
            None,
            None,
            None,
            // The one descriptor it carries names no blob, so the image references no
            // content: zero bytes, not an unknown size.
            Some(0),
        ),
        (
            "a tag at the length ceiling of a sensible tag, and a long manifest",
            long_manifest.as_str(),
            Some("release-2026-01-01-build-00042"),
            None,
            None,
            None,
        ),
        (
            "a one-byte manifest, which is still addressable",
            "{",
            Some("odd"),
            Some("text/plain"),
            Some("text/plain"),
            None,
        ),
    ];

    for (case, manifest, tag, media_type_sent, media_type_expected, size_expected) in cases {
        let digest = manifest_digest(manifest);
        let mut request = json!({"repositoryName": "pushes", "imageManifest": manifest});
        if let Some(tag) = tag {
            request["imageTag"] = json!(tag);
        }
        if let Some(media_type) = media_type_sent {
            request["imageManifestMediaType"] = json!(media_type);
        }
        // Asserting the digest the push computes is accepted.
        request["imageDigest"] = json!(digest.clone());

        let pushed = ok(&router, case, "PutImage", &request).await;
        assert_eq!(
            at(&pushed, "/image/imageId/imageDigest"),
            json!(digest.clone()),
            "case {case:?}: the image is addressed by the digest of its manifest, got {:?}",
            at(&pushed, "/image/imageId/imageDigest")
        );
        assert_eq!(
            at(&pushed, "/image/imageId/imageTag"),
            tag.map_or(Value::Null, |t| json!(t)),
            "case {case:?}: the pushed tag is reported back, got {:?}",
            at(&pushed, "/image/imageId/imageTag")
        );
        assert_eq!(
            at(&pushed, "/image/imageManifest"),
            json!(manifest),
            "case {case:?}: the stored manifest is the one pushed"
        );
        assert_eq!(
            at(&pushed, "/image/registryId"),
            json!(REGISTRY_ID),
            "case {case:?}: the image names the registry it was pushed to"
        );
        assert_eq!(
            at(&pushed, "/image/repositoryName"),
            json!("pushes"),
            "case {case:?}: the image names the repository it was pushed to"
        );

        let described = ok(
            &router,
            case,
            "DescribeImages",
            &json!({
                "repositoryName": "pushes",
                "imageIds": [{"imageDigest": digest.clone()}],
            }),
        )
        .await;
        let detail = at(&described, "/imageDetails/0");
        assert_eq!(
            at(&detail, "/imageDigest"),
            json!(digest.clone()),
            "case {case:?}: DescribeImages must report the stored digest"
        );
        assert_eq!(
            at(&detail, "/imageSizeInBytes"),
            size_expected.map_or(Value::Null, |size| json!(size)),
            "case {case:?}: expected imageSizeInBytes {size_expected:?}, got {:?}",
            at(&detail, "/imageSizeInBytes")
        );
        assert!(
            at(&detail, "/imagePushedAt").as_f64().is_some_and(|t| t > 0.0),
            "case {case:?}: imagePushedAt must be the instant of the push, got {:?}",
            at(&detail, "/imagePushedAt")
        );
        assert_eq!(
            at(&detail, "/imageTags"),
            tag.map_or(Value::Null, |t| json!([t])),
            "case {case:?}: the tags on the digest are the ones attached to it, got {:?}",
            at(&detail, "/imageTags")
        );
        assert_eq!(
            at(&detail, "/imageManifestMediaType"),
            media_type_expected.map_or(Value::Null, |t| json!(t)),
            "case {case:?}: expected media type {media_type_expected:?}, got {:?}",
            at(&detail, "/imageManifestMediaType")
        );

        // The identifier the listing reports is the stored (digest, tag) pair.
        let listed = ok(
            &router,
            case,
            "ListImages",
            &json!({"repositoryName": "pushes"}),
        )
        .await;
        assert!(
            listed_pairs(&listed).contains(&(digest.clone(), tag.map(ToString::to_string))),
            "case {case:?}: ListImages must report ({digest}, {tag:?}), got {:?}",
            listed["imageIds"]
        );
    }
}

/// A push that asserts a digest other than the manifest's own is refused, whatever the
/// asserted value looks like.
#[tokio::test]
async fn a_digest_that_disagrees_with_the_manifest_is_refused() {
    let manifest = "{\"schemaVersion\":2,\"config\":{\"size\":42}}";
    let other = manifest_digest("{\"schemaVersion\":2}");
    let cases = [
        ("the digest of another manifest", other.as_str()),
        (
            "a well-formed digest of the right shape",
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        ),
        ("a digest with another algorithm", "sha512:abcd"),
    ];

    let router = emulator();
    given_repository(&router, "digest-check", json!({})).await;

    for (case, asserted) in cases {
        let answer = call(
            &router,
            "PutImage",
            &json!({
                "repositoryName": "digest-check",
                "imageManifest": manifest,
                "imageDigest": asserted,
                "imageTag": "v1",
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ImageDigestDoesNotMatchException",
            "case {case:?}: expected ImageDigestDoesNotMatchException, got {:?}",
            answer.body
        );
        // And nothing was stored: the manifest is still absent.
        let listed = ok(
            &router,
            case,
            "ListImages",
            &json!({"repositoryName": "digest-check"}),
        )
        .await;
        assert!(
            listed_pairs(&listed).is_empty(),
            "case {case:?}: a refused push must store nothing, got {:?}",
            listed["imageIds"]
        );
    }
}

/// Pushing a manifest the repository already holds, under a tag that already points at
/// it, adds nothing — and neither does an untagged push of a manifest already stored.
#[tokio::test]
async fn pushing_what_is_already_stored_is_refused() {
    let cases = [
        ("tagged", Some("v1")),
        ("untagged", None),
        ("another tag, pushed twice", Some("latest")),
    ];

    let router = emulator();
    given_repository(&router, "repush", json!({})).await;

    for (case, tag) in cases {
        // One manifest per case, so each row starts from a manifest nothing holds.
        let manifest = format!("{{\"schemaVersion\":2,\"case\":\"{case}\"}}");
        let mut request = json!({"repositoryName": "repush", "imageManifest": manifest});
        if let Some(tag) = tag {
            request["imageTag"] = json!(tag);
        }
        ok(&router, case, "PutImage", &request).await;

        let again = call(&router, "PutImage", &request).await;
        assert_eq!(
            again.status, 400,
            "case {case:?}: expected HTTP 400 on the repeated push, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ImageAlreadyExistsException",
            "case {case:?}: expected ImageAlreadyExistsException, got {:?}",
            again.body
        );
    }
}

/// A tag already pointing at one manifest may be moved to another only where the
/// repository's mutability setting allows that tag to be overwritten; otherwise the push
/// is refused and the tag stays where it was.
///
/// The rows span every `ImageTagMutability` value, and for the two `*_WITH_EXCLUSION`
/// settings both a tag the filters match and one they do not.
#[tokio::test]
async fn an_existing_tag_moves_only_where_the_repositorys_mutability_allows_it() {
    let filters = json!([
        {"filterType": "WILDCARD", "filter": "release-*"},
        {"filterType": "WILDCARD", "filter": "dev"},
    ]);
    let cases: Vec<(&str, &str, Value, &str, bool)> = vec![
        // case, mutability, filters, tag, the tag may move
        ("mutable", "MUTABLE", Value::Null, "v1", true),
        ("immutable", "IMMUTABLE", Value::Null, "v1", false),
        (
            "mutable except a matching wildcard filter",
            "MUTABLE_WITH_EXCLUSION",
            filters.clone(),
            "release-7",
            false,
        ),
        (
            "mutable except a matching literal filter",
            "MUTABLE_WITH_EXCLUSION",
            filters.clone(),
            "dev",
            false,
        ),
        (
            "mutable, and the filters match nothing",
            "MUTABLE_WITH_EXCLUSION",
            filters.clone(),
            "v1",
            true,
        ),
        (
            "immutable except a matching wildcard filter",
            "IMMUTABLE_WITH_EXCLUSION",
            filters.clone(),
            "release-7",
            true,
        ),
        (
            "immutable, and the filters match nothing",
            "IMMUTABLE_WITH_EXCLUSION",
            filters.clone(),
            "v1",
            false,
        ),
    ];

    // Every modelled mutability value is exercised by some row.
    for value in IMAGE_TAG_MUTABILITY {
        assert!(
            cases.iter().any(|(_, mutability, ..)| *mutability == value),
            "no case exercises the {value} tag mutability setting"
        );
    }

    for (index, (case, mutability, filters, tag, may_move)) in cases.into_iter().enumerate() {
        let router = emulator();
        let name = format!("mutability-{index}");
        let mut create = json!({"imageTagMutability": mutability});
        if !filters.is_null() {
            create["imageTagMutabilityExclusionFilters"] = filters;
        }
        given_repository(&router, &name, create).await;

        let first = format!("{{\"schemaVersion\":2,\"image\":\"first-{index}\"}}");
        let second = format!("{{\"schemaVersion\":2,\"image\":\"second-{index}\"}}");
        let first_digest = manifest_digest(&first);
        let second_digest = manifest_digest(&second);

        ok(
            &router,
            case,
            "PutImage",
            &json!({"repositoryName": name, "imageManifest": first, "imageTag": tag}),
        )
        .await;

        let moved = call(
            &router,
            "PutImage",
            &json!({"repositoryName": name, "imageManifest": second, "imageTag": tag}),
        )
        .await;

        let listed = ok(
            &router,
            case,
            "ListImages",
            &json!({"repositoryName": name}),
        )
        .await;
        let pairs = listed_pairs(&listed);

        if may_move {
            assert_eq!(
                moved.status, 200,
                "case {case:?}: expected the tag to move, got {} with {:?}",
                moved.status, moved.body
            );
            // The tag now names the new manifest, and the old one is still there,
            // addressable by digest with no tag on it.
            assert_eq!(
                pairs,
                BTreeSet::from([
                    (first_digest.clone(), None),
                    (second_digest.clone(), Some(tag.to_string())),
                ]),
                "case {case:?}: expected the tag on {second_digest} and {first_digest} untagged, \
                 got {pairs:?}"
            );
        } else {
            assert_eq!(
                moved.status, 400,
                "case {case:?}: expected the push to be refused, got {} with {:?}",
                moved.status, moved.body
            );
            assert_eq!(
                moved.error_code(),
                "ImageTagAlreadyExistsException",
                "case {case:?}: expected ImageTagAlreadyExistsException, got {:?}",
                moved.body
            );
            assert_eq!(
                pairs,
                BTreeSet::from([(first_digest.clone(), Some(tag.to_string()))]),
                "case {case:?}: a refused push leaves the tag and stores nothing, got {pairs:?}"
            );
        }
    }
}

/// `ListImages` reports one identifier per (digest, tag) pair and one tagless identifier
/// per untagged digest, filtered by `filter.tagStatus` and `filter.imageStatus`, and
/// pages through that one order without losing or repeating an entry.
#[tokio::test]
async fn list_images_filters_and_pages_over_the_stored_pairs() {
    let router = emulator();
    given_repository(&router, "listing", json!({})).await;

    // Two tags on one manifest, one tag on another, and a third manifest with none.
    let two_tagged = "{\"schemaVersion\":2,\"image\":\"two-tags\"}";
    let one_tagged = "{\"schemaVersion\":2,\"image\":\"one-tag\"}";
    let untagged = "{\"schemaVersion\":2,\"image\":\"untagged\"}";
    for (manifest, tag) in [
        (two_tagged, Some("v1")),
        (two_tagged, Some("latest")),
        (one_tagged, Some("v2")),
        (untagged, None),
    ] {
        let mut request = json!({"repositoryName": "listing", "imageManifest": manifest});
        if let Some(tag) = tag {
            request["imageTag"] = json!(tag);
        }
        ok(&router, "seed", "PutImage", &request).await;
    }

    let tagged = BTreeSet::from([
        (manifest_digest(two_tagged), Some("latest".to_string())),
        (manifest_digest(two_tagged), Some("v1".to_string())),
        (manifest_digest(one_tagged), Some("v2".to_string())),
    ]);
    let untagged_pairs = BTreeSet::from([(manifest_digest(untagged), None)]);
    let every_pair: BTreeSet<_> = tagged.union(&untagged_pairs).cloned().collect();

    let cases: Vec<FilterCase<'_>> = vec![
        ("no filter at all", json!({}), every_pair.clone()),
        (
            "tagStatus ANY",
            json!({"tagStatus": "ANY"}),
            every_pair.clone(),
        ),
        ("tagStatus TAGGED", json!({"tagStatus": "TAGGED"}), tagged),
        (
            "tagStatus UNTAGGED",
            json!({"tagStatus": "UNTAGGED"}),
            untagged_pairs,
        ),
        (
            "imageStatus ACTIVE, which every stored image is",
            json!({"imageStatus": "ACTIVE"}),
            every_pair.clone(),
        ),
        (
            "imageStatus ARCHIVED, which no stored image is",
            json!({"imageStatus": "ARCHIVED"}),
            BTreeSet::new(),
        ),
        (
            "both filters together",
            json!({"tagStatus": "ANY", "imageStatus": "ANY"}),
            every_pair.clone(),
        ),
    ];

    for (case, filter, expected) in cases {
        let mut request = json!({"repositoryName": "listing"});
        if filter.as_object().is_some_and(|f| !f.is_empty()) {
            request["filter"] = filter;
        }
        let listed = ok(&router, case, "ListImages", &request).await;
        let pairs = listed_pairs(&listed);
        assert_eq!(
            pairs, expected,
            "case {case:?}: expected {expected:?}, got {pairs:?}"
        );
    }

    // Paging with every page size from one entry to one past the whole set walks the
    // same order exactly once.
    for page_size in 1..=(every_pair.len() + 1) {
        let mut seen: Vec<ImagePair> = Vec::new();
        let mut token: Option<String> = None;
        let case = format!("maxResults {page_size}");
        loop {
            let mut request = json!({"repositoryName": "listing", "maxResults": page_size});
            if let Some(token) = token.as_ref() {
                request["nextToken"] = json!(token);
            }
            let page = ok(&router, &case, "ListImages", &request).await;
            let ids = page["imageIds"].as_array().expect("an imageIds list");
            assert!(
                ids.len() <= page_size,
                "case {case:?}: a page may hold at most {page_size} entries, got {}",
                ids.len()
            );
            seen.extend(listed_pairs(&page));
            token = page["nextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        let walked: BTreeSet<_> = seen.iter().cloned().collect();
        assert_eq!(
            walked, every_pair,
            "case {case:?}: paging must walk every pair, got {walked:?}"
        );
        assert_eq!(
            walked.len(),
            seen.len(),
            "case {case:?}: paging must not repeat an entry, got {seen:?}"
        );
    }
}

/// `DescribeRepositories` pages through the stored repositories in one stable order,
/// for every page size.
#[tokio::test]
async fn describe_repositories_pages_in_a_stable_order() {
    let router = emulator();
    let names = ["alpha", "bravo", "charlie", "delta", "echo"];
    for name in names {
        given_repository(&router, name, json!({})).await;
    }

    for page_size in 1..=names.len() + 1 {
        let case = format!("maxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = token.as_ref() {
                request["nextToken"] = json!(token);
            }
            let page = ok(&router, &case, "DescribeRepositories", &request).await;
            let listed = page["repositories"].as_array().expect("a repositories list");
            assert!(
                listed.len() <= page_size,
                "case {case:?}: a page may hold at most {page_size} entries, got {}",
                listed.len()
            );
            seen.extend(
                listed
                    .iter()
                    .map(|repo| repo["repositoryName"].as_str().unwrap_or_default().to_string()),
            );
            token = page["nextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        let mut expected: Vec<String> = names.iter().map(ToString::to_string).collect();
        expected.sort();
        assert_eq!(
            seen, expected,
            "case {case:?}: paging must walk every repository once, in name order"
        );
    }

    // A token that was not minted by this service is refused rather than served.
    let bad = call(
        &router,
        "DescribeRepositories",
        &json!({"nextToken": "not-a-token"}),
    )
    .await;
    assert_eq!(
        bad.status, 400,
        "an unreadable nextToken is refused, got {} with {:?}",
        bad.status, bad.body
    );
    assert_eq!(
        bad.error_code(),
        "InvalidParameterException",
        "expected InvalidParameterException for a bad token, got {:?}",
        bad.body
    );
}

/// An `imageIds` entry that names no stored image is refused, however it names it.
#[tokio::test]
async fn describe_images_refuses_an_identifier_that_names_no_stored_image() {
    let router = emulator();
    given_repository(&router, "lookups", json!({})).await;
    let manifest = "{\"schemaVersion\":2,\"image\":\"only\"}";
    let digest = manifest_digest(manifest);
    ok(
        &router,
        "seed",
        "PutImage",
        &json!({"repositoryName": "lookups", "imageManifest": manifest, "imageTag": "v1"}),
    )
    .await;

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a tag nothing carries",
            json!({"imageTag": "nope"}),
            "ImageNotFoundException",
        ),
        (
            "a digest nothing was pushed under",
            json!({"imageDigest": manifest_digest("{\"other\":true}")}),
            "ImageNotFoundException",
        ),
        (
            "a stored digest with a tag that is not on it",
            json!({"imageDigest": digest.clone(), "imageTag": "nope"}),
            "ImageNotFoundException",
        ),
        (
            "an entry naming neither a digest nor a tag",
            json!({}),
            "InvalidParameterException",
        ),
    ];

    for (case, image_id, expected) in cases {
        let answer = call(
            &router,
            "DescribeImages",
            &json!({"repositoryName": "lookups", "imageIds": [image_id]}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            expected,
            "case {case:?}: expected {expected}, got {:?}",
            answer.body
        );
    }

    // The identifier that does name the stored image resolves, by either part.
    for (case, image_id) in [
        ("by digest", json!({"imageDigest": digest.clone()})),
        ("by tag", json!({"imageTag": "v1"})),
        (
            "by both",
            json!({"imageDigest": digest.clone(), "imageTag": "v1"}),
        ),
    ] {
        let described = ok(
            &router,
            case,
            "DescribeImages",
            &json!({"repositoryName": "lookups", "imageIds": [image_id]}),
        )
        .await;
        assert_eq!(
            at(&described, "/imageDetails/0/imageDigest"),
            json!(digest.clone()),
            "case {case:?}: expected the stored image, got {:?}",
            described["imageDetails"]
        );
    }
}

/// A repository that still holds images is deleted only with `force`; without it the
/// call is refused and the repository and its images stay. After a forced delete the
/// repository is gone, and so is every operation's view of it.
#[tokio::test]
async fn delete_repository_honours_force_and_removes_the_images_with_it() {
    let cases = [
        ("an empty repository", 0_usize),
        ("one image", 1),
        ("three images", 3),
    ];

    for (case, image_count) in cases {
        let router = emulator();
        let name = "deletable";
        let created = given_repository(&router, name, json!({})).await;
        let stored = at(&created, "/repository");
        for index in 0..image_count {
            ok(
                &router,
                case,
                "PutImage",
                &json!({
                    "repositoryName": name,
                    "imageManifest": format!("{{\"schemaVersion\":2,\"n\":{index}}}"),
                    "imageTag": format!("v{index}"),
                }),
            )
            .await;
        }

        let unforced = call(
            &router,
            "DeleteRepository",
            &json!({"repositoryName": name, "force": false}),
        )
        .await;
        if image_count == 0 {
            assert_eq!(
                unforced.status, 200,
                "case {case:?}: an empty repository needs no force, got {} with {:?}",
                unforced.status, unforced.body
            );
            assert_eq!(
                at(&unforced.body, "/repository"),
                stored,
                "case {case:?}: the delete returns the repository as it was stored"
            );
        } else {
            assert_eq!(
                unforced.status, 400,
                "case {case:?}: expected HTTP 400 without force, got {} with {:?}",
                unforced.status, unforced.body
            );
            assert_eq!(
                unforced.error_code(),
                "RepositoryNotEmptyException",
                "case {case:?}: expected RepositoryNotEmptyException, got {:?}",
                unforced.body
            );

            // The refusal changed nothing: the repository and its images are still there.
            let described = ok(
                &router,
                case,
                "DescribeRepositories",
                &json!({"repositoryNames": [name]}),
            )
            .await;
            assert_eq!(
                at(&described, "/repositories"),
                json!([stored]),
                "case {case:?}: a refused delete leaves the repository as it was"
            );
            let listed = ok(&router, case, "ListImages", &json!({"repositoryName": name})).await;
            assert_eq!(
                listed_pairs(&listed).len(),
                image_count,
                "case {case:?}: a refused delete leaves every image, got {:?}",
                listed["imageIds"]
            );

            let forced = ok(
                &router,
                case,
                "DeleteRepository",
                &json!({"repositoryName": name, "force": true}),
            )
            .await;
            assert_eq!(
                at(&forced, "/repository"),
                stored,
                "case {case:?}: the forced delete returns the repository as it was stored"
            );
        }

        // Whatever the row, the repository is gone now and every operation says so.
        for (operation, request) in [
            (
                "DescribeRepositories",
                json!({"repositoryNames": [name]}),
            ),
            ("ListImages", json!({"repositoryName": name})),
            ("DeleteRepository", json!({"repositoryName": name})),
        ] {
            let answer = call(&router, operation, &request).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400 from {operation} after the delete, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "RepositoryNotFoundException",
                "case {case:?}: expected RepositoryNotFoundException from {operation}, got {:?}",
                answer.body
            );
        }

        // And the name is free again: creating it anew succeeds and holds no images.
        let recreated = given_repository(&router, name, json!({})).await;
        assert_ne!(
            at(&recreated, "/repository/createdAt"),
            Value::Null,
            "case {case:?}: the recreated repository carries its own creation instant"
        );
        let listed = ok(&router, case, "ListImages", &json!({"repositoryName": name})).await;
        assert!(
            listed_pairs(&listed).is_empty(),
            "case {case:?}: a recreated repository holds none of the deleted images, got {:?}",
            listed["imageIds"]
        );
    }
}

/// The store is bounded: past its caps a create and a push are refused with the
/// `LimitExceededException` both operations model, rather than growing without end.
/// A page is bounded too — a request may not ask for more than the documented maximum.
#[tokio::test]
async fn the_store_and_the_page_it_serves_are_bounded() {
    use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::{
        DEFAULT_PAGE_SIZE, MAX_IMAGES_PER_REPOSITORY, MAX_REPOSITORIES,
    };

    let router = emulator();

    // Fill the registry to its cap, then one more.
    for index in 0..MAX_REPOSITORIES {
        ok(
            &router,
            "filling the registry",
            "CreateRepository",
            &json!({"repositoryName": format!("repo-{index:05}")}),
        )
        .await;
    }
    let refused = call(
        &router,
        "CreateRepository",
        &json!({"repositoryName": "one-too-many"}),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "a create past the cap is refused, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "LimitExceededException",
        "expected LimitExceededException past {MAX_REPOSITORIES} repositories, got {:?}",
        refused.body
    );

    // A page is never larger than the documented maximum, however much is asked for.
    let page = ok(
        &router,
        "a page larger than the maximum",
        "DescribeRepositories",
        &json!({"maxResults": 1000}),
    )
    .await;
    assert_eq!(
        page["repositories"].as_array().map(Vec::len),
        Some(DEFAULT_PAGE_SIZE),
        "a page holds at most {DEFAULT_PAGE_SIZE} entries, got {:?}",
        page["repositories"].as_array().map(Vec::len)
    );
    assert!(
        page["nextToken"].is_string(),
        "a truncated page carries the token for the next one, got {:?}",
        page["nextToken"]
    );
    // And a non-positive page size is refused rather than read as "nothing".
    let bad_page = call(
        &router,
        "DescribeRepositories",
        &json!({"maxResults": 0}),
    )
    .await;
    assert_eq!(
        bad_page.status, 400,
        "maxResults 0 is refused, got {} with {:?}",
        bad_page.status, bad_page.body
    );

    // The same bound holds for the images one repository holds.
    let name = "repo-00000";
    for index in 0..MAX_IMAGES_PER_REPOSITORY {
        ok(
            &router,
            "filling the repository",
            "PutImage",
            &json!({
                "repositoryName": name,
                "imageManifest": format!("{{\"schemaVersion\":2,\"n\":{index}}}"),
            }),
        )
        .await;
    }
    let refused = call(
        &router,
        "PutImage",
        &json!({
            "repositoryName": name,
            "imageManifest": "{\"schemaVersion\":2,\"n\":\"one too many\"}",
        }),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "a push past the cap is refused, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "LimitExceededException",
        "expected LimitExceededException past {MAX_IMAGES_PER_REPOSITORY} images, got {:?}",
        refused.body
    );
}

/// Every operation that names a repository refuses one the registry does not hold — with
/// the modelled error, never a 501.
#[tokio::test]
async fn an_unknown_repository_is_refused_by_every_operation_that_names_one() {
    let router = emulator();
    given_repository(&router, "present", json!({})).await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "DescribeRepositories",
            json!({"repositoryNames": ["missing-repo"]}),
        ),
        (
            "DescribeRepositories, mixed with a name that does exist",
            json!({"repositoryNames": ["present", "missing-repo"]}),
        ),
        ("ListImages", json!({"repositoryName": "missing-repo"})),
        ("DescribeImages", json!({"repositoryName": "missing-repo"})),
        (
            "PutImage",
            json!({
                "repositoryName": "missing-repo",
                "imageManifest": "{\"schemaVersion\":2}",
                "imageTag": "v1",
            }),
        ),
        (
            "DeleteRepository",
            json!({"repositoryName": "missing-repo", "force": true}),
        ),
        (
            "PutImageScanningConfiguration",
            json!({
                "repositoryName": "missing-repo",
                "imageScanningConfiguration": {"scanOnPush": true},
            }),
        ),
    ];

    for (case, request) in cases {
        let operation = case.split(',').next().unwrap_or(case);
        let answer = call(&router, operation, &request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "RepositoryNotFoundException",
            "case {case:?}: expected RepositoryNotFoundException, got {:?}",
            answer.body
        );
    }
}

/// A request naming a registry other than this emulator's is refused: there is one
/// registry here, and a repository in another account's registry is not this one's.
#[tokio::test]
async fn a_request_naming_another_registry_is_refused() {
    let router = emulator();
    given_repository(&router, "present", json!({})).await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "CreateRepository",
            json!({"repositoryName": "elsewhere", "registryId": "123456789012"}),
        ),
        (
            "DescribeRepositories",
            json!({"registryId": "123456789012"}),
        ),
        (
            "ListImages",
            json!({"repositoryName": "present", "registryId": "123456789012"}),
        ),
        (
            "DescribeImages",
            json!({"repositoryName": "present", "registryId": "123456789012"}),
        ),
        (
            "DeleteRepository",
            json!({"repositoryName": "present", "registryId": "123456789012"}),
        ),
        (
            "PutImageScanningConfiguration",
            json!({
                "repositoryName": "present",
                "registryId": "123456789012",
                "imageScanningConfiguration": {"scanOnPush": true},
            }),
        ),
    ];

    for (operation, request) in cases {
        let answer = call(&router, operation, &request).await;
        assert_eq!(
            answer.status, 400,
            "case {operation:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidParameterException",
            "case {operation:?}: expected InvalidParameterException, got {:?}",
            answer.body
        );
    }
}

/// `CreateRepository` refuses a configuration it cannot honour, with the error the model
/// declares for that kind of refusal.
#[tokio::test]
async fn create_repository_refuses_a_configuration_it_cannot_honour() {
    let many_tags: Vec<Value> = (0..51)
        .map(|index| json!({"Key": format!("key-{index}"), "Value": "v"}))
        .collect();
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            // The boundary validator refuses a value outside `EncryptionType`'s enum
            // before the handler runs, and this service states such a refusal with the
            // client-input error its model declares (see `src/service.rs`).
            "an encryption type outside the modelled set",
            json!({"repositoryName": "bad-encryption", "encryptionConfiguration": {
                "encryptionType": "aes256",
            }}),
            "InvalidParameterException",
        ),
        (
            "a KMS key beside the AES256 encryption type",
            json!({"repositoryName": "aes-with-key", "encryptionConfiguration": {
                "encryptionType": "AES256",
                "kmsKey": "arn:aws:kms:eu-west-1:000000000000:key/abcd",
            }}),
            "InvalidParameterException",
        ),
        (
            "more tags than a repository can carry",
            json!({"repositoryName": "over-tagged", "tags": many_tags}),
            "TooManyTagsException",
        ),
        (
            // The same rule `PutImageTagMutability` applies: a filter beside a setting
            // that has no exclusion to apply could never be consulted.
            "exclusion filters beside a setting that excludes nothing",
            json!({"repositoryName": "filters-without-exclusion",
                   "imageTagMutability": "IMMUTABLE",
                   "imageTagMutabilityExclusionFilters": [
                       {"filterType": "WILDCARD", "filter": "release-*"},
                   ]}),
            "InvalidParameterException",
        ),
        (
            "a tag key past its maximum character length",
            json!({"repositoryName": "long-key", "tags": [
                {"Key": "k".repeat(129), "Value": "v"},
            ]}),
            "InvalidTagParameterException",
        ),
        (
            "a tag value past its maximum length",
            json!({"repositoryName": "long-value", "tags": [
                {"Key": "k", "Value": "v".repeat(257)},
            ]}),
            "InvalidTagParameterException",
        ),
    ];

    let router = emulator();
    for (case, request, expected) in cases {
        let answer = call(&router, "CreateRepository", &request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            expected,
            "case {case:?}: expected {expected}, got {:?}",
            answer.body
        );
        // A refused create stores nothing.
        let name = request["repositoryName"].as_str().unwrap_or_default();
        let described = call(
            &router,
            "DescribeRepositories",
            &json!({"repositoryNames": [name]}),
        )
        .await;
        assert_eq!(
            described.error_code(),
            "RepositoryNotFoundException",
            "case {case:?}: a refused create must store no repository, got {:?}",
            described.body
        );
    }
}
