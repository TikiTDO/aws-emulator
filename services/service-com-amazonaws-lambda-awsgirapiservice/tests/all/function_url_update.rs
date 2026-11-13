// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateFunctionUrlConfig`: "Updates the configuration for a Lambda
//! function URL."
//!
//! The rule under test: an update of the function URL configuration a qualifier holds
//! replaces exactly the members the request states, leaves every member it omits at the
//! value the store holds — the URL ADDRESS included, which only a create ever mints — and
//! stamps `LastModifiedTime` while `CreationTime` keeps the instant the create stamped. A
//! qualifier that holds no configuration, including one whose configuration has been
//! deleted, is refused with the modelled `ResourceNotFoundException`, so a client can tell
//! a configuration that is gone from an API that is unavailable.
//!
//! Every row drives real HTTP requests through the router and reads the configuration back
//! through `GetFunctionUrlConfig`, so an update that answered 200 without persisting the
//! change fails here. The rows differ along what the rule generalises over: WHICH members
//! the update states (the auth type alone, the invoke mode alone, the CORS block alone, all
//! of them, and none of them), which qualifier of the function carries the configuration
//! (its unpublished version and each of two aliases), and the documented `FunctionName`
//! form the request addresses it by.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
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

/// The message the function-URL operations refuse with when the configuration asked about
/// does not exist (`state::FUNCTION_URL_NOT_FOUND_MESSAGE`).
const NOT_FOUND_MESSAGE: &str = "The resource you requested does not exist.";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
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

    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

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

    fn message(&self) -> String {
        self.string("message")
            .or_else(|| self.string("Message"))
            .unwrap_or_default()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
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

/// The `/url` endpoint of the function `reference` names, with an optional `Qualifier`.
fn url_uri(reference: &str, qualifier: Option<&str>) -> String {
    let query = qualifier.map_or_else(String::new, |value| format!("?Qualifier={value}"));
    format!("/2021-10-31/functions/{reference}/url{query}")
}

/// Create a function with one published version, so an alias has a version to invoke.
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

/// Create an alias of `name` invoking version 1.
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

/// The stored function URL configuration of `qualifier`, as `(url id, auth type, the
/// invoke mode it SERVES IN)` — the last being the mode configured on it, or the
/// documented default while none is.
fn stored_config(
    engine: &Arc<engine::InMemoryEngine>,
    function: &str,
    qualifier: &str,
) -> Option<(String, String, String)> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(function)
        .and_then(|record| record.url_configs.get(qualifier))
        .map(|config| {
            (
                config.url_id.clone(),
                config.auth_type.clone(),
                config.served_invoke_mode(),
            )
        })
}

/// One update row: which qualifier carries the configuration, what the create stated, what
/// the update states, and how the update addresses the function.
struct UpdateCase {
    name: &'static str,
    function: &'static str,
    /// The qualifier whose configuration is updated; `None` is the unpublished version.
    qualifier: Option<&'static str>,
    create: Value,
    update: Value,
    /// How the update addresses the function, built from the function name.
    reference: fn(&str) -> String,
    /// The `Qualifier` query parameter the update carries, if any.
    query_qualifier: Option<&'static str>,
    /// What the configuration must report afterwards: `(AuthType, the InvokeMode it
    /// serves in, Cors)`.
    expected: (&'static str, &'static str, Value),
    /// The `InvokeMode` the UPDATE itself must answer: the mode configured on the
    /// configuration once this update has been applied, and nothing at all while no
    /// caller has configured one.
    answered_invoke_mode: Value,
}

