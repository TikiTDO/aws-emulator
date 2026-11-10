// SPDX-License-Identifier: CC0-1.0

//! The registry address this service ANNOUNCES for a repository, driven end to end over
//! the wire.
//!
//! The rule under test: `Repository$repositoryUri` ("The URI for the repository. You can
//! use this URI for container image `push` and `pull` operations") is an address the
//! client then DIALS, so every operation that reports a repository announces it at the
//! endpoint THIS emulator serves the registry API at — never the
//! `<accountId>.dkr.ecr.<region>.amazonaws.com` host AWS composes from an account and a
//! Region, which resolves off the machine this emulator runs on. Which endpoint that is
//! follows the tree's one reading of the question: the endpoint the CALLER reached this
//! emulator on when a registry client can use it (it names no service, so this registry's
//! `/v2` plane claims a request addressed there), and the address this emulator is served
//! at otherwise. The `proxyEndpoint` an authorization token carries is pinned against the
//! repository's URI in the same rows, so the two addresses cannot drift.
//!
//! The class the rule generalises over is the endpoint the call arrived on crossed with
//! the repository name and the operation reporting it, so the rows span the endpoint forms
//! a caller reaches this emulator by — the registry host ECR publishes (whose first label
//! is an account id, so it names no service), the emulator's own socket, a loopback name
//! with a port, an endpoint carried only on the request URI, this service's control-plane
//! endpoint host (which names a service, so no registry client can use it) and a `Host`
//! that is no endpoint at all — crossed with the repository-name forms `RepositoryName`'s
//! pattern allows. No row is the recorded request; the recorded case is one name at one
//! endpoint.
//!
//! The last test follows the announced address the way a container client does: it pulls
//! the manifest at the very host the `repositoryUri` named, with the credential
//! `GetAuthorizationToken` issued, so what is announced is proved to be an address this
//! emulator serves the registry API at.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    announced_registry_endpoint, at, endpoint_host, given_layer, ok, REGION, REGISTRY_ID,
};

/// One endpoint a caller reaches this emulator on: where the endpoint travels (the `Host`
/// header, the request URI's authority, or neither), the address every registry address of
/// that call must then be announced at, and whether a registry client can dial that
/// address in this test (it can, exactly when the announced endpoint is not this
/// emulator's own unserved base).
struct Endpoint {
    case: &'static str,
    host: Option<String>,
    uri_authority: Option<&'static str>,
    announced: String,
    dialable: bool,
}

/// The endpoints the table drives, spanning the forms a caller reaches this emulator by.
fn endpoints() -> Vec<Endpoint> {
    vec![
        Endpoint {
            case: "the registry endpoint host ECR publishes for this account and Region",
            host: Some(format!("{REGISTRY_ID}.dkr.ecr.{REGION}.amazonaws.com")),
            uri_authority: None,
            announced: format!("{REGISTRY_ID}.dkr.ecr.{REGION}.amazonaws.com"),
            dialable: true,
        },
        Endpoint {
            case: "the emulator's own socket",
            host: Some("127.0.0.1:9297".to_string()),
            uri_authority: None,
            announced: "127.0.0.1:9297".to_string(),
            dialable: true,
        },
        Endpoint {
            case: "a loopback name with another port",
            host: Some("localhost:39355".to_string()),
            uri_authority: None,
            announced: "localhost:39355".to_string(),
            dialable: true,
        },
        Endpoint {
            case: "an endpoint carried only on the request URI",
            host: None,
            uri_authority: Some("127.0.0.1:8443"),
            announced: "127.0.0.1:8443".to_string(),
            dialable: true,
        },
        Endpoint {
            case: "this service's control-plane endpoint host, which names a service",
            host: Some(endpoint_host(REGION)),
            uri_authority: None,
            announced: announced_registry_endpoint().to_string(),
            dialable: false,
        },
        Endpoint {
            case: "a Host that is no endpoint at all",
            host: Some("elsewhere.example.com/registry".to_string()),
            uri_authority: None,
            announced: announced_registry_endpoint().to_string(),
            dialable: false,
        },
    ]
}

/// The media type a Docker v2 manifest is pushed and served under.
const DOCKER_V2_MANIFEST: &str = "application/vnd.docker.distribution.manifest.v2+json";

/// The media type of the config blob a Docker v2 manifest references.
const DOCKER_CONFIG_MEDIA_TYPE: &str = "application/vnd.docker.container.image.v1+json";

