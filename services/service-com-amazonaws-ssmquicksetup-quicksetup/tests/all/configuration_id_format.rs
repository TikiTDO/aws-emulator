// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHICH STAGE refuses a configuration id a request carries,
//! and in which words.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handler — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: `GetConfiguration`'s `ConfigurationId` carries a value this
//! service GENERATED, so its form is judged as input before anything is looked up —
//! text outside the form this service mints names no configuration that could ever have
//! existed, which is a permanent fault in the request (HTTP 400 `ValidationException`,
//! the modelled `Message` carrying the fixed text `Configuration id ` … ` has invalid
//! format.` around the id the request sent), while an id IN that form which the store
//! does not hold is a fact about the store (the modelled HTTP 404
//! `ResourceNotFoundException`). The form itself is the one place this crate states it
//! (`crate::state::is_configuration_id`), so what the service hands out is exactly what
//! it takes back.
//!
//! The rows span what the form generalises over: the number of digits, the characters
//! used, the separators, and the case of the digits — including values the modelled
//! `@httpLabel` pattern `^[a-zA-Z0-9-_/:]{1,100}$` itself admits, which is what makes
//! the stage that refuses them observable.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    is_configuration_id, CONFIGURATION_ID_DIGITS,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in [`REGION`].
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
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

/// Status and parsed body of one call, with the `x-amzn-errortype` header folded into
/// the body under `__type` so one accessor reads either.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
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

