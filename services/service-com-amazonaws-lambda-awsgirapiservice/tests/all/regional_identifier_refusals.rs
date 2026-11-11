// SPDX-License-Identifier: CC0-1.0

//! An identifier that names ANOTHER Region's resource, and the refusals AWS words for it.
//!
//! Three rules are under test, each over a table of inputs the recorded case is one row of.
//!
//! 1. **A Lambda function is a REGIONAL resource, so a `FunctionName` whose ARN carries a
//!    Region other than the endpoint's names a function that endpoint cannot reach**: every
//!    operation taking the member refuses it with the modelled `ResourceNotFoundException`
//!    (404) and the sentence naming the two Regions that disagree — `Functions from
//!    '<the Region the identifier named>' are not reachable in this region ('<the Region the
//!    request was made in>')` — with no resource named, because the Region is settled before
//!    anything is looked up. The rows span the operation (a read of the configuration, a
//!    read of the policy, a configuration write, a read of the function, a listing), the
//!    Region the identifier names (one that holds a function of the same name, one that
//!    holds none), whether the identifier is qualified, and the endpoint the call is made
//!    at — two of them, so neither Region of a pair is the one the rule is read off. The
//!    write is checked to have changed nothing: the local function's description is read
//!    back afterwards.
//! 2. **A layer is a REGIONAL resource too, so a `LayerName` whose ARN carries a Region
//!    other than the endpoint's is not the name of a layer there at all**: every operation
//!    taking the member refuses it with the modelled `InvalidParameterValueException` (400)
//!    and `Invalid Layer name: <the value the request sent>`. The rows span the operation
//!    (the version's read, the version listing, the delete, the publish, the policy read,
//!    the policy write), the Region the ARN names and the endpoint the call is made at, with
//!    the bare name and this Region's own ARN as the controls — and the local version is
//!    read back after the refused delete, so a refusal that had already removed it fails
//!    here.
//! 3. **`ListFunctions` refuses a `MasterRegion` given without `FunctionVersion=ALL` naming
//!    the COMBINATION it was given**: `The combination MasterArn=<MasterRegion> and
//!    FunctionVersion=<FunctionVersion> is not supported. Please see our documentation for
//!    valid combinations of ListFunctions options.`, with a parameter the request omitted
//!    named as the absent value it is. The rows span the `MasterRegion` value (`ALL`, three
//!    Regions) with the control that the same `MasterRegion` IS accepted alongside
//!    `FunctionVersion=ALL`.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{ACTIVATION_DELAY, UPDATE_DELAY};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `PublishLayerVersion` is bound.
const LAYERS_URI: &str = "/2018-10-31/layers";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The label the unpublished version is stored under.
const LATEST: &str = "$LATEST";

/// The function each endpoint below holds, named after that endpoint's own Region: the
/// identifier rows then name a function this endpoint DOES hold, through another Region's
/// ARN, so a refusal cannot be about the name being unknown.
fn function_name(endpoint: &str) -> String {
    format!("region-rule-probe-{endpoint}")
}

/// The one layer name every endpoint below holds a version of, for the same reason.
const LAYER: &str = "region-rule-layer";

/// The description a function is created with, which a refused write must leave in place.
const DESCRIPTION: &str = "as created";

/// The two endpoints every row of rules 1 and 2 is driven at. Each holds its own function
/// and its own layer, so the Region named by an identifier is the only thing that differs.
const ENDPOINTS: &[&str] = &["us-east-1", "eu-north-1"];