/// The media type of a layer blob a Docker v2 manifest references.
const DOCKER_LAYER_MEDIA_TYPE: &str =
    "application/vnd.docker.image.rootfs.diff.tar.gzip";

/// The repository names the table creates, spanning the forms `RepositoryName`'s pattern
/// allows — `^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*$`
/// with `@length(min: 2, max: 256)` — read with:
///
/// ```text
/// smithy_query(query="shape",shape="Repository",model_path="services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json")
/// ```
const REPOSITORY_NAMES: [&str; 5] = ["app", "ab", "team/app", "org/team/app", "a.b_c--d"];

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation of this service, addressed to `endpoint` — the harness in
/// `crate::wire` fixes the endpoint a call arrives on, and this suite is what varies it.
async fn call_at(
    router: &MultiServiceRouter,
    endpoint: &Endpoint,
    operation: &str,
    body: &Value,
) -> Value {
    let uri = match endpoint.uri_authority {
        Some(authority) => format!("https://{authority}/"),
        None => "/".to_string(),
    };
    let mut request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ecr/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AmazonEC2ContainerRegistry_V20150921.{operation}"),
        );
    if let Some(host) = &endpoint.host {
        request = request.header("host", host.clone());
    }
    let request = request
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let answered: Value = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "case {:?}: {operation} answered a body that is not JSON ({error}): {:?}",
            endpoint.case,
            String::from_utf8_lossy(&bytes)
        )
    });
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from {operation}, got {status} with {answered}",
        endpoint.case
    );
    answered
}

/// The string at `pointer`, or a panic naming the case and the body that carried none.
fn text(case: &str, body: &Value, pointer: &str) -> String {
    at(body, pointer)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string at {pointer}, got {body}"))
        .to_string()
}

