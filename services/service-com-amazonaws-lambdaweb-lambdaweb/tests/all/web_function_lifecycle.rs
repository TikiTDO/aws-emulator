// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Lambda Web function lifecycle: `CreateWebFunction`,
//! `CreateWebFunctionRevision`, `GetWebFunctionRevision`, `GetWebFunction`,
//! `ListWebFunctions` and `DeleteWebFunction`.
//!
//! Every assertion goes over the wire: a request is built, dispatched through the
//! same [`MultiServiceRouter`] the production emulator mounts, and only the HTTP
//! status, the `x-amzn-errortype` header and the JSON body are inspected. Nothing
//! reaches into the service's state struct, so a response that is not computed from
//! persisted state cannot pass.
//!
//! The tests are tables: each row is one input along the axis the rule under test
//! generalises over (name form and length, revision configuration, page size,
//! function count, filter value, signing region), and the `demo-fn` case from the
//! bootstrap issue is only one row.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_lambdaweb_lambdaweb::state::{
    FUNCTION_NAME_MAX_CHARS, MAX_MAX_RESULTS, PROVISIONING_DELAY,
};
use service_com_amazonaws_lambdaweb_lambdaweb::EmulatedService;

/// The region every test signs with unless it is exercising the region rule itself.
const REGION: &str = "us-east-1";

/// The URI prefix every operation of this service is bound under (model:
/// `smithy_query(query="digest")` — `/2025-03-07/...`).
const PREFIX: &str = "/2025-03-07";

/// How long a test waits before expecting the clock-driven `Pending -> Active`
/// transition: the service's own span plus slack for a loaded test machine.
fn settle_delay() -> Duration {
    PROVISIONING_DELAY + Duration::from_millis(250)
}

/// One wire answer, reduced to what a client branches on. Shared with the sibling
/// suites of this test crate, which drive the same emulator over the same wire.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    error_type: String,
    pub(crate) body: Value,
}

impl Reply {
    pub(crate) fn field(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    pub(crate) fn string(&self, name: &str) -> Option<String> {
        self.field(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The error shape this answer names: the `x-amzn-errortype` header restJson1
    /// carries, falling back to the body's `__type`.
    pub(crate) fn error(&self) -> String {
        if !self.error_type.is_empty() {
            return self
                .error_type
                .split(['#', ':'])
                .next_back()
                .unwrap_or_default()
                .to_string();
        }
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// An emulator with this service mounted on a fresh engine, driven in-process through the
/// production router — the ONE request plumbing every suite of this test crate uses.
///
/// The engine and the router are kept, so a suite that needs more than the wire can reach
/// them: [`Emulator::engine`] for this service's own store (the published data plane is
/// served from it) and [`Emulator::router`] for an in-process client of another service
/// mounted beside this one.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        Self::assembled(false)
    }

    /// This service with the emulator's own Amazon S3 mounted beside it on ONE engine —
    /// exactly as the assembled router mounts them — for a suite whose behaviour depends
    /// on the owner of objects (a revision's build fetches its deployment package from
    /// it).
    pub(crate) fn with_object_store() -> Self {
        Self::assembled(true)
    }

    fn assembled(object_store: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by
        // the SigV4 credential scope below, which is also where an ARN's region
        // comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        if object_store {
            router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
                Arc::clone(&engine),
            ));
        }
        Self { router, engine }
    }

    /// The ONE shared engine this emulator's services are mounted on.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// The router every request goes through, for a suite that drives another mounted
    /// service through its own SDK client over the in-process callback.
    pub(crate) fn router(&self) -> &MultiServiceRouter {
        &self.router
    }

    pub(crate) async fn send(
        &self,
        method: &str,
        path: &str,
        region: &str,
        body: Option<Value>,
    ) -> Reply {
        // This service's sigv4 signing name is `lambda` (model: `sigv4Name`).
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/lambda/aws4_request, \
             SignedHeaders=host, Signature=deadbeef"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(format!("{PREFIX}{path}"))
            .header("host", format!("lambda.{region}.amazonaws.com"))
            .header("authorization", authorization);
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("the request body serializes"),
                ))
                .expect("a well-formed request"),
            None => builder.body(Bytes::new()).expect("a well-formed request"),
        };

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    async fn create(&self, body: Value) -> Reply {
        self.create_in(REGION, body).await
    }

    async fn create_in(&self, region: &str, body: Value) -> Reply {
        self.send("PUT", "/web-functions", region, Some(body)).await
    }

    async fn get(&self, name: &str) -> Reply {
        self.send("GET", &format!("/web-functions/{name}"), REGION, None)
            .await
    }

    async fn delete(&self, name: &str) -> Reply {
        self.send("DELETE", &format!("/web-functions/{name}"), REGION, None)
            .await
    }

    async fn create_revision(&self, name: &str, body: Value) -> Reply {
        self.send(
            "POST",
            &format!("/web-functions/{name}/revisions"),
            REGION,
            Some(body),
        )
        .await
    }

    async fn get_revision(&self, name: &str, revision_id: &str) -> Reply {
        self.send(
            "GET",
            &format!("/web-functions/{name}/revisions/{revision_id}"),
            REGION,
            None,
        )
        .await
    }

    async fn list(&self, body: Value) -> Reply {
        self.send("POST", "/web-functions", REGION, Some(body)).await
    }
}

