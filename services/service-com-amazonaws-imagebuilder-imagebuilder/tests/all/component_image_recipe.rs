// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the component and image-recipe slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! * `CreateComponent` persists one BUILD VERSION of the component the request
//!   describes, under an ARN derived from a normalized form of the name plus the
//!   version and the build number, and derives the component TYPE and its parameter
//!   details from the document rather than from the request; `GetComponent` reports that
//!   stored record, resolving a version ARN to the latest matching build version.
//! * `CreateImageRecipe` persists the recipe, with every `componentArn` resolved against
//!   the stored components and the platform taken from them; `GetImageRecipe` reports it
//!   back, components in creation order, and `ListImageRecipes` enumerates, filters and
//!   paginates the stored recipes.
//! * A delete removes the record and reports the ARN it removed, and a component a
//!   stored recipe still includes is not removed at all.
//!
//! None of the EC2 values a recipe names is resolved at create time — a parent image is
//! "AMI ID", an "Image Builder image Amazon Resource Name (ARN)", an SSM parameter or a
//! Marketplace product id, none of which any other service in this emulator holds — so
//! the rows below name values nothing else knows and still expect a 200.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for when it only needs one; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// Members a create carries that the stored COMPONENT does not report back under the
/// same name: `clientToken` and `dryRun` describe the call, `semanticVersion` is
/// reported as `version`, and `uri` names WHERE the document was fetched from — the
/// document itself is reported as the component's own `data`
/// (`crate::component_document_sources`).
const NOT_COMPONENT_MEMBERS: &[&str] = &["clientToken", "dryRun", "semanticVersion", "uri"];

/// Members a create carries that the stored RECIPE does not report back under the same
/// name: `clientToken` and `dryRun` describe the call, and `semanticVersion` is reported
/// as `version`.
const NOT_RECIPE_MEMBERS: &[&str] = &["clientToken", "dryRun", "semanticVersion"];

/// A build component document: one `build` phase, so the derived type is `BUILD`.
const BUILD_DOCUMENT: &str =
    "name: HelloWorld\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: Greet\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo hello\n";

/// A test component document: a single phase named `test`, so the derived type is
/// `TEST`.
const TEST_DOCUMENT: &str =
    "name: CheckWorld\nschemaVersion: 1.0\nphases:\n  - name: test\n    steps:\n      - name: Check\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo ok\n";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `imagebuilder` in `region` — the signal the router routes on
/// and the handlers read the Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{region}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// `CreateComponent` with `body`, in `region`.
async fn create_component_in(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(router, region, "PUT", "/CreateComponent", Some(body)).await
}

/// `CreateComponent` with `body`, in [`REGION`].
async fn create_component(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    create_component_in(router, REGION, body).await
}

/// `GetComponent` for `arn`, in `region`.
async fn get_component_in(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/GetComponent?componentBuildVersionArn={arn}"),
        None,
    )
    .await
}

/// `GetComponent` for `arn`, in [`REGION`].
async fn get_component(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    get_component_in(router, REGION, arn).await
}

/// `DeleteComponent` for `arn`, in [`REGION`].
async fn delete_component(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call_in(
        router,
        REGION,
        "DELETE",
        &format!("/DeleteComponent?componentBuildVersionArn={arn}"),
        None,
    )
    .await
}

/// `CreateImageRecipe` with `body`, in [`REGION`].
async fn create_recipe(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call_in(router, REGION, "PUT", "/CreateImageRecipe", Some(body)).await
}

/// `GetImageRecipe` for `arn`, in [`REGION`].
async fn get_recipe(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call_in(
        router,
        REGION,
        "GET",
        &format!("/GetImageRecipe?imageRecipeArn={arn}"),
        None,
    )
    .await
}

/// `DeleteImageRecipe` for `arn`, in [`REGION`].
async fn delete_recipe(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call_in(
        router,
        REGION,
        "DELETE",
        &format!("/DeleteImageRecipe?imageRecipeArn={arn}"),
        None,
    )
    .await
}

/// `ListImageRecipes` with `body`, in [`REGION`].
async fn list_recipes(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call_in(router, REGION, "POST", "/ListImageRecipes", Some(body)).await
}

/// A `CreateComponent` body: the required members, `clientToken`, and whatever else
/// `extra` names.
fn component_body(name: &str, version: &str, client_token: &str, extra: Value) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!(version));
    body.insert("platform".to_string(), json!("Linux"));
    body.insert("clientToken".to_string(), json!(client_token));
    body.insert("data".to_string(), json!(BUILD_DOCUMENT));
    merge(&mut body, extra);
    Value::Object(body)
}

/// A `CreateImageRecipe` body: the required members, `clientToken`, and whatever else
/// `extra` names.
fn recipe_body(name: &str, version: &str, client_token: &str, extra: Value) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!(version));
    body.insert("parentImage".to_string(), json!("ami-0123456789abcdef0"));
    body.insert("clientToken".to_string(), json!(client_token));
    merge(&mut body, extra);
    Value::Object(body)
}

/// Overlay `extra`'s members onto `body`, so a row can both add and replace one.
fn merge(body: &mut Map<String, Value>, extra: Value) {
    if let Value::Object(extra) = extra {
        for (member, value) in extra {
            body.insert(member, value);
        }
    }
}

/// The ARN one build version of the component `normalized` carries in `region`.
fn component_arn(region: &str, normalized: &str, version: &str, build: u32) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:component/{normalized}/{version}/{build}")
}

/// The ARN the image recipe `normalized` carries at `version` in `region`.
fn recipe_arn(region: &str, normalized: &str, version: &str) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:image-recipe/{normalized}/{version}")
}

