// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a function's URL: `CreateFunctionUrlConfig` creates "a
//! dedicated HTTP(S) endpoint that you can use to invoke your function", and
//! `GetFunctionUrlConfig` reports it.
//!
//! The rule under test: `CreateFunctionUrlConfig` creates, for the qualifier the request
//! names, the ONE function URL that qualifier may have — persisting exactly the
//! configuration the request described (`AuthType` as supplied, `Cors` as supplied and
//! nothing more, `InvokeMode` as supplied and UNSET when the request named none) — and
//! answers that configuration with the ARN of the function it is attached to and the
//! instant it was created; `GetFunctionUrlConfig` reports that same stored record, with
//! the mode the URL SERVES IN (the documented `BUFFERED` default while none is
//! configured), and a second create on a qualifier that already has a function URL is
//! refused.
//!
//! Every row drives real HTTP requests through the router and reads the configuration
//! back through `GetFunctionUrlConfig`, so a URL that was answered but never stored
//! fails here. The rows differ along what the rule generalises over: the auth type, the
//! invoke mode (absent, and each modelled value), how much of the CORS structure is
//! configured (none, one member, every member), the `MaxAge` the CORS settings carry
//! (each published bound) and the documented `FunctionName` form the request addresses
//! the function by.

use std::collections::BTreeSet;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs and the
/// function URL carry. Nothing in it is verified: the harness turns request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `InvokeMode` the model documents as the default: "BUFFERED – This is the default
/// option."
const DEFAULT_INVOKE_MODE: &str = "BUFFERED";

/// The bounds the model puts on `FunctionUrl` (`@length(min: 40, max: 100)`).
const FUNCTION_URL_MIN_LEN: usize = 40;
const FUNCTION_URL_MAX_LEN: usize = 100;

/// The bounds the model puts on `Cors$MaxAge` (`@range(min: 0, max: 86400)`).
const MAX_AGE_MIN: i64 = 0;
const MAX_AGE_MAX: i64 = 86_400;

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

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// `CreateFunctionUrlConfig` on the function `reference` names, with an optional
/// `Qualifier` query parameter.
async fn create_url_config_qualified(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
    body: &Value,
) -> Answer {
    let query = qualifier.map_or_else(String::new, |value| format!("?Qualifier={value}"));
    call(
        router,
        "POST",
        &format!("/2021-10-31/functions/{reference}/url{query}"),
        body,
    )
    .await
}

/// `CreateFunctionUrlConfig` on the function `reference` names.
async fn create_url_config(router: &MultiServiceRouter, reference: &str, body: &Value) -> Answer {
    create_url_config_qualified(router, reference, None, body).await
}

/// `GetFunctionUrlConfig` on the function `reference` names, with an optional
/// `Qualifier` query parameter.
async fn get_url_config(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> Answer {
    let query = qualifier.map_or_else(String::new, |value| format!("?Qualifier={value}"));
    call(
        router,
        "GET",
        &format!("/2021-10-31/functions/{reference}/url{query}"),
        &Value::Null,
    )
    .await
}

/// Create a function to attach a URL to, publishing version 1 when asked.
async fn create_function_publishing(router: &MultiServiceRouter, name: &str, publish: bool) {
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
            "Publish": publish,
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

/// Create a function to attach a URL to.
async fn create_function(router: &MultiServiceRouter, name: &str) {
    create_function_publishing(router, name, false).await;
}

/// Assert the endpoint is a function URL of the documented form:
/// `https://<url-id>.lambda-url.<region>.on.aws/`, with an opaque lowercase
/// alphanumeric id, inside the modelled length bounds. Answers the id.
fn url_id_of(case: &str, url: &str) -> String {
    assert!(
        url.len() >= FUNCTION_URL_MIN_LEN && url.len() <= FUNCTION_URL_MAX_LEN,
        "case {case:?}: FunctionUrl must satisfy the modelled {FUNCTION_URL_MIN_LEN}..\
         {FUNCTION_URL_MAX_LEN} length, got {} in {url:?}",
        url.len()
    );
    let suffix = format!(".lambda-url.{REGION}.on.aws/");
    let id = url
        .strip_prefix("https://")
        .and_then(|rest| rest.strip_suffix(&suffix))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected https://<url-id>{suffix}, got {url:?}")
        });
    assert!(
        !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
        "case {case:?}: the url id must be a non-empty lowercase alphanumeric label, got {id:?}"
    );
    id.to_string()
}

