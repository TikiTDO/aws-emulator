// SPDX-License-Identifier: CC0-1.0

//! The Docker Registry v2 data plane — the endpoint a `docker pull` talks to — end to end
//! over the wire, with CloudWatch mounted beside ECR on ONE engine so a pull's metric is
//! asserted where a client reads it (`GetMetricStatistics`).
//!
//! The RULE asserted over the tables below: **a request on this registry's `/v2` path
//! space, presenting the `docker login` credential `GetAuthorizationToken` minted, is
//! served by ECR from what the named repository HOLDS — the manifest a reference (a tag or
//! a digest) resolves to, in a schema the request's `Accept` admits, and the bytes of a
//! blob by digest — and a manifest `GET` is the pull itself, so ECR counts one `AWS/ECR`
//! `RepositoryPullCount` datapoint for that repository and stamps the image's
//! `lastRecordedPullTime`.** A request presenting no credential this registry honours
//! reaches no repository, and a repository nothing pulled from publishes no datapoint at
//! all.
//!
//! The rows differ along what the rule generalises over: which reference form addresses the
//! manifest, how many namespace levels the repository name carries, which method the client
//! uses, which schemas it accepts, how many pulls it makes, and which credential it
//! presents.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921 as ecr;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{at, given_layer, ok, REGION, REGISTRY_ID};

use ecr::state;
use ecr::state::pull_metrics::{
    METRIC_NAMESPACE, PULL_COUNT_UNIT, REPOSITORY_NAME_DIMENSION, REPOSITORY_PULL_COUNT_METRIC,
};

/// The media type of the Docker image manifest V2 schema 2, which this suite pushes in.
/// Spelled out here rather than imported so a wrong media type in the implementation fails
/// these tests (`smithy_query(query="doc", shape="BatchGetImageRequest",
/// member="acceptedMediaTypes")` lists it as a value a client may accept).
const DOCKER_V2_MANIFEST: &str = "application/vnd.docker.distribution.manifest.v2+json";

/// The media type of the OCI image manifest — the other interchangeable image-manifest
/// schema, from the same list.
const OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";

/// The media type the Docker schema spells an image configuration blob with.
const DOCKER_CONFIG_MEDIA_TYPE: &str = "application/vnd.docker.container.image.v1+json";

/// The media type the Docker schema spells a gzipped filesystem layer with.
const DOCKER_LAYER_MEDIA_TYPE: &str = "application/vnd.docker.image.rootfs.diff.tar.gzip";

/// The media type a blob is served under: a layer is opaque bytes to a registry.
const BLOB_CONTENT_TYPE: &str = "application/octet-stream";

/// The media type the registry API's own documents carry: the version check's body and
/// every error document.
const REGISTRY_JSON: &str = "application/json";

/// The header a registry states its API version in, and the value for this API.
const API_VERSION_HEADER: &str = "docker-distribution-api-version";

/// See [`API_VERSION_HEADER`].
const API_VERSION: &str = "registry/2.0";

/// The header a registry states the content digest of what it served in.
const CONTENT_DIGEST_HEADER: &str = "docker-content-digest";

/// The registry host every request in this suite is addressed to: the endpoint ECR
/// publishes for this registry in this Region, which is the host `docker pull` uses.
fn registry_host() -> String {
    format!("{REGISTRY_ID}.dkr.ecr.{REGION}.amazonaws.com")
}

/// ECR plus CloudWatch on ONE engine — the shape `core/router/src/lib.rs` mounts them in,
/// so a metric this registry publishes is read through CloudWatch's own API.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(ecr::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// What a registry request was answered: the status, the headers a registry answer carries,
/// the raw bytes and the body as text.
struct Fetched {
    status: u16,
    content_type: String,
    content_length: String,
    content_digest: String,
    api_version: String,
    challenge: String,
    allow: String,
    bytes: Bytes,
    body: String,
}

impl Fetched {
    /// The error code the registry's error document states, or the empty string when the
    /// answer is not one.
    fn code(&self) -> String {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|document| {
                document["errors"][0]["code"]
                    .as_str()
                    .map(ToString::to_string)
            })
            .unwrap_or_default()
    }

    /// The message it states, or the empty string.
    fn message(&self) -> String {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|document| {
                document["errors"][0]["message"]
                    .as_str()
                    .map(ToString::to_string)
            })
            .unwrap_or_default()
    }
}