/// The ARN this service's resource-policy operations require for a web function
/// (`resourceArn` pattern `^arn:[^:]+:lambda:[^:]+:\d{12}:web-function/[^/]+$`, see
/// OPERATIONS.md): `arn:aws:lambda:<region>:<account>:web-function/<name>`. Written
/// out here rather than taken from the handler's helper, so the test pins the
/// format.
pub(crate) fn expected_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:lambda:{region}:{account}:web-function/{name}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// One value as ONE URI segment: every character outside RFC 3986's unreserved set
/// travels percent-encoded, which is what an SDK does to an ARN it puts in a path
/// label and to a tag key it puts in a query value — without it the request
/// addresses a different route, or the key arrives split. The encoding is the
/// maintained `percent-encoding` crate's; shared with the sibling suites of this
/// test crate, which address a resource by its ARN.
pub(crate) fn percent_encoded(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// A minimal, valid `buildConfig`: `codeConfig.s3Object` plus `runtimeConfig`
/// (both `@required`, model: `shape=BuildConfig`). `runtime` must match
/// `^[a-z][a-z0-9]*(\.[a-z0-9]+)*$`.
pub(crate) fn build_config(runtime: &str, bucket: &str, key: &str, version: Option<&str>) -> Value {
    let mut s3_object = json!({ "bucket": bucket, "key": key });
    if let Some(version) = version {
        s3_object["versionId"] = json!(version);
    }
    json!({
        "codeConfig": { "s3Object": s3_object },
        "runtimeConfig": { "runtime": runtime },
    })
}

/// A valid `executionRoleArn` (model pattern
/// `^arn:(aws[a-z-]*):iam::[0-9]{12}:role/[a-zA-Z0-9+=,.@_/-]+$`).
pub(crate) fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::123456789012:role/{name}")
}

/// `{functionName, revisionConfig}` for a create that also wants the initial
/// revision.
fn revision_config(description: Option<&str>, build: Value, service: Value) -> Value {
    let mut config = json!({ "buildConfig": build, "serviceConfig": service });
    if let Some(description) = description {
        config["description"] = json!(description);
    }
    config
}