/// One row of the function-URL table: the function, the reference the request addresses
/// it by, the request body, and the configuration the function URL must then hold.
struct UrlCase {
    name: &'static str,
    function: &'static str,
    /// The documented `FunctionName` form the request uses; `None` means the bare name.
    reference: Option<String>,
    request: Value,
    expected_auth_type: &'static str,
    /// The `InvokeMode` the CREATE must answer: what the request configured, and nothing
    /// at all when it configured none.
    expected_created_invoke_mode: Value,
    /// The `InvokeMode` a READ of the configuration must report: the mode the URL serves
    /// in, which is the documented default while none is configured.
    expected_invoke_mode: &'static str,
    expected_cors: Value,
}

/// The rows. Each differs from the others in the auth type, the invoke mode, how much of
/// the CORS structure it configures, the `MaxAge` it carries or the `FunctionName` form
/// it is addressed by — the dimensions the rule generalises over. The `AuthType` values
/// are the whole `FunctionUrlAuthType` enum (`NONE | AWS_IAM`) and the invoke modes the
/// whole `InvokeMode` enum (`BUFFERED | RESPONSE_STREAM`) plus its absence, both as
/// `smithy_query(query="shape", shape="FunctionUrlAuthType,InvokeMode")` lists them.
fn url_cases() -> Vec<UrlCase> {
    vec![
        // A public endpoint with no CORS settings and no invoke mode: the response must
        // carry the documented BUFFERED default and no CORS block at all.
        UrlCase {
            name: "a public url with nothing configured beyond its auth type",
            function: "public-url",
            reference: None,
            request: json!({ "AuthType": "NONE" }),
            expected_auth_type: "NONE",
            expected_created_invoke_mode: Value::Null,
            expected_invoke_mode: DEFAULT_INVOKE_MODE,
            expected_cors: Value::Null,
        },
        // An IAM-authenticated endpoint, addressed by its full ARN, streaming its
        // response, with a single CORS member set: only that member comes back.
        UrlCase {
            name: "an iam url addressed by its function arn, streaming, one cors member",
            function: "iam-url",
            reference: Some(format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:iam-url"
            )),
            request: json!({
                "AuthType": "AWS_IAM",
                "InvokeMode": "RESPONSE_STREAM",
                "Cors": { "AllowOrigins": ["https://www.example.com"] },
            }),
            expected_auth_type: "AWS_IAM",
            expected_created_invoke_mode: json!("RESPONSE_STREAM"),
            expected_invoke_mode: "RESPONSE_STREAM",
            expected_cors: json!({ "AllowOrigins": ["https://www.example.com"] }),
        },
        // Every CORS member set, with `MaxAge` at the top of the modelled range, and the
        // function addressed by its partial ARN.
        UrlCase {
            name: "every cors member, max age at the published maximum, partial arn",
            function: "full-cors-url",
            reference: Some(format!("{ACCOUNT}:function:full-cors-url")),
            request: json!({
                "AuthType": "NONE",
                "InvokeMode": "BUFFERED",
                "Cors": {
                    "AllowCredentials": true,
                    "AllowHeaders": ["Date", "Keep-Alive", "X-Custom-Header"],
                    "AllowMethods": ["GET", "POST", "DELETE"],
                    "AllowOrigins": ["https://www.example.com", "http://localhost:60905"],
                    "ExposeHeaders": ["Date", "X-Custom-Header"],
                    "MaxAge": MAX_AGE_MAX,
                },
            }),
            expected_auth_type: "NONE",
            expected_created_invoke_mode: json!("BUFFERED"),
            expected_invoke_mode: "BUFFERED",
            expected_cors: json!({
                "AllowCredentials": true,
                "AllowHeaders": ["Date", "Keep-Alive", "X-Custom-Header"],
                "AllowMethods": ["GET", "POST", "DELETE"],
                "AllowOrigins": ["https://www.example.com", "http://localhost:60905"],
                "ExposeHeaders": ["Date", "X-Custom-Header"],
                "MaxAge": MAX_AGE_MAX,
            }),
        },
        // `MaxAge` at the bottom of the modelled range ("By default, this is set to 0,
        // which means that the browser doesn't cache results"), wildcard CORS, and the
        // `function:` prefix form of the name.
        UrlCase {
            name: "wildcard cors with max age at the published minimum",
            function: "wildcard-cors-url",
            reference: Some("function:wildcard-cors-url".to_string()),
            request: json!({
                "AuthType": "AWS_IAM",
                "Cors": {
                    "AllowCredentials": false,
                    "AllowMethods": ["*"],
                    "AllowOrigins": ["*"],
                    "MaxAge": MAX_AGE_MIN,
                },
            }),
            expected_auth_type: "AWS_IAM",
            expected_created_invoke_mode: Value::Null,
            expected_invoke_mode: DEFAULT_INVOKE_MODE,
            expected_cors: json!({
                "AllowCredentials": false,
                "AllowMethods": ["*"],
                "AllowOrigins": ["*"],
                "MaxAge": MAX_AGE_MIN,
            }),
        },
        // Empty CORS lists are a configuration in their own right (`@length(min: 0)` on
        // each list), and are reported as configured rather than dropped.
        UrlCase {
            name: "cors configured with empty lists",
            function: "empty-cors-url",
            reference: None,
            request: json!({
                "AuthType": "NONE",
                "Cors": { "AllowHeaders": [], "AllowOrigins": [] },
            }),
            expected_auth_type: "NONE",
            expected_created_invoke_mode: Value::Null,
            expected_invoke_mode: DEFAULT_INVOKE_MODE,
            expected_cors: json!({ "AllowHeaders": [], "AllowOrigins": [] }),
        },
    ]
}