/// One request to the registry endpoint, exactly as a container client makes it: the
/// method, the `/v2` path, the registry host, and whatever credential and `Accept` the row
/// presents — never a SigV4 signature, which is the whole point of this plane.
async fn fetch(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    credential: Option<&str>,
    accept: Option<&str>,
) -> Fetched {
    let mut request = ::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", registry_host());
    if let Some(credential) = credential {
        request = request.header("authorization", credential);
    }
    if let Some(accept) = accept {
        request = request.header("accept", accept);
    }
    let request = request
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let content_type = header("content-type");
    let content_length = header("content-length");
    let content_digest = header(CONTENT_DIGEST_HEADER);
    let api_version = header(API_VERSION_HEADER);
    let challenge = header("www-authenticate");
    let allow = header("allow");
    let bytes = response.into_body();
    let body = String::from_utf8_lossy(&bytes).to_string();
    Fetched {
        status,
        content_type,
        content_length,
        content_digest,
        api_version,
        challenge,
        allow,
        bytes,
        body,
    }
}

/// The `docker login` credential this registry issues, as a client presents it: the token
/// IS the base64 of `AWS:<password>`, so `Basic <token>` is the header `docker login`
/// sends.
async fn given_credential(router: &MultiServiceRouter) -> String {
    let issued = ok(router, "credential", "GetAuthorizationToken", &json!({})).await;
    let token = at(&issued, "/authorizationData/0/authorizationToken")
        .as_str()
        .unwrap_or_else(|| panic!("GetAuthorizationToken must issue a token, got {issued}"))
        .to_string();
    format!("Basic {token}")
}

/// One image pushed into `repository` through the documented sequence, and what a pull of
/// it must answer with.
struct Pushed {
    /// The manifest document as stored, byte for byte.
    manifest: String,
    /// The digest the registry addresses it by.
    digest: String,
    /// The one layer blob of the image: its digest and its bytes.
    layer: (String, Vec<u8>),
}

/// Create `repository`, upload the blobs of one image into it and push the manifest under
/// `tag` — the whole documented push sequence, so what a pull serves is what a push stored.
async fn given_image(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    tag: &str,
) -> Pushed {
    ok(
        router,
        case,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;
    let config_bytes = format!("{repository}-config").into_bytes();
    let config = given_layer(router, case, repository, &config_bytes).await;
    let layer_bytes = format!("{repository}-layer-bytes").into_bytes();
    let layer = given_layer(router, case, repository, &layer_bytes).await;
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": DOCKER_V2_MANIFEST,
        "config": {
            "mediaType": DOCKER_CONFIG_MEDIA_TYPE,
            "digest": config,
            "size": config_bytes.len(),
        },
        "layers": [{
            "mediaType": DOCKER_LAYER_MEDIA_TYPE,
            "digest": layer,
            "size": layer_bytes.len(),
        }],
    })
    .to_string();
    ok(
        router,
        case,
        "PutImage",
        &json!({
            "repositoryName": repository,
            "imageManifest": manifest,
            "imageTag": tag,
        }),
    )
    .await;
    Pushed {
        digest: state::manifest_digest(&manifest),
        manifest,
        layer: (layer, layer_bytes),
    }
}