#[tokio::test]
async fn every_reported_repository_uri_names_the_registry_endpoint_this_emulator_serves() {
    for endpoint in endpoints() {
        let router = emulator();
        for name in REPOSITORY_NAMES {
            let case = format!("{}, repository {name}", endpoint.case);
            let expected = format!("{}/{name}", endpoint.announced);

            // Whatever the call arrived on, the announced address is never the host AWS
            // composes for this account and Region unless that is the very endpoint this
            // caller reached the emulator at.
            assert!(
                endpoint.announced == endpoint.host.clone().unwrap_or_default()
                    || !endpoint.announced.contains(".amazonaws.com"),
                "case {case:?}: the announced address {:?} is an AWS host this emulator \
                 does not answer on",
                endpoint.announced
            );

            let created = call_at(
                &router,
                &endpoint,
                "CreateRepository",
                &json!({"repositoryName": name}),
            )
            .await;
            let created_uri = text(&case, &created, "/repository/repositoryUri");
            assert_eq!(
                created_uri, expected,
                "case {case:?}: expected CreateRepository to announce {expected:?}, got \
                 {created_uri:?}"
            );

            // The Region a call is signed for names the ARN, which is a label, not an
            // address: it is unchanged by which endpoint the call arrived on.
            let arn = text(&case, &created, "/repository/repositoryArn");
            assert_eq!(
                arn,
                format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:repository/{name}"),
                "case {case:?}: expected the ARN to name the signed Region, got {arn:?}"
            );

            // The list read is how a client discovers where to pull from, so every entry
            // carries the same address.
            let listed = call_at(
                &router,
                &endpoint,
                "DescribeRepositories",
                &json!({"repositoryNames": [name]}),
            )
            .await;
            let listed_uri = text(&case, &listed, "/repositories/0/repositoryUri");
            assert_eq!(
                listed_uri, expected,
                "case {case:?}: expected DescribeRepositories to announce {expected:?}, got \
                 {listed_uri:?}"
            );

            // The echoed repository a delete reports is announced at the same address.
            let deleted = call_at(
                &router,
                &endpoint,
                "DeleteRepository",
                &json!({"repositoryName": name}),
            )
            .await;
            let deleted_uri = text(&case, &deleted, "/repository/repositoryUri");
            assert_eq!(
                deleted_uri, expected,
                "case {case:?}: expected DeleteRepository to announce {expected:?}, got \
                 {deleted_uri:?}"
            );

            // One registry, one address: the `docker login` endpoint an issued token
            // names is the host the repository was announced under.
            let issued = call_at(&router, &endpoint, "GetAuthorizationToken", &json!({})).await;
            let proxy_endpoint = text(&case, &issued, "/authorizationData/0/proxyEndpoint");
            assert_eq!(
                proxy_endpoint,
                format!("https://{}", endpoint.announced),
                "case {case:?}: expected the token's registry URL to name the same \
                 endpoint as the repository, got {proxy_endpoint:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_repository_is_announced_at_the_endpoint_each_read_came_in_on() {
    // The stored repository is one record; the address it is announced at belongs to the
    // call. So a repository created through one endpoint is announced to every later
    // reader at the endpoint that reader reached, and nothing about the create is frozen
    // into it.
    let router = emulator();
    let endpoints = endpoints();
    let creating = &endpoints[0];
    let name = "announced-per-read";
    let created = call_at(
        &router,
        creating,
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;
    let created_uri = text(creating.case, &created, "/repository/repositoryUri");
    assert_eq!(
        created_uri,
        format!("{}/{name}", creating.announced),
        "case {:?}: expected the create to announce its own endpoint, got {created_uri:?}",
        creating.case
    );

    for endpoint in &endpoints {
        let listed = call_at(&router, endpoint, "DescribeRepositories", &json!({})).await;
        let listed_uri = text(endpoint.case, &listed, "/repositories/0/repositoryUri");
        assert_eq!(
            listed_uri,
            format!("{}/{name}", endpoint.announced),
            "case {:?}: expected the read to announce the endpoint it is answered at, got \
             {listed_uri:?}",
            endpoint.case
        );
        let arn = text(endpoint.case, &listed, "/repositories/0/repositoryArn");
        assert_eq!(
            arn,
            format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:repository/{name}"),
            "case {:?}: expected the stored ARN to be unchanged by the read's endpoint, \
             got {arn:?}",
            endpoint.case
        );
    }
}

#[tokio::test]
async fn a_pull_at_the_announced_repository_uri_reaches_this_registry() {
    // What the rule is FOR: a client that follows the address the API handed it must
    // arrive at this emulator's registry. Each row takes the announced `repositoryUri`,
    // splits it into the host and the repository path exactly as a container client does,
    // and pulls the manifest there with the credential the token names.
    //
    // The rows whose announced address is this emulator's own base are left out: that base
    // is what a SERVED emulator publishes its data port at, and a router given no serving
    // address publishes the unserved base, which by construction no client dials (it is
    // reached in process). What those rows assert is in the tables above — that the
    // announced address is neither the AWS registry host nor a service endpoint host no
    // registry client can use.
    for endpoint in endpoints().iter().filter(|endpoint| endpoint.dialable) {
        let router = emulator();
        let name = "followed/by-a-client";
        let case = endpoint.case;

        let created = call_at(
            &router,
            endpoint,
            "CreateRepository",
            &json!({"repositoryName": name}),
        )
        .await;
        let announced = text(case, &created, "/repository/repositoryUri");
        let (announced_host, announced_path) = announced
            .split_once('/')
            .unwrap_or_else(|| panic!("case {case:?}: {announced:?} names no repository path"));

        // One image, pushed through the documented sequence, so the pull serves what the
        // push stored.
        let config_bytes = b"announced-config".to_vec();
        let config = given_layer(&router, case, name, &config_bytes).await;
        let layer_bytes = b"announced-layer".to_vec();
        let layer = given_layer(&router, case, name, &layer_bytes).await;
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
            &router,
            case,
            "PutImage",
            &json!({
                "repositoryName": name,
                "imageManifest": manifest,
                "imageTag": "v1",
            }),
        )
        .await;

        // `docker login` presents the issued token as a Basic credential, which is what
        // the token decodes to (`AWS:<password>`).
        let issued = call_at(&router, endpoint, "GetAuthorizationToken", &json!({})).await;
        let token = text(case, &issued, "/authorizationData/0/authorizationToken");

        let request = ::http::Request::builder()
            .method("GET")
            .uri(format!("/v2/{announced_path}/manifests/v1"))
            .header("host", announced_host)
            .header("authorization", format!("Basic {token}"))
            .header("accept", DOCKER_V2_MANIFEST)
            .body(Bytes::new())
            .expect("the pull request is well formed");
        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(&response.into_body()).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: a pull at the announced address {announced:?} must reach this \
             registry, got HTTP {status} with {body:?}"
        );
        assert_eq!(
            body, manifest,
            "case {case:?}: expected the pushed manifest from the announced address, got \
             {body:?}"
        );
    }
}