/// Drop the JSON members that are absent, so a row states only the CORS members it
/// configured and a member the emulator invented shows up as a difference.
fn without_nulls(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(name, member)| (name.clone(), without_nulls(member)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The configuration a `CreateFunctionUrlConfig` or `GetFunctionUrlConfig` answer
/// reports, as the members this rule is about.
fn reported(answer: &Answer) -> (Value, Value, Value, Value) {
    let body = answer.json();
    (
        body["FunctionUrl"].clone(),
        body["FunctionArn"].clone(),
        body["AuthType"].clone(),
        body["InvokeMode"].clone(),
    )
}

#[tokio::test]
async fn a_function_url_is_created_from_the_request_and_read_back() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let mut minted_ids: BTreeSet<String> = BTreeSet::new();

    for case in url_cases() {
        let label = case.name;
        create_function(&router, case.function).await;
        let reference = case
            .reference
            .clone()
            .unwrap_or_else(|| case.function.to_string());

        let created = create_url_config(&router, &reference, &case.request).await;
        assert_eq!(
            created.status,
            201,
            "case {label:?}: expected 201 from CreateFunctionUrlConfig, got {} {}",
            created.status,
            created.text()
        );

        let (url, arn, auth_type, invoke_mode) = reported(&created);
        let url = url
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no FunctionUrl in {}", created.text()))
            .to_string();
        let id = url_id_of(label, &url);
        assert!(
            minted_ids.insert(id.clone()),
            "case {label:?}: every function URL gets its own id, but {id:?} was minted twice"
        );
        assert_eq!(
            arn,
            json!(function_arn(case.function)),
            "case {label:?}: expected the ARN of the function the URL is attached to, got {arn:?}"
        );
        assert_eq!(
            auth_type,
            json!(case.expected_auth_type),
            "case {label:?}: expected AuthType {:?}, got {auth_type:?}",
            case.expected_auth_type
        );
        assert_eq!(
            invoke_mode,
            case.expected_created_invoke_mode,
            "case {label:?}: the create must answer the InvokeMode it configured ({:?}), \
             got {invoke_mode:?}",
            case.expected_created_invoke_mode
        );
        let created_cors = created.json()["Cors"].clone();
        assert_eq!(
            without_nulls(&created_cors),
            without_nulls(&case.expected_cors),
            "case {label:?}: expected the configured CORS settings {:?}, got {created_cors:?}",
            case.expected_cors
        );
        let creation_time = created.string("CreationTime").unwrap_or_default();
        assert!(
            creation_time.contains('T') && creation_time.ends_with("+0000"),
            "case {label:?}: CreationTime must be an ISO-8601 instant with the numeric UTC \
             offset this service renders (the form the model's own instant example, \
             `2018-11-27T15:10:45.123+0000`, spells), got {creation_time:?}"
        );

        // Read the configuration back: a URL that was answered but never persisted
        // fails here.
        let fetched = get_url_config(&router, case.function, None).await;
        assert_eq!(
            fetched.status,
            200,
            "case {label:?}: expected 200 from GetFunctionUrlConfig, got {} {}",
            fetched.status,
            fetched.text()
        );
        assert_eq!(
            reported(&fetched),
            (
                json!(url),
                arn,
                auth_type,
                json!(case.expected_invoke_mode)
            ),
            "case {label:?}: GetFunctionUrlConfig must report the created function URL and \
             the mode it serves in, got {}",
            fetched.text()
        );
        let fetched_cors = fetched.json()["Cors"].clone();
        assert_eq!(
            without_nulls(&fetched_cors),
            without_nulls(&case.expected_cors),
            "case {label:?}: expected the stored CORS settings {:?}, got {fetched_cors:?}",
            case.expected_cors
        );
        assert_eq!(
            fetched.string("CreationTime").as_deref(),
            Some(creation_time.as_str()),
            "case {label:?}: expected the stored CreationTime {creation_time:?}, got {:?}",
            fetched.string("CreationTime")
        );
        assert_eq!(
            fetched.string("LastModifiedTime").as_deref(),
            Some(creation_time.as_str()),
            "case {label:?}: nothing has changed the configuration, so LastModifiedTime \
             must still be the creation instant {creation_time:?}, got {:?}",
            fetched.string("LastModifiedTime")
        );

        // A read changes nothing a later caller sees: the same URL comes back again.
        let again = get_url_config(&router, case.function, None).await;
        assert_eq!(
            again.string("FunctionUrl").as_deref(),
            Some(url.as_str()),
            "case {label:?}: the function URL must be stable across reads, got {:?}",
            again.string("FunctionUrl")
        );

        // The store itself holds exactly one function URL, on the unpublished version,
        // with the id the wire reported.
        let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
        let store = state.lock();
        let record = store
            .functions
            .get(case.function)
            .unwrap_or_else(|| panic!("case {label:?}: the function must be stored"));
        let stored: Vec<(String, String)> = record
            .url_configs
            .iter()
            .map(|(qualifier, config)| (qualifier.clone(), config.url_id.clone()))
            .collect();
        assert_eq!(
            stored,
            vec![("$LATEST".to_string(), id.clone())],
            "case {label:?}: expected one stored function URL on $LATEST with id {id:?}, \
             got {stored:?}"
        );
    }
}

#[tokio::test]
async fn a_qualifier_has_at_most_one_function_url() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "single-url").await;

    let first = create_url_config(&router, "single-url", &json!({ "AuthType": "NONE" })).await;
    assert_eq!(
        first.status,
        201,
        "the first create must succeed, got {} {}",
        first.status,
        first.text()
    );
    let url = first.string("FunctionUrl").unwrap_or_default();

    // A second create on the same qualifier conflicts; it neither replaces the URL nor
    // changes its configuration.
    for (label, body) in [
        ("the same configuration", json!({ "AuthType": "NONE" })),
        (
            "a different configuration",
            json!({ "AuthType": "AWS_IAM", "InvokeMode": "RESPONSE_STREAM" }),
        ),
    ] {
        let second = create_url_config(&router, "single-url", &body).await;
        assert_eq!(
            second.status,
            409,
            "case {label:?}: expected 409 for a function URL that already exists, got {} {}",
            second.status,
            second.text()
        );
        assert!(
            second.error_code().contains("ResourceConflictException"),
            "case {label:?}: expected ResourceConflictException, got {}",
            second.error_code()
        );
    }

    let fetched = get_url_config(&router, "single-url", None).await;
    assert_eq!(
        (
            fetched.string("FunctionUrl"),
            fetched.string("AuthType"),
            fetched.string("InvokeMode")
        ),
        (
            Some(url.clone()),
            Some("NONE".to_string()),
            Some(DEFAULT_INVOKE_MODE.to_string())
        ),
        "a refused create must leave the first function URL exactly as it was, got {}",
        fetched.text()
    );
}

