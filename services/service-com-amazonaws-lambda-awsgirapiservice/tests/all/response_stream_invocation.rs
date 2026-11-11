// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `InvokeWithResponseStream`: `POST
//! /2021-11-15/functions/{FunctionName}/response-streaming-invocations`, whose output binds
//! the `@streaming` union `InvokeWithResponseStreamResponseEvent` as its `@httpPayload`.
//!
//! The rule under test: the route is SERVED, so every request to it is answered from this
//! service's own store rather than reported as an unimplemented route — the
//! `FunctionName`/`Qualifier` pair is resolved the way every other operation on this
//! service resolves one, a pair naming no function or no such version is the modelled
//! `ResourceNotFoundException` (404), a function still being provisioned is the modelled
//! `ResourceConflictException` (409), and `InvocationType=DryRun` ("Validate parameter
//! values and verify that the user or role has permission to invoke the function") answers
//! the status the model's own `StatusCode` documentation states for it, carrying the version
//! the invocation resolved to. Running the deployment package is the one path this emulator
//! cannot serve, and it says so rather than inventing a chunk sequence.
//!
//! Every row drives real HTTP requests through the router. The rows differ along what the
//! rule generalises over: the documented `FunctionName` form the request addresses the
//! function by (the bare name, the full ARN, the partial ARN, the `function:` prefix form,
//! a name qualified with `:`), whether the version is named in the name or in the
//! `Qualifier` parameter, which qualifier it is (the unpublished version, a published
//! version, an alias), and whether the pair resolves at all.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
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

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(100);

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

    /// The `X-Amz-Executed-Version` response header the output's own `@httpHeader` binding
    /// names.
    fn executed_version(&self) -> Option<String> {
        self.headers
            .get("x-amz-executed-version")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
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

/// Dispatch one request, with the headers `headers` names.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// `InvokeWithResponseStream` on the function `reference` names.
async fn invoke_with_response_stream(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
    invocation_type: Option<&str>,
    payload: &[u8],
) -> Answer {
    let query = qualifier.map_or_else(String::new, |value| format!("?Qualifier={value}"));
    let uri =
        format!("/2021-11-15/functions/{reference}/response-streaming-invocations{query}");
    let headers: Vec<(&str, &str)> = invocation_type
        .map(|value| vec![("x-amz-invocation-type", value)])
        .unwrap_or_default();
    call(router, "POST", &uri, payload, &headers).await
}

/// Create a function with one published version and one alias, so every qualifier form is
/// reachable, and wait out the provisioning window the create documents.
async fn create_active_function(router: &MultiServiceRouter, name: &str) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": true,
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }))
        .expect("the create body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    let alias = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/aliases"),
        &serde_json::to_vec(&json!({ "Name": "live", "FunctionVersion": "1" }))
            .expect("the alias body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        alias.status,
        201,
        "CreateAlias on {name:?} must succeed, got {} {}",
        alias.status,
        alias.text()
    );
    // "During this time, you can't invoke or modify the function": the move to `Active` is
    // owed by the clock, and this waits past the instant the create made it due.
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// One dry-run row: how the request addresses the function and the version it must report
/// as the one it resolved.
struct DryRunCase {
    name: &'static str,
    /// How the request addresses the function, built from the function name.
    reference: fn(&str) -> String,
    /// The `Qualifier` query parameter the request carries, if any.
    qualifier: Option<&'static str>,
    /// The `X-Amz-Executed-Version` the answer must carry.
    executed_version: &'static str,
    /// The request body, which a dry run validates without running anything.
    payload: &'static str,
}