/// The error code a refusal carries, from `__type` (restJson1 renders the shape there,
/// optionally namespace-qualified).
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .expect("a non-empty error type")
        .to_string()
}

/// Assert `status` and the error code of a refusal.
fn assert_refused(case: &str, status: u16, body: &Value, expect_status: u16, expect_code: &str) {
    assert_eq!(
        status, expect_status,
        "case {case:?}: expected HTTP {expect_status}, got {status} with {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, expect_code,
        "case {case:?}: expected the error code {expect_code:?}, got {code:?}"
    );
}

/// One member of a response, or a panic naming the case.
fn member<'body>(case: &str, body: &'body Value, name: &str) -> &'body Value {
    body.get(name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} in {body}"))
}

/// The text one member of a response carries.
fn text(case: &str, body: &Value, name: &str) -> String {
    member(case, body, name)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected {name} to be text in {body}"))
        .to_string()
}

/// Assert a response carries an instant in the form this model's `DateTime` members use.
fn assert_instant(case: &str, body: &Value, name: &str) {
    let instant = text(case, body, name);
    assert!(
        instant.contains('T') && instant.ends_with('Z') && instant.len() >= 20,
        "case {case:?}: expected {name} to be an ISO 8601 instant, got {instant:?}"
    );
}

/// Assert every RESOURCE member of `sent` comes back from the read as it was sent.
fn assert_round_tripped(case: &str, sent: &Value, reported: &Value, not_reported: &[&str]) {
    let sent = sent.as_object().expect("a create body is an object");
    for (name, value) in sent {
        if not_reported.contains(&name.as_str()) {
            continue;
        }
        let actual = reported.get(name);
        assert_eq!(
            actual,
            Some(value),
            "case {case:?}: expected the stored {name} to be {value}, got {actual:?}"
        );
    }
    // `semanticVersion` is what a create assigns and `version` is what a read reports.
    let expected_version = sent.get("semanticVersion");
    let actual_version = reported.get("version");
    assert_eq!(
        actual_version,
        expected_version,
        "case {case:?}: expected version {expected_version:?}, got {actual_version:?}"
    );
}

/// One row of the component round-trip table.
struct ComponentRow {
    case: &'static str,
    name: &'static str,
    normalized: &'static str,
    version: &'static str,
    extra: fn() -> Value,
    component_type: &'static str,
}

/// The component round-trip table. The rows differ along what the rule generalises
/// over: which optional members are present, how many entries a list or a map carries,
/// the platform, the version's shape, which of the two documented document methods the
/// request used, and which type the document implies.
fn component_rows() -> Vec<ComponentRow> {
    vec![
        ComponentRow {
            case: "only the required members",
            name: "minimal-component",
            normalized: "minimal-component",
            version: "1.0.0",
            extra: || json!({}),
            component_type: "BUILD",
        },
        ComponentRow {
            case: "a name that normalizes, every description and one tag",
            name: "Golden_Image Build",
            normalized: "golden-image-build",
            version: "2.11.3",
            extra: || {
                json!({
                    "description": "installs the baseline packages",
                    "changeDescription": "adds the agent",
                    "tags": {"owner": "platform"},
                })
            },
            component_type: "BUILD",
        },
        ComponentRow {
            case: "a test document, three supported OS versions and a KMS key",
            name: "smoke-test",
            normalized: "smoke-test",
            version: "0.0.1",
            extra: || {
                json!({
                    "data": TEST_DOCUMENT,
                    "supportedOsVersions": ["Amazon Linux 2", "Amazon Linux 2023", "Ubuntu 22"],
                    "kmsKeyId": format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/11111111-2222-3333-4444-555555555555"),
                })
            },
            component_type: "TEST",
        },
        ComponentRow {
            case: "a Windows component with three tags and a date-shaped version",
            name: "windows-prep",
            normalized: "windows-prep",
            version: "2026.1.1",
            extra: || {
                json!({
                    "platform": "Windows",
                    "tags": {"stage": "build", "team": "images", "costCenter": "4711"},
                })
            },
            component_type: "BUILD",
        },
        ComponentRow {
            case: "a macOS component at the documented version bound",
            name: "macos-prep",
            normalized: "macos-prep",
            version: "1073741823.1073741823.1073741823",
            extra: || json!({"platform": "macOS"}),
            component_type: "BUILD",
        },
    ]
}

#[tokio::test]
async fn a_created_component_is_reported_back_member_for_member() {
    let router = emulator();

    for (n, row) in component_rows().into_iter().enumerate() {
        let body = component_body(row.name, row.version, &format!("ctok-{n:04}"), (row.extra)());

        let (status, created) = create_component(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the create, got {status} with {created}",
            row.case
        );

        let expected_arn = component_arn(REGION, row.normalized, row.version, 1);
        let arn = text(row.case, &created, "componentBuildVersionArn");
        assert_eq!(
            arn, expected_arn,
            "case {:?}: expected the ARN {expected_arn:?}, got {arn:?}",
            row.case
        );
        let echoed = text(row.case, &created, "clientToken");
        assert_eq!(
            echoed,
            format!("ctok-{n:04}"),
            "case {:?}: expected the clientToken echoed, got {echoed:?}",
            row.case
        );
        assert!(
            !text(row.case, &created, "requestId").is_empty(),
            "case {:?}: expected a requestId in {created}",
            row.case
        );

        let (status, read) = get_component(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the read, got {status} with {read}",
            row.case
        );
        let component = member(row.case, &read, "component").clone();
        assert_round_tripped(row.case, &body, &component, NOT_COMPONENT_MEMBERS);

        let derived: Vec<(&str, Value)> = vec![
            ("arn", json!(expected_arn)),
            ("type", json!(row.component_type)),
            ("state", json!({"status": "ACTIVE"})),
            ("owner", json!(ACCOUNT)),
            ("encrypted", json!(true)),
        ];
        for (name, expected) in derived {
            let actual = component.get(name);
            assert_eq!(
                actual,
                Some(&expected),
                "case {:?}: expected the derived {name} to be {expected}, got {actual:?}",
                row.case
            );
        }
        assert_instant(row.case, &component, "dateCreated");
    }
}

