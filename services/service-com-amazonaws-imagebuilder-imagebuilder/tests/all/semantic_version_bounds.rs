// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the bound a semantic version's assignable nodes carry.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, this service's front door and the handlers behind it — so what is asserted
//! is what a signed AWS SDK request observes.
//!
//! The rule under test: `semanticVersion`'s three assignable nodes each take "any positive
//! integer value, including zero. The upper limit is 2^30-1, or 1073741823, for each node" —
//! a bound no `@pattern` can state, so a value that satisfies the modelled pattern and
//! misses the bound is a value of THAT parameter this service cannot accept. It is refused
//! the way this service refuses every such value: HTTP 400
//! `InvalidParameterValueException`, naming the member and stating the format the value had
//! to follow, and — on a member whose own modelled pattern admits the `x` wildcard
//! (`CreateImageRecipeRequest.semanticVersion`) — the wildcard rule as well. A version
//! inside the bound is stored and reads back carrying exactly the nodes it was created
//! with.
//!
//! The rows span what the rule generalises over: which node carries the out-of-bound value,
//! how far past the bound it is (one past it, far past it, past what a 64-bit integer holds
//! at all), the wildcard node the recipe's pattern admits for FILTERING rather than
//! assignment, and which operation the member arrived on — with the bound itself and both
//! ends of the three-node syntax among the accepted rows, since a test that only refuses
//! would pass against a service that refuses everything.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here is signed for; the ARNs it builds carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The error this service refuses a parameter value with
/// (`smithy_query(query="shape",shape="InvalidParameterValueException")`: a client error,
/// `@httpError` 400).
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The largest value an assigned node may carry: "The upper limit is 2^30-1, or 1073741823,
/// for each node" (`semanticVersion`).
const MAX_NODE: u64 = 1_073_741_823;

/// A component document with one build phase, so a `CreateComponent` row is answered for its
/// version rather than for its document.
const BUILD_DOCUMENT: &str = concat!(
    "name: Probe\n",
    "schemaVersion: 1.0\n",
    "phases:\n",
    "  - name: build\n",
    "    steps:\n",
    "      - name: run\n",
    "        action: ExecuteBash\n",
    "        inputs:\n",
    "          commands:\n",
    "            - echo hi\n",
);

/// The parent image every recipe row builds from: an AMI id, which the member admits.
const PARENT_IMAGE: &str = "ami-0123456789abcdef0";

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

/// Status and parsed body of one call, signed for [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{REGION}/imagebuilder/\
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
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The error shape a refusal names, reduced to its shape name.
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The sentence a refusal states.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A `CreateComponent` body carrying `version`.
fn component_body(name: &str, version: &str, token: &str) -> Value {
    json!({
        "name": name,
        "semanticVersion": version,
        "platform": "Linux",
        "clientToken": token,
        "data": BUILD_DOCUMENT,
    })
}

/// A `CreateImageRecipe` body carrying `version`.
///
/// `components` is left out: the member's list carries `@length(1..)`, and a recipe created
/// without any is what this suite is about — the version, not the components.
fn recipe_body(name: &str, version: &str, token: &str) -> Value {
    json!({
        "name": name,
        "semanticVersion": version,
        "parentImage": PARENT_IMAGE,
        "clientToken": token,
    })
}

/// Which create a row drives, and everything that differs between the two: the request it
/// sends, the member of the response that names what it minted, the read that reports the
/// stored resource, and the sentence a refused version earns — the recipe's version member
/// admits the `x` wildcard, so its requirement states the wildcard rule too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Create {
    Component,
    ImageRecipe,
}