#[tokio::test]
async fn a_function_url_that_was_never_created_is_not_found() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "has-no-url").await;

    // Each row names a resource that does not exist: a function with no URL, an unknown
    // function, and a qualifier of a known function that does not exist (no alias has
    // been created).
    for (label, reference, qualifier) in [
        ("a function with no function URL", "has-no-url", None),
        ("an unknown function", "absent", None),
        ("an alias that does not exist", "has-no-url", Some("live")),
        ("an unknown function, qualified", "absent", Some("live")),
    ] {
        let fetched = get_url_config(&router, reference, qualifier).await;
        assert_eq!(
            fetched.status,
            404,
            "case {label:?}: expected 404 from GetFunctionUrlConfig, got {} {}",
            fetched.status,
            fetched.text()
        );
        assert!(
            fetched.error_code().contains("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {}",
            fetched.error_code()
        );
    }

    // The same holds for a create against a function, or a qualifier of one, that does
    // not exist.
    for (label, reference) in [
        ("an unknown function", "absent".to_string()),
        (
            "an alias of a known function that does not exist",
            "has-no-url:live".to_string(),
        ),
        (
            "an unknown function by arn",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:absent"),
        ),
    ] {
        let created = create_url_config(&router, &reference, &json!({ "AuthType": "NONE" })).await;
        assert_eq!(
            created.status,
            404,
            "case {label:?}: expected 404 from CreateFunctionUrlConfig, got {} {}",
            created.status,
            created.text()
        );
        assert!(
            created.error_code().contains("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {}",
            created.error_code()
        );
    }

    // Nothing was persisted for any of them.
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    let urls: usize = store
        .functions
        .values()
        .map(|record| record.url_configs.len())
        .sum();
    assert_eq!(
        urls, 0,
        "a refused create must persist no function URL, got {urls} stored"
    );
}

#[tokio::test]
async fn a_configuration_outside_the_modelled_constraints_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "constrained").await;

    // One past each published bound, and a value outside each modelled enum. The bounds
    // themselves are exercised by the table above.
    for (label, body) in [
        (
            "MaxAge one past the published maximum",
            json!({ "AuthType": "NONE", "Cors": { "MaxAge": MAX_AGE_MAX + 1 } }),
        ),
        (
            "MaxAge one below the published minimum",
            json!({ "AuthType": "NONE", "Cors": { "MaxAge": MAX_AGE_MIN - 1 } }),
        ),
        (
            "an auth type the FunctionUrlAuthType enum does not have",
            json!({ "AuthType": "IAM" }),
        ),
        (
            "an invoke mode the InvokeMode enum does not have",
            json!({ "AuthType": "NONE", "InvokeMode": "STREAMING" }),
        ),
        ("no auth type at all", json!({})),
    ] {
        let answer = create_url_config(&router, "constrained", &body).await;
        assert_eq!(
            answer.status,
            400,
            "case {label:?}: expected 400 for a request outside the modelled constraints, \
             got {} {}",
            answer.status,
            answer.text()
        );
    }

    let fetched = get_url_config(&router, "constrained", None).await;
    assert_eq!(
        fetched.status,
        404,
        "a refused create must persist no function URL, got {} {}",
        fetched.status,
        fetched.text()
    );
}