/// The rows. Each states a different subset of the updatable members over a different
/// starting configuration, on a different qualifier, addressed through a different
/// documented `FunctionName` form.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            name: "the auth type alone, on the unpublished version",
            function: "uu-auth-only",
            qualifier: None,
            create: json!({ "AuthType": "NONE" }),
            update: json!({ "AuthType": "AWS_IAM" }),
            reference: |name| name.to_string(),
            query_qualifier: None,
            expected: ("AWS_IAM", "BUFFERED", Value::Null),
            answered_invoke_mode: Value::Null,
        },
        UpdateCase {
            name: "the invoke mode alone, on an alias addressed by the colon form",
            function: "uu-mode-only",
            qualifier: Some("live"),
            create: json!({ "AuthType": "AWS_IAM" }),
            update: json!({ "InvokeMode": "RESPONSE_STREAM" }),
            reference: |name| format!("{name}:live"),
            query_qualifier: None,
            expected: ("AWS_IAM", "RESPONSE_STREAM", Value::Null),
            answered_invoke_mode: json!("RESPONSE_STREAM"),
        },
        UpdateCase {
            name: "the cors block alone, on the unpublished version by full arn",
            function: "uu-cors-only",
            qualifier: None,
            create: json!({ "AuthType": "NONE" }),
            update: json!({
                "Cors": { "AllowOrigins": ["https://www.example.com"], "MaxAge": 600 },
            }),
            reference: |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
            query_qualifier: None,
            expected: (
                "NONE",
                "BUFFERED",
                json!({ "AllowOrigins": ["https://www.example.com"], "MaxAge": 600 }),
            ),
            answered_invoke_mode: Value::Null,
        },
        UpdateCase {
            name: "every updatable member at once, on an alias by the Qualifier parameter",
            function: "uu-all-members",
            qualifier: Some("staging"),
            create: json!({
                "AuthType": "NONE",
                "InvokeMode": "BUFFERED",
                "Cors": { "AllowMethods": ["GET"], "AllowOrigins": ["https://old.example.com"] },
            }),
            update: json!({
                "AuthType": "AWS_IAM",
                "InvokeMode": "RESPONSE_STREAM",
                "Cors": { "AllowMethods": ["*"], "AllowOrigins": ["*"] },
            }),
            reference: |name| name.to_string(),
            query_qualifier: Some("staging"),
            expected: (
                "AWS_IAM",
                "RESPONSE_STREAM",
                json!({ "AllowMethods": ["*"], "AllowOrigins": ["*"] }),
            ),
            answered_invoke_mode: json!("RESPONSE_STREAM"),
        },
        UpdateCase {
            name: "no updatable member at all, which keeps every stored value",
            function: "uu-no-members",
            qualifier: None,
            create: json!({
                "AuthType": "AWS_IAM",
                "InvokeMode": "RESPONSE_STREAM",
                "Cors": { "AllowOrigins": ["https://kept.example.com"] },
            }),
            update: json!({}),
            reference: |name| format!("{ACCOUNT}:function:{name}"),
            query_qualifier: None,
            expected: (
                "AWS_IAM",
                "RESPONSE_STREAM",
                json!({ "AllowOrigins": ["https://kept.example.com"] }),
            ),
            answered_invoke_mode: json!("RESPONSE_STREAM"),
        },
    ]
}