impl Create {
    /// The request line this create is driven through.
    fn uri(self) -> &'static str {
        match self {
            Self::Component => "/CreateComponent",
            Self::ImageRecipe => "/CreateImageRecipe",
        }
    }

    /// The request body for `name` at `version`.
    fn body(self, name: &str, version: &str, token: &str) -> Value {
        match self {
            Self::Component => component_body(name, version, token),
            Self::ImageRecipe => recipe_body(name, version, token),
        }
    }

    /// The requirement a refused version states, which is the whole of the refusal's
    /// sentence after the parameter it names.
    fn requirement(self) -> &'static str {
        match self {
            Self::Component => "The supplied semantic version does not follow the required format.",
            Self::ImageRecipe => {
                "The supplied semantic version does not follow the required format. Only one 'x' \
                 wildcard is allowed per version."
            }
        }
    }

    /// The ARN the stored resource is read back through, for `name` at `version`.
    fn arn(self, name: &str, version: &str) -> String {
        match self {
            // A component ARN carries the build number Image Builder assigns as its fourth
            // node; the first build of a name+version pair is build 1.
            Self::Component => format!(
                "arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/{name}/{version}/1"
            ),
            Self::ImageRecipe => {
                format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image-recipe/{name}/{version}")
            }
        }
    }

    /// The read that reports the stored resource, and the member of its answer that carries
    /// the stored version.
    fn read(self, name: &str, version: &str) -> (String, &'static str) {
        match self {
            Self::Component => (
                format!(
                    "/GetComponent?componentBuildVersionArn={}",
                    self.arn(name, version)
                ),
                "component",
            ),
            Self::ImageRecipe => (
                format!("/GetImageRecipe?imageRecipeArn={}", self.arn(name, version)),
                "imageRecipe",
            ),
        }
    }
}

/// Every version row: the case, the value, and whether this service can accept it.
fn version_rows() -> Vec<(&'static str, String, bool)> {
    vec![
        ("the smallest assignable version", "0.0.0".to_string(), true),
        ("an ordinary version", "1.2.3".to_string(), true),
        (
            "every node at the documented bound",
            format!("{MAX_NODE}.{MAX_NODE}.{MAX_NODE}"),
            true,
        ),
        (
            "the major node one past the bound",
            format!("{}.0.0", MAX_NODE + 1),
            false,
        ),
        (
            "the minor node one past the bound",
            format!("0.{}.0", MAX_NODE + 1),
            false,
        ),
        (
            "the patch node one past the bound",
            format!("0.0.{}", MAX_NODE + 1),
            false,
        ),
        (
            "a node far past the bound",
            "9999999999.0.0".to_string(),
            false,
        ),
        (
            "a node past what a 64-bit integer holds at all",
            "99999999999999999999.0.0".to_string(),
            false,
        ),
        (
            "two nodes past the bound at once",
            format!("{}.{}.0", MAX_NODE + 1, MAX_NODE + 7),
            false,
        ),
    ]
}

/// A name no other row uses, derived from the row's index so each create stands alone.
fn resource_name(create: Create, row: usize) -> String {
    let kind = match create {
        Create::Component => "component",
        Create::ImageRecipe => "recipe",
    };
    format!("version-{kind}-{row}")
}

