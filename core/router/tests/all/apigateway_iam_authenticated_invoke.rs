// SPDX-License-Identifier: CC0-1.0

//! An `AWS_IAM`-authorized REST method invoked on an emulator that VERIFIES request
//! signatures: which credentials get past the stage's own SigV4 authentication and which do
//! not, driven end to end through the whole assembled router.
//!
//! The RULE the rows assert, one level above the recorded case:
//!
//! > A request to a deployed method whose `authorizationType` is `AWS_IAM` is AUTHENTICATED
//! > before anything else is decided about it: the access key id in its SigV4 credential must
//! > name a credential this emulator holds, and the signature must be the one that
//! > credential's secret produces over the canonical request — in the `Authorization` header
//! > or in a presigned URL's query. An access key id that names no credential, a credential
//! > whose scope names another service, and a real one signed with any other secret or over
//! > any other request, are refused HTTP `403` with no integration performed, each carrying
//! > the sentence AWS words its class with. A method
//! > that authenticates nobody (`authorizationType` `NONE`) is served whatever its request's
//! > `Authorization` header says, because AWS authenticates no caller for one.
//!
//! The rows span what that rule generalises over: WHICH credential signs (the key this
//! emulator published, an access key id never issued in each of the two prefixes AWS mints
//! long-term and temporary keys under, the right key id with another secret), WHERE the
//! credential travels (the `Authorization` header, a presigned query), WHICH service the
//! credential's scope names (the data plane's own, and the control plane's), WHETHER the
//! signed request is the request sent (a query and a header changed after signing), and WHICH
//! `authorizationType` the method declares (`AWS_IAM`, `NONE`).
//!
//! Only the assembled router can pin this: the credential store and the verifier are the
//! emulator's, and the stage that asks them is API Gateway's `execute-api` data plane.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the same
//! reason `boot_serve.rs` does.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_credential_types::Credentials;
use aws_emulator_router::{build_router, InMemoryEngine};
use aws_sigv4::http_request::{
    sign, SignableBody, SignableRequest, SignatureLocation, SigningSettings,
};
use aws_sigv4::sign::v4::SigningParams;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every call is signed for, and so the region of the api's own invoke host.
const REGION: &str = "us-east-1";

/// The stage the api is deployed to.
const STAGE: &str = "prod";

/// The signing name an invoke of a deployed stage is signed under: the `execute-api` service
/// AWS publishes the invoke endpoint and the `execute-api:Invoke` action under, which is NOT
/// this service's control-plane name (`apigateway`).
const INVOKE_SIGNING_NAME: &str = "execute-api";

/// The resource paths the api serves: one method behind `AWS_IAM`, one open to everyone.
const IAM_PATH: &str = "orders";
const OPEN_PATH: &str = "public";

/// The bodies the two `MOCK` integrations answer with.
const IAM_BODY: &str = r#"{"served":"iam-protected"}"#;
const OPEN_BODY: &str = r#"{"served":"open"}"#;

/// The sentence AWS refuses a credential whose access key id names nothing with.
const INVALID_TOKEN: &str = "The security token included in the request is invalid.";

/// The sentence AWS refuses a signature that is not the one the credential's secret produces
/// with.
const SIGNATURE_MISMATCH: &str = "The request signature we calculated does not match the \
                                  signature you provided. Check your AWS Secret Access Key \
                                  and signing method. Consult the service documentation for \
                                  details.";

/// The sentence AWS refuses a credential whose scope names another service than the one the
/// invoke endpoint is signed under with.
const MISSCOPED_CREDENTIAL: &str =
    "Credential should be scoped to correct service: 'execute-api'.";

/// The sentence an `AWS_IAM` method reached with no credential at all is answered with.
const MISSING_TOKEN: &str = "Missing Authentication Token";

/// The header an SDK reads the error class of a refused request out of.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The access key id prefixes AWS mints long-term and temporary credentials under, used to
/// build ids this emulator never issued.
const NEVER_ISSUED: &[(&str, &str)] = &[
    ("a long-term access key id never issued", "AKIAIOSFODNN7EXAMPLE"),
    ("a temporary access key id never issued", "ASIAIOSFODNN7EXAMPLE"),
];

/// One request dispatched through the router, and what came back.
struct Answer {
    status: u16,
    error_type: Option<String>,
    text: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }

    /// The `message` member every gateway response carries, or the whole body.
    fn message(&self) -> String {
        self.json()
            .get("message")
            .and_then(Value::as_str)
            .map_or_else(|| self.text.clone(), str::to_owned)
    }

    fn summary(&self) -> String {
        format!("{} {:?} {}", self.status, self.error_type, self.text)
    }
}