/// The configuration's `(AuthType, InvokeMode, Cors)` as an answer reports them, with
/// the invoke mode as the raw JSON member so a value the emulator invented for an unset
/// one shows up as a difference.
fn reported(answer: &Answer) -> (String, Value, Value) {
    let body = answer.json();
    (
        answer.string("AuthType").unwrap_or_default(),
        body.get("InvokeMode").cloned().unwrap_or(Value::Null),
        body.get("Cors").cloned().unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn an_update_replaces_the_members_it_states_and_keeps_the_rest() {
    for case in update_cases() {
        let label = case.name;
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        create_function(&router, case.function).await;
        create_alias(&router, case.function, "live").await;
        create_alias(&router, case.function, "staging").await;

        let created = call(
            &router,
            "POST",
            &url_uri(case.function, case.qualifier),
            &case.create,
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {label:?}: the configuration under test must be created, got {} {}",
            created.status,
            created.text()
        );
        let url = created
            .string("FunctionUrl")
            .unwrap_or_else(|| panic!("case {label:?}: the create answers a FunctionUrl"));
        let creation_time = created
            .string("CreationTime")
            .unwrap_or_else(|| panic!("case {label:?}: the create answers a CreationTime"));

        // A configuration on another qualifier that no update names: it must be untouched.
        let other_qualifier = if case.qualifier == Some("live") {
            "staging"
        } else {
            "live"
        };
        let bystander = call(
            &router,
            "POST",
            &url_uri(case.function, Some(other_qualifier)),
            &json!({ "AuthType": "NONE" }),
        )
        .await;
        assert_eq!(
            bystander.status,
            201,
            "case {label:?}: the bystander configuration must be created, got {} {}",
            bystander.status,
            bystander.text()
        );

        let reference = (case.reference)(case.function);
        let updated = call(
            &router,
            "PUT",
            &url_uri(&reference, case.query_qualifier),
            &case.update,
        )
        .await;
        assert_eq!(
            updated.status,
            200,
            "case {label:?}: expected the modelled 200 from UpdateFunctionUrlConfig, got {} {}",
            updated.status,
            updated.text()
        );

        // The write answers what it persisted: the invoke mode is the one configured on
        // the configuration, and absent while no caller has configured one.
        let answered = (
            case.expected.0.to_string(),
            case.answered_invoke_mode.clone(),
            case.expected.2.clone(),
        );
        assert_eq!(
            reported(&updated),
            answered,
            "case {label:?}: expected the update to answer {answered:?}, got {}",
            updated.text()
        );
        // A read reports the mode the URL SERVES IN, which is the documented default
        // while none is configured.
        let expected = (
            case.expected.0.to_string(),
            json!(case.expected.1),
            case.expected.2.clone(),
        );
        assert_eq!(
            (
                updated.string("FunctionUrl"),
                updated.string("CreationTime")
            ),
            (Some(url.clone()), Some(creation_time.clone())),
            "case {label:?}: an update keeps the URL address and the creation time, got {}",
            updated.text()
        );
        assert!(
            updated.string("LastModifiedTime").is_some(),
            "case {label:?}: the answer carries the required LastModifiedTime, got {}",
            updated.text()
        );
        assert_eq!(
            updated.string("FunctionArn"),
            Some(format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:{}{}",
                case.function,
                case.qualifier
                    .map(|qualifier| format!(":{qualifier}"))
                    .unwrap_or_default()
            )),
            "case {label:?}: the answer names the qualifier it updated, got {}",
            updated.text()
        );

        // Read back: the update persisted what it answered.
        let read = call(
            &router,
            "GET",
            &url_uri(case.function, case.qualifier),
            &Value::Null,
        )
        .await;
        assert_eq!(
            (read.status, reported(&read), read.string("FunctionUrl")),
            (200, expected, Some(url)),
            "case {label:?}: GetFunctionUrlConfig must report what the update stored, got {} {}",
            read.status,
            read.text()
        );

        let untouched = stored_config(&engine, case.function, other_qualifier);
        assert_eq!(
            untouched.as_ref().map(|held| (held.1.clone(), held.2.clone())),
            Some(("NONE".to_string(), "BUFFERED".to_string())),
            "case {label:?}: every other qualifier's configuration must be untouched, got \
             {untouched:?}"
        );
    }
}

#[tokio::test]
async fn an_update_of_a_configuration_that_does_not_exist_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "uu-refusals").await;
    create_alias(&router, "uu-refusals", "live").await;

    // The configuration under test is created and then DELETED, which is the case a
    // client cannot otherwise tell from an unavailable API.
    let created = call(
        &router,
        "POST",
        &url_uri("uu-refusals", None),
        &json!({ "AuthType": "NONE" }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the configuration under test must be created, got {} {}",
        created.status,
        created.text()
    );
    let deleted = call(
        &router,
        "DELETE",
        &url_uri("uu-refusals", None),
        &Value::Null,
    )
    .await;
    assert_eq!(
        deleted.status,
        204,
        "the configuration must be deletable, got {} {}",
        deleted.status,
        deleted.text()
    );

    // Each row names a configuration that does not exist: one that was deleted, one on a
    // qualifier that never had one, and one on a function or qualifier that does not
    // exist at all. The first two are the function-URL wording (naming no resource), the
    // rest the function-lifecycle wording (naming the function it could not find).
    for (label, reference, qualifier, holds_no_config) in [
        ("a configuration that was deleted", "uu-refusals", None, true),
        (
            "an alias that never had one",
            "uu-refusals",
            Some("live"),
            true,
        ),
        ("a function that does not exist", "uu-absent", None, false),
        (
            "an alias of a known function that does not exist",
            "uu-refusals",
            Some("absent-alias"),
            false,
        ),
    ] {
        let answer = call(
            &router,
            "PUT",
            &url_uri(reference, qualifier),
            &json!({ "AuthType": "AWS_IAM" }),
        )
        .await;
        assert_eq!(
            answer.status,
            404,
            "case {label:?}: expected 404 from UpdateFunctionUrlConfig, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {}",
            answer.error_code()
        );
        if holds_no_config {
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

    // A published version is not a qualifier a function URL can be attached to, so an
    // update addressed by one is refused for failing that member's modelled pattern —
    // the same reading the create and the delete apply.
    for (label, reference, qualifier) in [
        ("a version in the function name", "uu-refusals:1", None),
        ("a version in the Qualifier parameter", "uu-refusals", Some("1")),
        (
            "the unpublished version named explicitly",
            "uu-refusals",
            Some(LATEST),
        ),
    ] {
        let answer = call(
            &router,
            "PUT",
            &url_uri(reference, qualifier),
            &json!({ "AuthType": "AWS_IAM" }),
        )
        .await;
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

    assert!(
        stored_config(&engine, "uu-refusals", LATEST).is_none(),
        "a refused update must create nothing, got {:?}",
        stored_config(&engine, "uu-refusals", LATEST)
    );
}
