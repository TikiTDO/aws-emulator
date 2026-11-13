// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteFunctionUrlConfig`: "Deletes a Lambda function URL. When
//! you delete a function URL, you can't recover it. Creating a new function URL results
//! in a different URL address."
//!
//! The rule under test: `DeleteFunctionUrlConfig` removes the ONE stored function URL of
//! the qualifier the request names — through any documented `FunctionName` form, with the
//! qualifier named in the name or in the `Qualifier` parameter — answering 204 with no
//! body; afterwards that function URL does not exist (a read of it, and a second delete
//! of it, are refused with `ResourceNotFoundException`), every other qualifier's function
//! URL is untouched, and a later create on the emptied qualifier mints a different URL
//! address. A qualifier that holds no function URL names nothing to delete and is refused
//! the same way, and a qualifier the modelled `FunctionUrlQualifier` pattern excludes is
//! refused with `ValidationException` without deleting anything.
//!
//! Every row drives real HTTP requests through the router and checks the store and the
//! `GetFunctionUrlConfig` reader afterwards, so a delete that answered 204 without
//! removing the record — or that removed more than the one it was asked for — fails here.
//! The rows differ along what the rule generalises over: which qualifier of the function
//! is deleted (its unpublished version, or one of two aliases), how that qualifier is
//! addressed (the bare name, a name qualified with `:`, the full ARN, the partial ARN, the
//! `function:` prefix form, and the `Qualifier` query parameter) and the configuration the
//! deleted URL carried.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here. Nothing in it is verified: the
/// harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The label the unpublished version is stored under.
const LATEST: &str = "$LATEST";