/// One CloudWatch call, on the awsJson wire CloudWatch answers beside its query one.
async fn cloudwatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("monitoring.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("GraniteServiceVersion20100801.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/monitoring/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// Epoch seconds, for the window a statistics read asks about.
fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The pull-count datapoints CloudWatch holds for `repository`, read at the one-minute
/// period the series is published at.
async fn pull_count_datapoints(router: &MultiServiceRouter, repository: &str) -> Vec<Value> {
    let now = epoch_seconds();
    let (status, stats) = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": METRIC_NAMESPACE,
            "MetricName": REPOSITORY_PULL_COUNT_METRIC,
            "Dimensions": [{ "Name": REPOSITORY_NAME_DIMENSION, "Value": repository }],
            "StartTime": now - 600.0,
            "EndTime": now + 600.0,
            "Period": 60,
            "Statistics": ["Sum", "SampleCount"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {repository:?}: GetMetricStatistics answered {status} {stats}"
    );
    stats["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// How many pulls CloudWatch reports for `repository`: the `Sum` over every datapoint, and
/// how many datapoints there were — a series that does not exist has none.
async fn pull_count(router: &MultiServiceRouter, repository: &str) -> (usize, f64) {
    let points = pull_count_datapoints(router, repository).await;
    let sum = points
        .iter()
        .filter_map(|point| point["Sum"].as_f64())
        .sum::<f64>();
    (points.len(), sum)
}

/// The `lastRecordedPullTime` `DescribeImages` reports for the one image of `repository`.
async fn last_recorded_pull_time(router: &MultiServiceRouter, repository: &str) -> Value {
    let described = ok(
        router,
        repository,
        "DescribeImages",
        &json!({"repositoryName": repository}),
    )
    .await;
    at(&described, "/imageDetails/0/lastRecordedPullTime")
}

/// A manifest `GET` or `HEAD` is served from what the repository holds, for every form a
/// reference takes and every repository name shape.
///
/// The rows differ along what the rule generalises over: the reference form (a tag, a
/// second tag on the same image, the content digest), the namespace levels of the
/// repository name, and the read method. The report's own case is the first row — a tag on
/// a flat repository name, fetched with `GET`.
#[tokio::test]
async fn a_manifest_is_served_for_every_reference_form() {
    let (router, _engine) = emulator();
    let credential = given_credential(&router).await;

    let flat = given_image(&router, "flat", "sim-registry-pull", "v1").await;
    let nested = given_image(&router, "nested", "team/platform/service", "latest").await;
    // A second tag on the stored image: a reference is resolved through the tag index, so
    // every tag pointing at the image addresses the same manifest.
    ok(
        &router,
        "second tag",
        "PutImage",
        &json!({
            "repositoryName": "sim-registry-pull",
            "imageManifest": flat.manifest,
            "imageTag": "release",
        }),
    )
    .await;

    // (case, method, repository, reference, the image the reference addresses)
    let rows: &[(&str, &str, &str, &str, &Pushed)] = &[
        ("by tag", "GET", "sim-registry-pull", "v1", &flat),
        (
            "by a second tag on the same image",
            "GET",
            "sim-registry-pull",
            "release",
            &flat,
        ),
        (
            "by content digest",
            "GET",
            "sim-registry-pull",
            &flat.digest,
            &flat,
        ),
        (
            "a repository name with namespace levels",
            "GET",
            "team/platform/service",
            "latest",
            &nested,
        ),
        (
            "HEAD reports on the manifest without its bytes",
            "HEAD",
            "sim-registry-pull",
            "v1",
            &flat,
        ),
    ];

    for (case, method, repository, reference, image) in rows {
        let path = format!("/v2/{repository}/manifests/{reference}");
        let fetched = fetch(&router, method, &path, Some(&credential), None).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected HTTP 200 from {path}, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.content_type, DOCKER_V2_MANIFEST,
            "case {case:?}: expected the stored manifest's media type, got {:?}",
            fetched.content_type
        );
        assert_eq!(
            fetched.content_digest, image.digest,
            "case {case:?}: expected the manifest's own digest, got {:?}",
            fetched.content_digest
        );
        assert_eq!(
            fetched.content_length,
            image.manifest.len().to_string(),
            "case {case:?}: expected the manifest's length, got {:?}",
            fetched.content_length
        );
        assert_eq!(
            fetched.api_version, API_VERSION,
            "case {case:?}: expected the registry API version header, got {:?}",
            fetched.api_version
        );
        let expected_body = if *method == "GET" {
            image.manifest.clone()
        } else {
            String::new()
        };
        assert_eq!(
            fetched.body, expected_body,
            "case {case:?}: expected the body {expected_body:?}, got {:?}",
            fetched.body
        );
    }
}

/// A manifest `GET` is the pull: it counts one `RepositoryPullCount` datapoint for the
/// repository and records the image's pull time. A `HEAD` of the same address does
/// neither, and a repository nothing pulled from has no series at all.
///
/// The rows differ along what the rule generalises over: how many pulls are made (one,
/// several), and how the repository was read (pulled, probed with `HEAD`, never touched).
#[tokio::test]
async fn a_pull_counts_the_repository_and_records_the_pull_time() {
    let (router, _engine) = emulator();
    let credential = given_credential(&router).await;

    // (case, repository, how many manifest GETs, how many HEADs)
    let rows: &[(&str, &str, usize, usize)] = &[
        ("one pull", "pulled-once", 1, 0),
        ("three pulls", "pulled-thrice", 3, 0),
        ("probed with HEAD only", "probed-only", 0, 2),
        ("never pulled", "never-pulled", 0, 0),
        ("probed and pulled", "probed-and-pulled", 2, 1),
    ];

    for (case, repository, pulls, probes) in rows {
        let image = given_image(&router, case, repository, "v1").await;
        let path = format!("/v2/{repository}/manifests/v1");
        for _ in 0..*probes {
            let probed = fetch(&router, "HEAD", &path, Some(&credential), None).await;
            assert_eq!(
                probed.status, 200,
                "case {case:?}: a HEAD of a stored manifest must answer 200, got {} {:?}",
                probed.status, probed.body
            );
        }
        for _ in 0..*pulls {
            let pulled = fetch(&router, "GET", &path, Some(&credential), None).await;
            assert_eq!(
                pulled.status, 200,
                "case {case:?}: a GET of a stored manifest must answer 200, got {} {:?}",
                pulled.status, pulled.body
            );
            assert_eq!(
                pulled.body, image.manifest,
                "case {case:?}: a pull must serve the stored manifest, got {:?}",
                pulled.body
            );
        }

        let (points, sum) = pull_count(&router, repository).await;
        let expected = *pulls as f64;
        assert_eq!(
            sum, expected,
            "case {case:?}: expected {expected} pulls counted, got {sum} over {points} datapoints"
        );
        if *pulls == 0 {
            assert_eq!(
                points, 0,
                "case {case:?}: a repository nothing pulled from must publish no datapoint, \
                 got {points}"
            );
        } else {
            assert!(
                points > 0,
                "case {case:?}: expected a published series, got {points} datapoints"
            );
        }

        let recorded = last_recorded_pull_time(&router, repository).await;
        if *pulls == 0 {
            assert_eq!(
                recorded,
                Value::Null,
                "case {case:?}: nothing pulled the image, so no pull time may be recorded, \
                 got {recorded:?}"
            );
        } else {
            assert!(
                recorded.as_f64().is_some(),
                "case {case:?}: expected a recorded pull time after {pulls} pulls, got \
                 {recorded:?}"
            );
        }
    }

    // Every counted pull carries the unit a tally is published in, and the series is the
    // one dimensioned on the repository that was pulled from.
    let points = pull_count_datapoints(&router, "pulled-thrice").await;
    for point in &points {
        assert_eq!(
            point["Unit"].as_str(),
            Some(PULL_COUNT_UNIT),
            "expected every pull-count datapoint to carry the unit {PULL_COUNT_UNIT}, got \
             {point:?}"
        );
    }
}

/// A blob `GET` serves the registered bytes, byte for byte, whatever they are.
///
/// The rows differ along what the rule generalises over: the size and content of the blob
/// (one byte, short text, bytes that are not text at all, a larger blob), and the read
/// method.
#[tokio::test]
async fn a_blob_is_served_byte_for_byte() {
    let (router, _engine) = emulator();
    let credential = given_credential(&router).await;
    let repository = "blob-serving";
    ok(
        &router,
        "seed",
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;

    // (case, the bytes the layer carries)
    let rows: Vec<(&str, Vec<u8>)> = vec![
        ("one byte", vec![0x7f]),
        ("short text", b"a layer of text".to_vec()),
        ("bytes that are not text", (0u8..=255).collect()),
        ("a larger blob", vec![0xa5; 64 * 1024]),
    ];

    for (case, content) in &rows {
        let digest = given_layer(&router, case, repository, content).await;
        let path = format!("/v2/{repository}/blobs/{digest}");
        let fetched = fetch(&router, "GET", &path, Some(&credential), None).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected HTTP 200 from {path}, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.bytes.as_ref(),
            content.as_slice(),
            "case {case:?}: expected the registered bytes, got {} bytes",
            fetched.bytes.len()
        );
        assert_eq!(
            fetched.content_type, BLOB_CONTENT_TYPE,
            "case {case:?}: expected {BLOB_CONTENT_TYPE}, got {:?}",
            fetched.content_type
        );
        assert_eq!(
            fetched.content_digest, digest,
            "case {case:?}: expected the blob's own digest, got {:?}",
            fetched.content_digest
        );

        let probed = fetch(&router, "HEAD", &path, Some(&credential), None).await;
        assert_eq!(
            probed.status, 200,
            "case {case:?}: a HEAD of a held blob must answer 200, got {}",
            probed.status
        );
        assert_eq!(
            probed.content_length,
            content.len().to_string(),
            "case {case:?}: a HEAD must report the blob's length, got {:?}",
            probed.content_length
        );
        assert!(
            probed.bytes.is_empty(),
            "case {case:?}: a HEAD must carry no body, got {} bytes",
            probed.bytes.len()
        );
    }

    // A fetch of a blob is a pure read: the pull count belongs to the manifest GET that is
    // the pull, so fetching blobs counts nothing.
    let (points, sum) = pull_count(&router, repository).await;
    assert_eq!(
        (points, sum),
        (0, 0.0),
        "a blob fetch must count no pull, got {sum} over {points} datapoints"
    );
}

/// What this registry refuses, in the registry API's own envelope.
///
/// The rows differ along what the rule generalises over: what is missing (the repository,
/// the tag, the digest, the blob) and which endpoint was asked for (one this registry
/// serves, one it does not, a method it does not serve).
#[tokio::test]
async fn a_registry_request_that_addresses_nothing_is_refused_in_the_registry_envelope() {
    let (router, _engine) = emulator();
    let credential = given_credential(&router).await;
    let held = given_image(&router, "seed", "refusal-probe", "v1").await;
    let absent_digest = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

    // (case, method, path, status, code, message)
    let rows: Vec<(&str, &str, String, u16, &str, &str)> = vec![
        (
            "a repository this registry does not hold",
            "GET",
            "/v2/no-such-repository/manifests/v1".to_string(),
            404,
            "NAME_UNKNOWN",
            "repository name not known to registry",
        ),
        (
            "a tag the repository does not carry",
            "GET",
            "/v2/refusal-probe/manifests/absent".to_string(),
            404,
            "MANIFEST_UNKNOWN",
            "manifest unknown",
        ),
        (
            "a digest the repository holds no manifest for",
            "GET",
            format!("/v2/refusal-probe/manifests/{absent_digest}"),
            404,
            "MANIFEST_UNKNOWN",
            "manifest unknown",
        ),
        (
            "a digest the repository holds no blob for",
            "GET",
            format!("/v2/refusal-probe/blobs/{absent_digest}"),
            404,
            "BLOB_UNKNOWN",
            "blob unknown to registry",
        ),
        (
            "a value that is not a digest at all",
            "GET",
            "/v2/refusal-probe/blobs/not-a-digest".to_string(),
            404,
            "BLOB_UNKNOWN",
            "blob unknown to registry",
        ),
        (
            "a blob of a repository this registry does not hold",
            "GET",
            format!("/v2/no-such-repository/blobs/{}", held.layer.0),
            404,
            "NAME_UNKNOWN",
            "repository name not known to registry",
        ),
        (
            "an endpoint this registry does not serve",
            "GET",
            "/v2/refusal-probe/tags/list".to_string(),
            405,
            "UNSUPPORTED",
            "the operation is unsupported",
        ),
        (
            "an upload session this registry never opened",
            "GET",
            "/v2/refusal-probe/blobs/uploads/abcdef".to_string(),
            404,
            "BLOB_UPLOAD_UNKNOWN",
            "blob upload unknown",
        ),
        (
            "a method this plane does not serve",
            "POST",
            "/v2/refusal-probe/manifests/v1".to_string(),
            405,
            "UNSUPPORTED",
            "the operation is unsupported",
        ),
    ];

    for (case, method, path, status, code, message) in &rows {
        let fetched = fetch(&router, method, path, Some(&credential), None).await;
        assert_eq!(
            fetched.status, *status,
            "case {case:?}: expected HTTP {status} from {path}, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.code(),
            *code,
            "case {case:?}: expected the code {code}, got {:?}",
            fetched.body
        );
        assert_eq!(
            fetched.message(),
            *message,
            "case {case:?}: expected the message {message:?}, got {:?}",
            fetched.body
        );
        assert_eq!(
            fetched.content_type, REGISTRY_JSON,
            "case {case:?}: expected the registry's own error document, got {:?}",
            fetched.content_type
        );
        if *status == 405 {
            assert_eq!(
                fetched.allow, if *method == "POST" { "GET, HEAD, PUT" } else { "GET, HEAD" },
                "case {case:?}: a 405 must state the methods served, got {:?}",
                fetched.allow
            );
        }
    }

    // Nothing above addressed a stored image, so nothing above is a pull.
    let (points, sum) = pull_count(&router, "refusal-probe").await;
    assert_eq!(
        (points, sum),
        (0, 0.0),
        "a refused request must count no pull, got {sum} over {points} datapoints"
    );
}

/// Only the credential this registry issued and still honours reaches a repository.
///
/// The rows differ along what the rule generalises over: no credential at all, a scheme
/// this plane does not accept, a password this registry never minted (under the published
/// user name and under another), a credential that is not base64, and a credential whose
/// window has closed.
#[tokio::test]
async fn only_an_honoured_credential_reaches_a_repository() {
    let (router, engine) = emulator();
    let credential = given_credential(&router).await;
    let image = given_image(&router, "seed", "credential-probe", "v1").await;
    let path = "/v2/credential-probe/manifests/v1";

    // A credential this registry issued for a window that has already closed, recorded in
    // the store the data plane reads: no sequence of calls can age one out inside a test,
    // so this row is seeded where `GetAuthorizationToken` writes.
    let expired_password = "a-credential-whose-window-has-closed";
    let state_handle = state::service_state(&*engine)
        .unwrap_or_else(|| panic!("the ecr store must be reachable through the shared engine"));
    let issued_at = SystemTime::now() - Duration::from_secs(60 * 60 * 24);
    state_handle.lock().record_registry_credential(
        expired_password,
        issued_at,
        issued_at + Duration::from_secs(60),
        "arn:aws:iam::000000000000:root",
    );
    let expired = format!(
        "Basic {}",
        aws_smithy_types::base64::encode(format!("AWS:{expired_password}"))
    );
    let never_minted = format!(
        "Basic {}",
        aws_smithy_types::base64::encode("AWS:never-minted-by-this-registry")
    );
    let wrong_user = format!(
        "Basic {}",
        aws_smithy_types::base64::encode("root:never-minted-by-this-registry")
    );
    let not_base64 = "Basic ~~~not-base64~~~".to_string();
    let bearer = credential.replacen("Basic", "Bearer", 1);

    // (case, the credential presented)
    let rows: Vec<(&str, Option<String>)> = vec![
        ("no credential at all", None),
        ("a password this registry never minted", Some(never_minted)),
        (
            "another user name with a password this registry never minted",
            Some(wrong_user),
        ),
        ("a credential whose window has closed", Some(expired)),
        ("a scheme this plane does not accept", Some(bearer)),
        ("a credential that is not base64", Some(not_base64)),
    ];

    for (case, presented) in &rows {
        let fetched = fetch(&router, "GET", path, presented.as_deref(), None).await;
        assert_eq!(
            fetched.status, 401,
            "case {case:?}: expected HTTP 401, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.code(),
            "UNAUTHORIZED",
            "case {case:?}: expected the code UNAUTHORIZED, got {:?}",
            fetched.body
        );
        assert_eq!(
            fetched.message(),
            "authentication required",
            "case {case:?}: expected the registry's own message, got {:?}",
            fetched.body
        );
        assert!(
            !fetched.body.contains("schemaVersion"),
            "case {case:?}: a refused request must learn nothing about the manifest, got {:?}",
            fetched.body
        );
        // The challenge names the scheme and the registry endpoint a client fetches a
        // credential for, which is how `docker login` discovers where to authenticate.
        let expected_challenge = format!(
            "Basic realm=\"https://{}/\",service=\"ecr.amazonaws.com\"",
            registry_host()
        );
        assert_eq!(
            fetched.challenge, expected_challenge,
            "case {case:?}: expected the challenge {expected_challenge:?}, got {:?}",
            fetched.challenge
        );
    }

    // The honoured credential, for the same address: the one row that is served.
    let served = fetch(&router, "GET", path, Some(&credential), None).await;
    assert_eq!(
        served.status, 200,
        "case \"the issued credential\": expected HTTP 200, got {} with {:?}",
        served.status, served.body
    );
    assert_eq!(
        served.body, image.manifest,
        "case \"the issued credential\": expected the stored manifest, got {:?}",
        served.body
    );

    // Not one refused attempt was a pull.
    let (_points, sum) = pull_count(&router, "credential-probe").await;
    assert_eq!(
        sum, 1.0,
        "only the served pull may be counted, got {sum} pulls"
    );
}

/// The version check every container client makes before anything else: `/v2/` answers the
/// API version once a credential is presented, and challenges one that is not.
///
/// The rows differ along what the rule generalises over: how the base path is written, and
/// whether a credential is presented.
#[tokio::test]
async fn the_version_check_answers_the_api_version() {
    let (router, _engine) = emulator();
    let credential = given_credential(&router).await;

    // (case, path, credential presented, status)
    let rows: Vec<(&str, &str, bool, u16)> = vec![
        ("the base path with a credential", "/v2/", true, 200),
        ("the base path without a trailing slash", "/v2", true, 200),
        ("the base path with no credential", "/v2/", false, 401),
    ];

    for (case, path, presented, status) in &rows {
        let credential = presented.then(|| credential.clone());
        let fetched = fetch(&router, "GET", path, credential.as_deref(), None).await;
        assert_eq!(
            fetched.status, *status,
            "case {case:?}: expected HTTP {status} from {path}, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.api_version, API_VERSION,
            "case {case:?}: expected the registry API version header, got {:?}",
            fetched.api_version
        );
        if *status == 200 {
            assert_eq!(
                fetched.body, "{}",
                "case {case:?}: expected the empty JSON document, got {:?}",
                fetched.body
            );
            assert_eq!(
                fetched.content_type, REGISTRY_JSON,
                "case {case:?}: expected {REGISTRY_JSON}, got {:?}",
                fetched.content_type
            );
        }
    }
}

/// The `/v2` space is not this registry's alone, so the claim is drawn narrowly: a path
/// that addresses no endpoint of the registry API, and a request that NAMES a service, are
/// left where they were — the router's own `UnknownService` — rather than answered by this
/// registry.
///
/// The rows differ along what the rule generalises over: a `/v2` path whose endpoint
/// segment belongs to another API, a `/v2` path with nothing but the repository name, and a
/// registry-shaped path addressed at an endpoint host that names a service.
#[tokio::test]
async fn a_v2_path_this_registry_does_not_publish_is_not_claimed() {
    let (router, _engine) = emulator();
    let credential = given_credential(&router).await;
    given_image(&router, "seed", "unclaimed-probe", "v1").await;

    // (case, path, the endpoint host the request is addressed to)
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "a /v2 path of another family's REST API",
            "/v2/email/contact-lists",
            registry_host(),
        ),
        (
            "a /v2 path naming no endpoint at all",
            "/v2/unclaimed-probe",
            registry_host(),
        ),
        (
            "a registry path at an endpoint host that names a service",
            "/v2/unclaimed-probe/manifests/v1",
            "mail-manager.eu-west-1.amazonaws.com".to_string(),
        ),
    ];

    for (case, path, host) in &rows {
        let request = ::http::Request::builder()
            .method("GET")
            .uri(*path)
            .header("host", host)
            .header("authorization", &credential)
            .body(Bytes::new())
            .expect("the test request is well formed");
        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(&response.into_body()).to_string();
        assert_eq!(
            status, 404,
            "case {case:?}: expected the router's own HTTP 404, got {status} with {body:?}"
        );
        assert!(
            body.contains("UnknownService"),
            "case {case:?}: expected the router's UnknownService, got {body:?}"
        );
    }
}