#[tokio::test]
async fn the_component_type_and_parameters_come_from_the_document() {
    let router = emulator();

    // Each row is a document and the type it implies: only a document whose phases are
    // the single phase named `test` is a TEST component.
    let rows: &[(&str, &str, &str)] = &[
        ("one build phase", BUILD_DOCUMENT, "BUILD"),
        ("one test phase", TEST_DOCUMENT, "TEST"),
        (
            "a build and a test phase",
            "name: Both\nphases:\n  - name: build\n    steps: []\n  - name: test\n    steps: []\n",
            "BUILD",
        ),
        (
            "a validate phase",
            "name: Validate\nphases:\n  - name: validate\n    steps: []\n",
            "BUILD",
        ),
    ];
    for (n, (case, document, expected)) in rows.iter().enumerate() {
        let body = component_body(
            &format!("typed-component-{n}"),
            "1.0.0",
            &format!("dtok-{n:04}"),
            json!({"data": document}),
        );
        let (status, created) = create_component(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the create, got {status} with {created}"
        );
        let arn = text(case, &created, "componentBuildVersionArn");
        let (_, read) = get_component(&router, &arn).await;
        let component = member(case, &read, "component").clone();
        let actual = component.get("type");
        assert_eq!(
            actual,
            Some(&json!(expected)),
            "case {case:?}: expected the derived type {expected:?}, got {actual:?}"
        );
    }

    // The parameters the document defines are reported as parameter details; a
    // parameter without a type declares nothing a read can report.
    let body = component_body(
        "parameterised-component",
        "1.0.0",
        "ptok-0001",
        json!({
            "data": "name: Parameterised\nschemaVersion: 1.0\nparameters:\n  - Size:\n      type: string\n      default: small\n      description: how big\n  - Hosts:\n      type: stringList\n      default:\n        - alpha\n        - beta\n  - Untyped:\n      description: declares nothing\nphases:\n  - name: build\n    steps: []\n",
        }),
    );
    let (status, created) = create_component(&router, &body).await;
    assert_eq!(
        status, 200,
        "case parameters: expected HTTP 200 from the create, got {status} with {created}"
    );
    let arn = text("parameters", &created, "componentBuildVersionArn");
    let (_, read) = get_component(&router, &arn).await;
    let component = member("parameters", &read, "component").clone();
    let actual = component.get("parameters");
    assert_eq!(
        actual,
        Some(&json!([
            {"name": "Size", "type": "string", "defaultValue": ["small"], "description": "how big"},
            {"name": "Hosts", "type": "stringList", "defaultValue": ["alpha", "beta"]},
        ])),
        "case parameters: expected the two typed parameters in document order, got {actual:?}"
    );
}

#[tokio::test]
async fn a_repeated_name_and_version_gets_the_next_build_version() {
    let router = emulator();

    // Three creates of one name+version pair, each with different content, so each is a
    // new build version of it.
    for (n, change) in ["first", "second", "third"].iter().enumerate() {
        let body = component_body(
            "Versioned Component",
            "1.4.0",
            &format!("btok-{n:04}"),
            json!({"changeDescription": *change}),
        );
        let (status, created) = create_component(&router, &body).await;
        assert_eq!(
            status, 200,
            "case build {n}: expected HTTP 200 from the create, got {status} with {created}"
        );
        let expected = component_arn(
            REGION,
            "versioned-component",
            "1.4.0",
            u32::try_from(n).expect("a small build number") + 1,
        );
        let arn = text("build version", &created, "componentBuildVersionArn");
        assert_eq!(
            arn, expected,
            "case build {n}: expected the ARN {expected:?}, got {arn:?}"
        );
    }

    // "If the content is also identical to the latest build version, the request fails
    // because the component already exists." A create whose resource already exists is
    // refused as the create-collision it is, naming the build version that holds it.
    let repeat = component_body(
        "Versioned Component",
        "1.4.0",
        "btok-repeat",
        json!({"changeDescription": "third"}),
    );
    let (status, refused) = create_component(&router, &repeat).await;
    assert_refused(
        "identical content",
        status,
        &refused,
        400,
        "ResourceAlreadyExistsException",
    );
    let message = text("identical content", &refused, "message");
    let expected = format!(
        "The following resource 'Component' already exists: '{}'.",
        component_arn(REGION, "versioned-component", "1.4.0", 3)
    );
    assert_eq!(
        message, expected,
        "case \"identical content\": expected {expected:?}, got {message:?}"
    );

    // A version ARN, with or without trailing wildcards, resolves to the latest build.
    let latest = component_arn(REGION, "versioned-component", "1.4.0", 3);
    let references: &[(&str, String)] = &[
        (
            "exact version",
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/versioned-component/1.4.0"),
        ),
        (
            "patch wildcard",
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/versioned-component/1.4.x"),
        ),
        (
            "minor and patch wildcard",
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/versioned-component/1.x.x"),
        ),
        (
            "every node a wildcard",
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/versioned-component/x.x.x"),
        ),
        ("the build version itself", latest.clone()),
    ];
    for (case, reference) in references {
        let (status, read) = get_component(&router, reference).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the read, got {status} with {read}"
        );
        let component = member(case, &read, "component").clone();
        let actual = component.get("arn");
        assert_eq!(
            actual,
            Some(&json!(latest)),
            "case {case:?}: expected the latest build {latest:?}, got {actual:?}"
        );
    }

    // An earlier build version is still its own record, and a version nothing was
    // stored under resolves to nothing.
    let (status, read) = get_component(
        &router,
        &component_arn(REGION, "versioned-component", "1.4.0", 1),
    )
    .await;
    assert_eq!(
        status, 200,
        "case first build: expected HTTP 200 from the read, got {status} with {read}"
    );
    let first = member("first build", &read, "component").clone();
    assert_eq!(
        first.get("changeDescription"),
        Some(&json!("first")),
        "case first build: expected the first build's own content, got {first}"
    );

    let (status, refused) = get_component(
        &router,
        &format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/versioned-component/9.9.9"),
    )
    .await;
    assert_refused(
        "a version nothing was stored under",
        status,
        &refused,
        404,
        "ResourceNotFoundException",
    );

    // A regional endpoint reaches only its own Region's components.
    let (status, refused) = get_component_in(&router, "eu-west-1", &latest).await;
    assert_refused(
        "another region's endpoint",
        status,
        &refused,
        404,
        "ResourceNotFoundException",
    );
}