/// Dispatch one request through the router.
async fn dispatch(router: &MultiServiceRouter, request: http::Request<Vec<u8>>) -> Answer {
    let (parts, body) = request.into_parts();
    let request = http::Request::from_parts(parts, bytes::Bytes::from(body));
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get(ERROR_TYPE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let text = String::from_utf8_lossy(&response.into_body()).to_string();
    Answer {
        status,
        error_type,
        text,
    }
}

/// The credential scope a signed SDK request carries, with no real signature: what the
/// SETUP calls use, before request authentication is turned on, so the router can name the
/// target service.
fn credential_scope(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One API Gateway REST control-plane call, which must answer `expected_status`.
async fn control_plane(
    router: &MultiServiceRouter,
    what: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected_status: u16,
) -> Value {
    let host = format!("apigateway.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", &host)
        .header("content-type", "application/json")
        .header("authorization", credential_scope("apigateway"))
        .body(
            body.map(|body| body.to_string().into_bytes())
                .unwrap_or_default(),
        )
        .expect("the test request is well formed");
    let answer = dispatch(router, request).await;
    assert_eq!(
        answer.status,
        expected_status,
        "setup {what}: must answer HTTP {expected_status}, got {}",
        answer.summary()
    );
    answer.json()
}

/// A required string member of a response body.
fn as_str(what: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("setup {what} must carry a non-empty {member:?}, got {body}"))
}

/// Put one `MOCK` method of `verb` on `resource`, answering `body`.
async fn mock_method(
    router: &MultiServiceRouter,
    api: &str,
    resource: &str,
    authorization: &str,
    body: &str,
) {
    let base = format!("/restapis/{api}/resources/{resource}/methods/GET");
    control_plane(
        router,
        "PutMethod",
        "PUT",
        &base,
        Some(json!({ "authorizationType": authorization })),
        201,
    )
    .await;
    control_plane(
        router,
        "PutIntegration",
        "PUT",
        &format!("{base}/integration"),
        Some(json!({ "type": "MOCK" })),
        201,
    )
    .await;
    control_plane(
        router,
        "PutMethodResponse",
        "PUT",
        &format!("{base}/responses/200"),
        Some(json!({})),
        201,
    )
    .await;
    control_plane(
        router,
        "PutIntegrationResponse",
        "PUT",
        &format!("{base}/integration/responses/200"),
        Some(json!({ "responseTemplates": { "application/json": body } })),
        201,
    )
    .await;
}

/// How a row's credential travels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Where {
    /// The `Authorization` header a signed SDK request carries.
    Header,
    /// The query a presigned URL carries its credential in.
    PresignedQuery,
}

/// What a row does to the request AFTER it was signed, which is how a signature over another
/// request is produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tamper {
    /// Nothing: the request sent is the request signed.
    None,
    /// The query string is changed, so the signature covers another request of the same
    /// method.
    Query,
    /// A signed header's value is changed.
    Header,
}

/// One signed invoke: the credential it signs with, where that credential travels, what was
/// changed after signing, the path it addresses, and the answer it must earn.
struct Row {
    case: &'static str,
    key_id: String,
    secret: String,
    /// The signing name the credential's scope names: the data plane's own
    /// ([`INVOKE_SIGNING_NAME`]) for every row but the mis-scoped one.
    signing_name: &'static str,
    place: Where,
    tamper: Tamper,
    path: &'static str,
    expected: Expected,
}

/// What the stage owes one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expected {
    /// The credential authenticated and the `MOCK` integration answered.
    Served(&'static str),
    /// `403` carrying this sentence, with no integration performed.
    Refused(&'static str),
}

/// Sign `request` with `key_id`/`secret` for `signing_name`, in `place`.
fn signed(
    request: http::Request<Vec<u8>>,
    key_id: &str,
    secret: &str,
    signing_name: &str,
    place: Where,
) -> http::Request<Vec<u8>> {
    let mut request = request;
    let identity = Credentials::new(key_id, secret, None, None, "test").into();
    let mut settings = SigningSettings::default();
    if place == Where::PresignedQuery {
        settings.signature_location = SignatureLocation::QueryParams;
        settings.expires_in = Some(Duration::from_secs(300));
    }
    let params = SigningParams::builder()
        .identity(&identity)
        .region(REGION)
        .name(signing_name)
        .time(SystemTime::now())
        .settings(settings)
        .build()
        .expect("the signing parameters are complete")
        .into();
    let (method, uri) = (request.method().to_string(), request.uri().to_string());
    let headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().expect("a text header").to_string(),
            )
        })
        .collect();
    let signable = SignableRequest::new(
        &method,
        &uri,
        headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        SignableBody::Bytes(request.body()),
    )
    .expect("the request is signable");
    let (instructions, _) = sign(signable, &params)
        .expect("the request signs")
        .into_parts();
    instructions.apply_to_request_http1x(&mut request);
    request
}