/// A manifest whose stored media type is not one a header may carry is served under the
/// generic registry JSON type, so a caller's document cannot decide what this plane's
/// headers say.
///
/// The rows differ along what the rule generalises over: a document stating no media type
/// at all, one stating an empty value, one carrying a line break, and one carrying a
/// control character.
#[tokio::test]
async fn a_media_type_no_header_may_carry_is_not_stated_as_one() {
    let (router, _engine) = emulator();
    let credential = given_credential(&router).await;
    let repository = "stated-media-types";
    ok(
        &router,
        "seed",
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;

    // (case, the `mediaType` the pushed document states, the tag it is pushed under)
    let rows: Vec<(&str, Option<&str>, &str)> = vec![
        ("no media type at all", None, "untyped"),
        ("an empty media type", Some(""), "empty"),
        ("a media type carrying a line break", Some("x\r\nX-Injected: 1"), "broken"),
        ("a media type carrying a control character", Some("x\u{7f}y"), "control"),
    ];

    for (case, stated, tag) in &rows {
        let mut document = json!({"schemaVersion": 2});
        if let Some(stated) = stated {
            document["mediaType"] = json!(stated);
        }
        let manifest = document.to_string();
        ok(
            &router,
            case,
            "PutImage",
            &json!({
                "repositoryName": repository,
                "imageManifest": manifest,
                "imageTag": tag,
            }),
        )
        .await;

        let path = format!("/v2/{repository}/manifests/{tag}");
        let fetched = fetch(&router, "GET", &path, Some(&credential), None).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected HTTP 200 from {path}, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.content_type, REGISTRY_JSON,
            "case {case:?}: expected {REGISTRY_JSON}, got {:?}",
            fetched.content_type
        );
        assert_eq!(
            fetched.body, manifest,
            "case {case:?}: expected the stored document, got {:?}",
            fetched.body
        );
    }
}