/// The rows: every documented `FunctionName` form, each qualifier kind, and the two places
/// a qualifier can be named.
fn dry_run_cases() -> Vec<DryRunCase> {
    vec![
        DryRunCase {
            name: "the unpublished version, addressed by the bare name",
            reference: |name| name.to_string(),
            qualifier: None,
            executed_version: LATEST,
            payload: "{}",
        },
        DryRunCase {
            name: "the unpublished version, addressed by the full function arn",
            reference: |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
            qualifier: None,
            executed_version: LATEST,
            payload: "",
        },
        DryRunCase {
            name: "the unpublished version, addressed by the partial arn",
            reference: |name| format!("{ACCOUNT}:function:{name}"),
            qualifier: None,
            executed_version: LATEST,
            payload: r#"{"order":{"id":7,"items":["a","b"]}}"#,
        },
        DryRunCase {
            name: "a published version, named in the Qualifier parameter",
            reference: |name| name.to_string(),
            qualifier: Some("1"),
            executed_version: "1",
            payload: "{}",
        },
        DryRunCase {
            name: "a published version, named in the function name",
            reference: |name| format!("{name}:1"),
            qualifier: None,
            executed_version: "1",
            payload: "[1,2,3]",
        },
        DryRunCase {
            name: "an alias, addressed by the function: prefix form",
            reference: |name| format!("function:{name}:live"),
            qualifier: None,
            // `ExecutedVersion` is "the version of the function that executed. When you
            // invoke a function with an alias, this indicates which version the alias
            // resolved to", so an alias reports that version and not its own name.
            executed_version: "1",
            payload: r#""a json string is a json document too""#,
        },
        DryRunCase {
            name: "an alias, named in the Qualifier parameter",
            reference: |name| name.to_string(),
            qualifier: Some("live"),
            executed_version: "1",
            payload: "{}",
        },
    ]
}

#[tokio::test]
async fn a_dry_run_is_served_from_the_store_for_every_documented_name_form() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, "stream-dry-run").await;

    for case in dry_run_cases() {
        let label = case.name;
        let reference = (case.reference)("stream-dry-run");
        let answer = invoke_with_response_stream(
            &router,
            &reference,
            case.qualifier,
            Some("DryRun"),
            case.payload.as_bytes(),
        )
        .await;
        // "For the DryRun invocation type, this status code is 204."
        assert_eq!(
            answer.status,
            204,
            "case {label:?}: expected the documented dry-run status, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.executed_version(),
            Some(case.executed_version.to_string()),
            "case {label:?}: expected the resolved version on X-Amz-Executed-Version, got {:?}",
            answer.executed_version()
        );
        assert!(
            answer.body.is_empty(),
            "case {label:?}: nothing ran, so no chunk is streamed, got {:?}",
            answer.text()
        );
    }
}

#[tokio::test]
async fn a_pair_that_names_no_function_is_the_modelled_not_found() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, "stream-held").await;

    // Each row names something the store does not hold, through a different name form, and
    // for each of the two invocation types — the refusal is about resolution, so it does
    // not depend on what the caller asked to be done once resolved. The expected message
    // names the resource the invocation addressed: the function AT the version it would
    // have run, which is `$LATEST` when the request names no qualifier. A name of ANOTHER
    // Region resolves no version at all — the Region is settled first — so its refusal
    // states the two Regions that disagree instead of naming a resolved ARN.
    let not_found = |arn: &str| format!("Function not found: {arn}");
    for (label, reference, qualifier, expected) in [
        (
            "a function that does not exist",
            "stream-absent".to_string(),
            None,
            not_found(&format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:stream-absent:{LATEST}"
            )),
        ),
        (
            "a function arn that names no function",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:stream-absent"),
            None,
            not_found(&format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:stream-absent:{LATEST}"
            )),
        ),
        (
            "a partial arn that names no function",
            format!("{ACCOUNT}:function:stream-absent"),
            None,
            not_found(&format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:stream-absent:{LATEST}"
            )),
        ),
        (
            "a held function addressed through another Region",
            format!("arn:aws:lambda:us-west-2:{ACCOUNT}:function:stream-held"),
            None,
            format!("Functions from 'us-west-2' are not reachable in this region ('{REGION}')"),
        ),
        (
            "a version a known function has not published",
            "stream-held".to_string(),
            Some("9"),
            not_found(&format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:stream-held:9"
            )),
        ),
        (
            "an alias a known function does not hold",
            "stream-held".to_string(),
            Some("absent-alias"),
            not_found(&format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:stream-held:absent-alias"
            )),
        ),
        (
            "a version named in the function name that does not exist",
            "stream-held:9".to_string(),
            None,
            not_found(&format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:stream-held:9"
            )),
        ),
    ] {
        for invocation_type in [None, Some("DryRun"), Some("RequestResponse")] {
            let answer = invoke_with_response_stream(
                &router,
                &reference,
                qualifier,
                invocation_type,
                b"{}",
            )
            .await;
            assert_eq!(
                answer.status,
                404,
                "case {label:?} ({invocation_type:?}): expected the modelled 404, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {label:?} ({invocation_type:?}): expected ResourceNotFoundException, \
                 got {}",
                answer.error_code()
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {label:?} ({invocation_type:?}): expected {expected:?}, got {:?}",
                answer.message()
            );
        }
    }
}