/// Regions an identifier can name, from the `MasterRegion` member's own pattern
/// (`^ALL|(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}$`): the two endpoints above
/// (so a row names a Region that DOES hold a resource of this name) and two Regions this
/// emulator holds nothing in at all.
const REGIONS: &[&str] = &["us-east-1", "eu-north-1", "ap-southeast-2", "us-gov-west-1"];

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The response body as JSON, or `Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The response body as text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The wire error code (`__type`) of an error response.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map_or_else(|| self.text(), str::to_string)
    }

    /// The class of the fault the body names, which every refusal of this service carries.
    fn fault_class(&self) -> String {
        self.json()
            .get("Type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The message of an error response, under either spelling the model's error shapes
    /// give the member.
    fn message(&self) -> String {
        let body = self.json();
        body.get("Message")
            .or_else(|| body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request to `region`'s endpoint, signed for that Region's scope.
/// Nothing in the signature is verified: the harness turns request authentication off, and
/// the scope is read only for the Region the caller addressed.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: &Value,
) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let host = format!("lambda.{region}.amazonaws.com");
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{region}/lambda/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    );
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://{host}{uri}"))
        .header("host", host)
        .header("authorization", authorization)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// Deployment-package or layer-archive bytes of a given length.
fn archive_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// A `ZipFile` member: restJson1 carries a blob base64-encoded in a JSON body.
fn zip_file(bytes: &[u8]) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// The unqualified ARN of the function `name` in `region`.
fn function_arn(region: &str, name: &str) -> String {
    format!("arn:aws:lambda:{region}:{ACCOUNT}:function:{name}")
}

/// The unqualified ARN of the layer `LAYER` in `region`, as `LayerArn` spells it.
fn layer_arn(region: &str) -> String {
    format!("arn:aws:lambda:{region}:{ACCOUNT}:layer:{LAYER}")
}

/// The refusal a `FunctionName` naming `named`'s function gets at `endpoint`.
fn not_reachable(named: &str, endpoint: &str) -> String {
    format!("Functions from '{named}' are not reachable in this region ('{endpoint}')")
}

/// `CreateFunction` in `region`, asserting it was accepted.
async fn create_function_in(router: &MultiServiceRouter, region: &str) {
    let answer = call_in(
        router,
        region,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": function_name(region),
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Description": DESCRIPTION,
            "Code": { "ZipFile": zip_file(&archive_bytes(64)) },
        }),
    )
    .await;
    assert!(
        answer.status < 400,
        "case {region}: the function must be created, got {} {}",
        answer.status,
        answer.text()
    );
}

/// `PublishLayerVersion` in `region`, asserting it was accepted.
async fn publish_layer_in(router: &MultiServiceRouter, region: &str) {
    let answer = call_in(
        router,
        region,
        "POST",
        &format!("{LAYERS_URI}/{LAYER}/versions"),
        &json!({ "Content": { "ZipFile": zip_file(&archive_bytes(96)) } }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {region}: the layer version must be published, got {} {}",
        answer.status,
        answer.text()
    );
}

/// The `Description` `GetFunctionConfiguration` reports for the function of `region`.
async fn described_as(router: &MultiServiceRouter, region: &str) -> String {
    let answer = call_in(
        router,
        region,
        "GET",
        &format!("{FUNCTIONS_URI}/{}/configuration", function_name(region)),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {region}: the configuration read must be served, got {} {}",
        answer.status,
        answer.text()
    );
    answer
        .json()
        .get("Description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One operation taking a `FunctionName`: how it is addressed and the body it takes.
struct FunctionCall {
    operation: &'static str,
    method: &'static str,
    suffix: &'static str,
    body: Option<Value>,
    /// Whether the row is also driven with the identifier qualified at the unpublished
    /// version: the member admits a qualifier, and the Region is settled before it.
    qualified_too: bool,
}

/// RULE 1: a `FunctionName` naming another Region's function is not reachable at this
/// endpoint, whatever the operation and whichever pair of Regions it is.
#[tokio::test]
async fn a_function_identifier_of_another_region_is_refused_naming_both_regions() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for endpoint in ENDPOINTS {
        create_function_in(&router, endpoint).await;
    }
    // "During this time, you can't invoke or modify the function": the move to `Active` is
    // owed by the clock, and this waits past the instant each create made it due, so the
    // write rows below are answered about the Region rather than about provisioning.
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;

    let calls = vec![
        FunctionCall {
            operation: "GetFunctionConfiguration",
            method: "GET",
            suffix: "/configuration",
            body: None,
            qualified_too: true,
        },
        FunctionCall {
            operation: "GetPolicy",
            method: "GET",
            suffix: "/policy",
            body: None,
            qualified_too: true,
        },
        FunctionCall {
            operation: "GetFunction",
            method: "GET",
            suffix: "",
            body: None,
            qualified_too: true,
        },
        FunctionCall {
            operation: "ListVersionsByFunction",
            method: "GET",
            suffix: "/versions",
            body: None,
            qualified_too: false,
        },
        FunctionCall {
            operation: "UpdateFunctionConfiguration",
            method: "PUT",
            suffix: "/configuration",
            body: Some(json!({ "Description": "written across Regions" })),
            qualified_too: false,
        },
    ];

    for endpoint in ENDPOINTS {
        let held = function_name(endpoint);
        for named in REGIONS.iter().filter(|named| *named != endpoint) {
            for call in &calls {
                let qualifiers: &[&str] = if call.qualified_too {
                    &["", LATEST]
                } else {
                    &[""]
                };
                for qualifier in qualifiers {
                    let suffix = if qualifier.is_empty() {
                        String::new()
                    } else {
                        format!(":{qualifier}")
                    };
                    let reference = format!("{}{suffix}", function_arn(named, &held));
                    let case = format!(
                        "{} at {endpoint} naming {named}{}",
                        call.operation,
                        if qualifier.is_empty() {
                            String::new()
                        } else {
                            format!(" qualified at {qualifier}")
                        }
                    );
                    let answer = call_in(
                        &router,
                        endpoint,
                        call.method,
                        &format!("{FUNCTIONS_URI}/{reference}{}", call.suffix),
                        call.body.as_ref().unwrap_or(&Value::Null),
                    )
                    .await;
                    assert_eq!(
                        answer.status,
                        404,
                        "case {case:?}: expected the modelled 404, got {} {}",
                        answer.status,
                        answer.text()
                    );
                    assert!(
                        answer.error_code().contains("ResourceNotFoundException"),
                        "case {case:?}: expected ResourceNotFoundException, got {:?}",
                        answer.error_code()
                    );
                    assert_eq!(
                        answer.fault_class(),
                        "User",
                        "case {case:?}: expected the caller's own fault class, got {:?}",
                        answer.fault_class()
                    );
                    let expected = not_reachable(named, endpoint);
                    assert_eq!(
                        answer.message(),
                        expected,
                        "case {case:?}: expected {expected:?}, got {:?}",
                        answer.message()
                    );
                }
            }
        }
    }

    // The CONTROL, and the proof that every refusal above was about the Region: at each
    // endpoint the SAME function is reachable by its bare name, by its own Region's ARN and
    // by the partial ARN, which names no Region at all — and the refused writes left the
    // description each function was created with in place.
    for endpoint in ENDPOINTS {
        let held = function_name(endpoint);
        for (case, reference) in [
            ("the bare name", held.clone()),
            ("this Region's own ARN", function_arn(endpoint, &held)),
            (
                "the partial ARN, which names no Region",
                format!("{ACCOUNT}:function:{held}"),
            ),
        ] {
            let answer = call_in(
                &router,
                endpoint,
                "GET",
                &format!("{FUNCTIONS_URI}/{reference}/configuration"),
                &Value::Null,
            )
            .await;
            assert_eq!(
                answer.status,
                200,
                "case {case} at {endpoint}: expected the read to be served, got {} {}",
                answer.status,
                answer.text()
            );
        }
        let described = described_as(&router, endpoint).await;
        assert_eq!(
            described, DESCRIPTION,
            "case {endpoint}: a refused cross-Region write must change nothing, got \
             {described:?}"
        );
    }
}

/// One operation taking a `LayerName`: how it is addressed and the body it takes.
struct LayerCall {
    operation: &'static str,
    method: &'static str,
    /// What follows the `{LayerName}` label, with `{version}` standing for the version
    /// number the row addresses.
    suffix: &'static str,
    body: Option<Value>,
}

/// RULE 2: a `LayerName` given as an ARN of another Region is not a layer name at this
/// endpoint, whatever the operation and whichever pair of Regions it is.
#[tokio::test]
async fn a_layer_name_of_another_region_is_refused_as_an_invalid_layer_name() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for endpoint in ENDPOINTS {
        publish_layer_in(&router, endpoint).await;
    }

    let calls = vec![
        LayerCall {
            operation: "GetLayerVersion",
            method: "GET",
            suffix: "/versions/1",
            body: None,
        },
        LayerCall {
            operation: "ListLayerVersions",
            method: "GET",
            suffix: "/versions",
            body: None,
        },
        LayerCall {
            operation: "DeleteLayerVersion",
            method: "DELETE",
            suffix: "/versions/1",
            body: None,
        },
        LayerCall {
            operation: "PublishLayerVersion",
            method: "POST",
            suffix: "/versions",
            body: Some(json!({ "Content": { "ZipFile": zip_file(&archive_bytes(32)) } })),
        },
        LayerCall {
            operation: "GetLayerVersionPolicy",
            method: "GET",
            suffix: "/versions/1/policy",
            body: None,
        },
        LayerCall {
            operation: "AddLayerVersionPermission",
            method: "POST",
            suffix: "/versions/1/policy",
            body: Some(json!({
                "StatementId": "share",
                "Action": "lambda:GetLayerVersion",
                "Principal": "*",
            })),
        },
    ];

    for endpoint in ENDPOINTS {
        for named in REGIONS.iter().filter(|named| *named != endpoint) {
            for call in &calls {
                let reference = layer_arn(named);
                let case = format!("{} at {endpoint} naming {named}", call.operation);
                let answer = call_in(
                    &router,
                    endpoint,
                    call.method,
                    &format!("{LAYERS_URI}/{reference}{}", call.suffix),
                    call.body.as_ref().unwrap_or(&Value::Null),
                )
                .await;
                assert_eq!(
                    answer.status,
                    400,
                    "case {case:?}: expected the modelled 400, got {} {}",
                    answer.status,
                    answer.text()
                );
                assert!(
                    answer.error_code().contains("InvalidParameterValueException"),
                    "case {case:?}: expected InvalidParameterValueException, got {:?}",
                    answer.error_code()
                );
                assert_eq!(
                    answer.fault_class(),
                    "User",
                    "case {case:?}: expected the caller's own fault class, got {:?}",
                    answer.fault_class()
                );
                let expected = format!("Invalid Layer name: {reference}");
                assert_eq!(
                    answer.message(),
                    expected,
                    "case {case:?}: expected {expected:?}, got {:?}",
                    answer.message()
                );
            }
        }
    }

    // The CONTROLS: at each endpoint the layer version is read by its bare name and by this
    // Region's own ARN — so the rows above refuse the Region the ARN names rather than the
    // ARN form — and it is STILL there, so the refused deletes removed nothing.
    for endpoint in ENDPOINTS {
        for (case, reference) in [
            ("the bare name", LAYER.to_string()),
            ("this Region's own ARN", layer_arn(endpoint)),
        ] {
            let answer = call_in(
                &router,
                endpoint,
                "GET",
                &format!("{LAYERS_URI}/{reference}/versions/1"),
                &Value::Null,
            )
            .await;
            assert_eq!(
                answer.status,
                200,
                "case {case} at {endpoint}: expected the read to be served, got {} {}",
                answer.status,
                answer.text()
            );
            let arn = answer
                .json()
                .get("LayerVersionArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let expected = format!("{}:1", layer_arn(endpoint));
            assert_eq!(
                arn, expected,
                "case {case} at {endpoint}: expected the version of this Region, got {arn:?}"
            );
        }
    }
}

/// RULE 3: `ListFunctions` refuses a `MasterRegion` given without `FunctionVersion=ALL`
/// naming the combination it was given.
#[tokio::test]
async fn the_list_functions_combination_refusal_names_both_parameters() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let endpoint = ENDPOINTS[0];
    create_function_in(&router, endpoint).await;
    tokio::time::sleep(UPDATE_DELAY + SLACK).await;

    // `FunctionVersion` is an enum whose only member is `ALL`, so the values the refusal can
    // carry are `ALL` — which is never refused — and the absent one, which is what every row
    // here leaves the parameter at. The `MasterRegion` half spans the member's own pattern:
    // `ALL` and three Regions.
    for master in ["ALL", "us-west-2", "eu-north-1", "ap-southeast-2"] {
        let answer = call_in(
            &router,
            endpoint,
            "GET",
            &format!("{FUNCTIONS_URI}?MasterRegion={master}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case MasterRegion={master}: expected the modelled 400, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains("InvalidParameterValueException"),
            "case MasterRegion={master}: expected InvalidParameterValueException, got {:?}",
            answer.error_code()
        );
        let expected = format!(
            "The combination MasterArn={master} and FunctionVersion=NULL is not supported. \
             Please see our documentation for valid combinations of ListFunctions options."
        );
        assert_eq!(
            answer.message(),
            expected,
            "case MasterRegion={master}: expected {expected:?}, got {:?}",
            answer.message()
        );

        // The CONTROL: the same `MasterRegion` alongside `FunctionVersion=ALL` is the
        // supported combination, so it is served.
        let served = call_in(
            &router,
            endpoint,
            "GET",
            &format!("{FUNCTIONS_URI}?MasterRegion={master}&FunctionVersion=ALL"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            served.status,
            200,
            "case MasterRegion={master} with FunctionVersion=ALL: expected the listing to be \
             served, got {} {}",
            served.status,
            served.text()
        );
    }
}