/// A function URL belongs to the unpublished version of a function or to one of its
/// aliases, never to a published numeric version — which is what the model's
/// `FunctionUrlQualifier` pattern (`^((?!^\d+$)^[0-9a-zA-Z-_]+$)$`) and the qualifier
/// group of `FunctionUrlFunctionName` say by excluding an all-digit qualifier (and the
/// `$LATEST` spelling, whose `$` neither pattern admits).
#[tokio::test]
async fn a_function_url_is_not_created_on_a_published_version() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Published, so the version a row names really exists and the refusal is about the
    // qualifier rather than about a missing version.
    create_function_publishing(&router, "versioned", true).await;
    let published = call(
        &router,
        "GET",
        "/2015-03-31/functions/versioned?Qualifier=1",
        &Value::Null,
    )
    .await;
    assert_eq!(
        published.status,
        200,
        "the version a row qualifies by must exist, got {} {}",
        published.status,
        published.text()
    );

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
        let answer = create_url_config_qualified(
            &router,
            reference,
            qualifier,
            &json!({ "AuthType": "NONE" }),
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

        // A read of the same qualifier is refused for the same reason, rather than
        // reporting that a version simply has no function URL.
        let fetched = get_url_config(&router, reference, qualifier).await;
        assert_eq!(
            fetched.status,
            400,
            "case {label:?}: expected 400 from GetFunctionUrlConfig too, got {} {}",
            fetched.status,
            fetched.text()
        );
    }

    // Nothing was persisted for any of them, so the function still has no function URL.
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    let urls: usize = store
        .functions
        .values()
        .map(|record| record.url_configs.len())
        .sum();
    assert_eq!(
        urls, 0,
        "a refused create must persist no function URL, got {urls} stored"
    );
}