#[tokio::test]
async fn a_replayed_client_token_answers_the_original_response() {
    let router = emulator();

    let body = component_body("idempotent-component", "1.0.0", "itok-0001", json!({}));
    let (status, first) = create_component(&router, &body).await;
    assert_eq!(
        status, 200,
        "case first create: expected HTTP 200, got {status} with {first}"
    );
    let (status, replay) = create_component(&router, &body).await;
    assert_eq!(
        status, 200,
        "case replay: expected HTTP 200, got {status} with {replay}"
    );
    assert_eq!(
        replay, first,
        "case replay: expected the original response, got {replay}"
    );

    // One component, not two: the replay did not create a second build version.
    let (status, refused) = get_component(
        &router,
        &component_arn(REGION, "idempotent-component", "1.0.0", 2),
    )
    .await;
    assert_refused(
        "a second build version",
        status,
        &refused,
        404,
        "ResourceNotFoundException",
    );

    // The same token with different creation parameters is the modelled mismatch, for
    // each create that takes one.
    let changed = component_body(
        "idempotent-component",
        "1.0.0",
        "itok-0001",
        json!({"description": "changed"}),
    );
    let (status, refused) = create_component(&router, &changed).await;
    assert_refused(
        "a component token with changed parameters",
        status,
        &refused,
        400,
        "IdempotentParameterMismatchException",
    );

    let recipe = recipe_body("idempotent-recipe", "1.0.0", "itok-0001", json!({}));
    let (status, first) = create_recipe(&router, &recipe).await;
    assert_eq!(
        status, 200,
        "case a recipe under a token a component used: expected HTTP 200, got {status} with \
         {first}"
    );
    let (status, replay) = create_recipe(&router, &recipe).await;
    assert_eq!(
        status, 200,
        "case recipe replay: expected HTTP 200, got {status} with {replay}"
    );
    assert_eq!(
        replay, first,
        "case recipe replay: expected the original response, got {replay}"
    );
    let changed = recipe_body(
        "idempotent-recipe",
        "1.0.0",
        "itok-0001",
        json!({"description": "changed"}),
    );
    let (status, refused) = create_recipe(&router, &changed).await;
    assert_refused(
        "a recipe token with changed parameters",
        status,
        &refused,
        400,
        "IdempotentParameterMismatchException",
    );
}

#[tokio::test]
async fn a_create_this_service_cannot_serve_is_refused_the_documented_way() {
    let router = emulator();

    // Each row is a create body and the refusal it earns. The version rows span the
    // documented bound (2^30-1) and one past it, both ends of the three-node syntax, and
    // the wildcard the recipe's version pattern admits for filtering only.
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a component naming both data and uri",
            component_body(
                "both-sources",
                "1.0.0",
                "rtok-0001",
                json!({"uri": "s3://bucket/key.yaml"}),
            ),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a component naming neither data nor uri",
            json!({
                "name": "no-source",
                "semanticVersion": "1.0.0",
                "platform": "Linux",
                "clientToken": "rtok-0002",
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a component version one past the documented bound",
            component_body("over-bound", "1073741824.0.0", "rtok-0003", json!({})),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a component version whose nodes are not numbers at all, refused by the modelled \
             pattern before the handler",
            component_body("not-numbers", "one.two.three", "rtok-0004", json!({})),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a component uri that is not an S3 URL",
            json!({
                "name": "bad-uri",
                "semanticVersion": "1.0.0",
                "platform": "Linux",
                "clientToken": "rtok-0005",
                "uri": "https://example.com/component.yaml",
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a component document that is not YAML",
            component_body(
                "bad-document",
                "1.0.0",
                "rtok-0006",
                json!({"data": "phases: [\n"}),
            ),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a component document built to drive the YAML reader's recursion",
            component_body(
                "deep-document",
                "1.0.0",
                "rtok-0008",
                json!({"data": format!("phases: {}", "[".repeat(8_000))}),
            ),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a component document declaring no phases",
            component_body(
                "no-phases-document",
                "1.0.0",
                "rtok-0009",
                json!({"data": "name: x\nschemaVersion: 1.0\n"}),
            ),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a component dry run",
            component_body("dry-run-component", "1.0.0", "rtok-0007", json!({"dryRun": true})),
            412,
            "DryRunOperationException",
        ),
    ];
    for (case, body, expect_status, expect_code) in rows {
        let (status, answered) = create_component(&router, &body).await;
        assert_refused(case, status, &answered, expect_status, expect_code);
    }

    // A dry run stored nothing, so the ARN it would have minted names nothing.
    let (status, refused) =
        get_component(&router, &component_arn(REGION, "dry-run-component", "1.0.0", 1)).await;
    assert_refused(
        "the dry run's ARN",
        status,
        &refused,
        404,
        "ResourceNotFoundException",
    );

    // The same rules on the recipe create: a wildcard version is for filtering, not for
    // assignment, and a dry run stores nothing.
    let recipe_rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a recipe version carrying a wildcard node",
            recipe_body("wildcard-recipe", "1.x.x", "rtok-0101", json!({})),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a recipe version one past the documented bound",
            recipe_body("over-bound-recipe", "0.0.1073741824", "rtok-0102", json!({})),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a recipe dry run",
            recipe_body("dry-run-recipe", "1.0.0", "rtok-0103", json!({"dryRun": true})),
            412,
            "DryRunOperationException",
        ),
    ];
    for (case, body, expect_status, expect_code) in recipe_rows {
        let (status, answered) = create_recipe(&router, &body).await;
        assert_refused(case, status, &answered, expect_status, expect_code);
    }
    let (status, refused) = get_recipe(&router, &recipe_arn(REGION, "dry-run-recipe", "1.0.0")).await;
    assert_refused(
        "the recipe dry run's ARN",
        status,
        &refused,
        404,
        "ResourceNotFoundException",
    );
}