#[tokio::test]
async fn a_semantic_version_outside_the_documented_bound_is_refused_as_that_parameters_value() {
    let router = emulator();

    for create in [Create::Component, Create::ImageRecipe] {
        for (row, (case, version, acceptable)) in version_rows().into_iter().enumerate() {
            let case = format!("{create:?}: {case}");
            let name = resource_name(create, row);
            let token = format!("vtok-{create:?}-{row}");
            let (status, answered) = call(
                &router,
                "PUT",
                create.uri(),
                Some(&create.body(&name, &version, &token)),
            )
            .await;
            if !acceptable {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400 for {version:?}, got {status} with \
                     {answered}"
                );
                let code = error_code(&case, &answered);
                assert_eq!(
                    code, INVALID_PARAMETER_VALUE,
                    "case {case:?}: expected the error code {INVALID_PARAMETER_VALUE:?}, got \
                     {code:?}"
                );
                let expected = format!(
                    "The value supplied for parameter 'semanticVersion' is not valid. {}",
                    create.requirement()
                );
                let actual = error_message(&answered);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected the message {expected:?}, got {actual:?}"
                );
                // Nothing was minted: the ARN the create would have carried names nothing.
                let (read_uri, _) = create.read(&name, &version);
                let (status, missing) = call(&router, "GET", &read_uri, None).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: a refused create must store nothing, got {status} with \
                     {missing}"
                );
                continue;
            }
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for {version:?}, got {status} with {answered}"
            );
            // Round trip: the stored resource reports the very nodes the create assigned.
            let (read_uri, member) = create.read(&name, &version);
            let (status, read) = call(&router, "GET", &read_uri, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the created resource must read back, got {status} with {read}"
            );
            let reported = read[member]["version"].as_str().unwrap_or_default();
            assert_eq!(
                reported, version,
                "case {case:?}: expected the stored version {version:?}, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_wildcard_node_is_for_filtering_not_for_assignment() {
    let router = emulator();

    // The recipe's own version pattern admits `x` in any node
    // (`smithy_query(query="shape",shape="WildcardVersionNumber")`), which is a FILTERING
    // pattern: a create assigns three integers, so every one of these is refused as the
    // parameter value it is — with the same sentence, whichever node carries the wildcard
    // and however many do.
    let rows = [
        ("one trailing wildcard", "1.0.x"),
        ("two trailing wildcards", "1.x.x"),
        ("every node a wildcard", "x.x.x"),
        ("a wildcard in the leading node", "x.0.0"),
    ];
    for (row, (case, version)) in rows.into_iter().enumerate() {
        let name = format!("wildcard-recipe-{row}");
        let token = format!("wtok-{row}");
        let (status, answered) = call(
            &router,
            "PUT",
            "/CreateImageRecipe",
            Some(&recipe_body(&name, version, &token)),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {version:?}, got {status} with {answered}"
        );
        let code = error_code(case, &answered);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {case:?}: expected the error code {INVALID_PARAMETER_VALUE:?}, got {code:?}"
        );
        let expected = format!(
            "The value supplied for parameter 'semanticVersion' is not valid. {}",
            Create::ImageRecipe.requirement()
        );
        let actual = error_message(&answered);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn the_same_client_token_with_different_parameters_states_the_mismatch_the_same_way() {
    let router = emulator();

    // "If you retry a request with the same client token, Image Builder returns the original
    // response without running the operation again", and the same token carrying different
    // creation parameters is the modelled mismatch. The sentence it is stated in is FIXED
    // text — it names neither the token nor the parameters that differ — so every create of
    // this service states it identically, whichever member the retry changed.
    const EXPECTED: &str =
        "The request parameters do not match those in the original idempotent request.";

    // (case, the create, the original body, the body a retry carries under the same token)
    let rows: Vec<(&str, &str, Value, Value)> = vec![
        (
            "a component retried with another description",
            "/CreateComponent",
            {
                let mut body = component_body("mismatch-component", "1.0.0", "mtok-0001");
                body["description"] = json!("original description");
                body
            },
            {
                let mut body = component_body("mismatch-component", "1.0.0", "mtok-0001");
                body["description"] = json!("a totally different description");
                body
            },
        ),
        (
            "a component retried with another version",
            "/CreateComponent",
            component_body("mismatch-component-2", "1.0.0", "mtok-0002"),
            component_body("mismatch-component-2", "2.0.0", "mtok-0002"),
        ),
        (
            "a recipe retried with another parent image",
            "/CreateImageRecipe",
            recipe_body("mismatch-recipe", "1.0.0", "mtok-0003"),
            {
                let mut body = recipe_body("mismatch-recipe", "1.0.0", "mtok-0003");
                body["parentImage"] = json!("ami-00000000000000000");
                body
            },
        ),
    ];

    for (case, uri, original, retried) in rows {
        let (status, answered) = call(&router, "PUT", uri, Some(&original)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the original create must succeed, got {status} with {answered}"
        );
        let (status, refused) = call(&router, "PUT", uri, Some(&retried)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        let code = error_code(case, &refused);
        assert_eq!(
            code, "IdempotentParameterMismatchException",
            "case {case:?}: expected IdempotentParameterMismatchException, got {code:?}"
        );
        let actual = error_message(&refused);
        assert_eq!(
            actual, EXPECTED,
            "case {case:?}: expected the message {EXPECTED:?}, got {actual:?}"
        );
    }
}