/// The error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The message a refusal carried, under either casing this endpoint's two refusal layers
/// name it with.
fn error_message(body: &Value) -> String {
    ["Message", "message"]
        .iter()
        .filter_map(|key| body.get(*key))
        .filter_map(Value::as_str)
        .next()
        .unwrap_or("<no message>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an SDK
/// does: every byte outside RFC 3986's unreserved set travels as `%XX`.
fn label(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// `GET /getConfiguration/{ConfigurationId}`.
async fn get_configuration(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/getConfiguration/{}", label(id)),
        None,
    )
    .await
}

/// One manager with one definition deployed into one Region, so the store holds a
/// configuration and a refusal below is about the id that was sent rather than about an
/// empty store. Answers the id of that configuration.
async fn stored_configuration_id(router: &MultiServiceRouter) -> String {
    let type_ = "AWSQuickSetupType-SSMOpsCenter";
    let (status, answer) = call(
        router,
        "POST",
        "/configurationManager",
        Some(&json!({
            "Name": "id-format",
            "ConfigurationDefinitions": [{
                "Type": type_,
                "Parameters": super::type_parameter_fixtures::with_required_parameters(
                    type_,
                    json!({ "TargetRegions": REGION }),
                ),
            }],
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    let (status, listed) = call(router, "POST", "/listConfigurations", Some(&json!({}))).await;
    assert_eq!(
        status, 200,
        "ListConfigurations must succeed, got {status} / {listed:?}"
    );
    listed
        .get("ConfigurationsList")
        .and_then(Value::as_array)
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("Id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected one deployed configuration, got {listed:?}"))
        .to_string()
}

/// One row: the id a request carries, and whether it is in the form this service mints —
/// stated LITERALLY per row rather than computed, so the form itself is under test and not
/// only the handler's agreement with it.
struct Row {
    case: &'static str,
    id: String,
    in_form: bool,
}

impl Row {
    /// A row whose id is NOT in the service's id form.
    fn out_of_form(case: &'static str, id: impl Into<String>) -> Self {
        Self {
            case,
            id: id.into(),
            in_form: false,
        }
    }

    /// A row whose id IS in that form, so it reaches the lookup.
    fn in_form(case: &'static str, id: impl Into<String>) -> Self {
        Self {
            case,
            id: id.into(),
            in_form: true,
        }
    }
}

/// An id outside the service's id form is refused as input, with the invalid-format
/// sentence built from the id the request sent; an id in that form which the store does
/// not hold is the modelled 404. Both are asserted against a store that DOES hold a
/// configuration, so neither answer is about an empty store.
#[tokio::test]
async fn an_id_outside_the_services_id_form_is_refused_before_the_lookup() {
    let router = emulator();
    let minted = stored_configuration_id(&router).await;
    assert!(
        is_configuration_id(&minted),
        "the id the service handed out must be in the form it accepts back, got {minted:?}"
    );

    let rows = vec![
        // The `8-4-4-4-12` separated spelling of the same digit count: the modelled
        // label pattern admits it, and it is not the form.
        Row::out_of_form("a dashed uuid", "c68e30b1-b8b1-44a7-b92a-293f08358ef9"),
        Row::out_of_form("one digit short", "0".repeat(CONFIGURATION_ID_DIGITS - 1)),
        Row::out_of_form("one digit too many", "0".repeat(CONFIGURATION_ID_DIGITS + 1)),
        Row::out_of_form("the shortest value the label pattern admits", "a"),
        Row::out_of_form("the longest value the label pattern admits", "c".repeat(100)),
        Row::out_of_form(
            "every character class the label pattern admits",
            "Ab9-_/:",
        ),
        Row::out_of_form(
            // Hex digits only, but one character outside them.
            "a non-hex digit in the right number of characters",
            format!("{}g", "0".repeat(CONFIGURATION_ID_DIGITS - 1)),
        ),
        Row::out_of_form("the manager ARN's own resource form", "configuration/abc"),
        Row::in_form("digits no create minted", "0".repeat(CONFIGURATION_ID_DIGITS)),
        Row::in_form("every hex digit", "0123456789abcdef0123456789abcdef"),
        Row::in_form(
            // Hex is case-insensitive, so this is the form too.
            "the same digits in upper case",
            "0123456789ABCDEF0123456789ABCDEF",
        ),
        Row::in_form("the id the service minted", minted.clone()),
    ];

    for row in rows {
        let case = row.case;
        // The form itself, against the row's own statement of it: a form that drifted to
        // admit (or refuse) one of these rows fails here rather than being followed.
        assert_eq!(
            is_configuration_id(&row.id),
            row.in_form,
            "case {case:?}: expected is_configuration_id({:?}) to be {}",
            row.id,
            row.in_form
        );
        let (status, answer) = get_configuration(&router, &row.id).await;
        if row.id == minted {
            assert_eq!(
                status, 200,
                "case {case:?}: a stored configuration must be readable, got {status} / {answer:?}"
            );
            continue;
        }
        if row.in_form {
            assert_eq!(
                status, 404,
                "case {case:?}: an id of the service's own form that names nothing must be \
                 the modelled 404, got {status} / {answer:?}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {code:?} in {answer:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: an id outside the service's id form must be refused as input, \
             got {status} / {answer:?}"
        );
        let code = error_code(&answer);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected ValidationException, got {code:?} in {answer:?}"
        );
        let expected = format!("Configuration id {} has invalid format.", row.id);
        let actual = error_message(&answer);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        // The modelled `ValidationException` of this service carries `Message`: the
        // sentence is the service's own, not a restatement of a value constraint.
        assert!(
            answer.get("Message").is_some(),
            "case {case:?}: the modelled exception's own member must carry the sentence, \
             got {answer:?}"
        );
    }
}

/// The refusal is about the id's form alone: the same out-of-form id earns the same
/// answer whether or not the store holds anything, and reading one never changes what a
/// later caller sees.
#[tokio::test]
async fn the_invalid_format_refusal_is_independent_of_what_the_store_holds() {
    let empty = emulator();
    let id = "c68e30b1-b8b1-44a7-b92a-293f08358ef9";
    let (empty_status, empty_answer) = get_configuration(&empty, id).await;

    let populated = emulator();
    let minted = stored_configuration_id(&populated).await;
    let (status, answer) = get_configuration(&populated, id).await;
    assert_eq!(
        (status, error_message(&answer)),
        (empty_status, error_message(&empty_answer)),
        "the same out-of-form id must earn the same answer whatever the store holds, got \
         {status} / {answer:?} against {empty_status} / {empty_answer:?}"
    );

    // The refused read left the store exactly as it was.
    let (status, answer) = get_configuration(&populated, &minted).await;
    assert_eq!(
        status, 200,
        "the stored configuration must still be readable, got {status} / {answer:?}"
    );
}