#[tokio::test]
async fn a_request_the_function_cannot_take_is_refused_on_its_own_terms() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // A function that is still being provisioned cannot be invoked: "During this time, you
    // can't invoke or modify the function."
    let package: Vec<u8> = (0..32u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let created = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": "stream-pending",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }))
        .expect("the create body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the function under test must be created, got {} {}",
        created.status,
        created.text()
    );
    let while_pending =
        invoke_with_response_stream(&router, "stream-pending", None, Some("DryRun"), b"{}").await;
    assert_eq!(
        while_pending.status,
        409,
        "an invocation inside the provisioning window is refused, got {} {}",
        while_pending.status,
        while_pending.text()
    );
    assert!(
        while_pending
            .error_code()
            .contains("ResourceConflictException"),
        "expected ResourceConflictException, got {}",
        while_pending.error_code()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;

    // A body that is not the JSON event the function takes, over a set of non-JSON bodies.
    for (label, payload) in [
        ("a bare word", &b"not-json"[..]),
        ("an unterminated object", &b"{\"a\":"[..]),
        ("binary bytes", &[0xff, 0xfe, 0x00, 0x01][..]),
    ] {
        let answer =
            invoke_with_response_stream(&router, "stream-pending", None, Some("DryRun"), payload)
                .await;
        assert_eq!(
            answer.status,
            400,
            "case {label:?}: expected the modelled 400 for a body that is not json, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer
                .error_code()
                .contains("InvalidRequestContentException"),
            "case {label:?}: expected InvalidRequestContentException, got {}",
            answer.error_code()
        );
    }

    // An invocation type the modelled enum does not admit is refused as invalid, not
    // served: `ResponseStreamingInvocationType` is `RequestResponse | DryRun`.
    for (label, invocation_type) in [
        ("the asynchronous type this operation does not model", "Event"),
        ("a type no enum value spells", "Streaming"),
    ] {
        let answer = invoke_with_response_stream(
            &router,
            "stream-pending",
            None,
            Some(invocation_type),
            b"{}",
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {label:?}: expected 400 for a value outside the modelled enum, got {} {}",
            answer.status,
            answer.text()
        );
    }
}

/// A package this emulator cannot READ is refused with the modelled failure for it, never
/// answered with a stream the function did not produce.
///
/// The function this suite creates carries 64 bytes that are not a .zip archive at all, so
/// there is no handler to run: "Lambda could not unzip the deployment package"
/// (`InvalidZipFileException`, `@httpError(502)`) is what that is. The two rows are the
/// same path, because the default invocation type is the synchronous one.
#[tokio::test]
async fn a_package_that_cannot_be_read_is_refused_rather_than_streamed() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, "stream-run").await;

    for (label, invocation_type) in [
        ("the default invocation type", None),
        ("RequestResponse stated explicitly", Some("RequestResponse")),
    ] {
        let answer =
            invoke_with_response_stream(&router, "stream-run", None, invocation_type, b"{}").await;
        assert_eq!(
            answer.status,
            502,
            "case {label:?}: expected the modelled 502, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains("InvalidZipFileException"),
            "case {label:?}: expected InvalidZipFileException, got {}",
            answer.error_code()
        );
        assert!(
            !answer.body.is_empty() && answer.json().get("Message").is_some(),
            "case {label:?}: the refusal is rendered in this service's own error envelope, \
             got {:?}",
            answer.text()
        );
    }
}