/// The message Lambda's function-URL operations refuse with when the function URL asked
/// about does not exist
/// (`state::RESOURCE_NOT_FOUND_MESSAGE`): `ResourceNotFoundException`'s own wording,
/// naming no resource.
const NOT_FOUND_MESSAGE: &str = "The resource you requested does not exist.";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// A `String` member of the JSON body.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.string("__type")
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The `message` of a wire error, however the protocol spells the member.
    fn message(&self) -> String {
        self.string("message")
            .or_else(|| self.string("Message"))
            .unwrap_or_default()
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The `/url` endpoint of the function `reference` names, with an optional `Qualifier`
/// query parameter.
fn url_uri(reference: &str, qualifier: Option<&str>) -> String {
    let query = qualifier.map_or_else(String::new, |value| format!("?Qualifier={value}"));
    format!("/2021-10-31/functions/{reference}/url{query}")
}

/// `CreateFunctionUrlConfig` on the qualifier `reference`/`qualifier` names.
async fn create_url_config(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
    body: &Value,
) -> Answer {
    call(router, "POST", &url_uri(reference, qualifier), body).await
}

/// `GetFunctionUrlConfig` on the qualifier `reference`/`qualifier` names.
async fn get_url_config(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> Answer {
    call(router, "GET", &url_uri(reference, qualifier), &Value::Null).await
}

/// `DeleteFunctionUrlConfig` on the qualifier `reference`/`qualifier` names.
async fn delete_url_config(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> Answer {
    call(
        router,
        "DELETE",
        &url_uri(reference, qualifier),
        &Value::Null,
    )
    .await
}

/// Create a function to attach function URLs to, publishing version 1 so an alias has a
/// version to invoke.
async fn create_function(router: &MultiServiceRouter, name: &str) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": true,
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Create an alias of `name` invoking version 1, so a function URL can be attached to it.
async fn create_alias(router: &MultiServiceRouter, name: &str, alias: &str) {
    let answer = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/aliases"),
        &json!({ "Name": alias, "FunctionVersion": "1" }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateAlias {alias:?} on {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// The function URL stored for each qualifier of `name`, as `(qualifier, url id)` pairs.
fn stored_urls(engine: &Arc<engine::InMemoryEngine>, name: &str) -> Vec<(String, String)> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(name)
        .map(|record| {
            record
                .url_configs
                .iter()
                .map(|(qualifier, config)| (qualifier.clone(), config.url_id.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the deletion table: which qualifier of the function is deleted, how the
/// request addresses it, and the configuration the deleted function URL carried.
struct DeleteCase {
    name: &'static str,
    function: &'static str,
    /// The qualifier whose function URL is deleted; `None` is the unpublished version.
    qualifier: Option<&'static str>,
    /// The configuration the deleted function URL was created with.
    request: Value,
    /// How the delete addresses the function, built from the function name.
    reference: fn(&str) -> String,
    /// The `Qualifier` query parameter the delete carries, if any.
    query_qualifier: Option<&'static str>,
}

/// The rows. Each differs in the qualifier it deletes, the documented `FunctionName`
/// form it addresses the function by, whether the qualifier is named in the name or in
/// the `Qualifier` parameter, and the configuration the URL carried. The name forms are
/// the ones `FunctionName` documents ("Function name", "Function ARN", "Partial ARN",
/// with the optional `function:` prefix and `:qualifier` suffix of the modelled
/// `FunctionUrlFunctionName` pattern).
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            name: "the unpublished version, addressed by the bare function name",
            function: "del-latest-bare",
            qualifier: None,
            request: json!({ "AuthType": "NONE" }),
            reference: |name| name.to_string(),
            query_qualifier: None,
        },
        DeleteCase {
            name: "the unpublished version, addressed by the full function arn",
            function: "del-latest-arn",
            qualifier: None,
            request: json!({ "AuthType": "AWS_IAM", "InvokeMode": "RESPONSE_STREAM" }),
            reference: |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
            query_qualifier: None,
        },
        DeleteCase {
            name: "the unpublished version, addressed by the partial arn",
            function: "del-latest-partial",
            qualifier: None,
            request: json!({
                "AuthType": "NONE",
                "Cors": { "AllowOrigins": ["https://www.example.com"], "MaxAge": 86_400 },
            }),
            reference: |name| format!("{ACCOUNT}:function:{name}"),
            query_qualifier: None,
        },
        DeleteCase {
            name: "an alias, addressed by the name qualified with a colon",
            function: "del-alias-colon",
            qualifier: Some("live"),
            request: json!({ "AuthType": "AWS_IAM" }),
            reference: |name| format!("{name}:live"),
            query_qualifier: None,
        },
        DeleteCase {
            name: "an alias, addressed by the Qualifier parameter",
            function: "del-alias-query",
            qualifier: Some("live"),
            request: json!({
                "AuthType": "NONE",
                "InvokeMode": "BUFFERED",
                "Cors": { "AllowMethods": ["*"], "AllowOrigins": ["*"] },
            }),
            reference: |name| name.to_string(),
            query_qualifier: Some("live"),
        },
        DeleteCase {
            name: "an alias, addressed by the function: prefix form with the qualifier",
            function: "del-alias-prefix",
            qualifier: Some("live"),
            request: json!({ "AuthType": "NONE" }),
            reference: |name| format!("function:{name}:live"),
            query_qualifier: None,
        },
    ]
}

#[tokio::test]
async fn deleting_a_function_url_removes_exactly_that_qualifiers_url() {
    for case in delete_cases() {
        let label = case.name;
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);

        create_function(&router, case.function).await;
        create_alias(&router, case.function, "live").await;
        create_alias(&router, case.function, "staging").await;
        for (qualifier, body) in [
            (None, case.request.clone()),
            (Some("live"), case.request.clone()),
            (Some("staging"), json!({ "AuthType": "NONE" })),
        ] {
            let created = create_url_config(&router, case.function, qualifier, &body).await;
            assert_eq!(
                created.status,
                201,
                "case {label:?}: CreateFunctionUrlConfig on {qualifier:?} must succeed, got {} {}",
                created.status,
                created.text()
            );
        }

        let target = case.qualifier.unwrap_or(LATEST).to_string();
        let before = stored_urls(&engine, case.function);
        assert_eq!(
            before.len(),
            3,
            "case {label:?}: the function must hold one URL per qualifier before the \
             delete, got {before:?}"
        );
        let deleted_id = before
            .iter()
            .find(|(qualifier, _)| *qualifier == target)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("case {label:?}: no stored URL on {target:?} in {before:?}"));

        let reference = (case.reference)(case.function);
        let answer = delete_url_config(&router, &reference, case.query_qualifier).await;
        assert_eq!(
            answer.status,
            204,
            "case {label:?}: expected the modelled 204 from DeleteFunctionUrlConfig, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.body.is_empty(),
            "case {label:?}: a 204 carries no body, got {:?}",
            answer.text()
        );

        // Exactly the one qualifier's URL is gone, and the other two are untouched —
        // same ids, still readable.
        let after = stored_urls(&engine, case.function);
        let expected: Vec<(String, String)> = before
            .iter()
            .filter(|(qualifier, _)| *qualifier != target)
            .cloned()
            .collect();
        assert_eq!(
            after, expected,
            "case {label:?}: expected only {target:?} removed, leaving {expected:?}, got {after:?}"
        );
        for (qualifier, id) in &expected {
            let survivor = get_url_config(
                &router,
                case.function,
                Some(qualifier).filter(|value| *value != LATEST).map(String::as_str),
            )
            .await;
            assert_eq!(
                survivor.status,
                200,
                "case {label:?}: the function URL of {qualifier:?} must survive, got {} {}",
                survivor.status,
                survivor.text()
            );
            let url = survivor.string("FunctionUrl").unwrap_or_default();
            assert!(
                url.contains(id.as_str()),
                "case {label:?}: {qualifier:?} must still report the url id {id:?}, got {url:?}"
            );
        }

        // "When you delete a function URL, you can't recover it": the read and a second
        // delete both report it does not exist.
        for (what, answer) in [
            (
                "GetFunctionUrlConfig",
                get_url_config(&router, &reference, case.query_qualifier).await,
            ),
            (
                "a second DeleteFunctionUrlConfig",
                delete_url_config(&router, &reference, case.query_qualifier).await,
            ),
        ] {
            assert_eq!(
                answer.status,
                404,
                "case {label:?}: expected 404 from {what} after the delete, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {label:?}: expected ResourceNotFoundException from {what}, got {}",
                answer.error_code()
            );
            assert_eq!(
                answer.message(),
                NOT_FOUND_MESSAGE,
                "case {label:?}: expected {NOT_FOUND_MESSAGE:?} from {what}, got {:?}",
                answer.message()
            );
        }

        // "Creating a new function URL results in a different URL address."
        let recreated =
            create_url_config(&router, &reference, case.query_qualifier, &case.request).await;
        assert_eq!(
            recreated.status,
            201,
            "case {label:?}: the qualifier must be free to take a new function URL, got {} {}",
            recreated.status,
            recreated.text()
        );
        let new_url = recreated.string("FunctionUrl").unwrap_or_default();
        assert!(
            !new_url.contains(deleted_id.as_str()),
            "case {label:?}: a new function URL must be a different address than the \
             deleted {deleted_id:?}, got {new_url:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_function_url_that_does_not_exist_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "has-no-url").await;
    create_function(&router, "has-a-url").await;
    create_alias(&router, "has-no-url", "live").await;
    let created = create_url_config(&router, "has-a-url", None, &json!({ "AuthType": "NONE" })).await;
    assert_eq!(
        created.status,
        201,
        "the untouched function URL must be created, got {} {}",
        created.status,
        created.text()
    );
    let untouched = stored_urls(&engine, "has-a-url");

    // Each row names a function URL that does not exist, either because the function
    // holds none on that qualifier (the function-URL wording, naming no resource) or
    // because the function or that qualifier of it does not exist at all (the
    // function-lifecycle wording, which names the function it could not find).
    for (label, reference, qualifier, holds_no_url) in [
        ("a function with no function URL", "has-no-url", None, true),
        (
            "an alias with no function URL",
            "has-no-url",
            Some("live"),
            true,
        ),
        (
            "an alias of a known function that does not exist",
            "has-no-url",
            Some("absent-alias"),
            false,
        ),
        ("an unknown function", "absent", None, false),
        ("an unknown function, qualified", "absent", Some("live"), false),
        (
            "an unknown function by arn",
            "arn:aws:lambda:us-east-1:000000000000:function:absent",
            None,
            false,
        ),
    ] {
        let answer = delete_url_config(&router, reference, qualifier).await;
        assert_eq!(
            answer.status,
            404,
            "case {label:?}: expected 404 from DeleteFunctionUrlConfig, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {}",
            answer.error_code()
        );
        if holds_no_url {
            assert_eq!(
                answer.message(),
                NOT_FOUND_MESSAGE,
                "case {label:?}: expected {NOT_FOUND_MESSAGE:?}, got {:?}",
                answer.message()
            );
        } else {
            assert!(
                answer.message().starts_with("Function not found:"),
                "case {label:?}: a function the request names but that does not exist is \
                 reported as such, got {:?}",
                answer.message()
            );
        }
    }

    // A refused delete removes nothing, here or on any other function.
    assert!(
        stored_urls(&engine, "has-no-url").is_empty(),
        "a refused delete must create nothing, got {:?}",
        stored_urls(&engine, "has-no-url")
    );
    assert_eq!(
        stored_urls(&engine, "has-a-url"),
        untouched,
        "a refused delete must leave every other function URL where it was, got {:?}",
        stored_urls(&engine, "has-a-url")
    );
}

/// A function URL belongs to the unpublished version of a function or to one of its
/// aliases, never to a published numeric version — which is what the model's
/// `FunctionUrlQualifier` pattern (`^((?!^\d+$)^[0-9a-zA-Z-_]+$)$`) and the qualifier
/// group of `FunctionUrlFunctionName` say by excluding an all-digit qualifier (and the
/// `$LATEST` spelling, whose `$` neither pattern admits). So a delete addressed by one is
/// refused for failing that member's modelled pattern rather than reported as a missing
/// resource.
#[tokio::test]
async fn a_function_url_is_not_deleted_on_a_published_version() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "versioned").await;
    let created = create_url_config(&router, "versioned", None, &json!({ "AuthType": "NONE" })).await;
    assert_eq!(
        created.status,
        201,
        "the function URL under test must be created, got {} {}",
        created.status,
        created.text()
    );
    let before = stored_urls(&engine, "versioned");

    for (label, reference, qualifier) in [
        ("a version in the function name", "versioned:1", None),
        ("a version in the Qualifier parameter", "versioned", Some("1")),
        (
            "a version in a qualified function arn",
            "arn:aws:lambda:us-east-1:000000000000:function:versioned:1",
            None,
        ),
        (
            "the unpublished version named explicitly",
            "versioned",
            Some("$LATEST"),
        ),
    ] {
        let answer = delete_url_config(&router, reference, qualifier).await;
        assert_eq!(
            answer.status,
            400,
            "case {label:?}: expected 400 for a qualifier the modelled pattern excludes, \
             got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains("ValidationException"),
            "case {label:?}: a value outside its member's modelled pattern is refused the \
             way the boundary walk refuses one, so expected ValidationException, got {}",
            answer.error_code()
        );
    }

    assert_eq!(
        stored_urls(&engine, "versioned"),
        before,
        "a refused delete must leave the function URL where it was, got {:?}",
        stored_urls(&engine, "versioned")
    );
}