/// Create the components the recipe cases include, and answer their build version ARNs.
async fn stored_components(router: &MultiServiceRouter) -> Vec<String> {
    let mut arns = Vec::new();
    for (n, (name, version, platform, document)) in [
        ("prep-linux", "1.0.0", "Linux", BUILD_DOCUMENT),
        ("check-linux", "2.5.0", "Linux", TEST_DOCUMENT),
        ("prep-windows", "1.0.0", "Windows", BUILD_DOCUMENT),
    ]
    .iter()
    .enumerate()
    {
        let body = component_body(
            name,
            version,
            &format!("stok-{n:04}"),
            json!({"platform": platform, "data": document}),
        );
        let (status, created) = create_component(router, &body).await;
        assert_eq!(
            status, 200,
            "case component {name}: expected HTTP 200, got {status} with {created}"
        );
        arns.push(text(name, &created, "componentBuildVersionArn"));
    }
    arns
}

#[tokio::test]
async fn a_created_recipe_is_reported_back_member_for_member() {
    let router = emulator();
    let components = stored_components(&router).await;
    let linux_build = components
        .first()
        .cloned()
        .expect("the linux build component");
    let linux_test = components.get(1).cloned().expect("the linux test component");

    // The rows differ along which optional members are present, how many components the
    // recipe includes and in which order, and whether a reference is a build version
    // ARN or a wildcard version ARN.
    let rows: Vec<(&str, &str, &str, &str, Value)> = vec![
        (
            "only the required members",
            "minimal-recipe",
            "minimal-recipe",
            "1.0.0",
            json!({}),
        ),
        (
            "one component, by build version ARN",
            "One_Component Recipe",
            "one-component-recipe",
            "1.2.3",
            json!({
                "description": "the golden image",
                "components": [{"componentArn": linux_build}],
                "workingDirectory": "/tmp/build",
            }),
        ),
        (
            "two components, test before build, with parameters",
            "ordered-recipe",
            "ordered-recipe",
            "4.0.0",
            json!({
                "components": [
                    {"componentArn": linux_test, "parameters": [{"name": "Size", "value": ["small"]}]},
                    {"componentArn": linux_build},
                ],
            }),
        ),
        (
            "a component named by a wildcard version ARN",
            "wildcard-reference-recipe",
            "wildcard-reference-recipe",
            "1.0.1",
            json!({
                "components": [{"componentArn": format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/prep-linux/1.x.x")}],
            }),
        ),
        (
            "block device mappings, instance configuration and both tag maps",
            "fully-described-recipe",
            "fully-described-recipe",
            "2026.1.1",
            json!({
                "components": [{"componentArn": components.first().cloned().unwrap_or_default()}],
                "blockDeviceMappings": [
                    {"deviceName": "/dev/xvda", "ebs": {"volumeSize": 30, "volumeType": "gp3", "deleteOnTermination": true}},
                    {"deviceName": "/dev/sdb", "virtualName": "ephemeral0"},
                ],
                "additionalInstanceConfiguration": {
                    "systemsManagerAgent": {"uninstallAfterBuild": true},
                    "userDataOverride": "IyEvYmluL2Jhc2gKZWNobyBoaQ==",
                },
                "amiTags": {"lineage": "golden"},
                "tags": {"owner": "platform", "team": "images"},
            }),
        ),
        (
            "a parent image named as an SSM parameter",
            "ssm-parent-recipe",
            "ssm-parent-recipe",
            "1.0.0",
            json!({
                "parentImage": "ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64",
            }),
        ),
    ];

    for (n, (case, name, normalized, version, extra)) in rows.into_iter().enumerate() {
        let body = recipe_body(name, version, &format!("rctok-{n:04}"), extra);
        let (status, created) = create_recipe(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the create, got {status} with {created}"
        );
        let expected_arn = recipe_arn(REGION, normalized, version);
        let arn = text(case, &created, "imageRecipeArn");
        assert_eq!(
            arn, expected_arn,
            "case {case:?}: expected the ARN {expected_arn:?}, got {arn:?}"
        );

        let (status, read) = get_recipe(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the read, got {status} with {read}"
        );
        let recipe = member(case, &read, "imageRecipe").clone();
        assert_round_tripped(case, &body, &recipe, NOT_RECIPE_MEMBERS);
        assert_eq!(
            recipe.get("arn"),
            Some(&json!(expected_arn)),
            "case {case:?}: expected the stored arn {expected_arn:?}, got {recipe}"
        );
        assert_eq!(
            recipe.get("owner"),
            Some(&json!(ACCOUNT)),
            "case {case:?}: expected the caller's account as owner, got {recipe}"
        );
        assert_instant(case, &recipe, "dateCreated");

        // Every recipe reports ONE platform: the platform of the components it includes,
        // or — for a recipe that includes none — the platform of its parent image. Every
        // row here names Linux components or a Linux parent image, so every row reports
        // Linux; `a_recipe_reports_the_platform_of_what_it_builds` spans the rest.
        let actual_platform = recipe.get("platform").cloned();
        assert_eq!(
            actual_platform,
            Some(json!("Linux")),
            "case {case:?}: expected platform Linux, got {actual_platform:?}"
        );
    }
}

#[tokio::test]
async fn a_recipe_is_refused_when_its_components_do_not_resolve() {
    let router = emulator();
    let components = stored_components(&router).await;
    let linux_build = components
        .first()
        .cloned()
        .expect("the linux build component");
    let windows_build = components
        .get(2)
        .cloned()
        .expect("the windows build component");

    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a componentArn naming a component nothing stored",
            json!({"components": [{"componentArn": format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/no-such-component/1.0.0/1")}]}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a componentArn naming a version nothing stored",
            json!({"components": [{"componentArn": format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/prep-linux/9.0.0")}]}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a componentArn naming a build version nothing stored",
            json!({"components": [{"componentArn": format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/prep-linux/1.0.0/7")}]}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a componentArn of another Region",
            json!({"components": [{"componentArn": format!("arn:aws:imagebuilder:eu-west-1:{ACCOUNT}:component/prep-linux/1.0.0/1")}]}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "components of two platforms",
            json!({"components": [
                {"componentArn": linux_build.clone()},
                {"componentArn": windows_build},
            ]}),
            400,
            "InvalidRequestException",
        ),
        (
            "one component named twice, by two references that resolve to it",
            json!({"components": [
                {"componentArn": linux_build.clone()},
                {"componentArn": format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/prep-linux/1.x.x")},
            ]}),
            400,
            "InvalidRequestException",
        ),
    ];
    for (n, (case, extra, expect_status, expect_code)) in rows.into_iter().enumerate() {
        let body = recipe_body(
            &format!("refused-recipe-{n}"),
            "1.0.0",
            &format!("rrtok-{n:04}"),
            extra,
        );
        let (status, refused) = create_recipe(&router, &body).await;
        assert_refused(case, status, &refused, expect_status, expect_code);

        // Nothing was stored for a refused create.
        let (status, missing) =
            get_recipe(&router, &recipe_arn(REGION, &format!("refused-recipe-{n}"), "1.0.0")).await;
        assert_refused(case, status, &missing, 404, "ResourceNotFoundException");
    }

    // "A recipe can contain a maximum of 20 build and test components in any
    // combination, by default." The count is refused before any reference is resolved,
    // so the entries need only be well-formed.
    let over_quota: Vec<Value> = (0..21)
        .map(|_| json!({"componentArn": linux_build.clone()}))
        .collect();
    let body = recipe_body(
        "over-quota-recipe",
        "1.0.0",
        "rrtok-quota",
        json!({"components": over_quota}),
    );
    let (status, refused) = create_recipe(&router, &body).await;
    assert_refused(
        "more components than the documented quota",
        status,
        &refused,
        402,
        "ServiceQuotaExceededException",
    );

    // One name+version pair is stored at most once.
    let body = recipe_body(
        "Unique Recipe",
        "1.0.0",
        "rrtok-unique",
        json!({"components": [{"componentArn": linux_build}]}),
    );
    let (status, created) = create_recipe(&router, &body).await;
    assert_eq!(
        status, 200,
        "case the first create: expected HTTP 200, got {status} with {created}"
    );
    let again = recipe_body(
        "unique_recipe",
        "1.0.0",
        "rrtok-unique-2",
        json!({"description": "another recipe under a name that normalizes alike"}),
    );
    let (status, refused) = create_recipe(&router, &again).await;
    assert_refused(
        "a name and version already taken",
        status,
        &refused,
        400,
        "ResourceAlreadyExistsException",
    );
}

#[tokio::test]
async fn listing_reports_the_stored_recipes_filtered_and_paginated() {
    let router = emulator();
    let components = stored_components(&router).await;
    let linux_build = components
        .first()
        .cloned()
        .expect("the linux build component");
    let windows_build = components
        .get(2)
        .cloned()
        .expect("the windows build component");

    // Three recipes, differing in name, parent image and platform — the three members
    // the `filters` member lists.
    let created: Vec<(&str, String)> = vec![
        (
            "alpha-recipe",
            recipe_arn(REGION, "alpha-recipe", "1.0.0"),
        ),
        ("beta-recipe", recipe_arn(REGION, "beta-recipe", "1.0.0")),
        (
            "gamma-recipe",
            recipe_arn(REGION, "gamma-recipe", "1.0.0"),
        ),
    ];
    let bodies = vec![
        recipe_body(
            "alpha-recipe",
            "1.0.0",
            "ltok-0001",
            json!({"components": [{"componentArn": linux_build.clone()}]}),
        ),
        recipe_body(
            "beta-recipe",
            "1.0.0",
            "ltok-0002",
            json!({
                "parentImage": "ami-ffffffffffffffffff",
                "components": [{"componentArn": windows_build}],
            }),
        ),
        recipe_body(
            "gamma-recipe",
            "1.0.0",
            "ltok-0003",
            json!({"components": [{"componentArn": linux_build}], "tags": {"owner": "platform"}}),
        ),
    ];
    for body in &bodies {
        let (status, answered) = create_recipe(&router, body).await;
        assert_eq!(
            status, 200,
            "case seeding: expected HTTP 200, got {status} with {answered}"
        );
    }

    /// The ARNs one page carries, in the order it carried them.
    fn page_arns(case: &str, body: &Value) -> Vec<String> {
        member(case, body, "imageRecipeSummaryList")
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list in {body}"))
            .iter()
            .map(|summary| {
                summary
                    .get("arn")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    // The whole listing, with no page size: every stored recipe and no token, since
    // nothing remains.
    let (status, listed) = list_recipes(&router, &json!({})).await;
    assert_eq!(
        status, 200,
        "case the whole listing: expected HTTP 200, got {status} with {listed}"
    );
    let all = page_arns("the whole listing", &listed);
    let expected: Vec<String> = created.iter().map(|(_, arn)| arn.clone()).collect();
    assert_eq!(
        all, expected,
        "case the whole listing: expected {expected:?}, got {all:?}"
    );
    assert_eq!(
        listed.get("nextToken"),
        None,
        "case the whole listing: expected no nextToken, got {listed}"
    );
    // A summary reports what the create stored, not an empty shell.
    let summaries = member("the whole listing", &listed, "imageRecipeSummaryList").clone();
    let gamma = summaries
        .as_array()
        .and_then(|summaries| summaries.iter().find(|summary| summary.get("name") == Some(&json!("gamma-recipe"))))
        .cloned()
        .unwrap_or_else(|| panic!("case the whole listing: expected gamma-recipe in {summaries}"));
    assert_eq!(
        gamma.get("tags"),
        Some(&json!({"owner": "platform"})),
        "case the whole listing: expected gamma-recipe's tags, got {gamma}"
    );
    assert_eq!(
        gamma.get("platform"),
        Some(&json!("Linux")),
        "case the whole listing: expected gamma-recipe's platform, got {gamma}"
    );
    assert_instant("the whole listing", &gamma, "dateCreated");

    // One recipe per page: each page carries a token while a recipe remains, and the
    // last one carries none.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for step in 0..created.len() {
        let mut body = Map::new();
        body.insert("maxResults".to_string(), json!(1));
        if let Some(token) = &token {
            body.insert("nextToken".to_string(), json!(token));
        }
        let (status, page) = list_recipes(&router, &Value::Object(body)).await;
        assert_eq!(
            status, 200,
            "case page {step}: expected HTTP 200, got {status} with {page}"
        );
        let arns = page_arns("a page", &page);
        assert_eq!(
            arns.len(),
            1,
            "case page {step}: expected one entry, got {arns:?}"
        );
        walked.extend(arns);
        token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        let expects_token = step + 1 < created.len();
        assert_eq!(
            token.is_some(),
            expects_token,
            "case page {step}: expected a nextToken={expects_token}, got {page}"
        );
    }
    assert_eq!(
        walked, expected,
        "case the walked pages: expected {expected:?}, got {walked:?}"
    );

    // A token this service did not issue for this listing is refused, however it is
    // wrong.
    let (_, first_page) = list_recipes(&router, &json!({"maxResults": 1})).await;
    let issued = first_page
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let tokens: Vec<(&str, Value)> = vec![
        ("a fabricated token", json!({"nextToken": "not-a-token", "maxResults": 1})),
        (
            "a token issued for another query",
            json!({"nextToken": issued, "maxResults": 1, "filters": [{"name": "name", "values": ["alpha-recipe"]}]}),
        ),
    ];
    for (case, body) in tokens {
        let (status, refused) = list_recipes(&router, &body).await;
        assert_refused(case, status, &refused, 400, "InvalidParameterValueException");
    }

    // Each documented filter name, including the two the issue did not cite, and an
    // unsupported one.
    let filtered: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "the name filter",
            json!({"filters": [{"name": "name", "values": ["beta-recipe"]}]}),
            vec![recipe_arn(REGION, "beta-recipe", "1.0.0")],
        ),
        (
            "the name filter over two values",
            json!({"filters": [{"name": "name", "values": ["alpha-recipe", "gamma-recipe"]}]}),
            vec![
                recipe_arn(REGION, "alpha-recipe", "1.0.0"),
                recipe_arn(REGION, "gamma-recipe", "1.0.0"),
            ],
        ),
        (
            "the parentImage filter",
            json!({"filters": [{"name": "parentImage", "values": ["ami-ffffffffffffffffff"]}]}),
            vec![recipe_arn(REGION, "beta-recipe", "1.0.0")],
        ),
        (
            "the platform filter",
            json!({"filters": [{"name": "platform", "values": ["Windows"]}]}),
            vec![recipe_arn(REGION, "beta-recipe", "1.0.0")],
        ),
        (
            "two filters at once",
            json!({"filters": [
                {"name": "platform", "values": ["Linux"]},
                {"name": "name", "values": ["gamma-recipe"]},
            ]}),
            vec![recipe_arn(REGION, "gamma-recipe", "1.0.0")],
        ),
        (
            "a filter value that is case-sensitive",
            json!({"filters": [{"name": "name", "values": ["BETA-RECIPE"]}]}),
            Vec::new(),
        ),
        (
            "the caller's own recipes",
            json!({"owner": "Self"}),
            expected.clone(),
        ),
        (
            "recipes Amazon owns, of which this account holds none",
            json!({"owner": "Amazon"}),
            Vec::new(),
        ),
    ];
    for (case, body, want) in filtered {
        let (status, listed) = list_recipes(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {listed}"
        );
        let arns = page_arns(case, &listed);
        assert_eq!(
            arns, want,
            "case {case:?}: expected {want:?}, got {arns:?}"
        );
    }

    // (case, the request, the error code the refusal carries): a filter naming a member
    // this operation does not filter on is a value the `filters` parameter does not admit,
    // which this service refuses as that parameter's value; a filter that is not a filter
    // at all — no name, no values — is a malformed request.
    let refusals: Vec<(&str, Value, &str)> = vec![
        (
            "an unsupported filter name",
            json!({"filters": [{"name": "owner", "values": ["Self"]}]}),
            "InvalidParameterValueException",
        ),
        (
            "a filter with no values",
            json!({"filters": [{"name": "name"}]}),
            "InvalidRequestException",
        ),
        (
            "a filter with no name",
            json!({"filters": [{"values": ["alpha-recipe"]}]}),
            "InvalidRequestException",
        ),
    ];
    for (case, body, code) in refusals {
        let (status, refused) = list_recipes(&router, &body).await;
        assert_refused(case, status, &refused, 400, code);
    }

    // The refusal names the parameter and the filters THIS operation allows — its own set,
    // not the component listing's.
    let (_, refused) = list_recipes(
        &router,
        &json!({"filters": [{"name": "owner", "values": ["Self"]}]}),
    )
    .await;
    let message = text("the unsupported-filter refusal", &refused, "message");
    assert_eq!(
        message,
        "The value supplied for parameter 'filters' is not valid. Allowed filters for \
         ListImageRecipes are: name, parentImage, platform.",
        "case \"the unsupported-filter refusal\": got {message:?}"
    );
}

#[tokio::test]
async fn a_delete_removes_the_record_and_respects_what_depends_on_it() {
    let router = emulator();
    let components = stored_components(&router).await;
    let linux_build = components
        .first()
        .cloned()
        .expect("the linux build component");

    // Two recipes: one naming the component by its build version ARN, one by a wildcard
    // version ARN that the same build version answers.
    let recipes: Vec<(&str, String, Value)> = vec![
        (
            "by build version ARN",
            recipe_arn(REGION, "exact-reference-recipe", "1.0.0"),
            recipe_body(
                "exact-reference-recipe",
                "1.0.0",
                "dltok-0001",
                json!({"components": [{"componentArn": linux_build.clone()}]}),
            ),
        ),
        (
            "by wildcard version ARN",
            recipe_arn(REGION, "wildcard-reference-recipe", "1.0.0"),
            recipe_body(
                "wildcard-reference-recipe",
                "1.0.0",
                "dltok-0002",
                json!({"components": [{"componentArn": format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/prep-linux/1.x.x")}]}),
            ),
        ),
    ];
    for (case, _, body) in &recipes {
        let (status, created) = create_recipe(&router, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the create, got {status} with {created}"
        );
    }

    // Each recipe in turn keeps the component alive.
    for (case, arn, _) in &recipes {
        let (status, refused) = delete_component(&router, &linux_build).await;
        assert_refused(case, status, &refused, 400, "ResourceDependencyException");
        let (status, deleted) = delete_recipe(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the recipe delete, got {status} with {deleted}"
        );
        assert_eq!(
            deleted.get("imageRecipeArn"),
            Some(&json!(arn)),
            "case {case:?}: expected the deleted ARN reported, got {deleted}"
        );
    }

    // With nothing depending on it, the component is deleted and reported.
    let (status, deleted) = delete_component(&router, &linux_build).await;
    assert_eq!(
        status, 200,
        "case the component delete: expected HTTP 200, got {status} with {deleted}"
    );
    assert_eq!(
        deleted.get("componentBuildVersionArn"),
        Some(&json!(linux_build)),
        "case the component delete: expected the deleted ARN reported, got {deleted}"
    );

    // Everything that was removed is gone, from the reads and from the listing, and a
    // second delete of either removes nothing.
    let first_recipe = recipes
        .first()
        .map(|(_, arn, _)| arn.clone())
        .unwrap_or_default();
    let (status, body) = get_component(&router, &linux_build).await;
    assert_refused(
        "the deleted component",
        status,
        &body,
        404,
        "ResourceNotFoundException",
    );
    let (status, body) = get_recipe(&router, &first_recipe).await;
    assert_refused(
        "the deleted recipe",
        status,
        &body,
        404,
        "ResourceNotFoundException",
    );
    let (status, body) = delete_component(&router, &linux_build).await;
    assert_refused(
        "a second component delete",
        status,
        &body,
        404,
        "ResourceNotFoundException",
    );
    let (status, body) = delete_recipe(&router, &first_recipe).await;
    assert_refused(
        "a second recipe delete",
        status,
        &body,
        404,
        "ResourceNotFoundException",
    );

    let (status, listed) = list_recipes(&router, &json!({})).await;
    assert_eq!(
        status, 200,
        "case the listing after the deletes: expected HTTP 200, got {status} with {listed}"
    );
    let remaining = member("the listing after the deletes", &listed, "imageRecipeSummaryList")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        remaining, 0,
        "case the listing after the deletes: expected no recipes, got {listed}"
    );

    // The components the deletes left alone are still there.
    let (status, read) = get_component(
        &router,
        components.get(1).map(String::as_str).unwrap_or_default(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case an untouched component: expected HTTP 200, got {status} with {read}"
    );
}