/// The `Accept` header selects the schema the manifest is served in — the same negotiation
/// `BatchGetImage`'s `acceptedMediaTypes` makes for the same stored image, so the API and
/// the registry endpoint cannot answer two different documents for one pull.
///
/// The rows differ along what the rule generalises over: no `Accept` at all, the stored
/// schema, only the other interchangeable schema, a wildcard, and a list carrying media
/// type parameters.
#[tokio::test]
async fn the_accept_header_selects_the_schema_a_manifest_is_served_in() {
    let (router, _engine) = emulator();
    let credential = given_credential(&router).await;
    let image = given_image(&router, "seed", "schema-negotiation", "v1").await;
    let path = "/v2/schema-negotiation/manifests/v1";

    let both_with_parameters = format!("{OCI_MANIFEST};q=0.9, {DOCKER_V2_MANIFEST};q=1.0");
    // (case, the Accept header presented, the media type the answer must carry)
    let rows: Vec<(&str, Option<&str>, &str)> = vec![
        ("no Accept at all", None, DOCKER_V2_MANIFEST),
        (
            "the stored schema",
            Some(DOCKER_V2_MANIFEST),
            DOCKER_V2_MANIFEST,
        ),
        ("a wildcard", Some("*/*"), DOCKER_V2_MANIFEST),
        ("only the other schema", Some(OCI_MANIFEST), OCI_MANIFEST),
        (
            "a list naming both, with parameters",
            Some(&both_with_parameters),
            DOCKER_V2_MANIFEST,
        ),
    ];

    for (case, accept, expected) in &rows {
        let fetched = fetch(&router, "GET", path, Some(&credential), *accept).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.content_type, *expected,
            "case {case:?}: expected the media type {expected}, got {:?}",
            fetched.content_type
        );
        let served: Value = serde_json::from_str(&fetched.body).unwrap_or_else(|error| {
            panic!("case {case:?}: the served manifest must be JSON ({error}): {:?}", fetched.body)
        });
        assert_eq!(
            served["mediaType"].as_str(),
            Some(*expected),
            "case {case:?}: the document must state the schema it is served in, got {served:?}"
        );
        // Re-spelling the schema changes no content: the same layer digest is referenced.
        assert_eq!(
            served["layers"][0]["digest"].as_str(),
            Some(image.layer.0.as_str()),
            "case {case:?}: the served manifest must reference the stored layer, got {served:?}"
        );
        assert_eq!(
            fetched.content_length,
            fetched.bytes.len().to_string(),
            "case {case:?}: the length must be the served document's, got {:?}",
            fetched.content_length
        );
    }
}