/// The request one row sends: built, signed, and then tampered with as the row says.
fn request_of(row: &Row, api: &str) -> http::Request<Vec<u8>> {
    let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
    let built = http::Request::builder()
        .method("GET")
        .uri(format!("https://{host}/{STAGE}/{}", row.path))
        .header("host", &host)
        // A header of the caller's own, signed with the rest: changing it after signing is
        // what makes the signature one over another request.
        .header("x-request-marker", "signed")
        .body(Vec::new())
        .expect("the test request is well formed");
    let mut request = signed(built, &row.key_id, &row.secret, row.signing_name, row.place);
    match row.tamper {
        Tamper::None => {}
        Tamper::Query => {
            let uri = format!("{}?tampered=1", request.uri());
            *request.uri_mut() = uri.parse().expect("the tampered uri is a uri");
        }
        Tamper::Header => {
            request.headers_mut().insert(
                "x-request-marker",
                http::HeaderValue::from_static("tampered"),
            );
        }
    }
    request
}

#[test]
fn an_iam_authorized_method_authenticates_the_credential_its_request_presents() {
    std::thread::Builder::new()
        .name("apigateway-iam-authenticated-invoke".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                // The IO and TIME drivers a service's background work needs; see the
                // sibling `apigateway_private_vpce_invoke` suite.
                .enable_all()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);

                // SETUP, with authentication off so the control-plane calls need no real
                // signature: one `AWS_IAM` method and one open method, both behind a `MOCK`
                // integration, deployed to one stage.
                router.set_request_authentication(false);
                let api = control_plane(
                    &router,
                    "CreateRestApi",
                    "POST",
                    "/restapis",
                    Some(json!({ "name": "iam-authenticated-api" })),
                    201,
                )
                .await;
                let api_id = as_str("CreateRestApi", &api, "id");
                let root = as_str("CreateRestApi", &api, "rootResourceId");
                for (path_part, authorization, body) in [
                    (IAM_PATH, "AWS_IAM", IAM_BODY),
                    (OPEN_PATH, "NONE", OPEN_BODY),
                ] {
                    let resource = control_plane(
                        &router,
                        "CreateResource",
                        "POST",
                        &format!("/restapis/{api_id}/resources/{root}"),
                        Some(json!({ "pathPart": path_part })),
                        201,
                    )
                    .await;
                    let resource_id = as_str("CreateResource", &resource, "id");
                    mock_method(&router, &api_id, &resource_id, authorization, body).await;
                }
                control_plane(
                    &router,
                    "CreateDeployment",
                    "POST",
                    &format!("/restapis/{api_id}/deployments"),
                    Some(json!({ "stageName": STAGE })),
                    201,
                )
                .await;

                // From here on the SAME router verifies request signatures, as the emulator
                // does when it is started with authentication on, and the keys it publishes
                // are the ones a caller must sign with.
                router.set_request_authentication(true);
                let published = ::emulator_server::SigV4Authenticator::minted_credentials()
                    .expect("turning authentication on publishes this process's keys");
                let (key_id, secret) = (
                    published.access_key_id.clone(),
                    published.secret_access_key.clone(),
                );

                let mut rows = vec![
                    Row {
                        case: "the key this emulator published, signed in the header",
                        key_id: key_id.clone(),
                        secret: secret.clone(),
                        signing_name: INVOKE_SIGNING_NAME,
                        place: Where::Header,
                        tamper: Tamper::None,
                        path: IAM_PATH,
                        expected: Expected::Served(IAM_BODY),
                    },
                    Row {
                        case: "the same key, carried in a presigned URL's query",
                        key_id: key_id.clone(),
                        secret: secret.clone(),
                        signing_name: INVOKE_SIGNING_NAME,
                        place: Where::PresignedQuery,
                        tamper: Tamper::None,
                        path: IAM_PATH,
                        expected: Expected::Served(IAM_BODY),
                    },
                    Row {
                        case: "the right key id with another secret",
                        key_id: key_id.clone(),
                        secret: "0123456789abcdef0123456789abcdef01234567".to_string(),
                        signing_name: INVOKE_SIGNING_NAME,
                        place: Where::Header,
                        tamper: Tamper::None,
                        path: IAM_PATH,
                        expected: Expected::Refused(SIGNATURE_MISMATCH),
                    },
                    Row {
                        case: "the right key and secret, over another query than the one sent",
                        key_id: key_id.clone(),
                        secret: secret.clone(),
                        signing_name: INVOKE_SIGNING_NAME,
                        place: Where::Header,
                        tamper: Tamper::Query,
                        path: IAM_PATH,
                        expected: Expected::Refused(SIGNATURE_MISMATCH),
                    },
                    Row {
                        case: "the right key and secret, with a signed header changed after \
                               signing",
                        key_id: key_id.clone(),
                        secret: secret.clone(),
                        signing_name: INVOKE_SIGNING_NAME,
                        place: Where::Header,
                        tamper: Tamper::Header,
                        path: IAM_PATH,
                        expected: Expected::Refused(SIGNATURE_MISMATCH),
                    },
                    Row {
                        case: "the right key and secret, scoped to the control plane's own \
                               signing name",
                        key_id: key_id.clone(),
                        secret: secret.clone(),
                        signing_name: "apigateway",
                        place: Where::Header,
                        tamper: Tamper::None,
                        path: IAM_PATH,
                        expected: Expected::Refused(MISSCOPED_CREDENTIAL),
                    },
                    Row {
                        case: "the same, carried in a presigned URL's query",
                        key_id: key_id.clone(),
                        secret: secret.clone(),
                        signing_name: "apigateway",
                        place: Where::PresignedQuery,
                        tamper: Tamper::None,
                        path: IAM_PATH,
                        expected: Expected::Refused(MISSCOPED_CREDENTIAL),
                    },
                    Row {
                        case: "a method that authenticates nobody, signed with another secret",
                        key_id: key_id.clone(),
                        secret: "0123456789abcdef0123456789abcdef01234567".to_string(),
                        signing_name: INVOKE_SIGNING_NAME,
                        place: Where::Header,
                        tamper: Tamper::None,
                        path: OPEN_PATH,
                        expected: Expected::Served(OPEN_BODY),
                    },
                ];
                for (case, never_issued) in NEVER_ISSUED {
                    rows.push(Row {
                        case,
                        key_id: (*never_issued).to_string(),
                        secret: secret.clone(),
                        signing_name: INVOKE_SIGNING_NAME,
                        place: Where::Header,
                        tamper: Tamper::None,
                        path: IAM_PATH,
                        expected: Expected::Refused(INVALID_TOKEN),
                    });
                    rows.push(Row {
                        case,
                        key_id: (*never_issued).to_string(),
                        secret: secret.clone(),
                        signing_name: INVOKE_SIGNING_NAME,
                        place: Where::PresignedQuery,
                        tamper: Tamper::None,
                        path: IAM_PATH,
                        expected: Expected::Refused(INVALID_TOKEN),
                    });
                }

                for row in &rows {
                    let answer = dispatch(&router, request_of(row, &api_id)).await;
                    match row.expected {
                        Expected::Served(body) => {
                            assert_eq!(
                                (answer.status, answer.message().as_str()),
                                (200, body),
                                "case {:?}: expected the integration's own answer, got {}",
                                row.case,
                                answer.summary()
                            );
                        }
                        Expected::Refused(message) => {
                            assert_eq!(
                                (answer.status, answer.message().as_str()),
                                (403, message),
                                "case {:?}: expected the stage's 403 for the class, got {}",
                                row.case,
                                answer.summary()
                            );
                            assert_ne!(
                                answer.text.trim(),
                                IAM_BODY,
                                "case {:?}: the integration must not have been performed, got \
                                 {}",
                                row.case,
                                answer.summary()
                            );
                        }
                    }
                }

                // The boundary: a credential is REQUIRED of an `AWS_IAM` method, and a
                // request carrying none is the missing-token refusal rather than a failed
                // authentication — the two are different answers to different omissions.
                let host = format!("{api_id}.execute-api.{REGION}.amazonaws.com");
                let unsigned = http::Request::builder()
                    .method("GET")
                    .uri(format!("https://{host}/{STAGE}/{IAM_PATH}"))
                    .header("host", &host)
                    .body(Vec::new())
                    .expect("the test request is well formed");
                let answer = dispatch(&router, unsigned).await;
                assert_eq!(
                    (answer.status, answer.message().as_str()),
                    (403, MISSING_TOKEN),
                    "case \"no credential at all\": expected the missing-token refusal, got {}",
                    answer.summary()
                );
            });
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread must not panic");
}