/// Every `functionName` of a listing page, in the order the page reported them.
fn page_names(reply: &Reply) -> Vec<String> {
    reply
        .field("functions")
        .and_then(Value::as_array)
        .map(|functions| {
            functions
                .iter()
                .map(|function| {
                    function
                        .get("functionName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Walk every page of `ListWebFunctions` with `page_size`, following `nextToken`,
/// and return the names in the order they were reported.
async fn walk_pages(emulator: &Emulator, page_size: u32, extra: Option<Value>) -> Vec<String> {
    let mut names = Vec::new();
    let mut next_token: Option<String> = None;
    // A page per function plus a couple of spare turns: a cursor that never
    // terminates is a bug, not a reason to loop forever.
    for _ in 0..64 {
        let mut body = json!({ "maxResults": page_size });
        if let Some(extra) = &extra {
            for (key, value) in extra.as_object().expect("extra is an object") {
                body[key] = value.clone();
            }
        }
        if let Some(token) = &next_token {
            body["nextToken"] = json!(token);
        }
        let reply = emulator.list(body).await;
        assert_eq!(
            reply.status, 200,
            "case page_size {page_size}: ListWebFunctions expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        names.extend(page_names(&reply));
        next_token = reply.string("nextToken");
        if next_token.is_none() {
            return names;
        }
    }
    panic!("case page_size {page_size}: ListWebFunctions never stopped handing out nextToken");
}

#[tokio::test]
async fn a_created_function_is_reported_by_get_and_by_list() {
    // The rule generalises over the name's form and the tags supplied, so the rows
    // differ along both. `demo-fn` (from the issue) is one row.
    let cases: Vec<(&str, Value)> = vec![
        ("demo-fn", json!({ "env": "dev" })),
        ("a", json!({})),
        ("UPPER_and_lower-123", json!({ "team": "payments", "env": "prod" })),
        ("9", json!({ "k": "" })),
        (
            // The longest name the service documents: exactly 64 characters.
            "f0123456789012345678901234567890123456789012345678901234567890ab",
            json!({}),
        ),
    ];

    for (name, tags) in cases {
        let emulator = Emulator::new();
        let mut body = json!({ "functionName": name });
        let tag_map = tags.as_object().expect("tags is an object").clone();
        if !tag_map.is_empty() {
            body["tags"] = tags.clone();
        }

        let created = emulator.create(body).await;
        assert_eq!(
            created.status, 202,
            "case {name:?}: CreateWebFunction expected 202, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.string("functionName").as_deref(),
            Some(name),
            "case {name:?}: expected the stored name, got {:?}",
            created.body
        );
        assert_eq!(
            created.string("functionArn"),
            Some(expected_arn(REGION, name)),
            "case {name:?}: expected the documented ARN, got {:?}",
            created.body
        );
        assert_eq!(
            created.string("state").as_deref(),
            Some("Pending"),
            "case {name:?}: a just-created function is still Pending, got {:?}",
            created.body
        );
        assert!(
            created
                .string("stateReason")
                .is_some_and(|reason| !reason.is_empty()),
            "case {name:?}: stateReason is @required, got {:?}",
            created.body
        );
        let created_at = created
            .field("createdAt")
            .cloned()
            .unwrap_or_else(|| panic!("case {name:?}: createdAt is @required: {:?}", created.body));
        assert_eq!(
            created.field("updatedAt"),
            Some(&created_at),
            "case {name:?}: nothing has mutated the function yet, so updatedAt is createdAt, got {:?}",
            created.body
        );
        if tag_map.is_empty() {
            assert_eq!(
                created.field("tags"),
                None,
                "case {name:?}: an untagged function reports no tags, got {:?}",
                created.body
            );
        } else {
            assert_eq!(
                created.field("tags"),
                Some(&tags),
                "case {name:?}: expected the stored tags, got {:?}",
                created.body
            );
        }
        assert_eq!(
            created.field("revision"),
            None,
            "case {name:?}: no revisionConfig was supplied, so no revision exists, got {:?}",
            created.body
        );

        // The round trip: Get reports the stored record, not echoed input.
        let fetched = emulator.get(name).await;
        assert_eq!(
            fetched.status, 200,
            "case {name:?}: GetWebFunction expected 200, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.string("functionArn"),
            created.string("functionArn"),
            "case {name:?}: Get must report the ARN create stored, got {:?}",
            fetched.body
        );
        assert_eq!(
            fetched.field("createdAt"),
            Some(&created_at),
            "case {name:?}: createdAt is fixed at creation, got {:?}",
            fetched.body
        );

        // And so does List, from the same record.
        let listed = emulator.list(json!({})).await;
        assert_eq!(
            listed.status, 200,
            "case {name:?}: ListWebFunctions expected 200, got {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            page_names(&listed),
            vec![name.to_string()],
            "case {name:?}: expected the created function in the listing, got {:?}",
            listed.body
        );
        let summary = listed.field("functions").and_then(Value::as_array).expect("functions")[0]
            .clone();
        assert_eq!(
            summary.get("functionArn").and_then(Value::as_str),
            created.string("functionArn").as_deref(),
            "case {name:?}: the summary's ARN comes from the same record, got {summary:?}"
        );
        assert_eq!(
            summary.get("createdAt"),
            Some(&created_at),
            "case {name:?}: the summary's createdAt comes from the same record, got {summary:?}"
        );
    }
}

#[tokio::test]
async fn a_name_outside_the_documented_rule_is_refused_and_stores_nothing() {
    // The documented rule: "The name can contain letters, numbers, hyphens (-), and
    // underscores (_), and can't begin or end with a hyphen or an underscore. The
    // name is limited to 64 characters in length." (source: smithy_query doc for
    // CreateWebFunctionRequest$functionName). The rows walk each clause, and the
    // length clause is tested on its published bound and one past it.
    let legal = "f0123456789012345678901234567890123456789012345678901234567890ab";
    assert_eq!(legal.len(), FUNCTION_NAME_MAX_CHARS, "the bound row is the bound");
    let too_long = format!("{legal}c");

    let refused: Vec<&str> = vec![
        "-leading-hyphen",
        "_leading-underscore",
        "trailing-hyphen-",
        "trailing-underscore_",
        "has.dot",
        "has space",
        "has:colon",
        "has/slash",
        "has+plus",
        &too_long,
    ];

    let emulator = Emulator::new();
    for name in refused {
        let reply = emulator.create(json!({ "functionName": name })).await;
        assert_eq!(
            reply.status, 400,
            "case {name:?}: expected 400, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error(),
            "ValidationException",
            "case {name:?}: expected ValidationException, got {:?}",
            reply.body
        );
        // Asserted through the listing rather than a GET: several of these names
        // cannot even ride in a URI path, which is part of why the service refuses
        // them.
        let listed = emulator.list(json!({})).await;
        assert!(
            page_names(&listed).is_empty(),
            "case {name:?}: a refused create stores nothing, got {:?}",
            listed.body
        );
    }

    // The bound itself is accepted, which is what makes the row above a bound.
    let accepted = emulator.create(json!({ "functionName": legal })).await;
    assert_eq!(
        accepted.status, 202,
        "case {legal:?}: a name of exactly {FUNCTION_NAME_MAX_CHARS} characters is legal, got {} with {:?}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn a_revision_is_read_back_exactly_as_it_was_stored() {
    // The rule generalises over the revision's configuration, so the rows differ
    // along the members a revision carries: which optional members are present, and
    // whether the two defaulted `serviceConfig` members are supplied (at each
    // published bound) or left to their defaults. `timeoutSeconds` is
    // @range(3..900) @default(30) and `maxConcurrencyPerEnvironment` is
    // @range(1..128) @default(64) (model: shape=ServiceConfig).
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            // The issue's own row: both defaulted members omitted.
            "defaults",
            build_config("python3.13", "demo-bucket", "app.zip", None),
            json!({ "executionRoleArn": role_arn("demo") }),
            json!({ "timeoutSeconds": 30, "maxConcurrencyPerEnvironment": 64 }),
        ),
        (
            "lower-bounds",
            build_config("nodejs22.x", "code.bucket", "nested/path/app.zip", Some("v1")),
            json!({
                "executionRoleArn": role_arn("role_with/path"),
                "timeoutSeconds": 3,
                "maxConcurrencyPerEnvironment": 1,
            }),
            json!({ "timeoutSeconds": 3, "maxConcurrencyPerEnvironment": 1 }),
        ),
        (
            "upper-bounds",
            build_config("java21", "abc", "k", None),
            json!({
                "executionRoleArn": role_arn("upper"),
                "timeoutSeconds": 900,
                "maxConcurrencyPerEnvironment": 128,
            }),
            json!({ "timeoutSeconds": 900, "maxConcurrencyPerEnvironment": 128 }),
        ),
        (
            "environment-and-telemetry",
            build_config("ruby3.4", "my-bucket", "a".repeat(200).as_str(), None),
            json!({
                "executionRoleArn": role_arn("telemetry"),
                "timeoutSeconds": 120,
                "environmentVariables": { "LOG_LEVEL": "debug", "A1_b": "x" },
                "telemetryConfig": {
                    "loggingConfig": {
                        "logGroup": "/aws/lambda/demo",
                        "applicationLogLevel": "DEBUG",
                        "systemLogLevel": "WARN",
                    }
                },
            }),
            json!({ "timeoutSeconds": 120, "maxConcurrencyPerEnvironment": 64 }),
        ),
    ];

    for (case, build, service, expected_defaults) in cases {
        let emulator = Emulator::new();
        let name = format!("fn-{case}");
        let created = emulator.create(json!({ "functionName": name })).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateWebFunction expected 202, got {} with {:?}",
            created.status, created.body
        );

        let mut request = json!({ "buildConfig": build, "serviceConfig": service });
        request["description"] = json!(format!("revision for {case}"));
        let revision = emulator.create_revision(&name, request).await;
        assert_eq!(
            revision.status, 202,
            "case {case:?}: CreateWebFunctionRevision expected 202, got {} with {:?}",
            revision.status, revision.body
        );
        let revision_id = revision
            .string("revisionId")
            .unwrap_or_else(|| panic!("case {case:?}: revisionId is @required: {:?}", revision.body));
        assert_eq!(
            revision.string("functionArn"),
            Some(expected_arn(REGION, &name)),
            "case {case:?}: the revision names its function's ARN, got {:?}",
            revision.body
        );
        assert_eq!(
            revision.string("revisionArn"),
            Some(format!(
                "{}/revision/{revision_id}",
                expected_arn(REGION, &name)
            )),
            "case {case:?}: the revision ARN is derived from the function ARN, got {:?}",
            revision.body
        );
        assert_eq!(
            revision.string("state").as_deref(),
            Some("Pending"),
            "case {case:?}: a just-created revision is still Pending, got {:?}",
            revision.body
        );
        assert_eq!(
            revision.field("buildConfig"),
            Some(&build),
            "case {case:?}: the buildConfig is stored verbatim, got {:?}",
            revision.body
        );
        // The defaulted members carry the model's declared values when the request
        // left them out, and the supplied ones otherwise.
        for (member, expected) in expected_defaults.as_object().expect("an object") {
            assert_eq!(
                revision
                    .field("serviceConfig")
                    .and_then(|config| config.get(member)),
                Some(expected),
                "case {case:?}: expected serviceConfig.{member} {expected}, got {:?}",
                revision.body
            );
        }

        // The round trip: Get returns exactly the revision that was persisted.
        let fetched = emulator.get_revision(&name, &revision_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetWebFunctionRevision expected 200, got {} with {:?}",
            fetched.status, fetched.body
        );
        for member in [
            "functionArn",
            "revisionArn",
            "revisionId",
            "description",
            "buildConfig",
            "serviceConfig",
            "createdAt",
        ] {
            assert_eq!(
                fetched.field(member),
                revision.field(member),
                "case {case:?}: Get must report the {member} create stored, got {:?}",
                fetched.body
            );
        }

        // An unknown revision of a known function, and any revision of an unknown
        // function, are both 404s.
        let unknown_revision = emulator
            .get_revision(&name, "00000000-0000-0000-0000-00000000dead")
            .await;
        assert_eq!(
            unknown_revision.status, 404,
            "case {case:?}: an unknown revisionId is a 404, got {} with {:?}",
            unknown_revision.status, unknown_revision.body
        );
        let unknown_function = emulator.get_revision("never-created", &revision_id).await;
        assert_eq!(
            unknown_function.status, 404,
            "case {case:?}: an unknown functionName is a 404, got {} with {:?}",
            unknown_function.status, unknown_function.body
        );
    }
}

#[tokio::test]
async fn an_initial_revision_supplied_to_create_is_the_revision_get_returns() {
    // `revisionConfig` on CreateWebFunction creates the function's initial
    // revision; the rows differ in which optional members that configuration
    // carries.
    let cases: Vec<(&str, Option<&str>)> = vec![
        ("with-description", Some("the first revision")),
        ("without-description", None),
    ];

    for (case, description) in cases {
        let emulator = Emulator::new();
        let name = format!("initial-{case}");
        let build = build_config("python3.13", "demo-bucket", "app.zip", None);
        let service = json!({ "executionRoleArn": role_arn("initial") });
        let created = emulator
            .create(json!({
                "functionName": name,
                "revisionConfig": revision_config(description, build.clone(), service),
            }))
            .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateWebFunction expected 202, got {} with {:?}",
            created.status, created.body
        );
        let revision = created
            .field("revision")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: a revisionConfig was supplied, so a revision summary is expected: {:?}", created.body));
        let revision_id = revision
            .get("revisionId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            revision.get("description").and_then(Value::as_str),
            description,
            "case {case:?}: the summary reports the stored description, got {revision:?}"
        );
        assert_eq!(
            revision.get("revisionArn").and_then(Value::as_str),
            Some(
                format!(
                    "{}/revision/{revision_id}",
                    expected_arn(REGION, &name)
                )
                .as_str()
            ),
            "case {case:?}: the summary's ARN is derived from the function ARN, got {revision:?}"
        );

        let fetched = emulator.get_revision(&name, &revision_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetWebFunctionRevision expected 200, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.field("buildConfig"),
            Some(&build),
            "case {case:?}: the initial revision stored the supplied buildConfig, got {:?}",
            fetched.body
        );
        assert_eq!(
            fetched.field("createdAt"),
            revision.get("createdAt"),
            "case {case:?}: one record, so one createdAt, got {:?}",
            fetched.body
        );
        assert_eq!(
            fetched
                .field("serviceConfig")
                .and_then(|config| config.get("timeoutSeconds")),
            Some(&json!(30)),
            "case {case:?}: the omitted timeoutSeconds carries its modelled default, got {:?}",
            fetched.body
        );
    }
}

#[tokio::test]
async fn a_second_create_of_the_same_name_conflicts() {
    // The rule generalises over the name, and over what the first create carried.
    let cases: Vec<(&str, bool)> = vec![("demo-fn", false), ("with_revision", true)];

    for (name, with_revision) in cases {
        let emulator = Emulator::new();
        let mut first = json!({ "functionName": name });
        if with_revision {
            first["revisionConfig"] = revision_config(
                None,
                build_config("python3.13", "demo-bucket", "app.zip", None),
                json!({ "executionRoleArn": role_arn("conflict") }),
            );
        }
        let created = emulator.create(first).await;
        assert_eq!(
            created.status, 202,
            "case {name:?}: CreateWebFunction expected 202, got {} with {:?}",
            created.status, created.body
        );

        let again = emulator.create(json!({ "functionName": name })).await;
        assert_eq!(
            again.status, 409,
            "case {name:?}: expected 409, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error(),
            "ConflictException",
            "case {name:?}: expected ConflictException, got {:?}",
            again.body
        );

        // The stored function is untouched by the refused create.
        let fetched = emulator.get(name).await;
        assert_eq!(
            fetched.field("createdAt"),
            created.field("createdAt"),
            "case {name:?}: a refused create must not move createdAt, got {:?}",
            fetched.body
        );
    }
}

#[tokio::test]
async fn pagination_reports_every_function_once_in_name_order() {
    // The rule generalises over how many functions are stored and how large a page
    // is, so the rows are (count, page size) pairs — including a page size of 1,
    // the model's `maxResults` maximum, a size that divides the count exactly and
    // one that does not, and an empty store.
    let cases: Vec<(usize, u32)> = vec![
        (0, 1),
        (1, 1),
        (3, 1),
        (4, 2),
        (7, 3),
        (5, MAX_MAX_RESULTS as u32),
        (12, 5),
    ];

    for (count, page_size) in cases {
        let emulator = Emulator::new();
        let mut expected: Vec<String> = Vec::new();
        for index in 0..count {
            // Names that do NOT sort the way they were created, so the listing's
            // order is the store's order and not the insertion order.
            let name = format!("fn-{:02}", (count - index) * 7 % 97);
            let created = emulator.create(json!({ "functionName": name })).await;
            assert_eq!(
                created.status, 202,
                "case ({count}, {page_size}): CreateWebFunction expected 202, got {} with {:?}",
                created.status, created.body
            );
            expected.push(name);
        }
        expected.sort();

        let walked = walk_pages(&emulator, page_size, None).await;
        assert_eq!(
            walked, expected,
            "case ({count}, {page_size}): every function must appear exactly once, in name order"
        );
        assert_eq!(
            walked.iter().collect::<BTreeSet<_>>().len(),
            walked.len(),
            "case ({count}, {page_size}): no function may be reported twice, got {walked:?}"
        );

        // The last page of a listing that fits in one page carries no cursor.
        let single = emulator
            .list(json!({ "maxResults": MAX_MAX_RESULTS }))
            .await;
        assert_eq!(
            single.string("nextToken"),
            None,
            "case ({count}, {page_size}): nextToken is present only when a page remains, got {:?}",
            single.body
        );
    }
}

#[tokio::test]
async fn a_deleted_function_is_gone_from_every_reader() {
    // The rule generalises over what the function carried when it was deleted.
    let cases: Vec<(&str, usize)> = vec![("no-revisions", 0), ("one-revision", 1), ("three-revisions", 3)];

    for (name, revisions) in cases {
        let emulator = Emulator::new();
        let created = emulator
            .create(json!({ "functionName": name, "tags": { "env": "dev" } }))
            .await;
        assert_eq!(
            created.status, 202,
            "case {name:?}: CreateWebFunction expected 202, got {} with {:?}",
            created.status, created.body
        );
        let mut revision_ids = Vec::new();
        for index in 0..revisions {
            let reply = emulator
                .create_revision(
                    name,
                    json!({
                        "buildConfig": build_config("python3.13", "demo-bucket", &format!("app-{index}.zip"), None),
                        "serviceConfig": { "executionRoleArn": role_arn("deleted") },
                    }),
                )
                .await;
            assert_eq!(
                reply.status, 202,
                "case {name:?}: CreateWebFunctionRevision expected 202, got {} with {:?}",
                reply.status, reply.body
            );
            revision_ids.push(reply.string("revisionId").expect("revisionId"));
        }

        let deleted = emulator.delete(name).await;
        assert_eq!(
            deleted.status, 204,
            "case {name:?}: DeleteWebFunction expected 204, got {} with {:?}",
            deleted.status, deleted.body
        );

        let fetched = emulator.get(name).await;
        assert_eq!(
            fetched.status, 404,
            "case {name:?}: GetWebFunction after delete expected 404, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.error(),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {:?}",
            fetched.body
        );
        for revision_id in &revision_ids {
            let revision = emulator.get_revision(name, revision_id).await;
            assert_eq!(
                revision.status, 404,
                "case {name:?}: the revisions went with the function, got {} with {:?}",
                revision.status, revision.body
            );
        }
        let listed = emulator.list(json!({})).await;
        assert!(
            page_names(&listed).is_empty(),
            "case {name:?}: a deleted function must not be listed, got {:?}",
            listed.body
        );

        // `@idempotent` is retry safety, not success for a name that never existed.
        let again = emulator.delete(name).await;
        assert_eq!(
            again.status, 404,
            "case {name:?}: deleting what is gone is a 404, got {} with {:?}",
            again.status, again.body
        );
    }
}

#[tokio::test]
async fn an_unknown_function_is_a_404_not_a_501() {
    // The rule generalises over the reader and the name asked for.
    let emulator = Emulator::new();
    let names = ["never-created", "demo-fn", "a"];
    for name in names {
        let fetched = emulator.get(name).await;
        assert_eq!(
            fetched.status, 404,
            "case {name:?}: GetWebFunction expected 404, got {} with {:?}",
            fetched.status, fetched.body
        );
        let revision = emulator
            .get_revision(name, "00000000-0000-0000-0000-000000000001")
            .await;
        assert_eq!(
            revision.status, 404,
            "case {name:?}: GetWebFunctionRevision expected 404, got {} with {:?}",
            revision.status, revision.body
        );
        let created_revision = emulator
            .create_revision(
                name,
                json!({
                    "buildConfig": build_config("python3.13", "demo-bucket", "app.zip", None),
                    "serviceConfig": { "executionRoleArn": role_arn("unknown") },
                }),
            )
            .await;
        assert_eq!(
            created_revision.status, 404,
            "case {name:?}: CreateWebFunctionRevision on an unknown function expected 404, got {} with {:?}",
            created_revision.status, created_revision.body
        );
        let deleted = emulator.delete(name).await;
        assert_eq!(
            deleted.status, 404,
            "case {name:?}: DeleteWebFunction expected 404, got {} with {:?}",
            deleted.status, deleted.body
        );
    }
}

#[tokio::test]
async fn the_pending_to_active_transition_is_owed_to_the_clock() {
    // The rule: a resource this service reports in a non-terminal state reaches its
    // terminal one after a span the clock measures — never instantly, and never by
    // being read. The rows differ in whether the function carries a revision, since
    // a revision has the same transition.
    for with_revision in [false, true] {
        let emulator = Emulator::new();
        let name = "settling-fn";
        let mut request = json!({ "functionName": name });
        if with_revision {
            request["revisionConfig"] = revision_config(
                None,
                build_config("python3.13", "demo-bucket", "app.zip", None),
                json!({ "executionRoleArn": role_arn("settling") }),
            );
        }
        let created = emulator.create(request).await;
        assert_eq!(
            created.string("state").as_deref(),
            Some("Pending"),
            "case with_revision {with_revision}: the transient state must be observable, got {:?}",
            created.body
        );
        let created_at = created.field("createdAt").cloned().expect("createdAt");

        // Reading does not move it.
        let early = emulator.get(name).await;
        assert_eq!(
            early.string("state").as_deref(),
            Some("Pending"),
            "case with_revision {with_revision}: a read must not advance the state, got {:?}",
            early.body
        );

        tokio::time::sleep(settle_delay()).await;

        let settled = emulator.get(name).await;
        assert_eq!(
            settled.string("state").as_deref(),
            Some("Active"),
            "case with_revision {with_revision}: the transition was due, got {:?}",
            settled.body
        );
        assert_eq!(
            settled.field("createdAt"),
            Some(&created_at),
            "case with_revision {with_revision}: createdAt is fixed at creation, got {:?}",
            settled.body
        );
        assert_ne!(
            settled.field("updatedAt"),
            Some(&created_at),
            "case with_revision {with_revision}: the transition is a change, so updatedAt moved, got {:?}",
            settled.body
        );

        // And the materialised values are stable: a second read reports the same
        // instant, not a fresh one.
        let again = emulator.get(name).await;
        assert_eq!(
            again.field("updatedAt"),
            settled.field("updatedAt"),
            "case with_revision {with_revision}: updatedAt must be stable across reads, got {:?}",
            again.body
        );

        if with_revision {
            let revision_id = created
                .field("revision")
                .and_then(|revision| revision.get("revisionId"))
                .and_then(Value::as_str)
                .expect("revisionId")
                .to_string();
            let revision = emulator.get_revision(name, &revision_id).await;
            assert_eq!(
                revision.string("state").as_deref(),
                Some("Active"),
                "case with_revision {with_revision}: the revision's transition was due too, got {:?}",
                revision.body
            );
        }
    }
}

#[tokio::test]
async fn the_state_filter_narrows_the_listing() {
    // `filters` supports the name `state` only (source: smithy_query doc for
    // ListWebFunctionsRequest$filters), and a function matches when its state is
    // among the filter's values.
    let emulator = Emulator::new();
    for name in ["fn-a", "fn-b", "fn-c"] {
        let created = emulator.create(json!({ "functionName": name })).await;
        assert_eq!(
            created.status, 202,
            "case {name:?}: CreateWebFunction expected 202, got {} with {:?}",
            created.status, created.body
        );
    }

    // While they are settling they are all Pending, and none of them is Active.
    let pending = emulator
        .list(json!({ "filters": [{ "name": "state", "values": ["Pending"] }] }))
        .await;
    assert_eq!(
        page_names(&pending),
        vec!["fn-a", "fn-b", "fn-c"],
        "case Pending: every settling function matches, got {:?}",
        pending.body
    );
    let active = emulator
        .list(json!({ "filters": [{ "name": "state", "values": ["Active"] }] }))
        .await;
    assert!(
        page_names(&active).is_empty(),
        "case Active: nothing is active yet, got {:?}",
        active.body
    );

    tokio::time::sleep(settle_delay()).await;

    // Once the clock has made the transition due, the same two filters answer the
    // other way round.
    let pending = emulator
        .list(json!({ "filters": [{ "name": "state", "values": ["Pending"] }] }))
        .await;
    assert!(
        page_names(&pending).is_empty(),
        "case Pending after settling: nothing is pending, got {:?}",
        pending.body
    );
    let active = emulator
        .list(json!({ "filters": [{ "name": "state", "values": ["Active"] }] }))
        .await;
    assert_eq!(
        page_names(&active),
        vec!["fn-a", "fn-b", "fn-c"],
        "case Active after settling: every function matches, got {:?}",
        active.body
    );

    // Several values are an OR, and paginate like any other listing.
    let walked = walk_pages(
        &emulator,
        2,
        Some(json!({ "filters": [{ "name": "state", "values": ["Active", "Failed"] }] })),
    )
    .await;
    assert_eq!(
        walked,
        vec!["fn-a", "fn-b", "fn-c"],
        "case Active|Failed: a filtered listing paginates too"
    );

    // An unsupported filter name, and a value that is not a FunctionState, are both
    // refused.
    for filter in [
        json!({ "name": "functionName", "values": ["fn-a"] }),
        json!({ "name": "State", "values": ["Active"] }),
        json!({ "name": "state", "values": ["active"] }),
        json!({ "name": "state", "values": ["NotAState"] }),
    ] {
        let reply = emulator.list(json!({ "filters": [filter.clone()] })).await;
        assert_eq!(
            reply.status, 400,
            "case {filter:?}: expected 400, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error(),
            "ValidationException",
            "case {filter:?}: expected ValidationException, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    // The rows all satisfy the model's `NextToken` pattern `^[a-f0-9-]{36}$`, so
    // the boundary passes them; none of them is a cursor this service minted.
    let emulator = Emulator::new();
    for name in ["fn-a", "fn-b"] {
        emulator.create(json!({ "functionName": name })).await;
    }
    let tokens = [
        "aaaaaaaaa-aaa-aaaa-aaaa-aaaaaaaaaaaa",
        "------------------------------------",
        "0000000000000000000000000000000000-0",
        "0-0000000000000000000000000000000000",
    ];
    for token in tokens {
        let reply = emulator
            .list(json!({ "maxResults": 1, "nextToken": token }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: expected 400, got {} with {:?}",
            reply.status, reply.body
        );
    }

    // A cursor this service DID mint resumes where it says.
    let first = emulator.list(json!({ "maxResults": 1 })).await;
    let token = first
        .string("nextToken")
        .expect("a second function remains, so a cursor is expected");
    let second = emulator
        .list(json!({ "maxResults": 1, "nextToken": token }))
        .await;
    assert_eq!(
        page_names(&second),
        vec!["fn-b"],
        "case minted: the cursor resumes at the next function, got {:?}",
        second.body
    );
}

#[tokio::test]
async fn a_function_can_be_named_by_name_or_by_arn() {
    // The member's documentation says the caller "can specify either of the
    // following: The function name, for example my-function. The function ARN."
    // (source: smithy_query doc for GetWebFunctionRequest$functionName). The rows
    // are the two documented spellings of ONE function; the ARN rides in the URI
    // label percent-encoded, the way an SDK sends it.
    let emulator = Emulator::new();
    let name = "named-either-way";
    let created = emulator
        .create(json!({
            "functionName": name,
            "revisionConfig": revision_config(
                None,
                build_config("python3.13", "demo-bucket", "app.zip", None),
                json!({ "executionRoleArn": role_arn("either") }),
            ),
        }))
        .await;
    assert_eq!(
        created.status, 202,
        "CreateWebFunction expected 202, got {} with {:?}",
        created.status, created.body
    );
    let arn = expected_arn(REGION, name);
    let revision_id = created
        .field("revision")
        .and_then(|revision| revision.get("revisionId"))
        .and_then(Value::as_str)
        .expect("revisionId")
        .to_string();
    let encoded_arn = arn.replace(':', "%3A").replace('/', "%2F");

    for spelling in [name.to_string(), encoded_arn.clone()] {
        let fetched = emulator.get(&spelling).await;
        assert_eq!(
            fetched.status, 200,
            "case {spelling:?}: GetWebFunction expected 200, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.string("functionArn"),
            Some(arn.clone()),
            "case {spelling:?}: both spellings name one record, got {:?}",
            fetched.body
        );

        let revision = emulator.get_revision(&spelling, &revision_id).await;
        assert_eq!(
            revision.status, 200,
            "case {spelling:?}: GetWebFunctionRevision expected 200, got {} with {:?}",
            revision.status, revision.body
        );

        let added = emulator
            .create_revision(
                &spelling,
                json!({
                    "buildConfig": build_config("nodejs22.x", "demo-bucket", "app.zip", None),
                    "serviceConfig": { "executionRoleArn": role_arn("either") },
                }),
            )
            .await;
        assert_eq!(
            added.status, 202,
            "case {spelling:?}: CreateWebFunctionRevision expected 202, got {} with {:?}",
            added.status, added.body
        );
        assert_eq!(
            added.string("functionArn"),
            Some(arn.clone()),
            "case {spelling:?}: the revision was added to the same function, got {:?}",
            added.body
        );
    }

    // And a delete by ARN removes the function the name resolves to.
    let deleted = emulator.delete(&encoded_arn).await;
    assert_eq!(
        deleted.status, 204,
        "case arn: DeleteWebFunction expected 204, got {} with {:?}",
        deleted.status, deleted.body
    );
    let fetched = emulator.get(name).await;
    assert_eq!(
        fetched.status, 404,
        "case arn: the function named by ARN is the one that was deleted, got {} with {:?}",
        fetched.status, fetched.body
    );
}

#[tokio::test]
async fn a_service_config_outside_its_modelled_range_is_refused() {
    // `timeoutSeconds` is @range(3..900) and `maxConcurrencyPerEnvironment` is
    // @range(1..128) (model: shape=ServiceConfig). Those ranges are MODELLED, so a
    // value outside them is refused before the handler runs — including an explicit
    // 0, which must not be read as "the member was absent". The rows are one past
    // each published bound, plus 0.
    let emulator = Emulator::new();
    let name = "ranged-fn";
    let created = emulator.create(json!({ "functionName": name })).await;
    assert_eq!(
        created.status, 202,
        "CreateWebFunction expected 202, got {} with {:?}",
        created.status, created.body
    );

    let refused: Vec<(&str, Value)> = vec![
        ("timeoutSeconds 0", json!({ "timeoutSeconds": 0 })),
        ("timeoutSeconds 2", json!({ "timeoutSeconds": 2 })),
        ("timeoutSeconds 901", json!({ "timeoutSeconds": 901 })),
        (
            "maxConcurrencyPerEnvironment 0",
            json!({ "maxConcurrencyPerEnvironment": 0 }),
        ),
        (
            "maxConcurrencyPerEnvironment 129",
            json!({ "maxConcurrencyPerEnvironment": 129 }),
        ),
    ];

    for (case, overrides) in refused {
        let mut service = json!({ "executionRoleArn": role_arn("ranged") });
        for (member, value) in overrides.as_object().expect("an object") {
            service[member] = value.clone();
        }
        let reply = emulator
            .create_revision(
                name,
                json!({
                    "buildConfig": build_config("python3.13", "demo-bucket", "app.zip", None),
                    "serviceConfig": service,
                }),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            reply.status, reply.body
        );
    }
}

#[tokio::test]
async fn a_function_arn_names_the_region_the_request_was_signed_for() {
    // The rule generalises over the signing region: the usual two-word form, a
    // three-word one, and a region whose number is more than one digit.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2", "us-gov-west-10"] {
        let emulator = Emulator::new();
        let name = "regional-fn";
        let created = emulator
            .create_in(region, json!({ "functionName": name }))
            .await;
        assert_eq!(
            created.status, 202,
            "case {region:?}: CreateWebFunction expected 202, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.string("functionArn"),
            Some(expected_arn(region, name)),
            "case {region:?}: expected the ARN to name the signing region, got {:?}",
            created.body
        );

        // And the stored ARN is what every later reader reports, whatever region
        // that reader signed for.
        let fetched = emulator.get(name).await;
        assert_eq!(
            fetched.string("functionArn"),
            Some(expected_arn(region, name)),
            "case {region:?}: Get must report the stored ARN, got {:?}",
            fetched.body
        );
    }
}

// `CreateWebFunction`'s initial endpoint — the `endpointConfig` member, the
// `endpoint` member of its answer and the `GetWebFunctionEndpoint` read-back — is
// covered by the `web_function_endpoint` suite beside this one.
