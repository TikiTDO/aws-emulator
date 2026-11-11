// SPDX-License-Identifier: CC0-1.0

//! What this service SAYS when it refuses, and the refusals a regional resource owes.
//!
//! Four rules are under test, each over a table of inputs the recorded case is one row of:
//!
//! 1. **An invocation whose payload is past the quota for its invocation type is refused
//!    naming that payload, that type and that type's quota** — `<n> byte payload is too
//!    large for the <InvocationType> invocation type (limit <limit> bytes)` — so one
//!    sentence serves every invocation type and every part but the fixed text is a value of
//!    the request. The rows span the invocation type (named, defaulted) and the size class
//!    (under, exactly at, one byte over, far over each type's own quota).
//! 2. **A Lambda function is a REGIONAL resource, so a `FunctionName` given as a full ARN
//!    whose Region field is not the endpoint's names a function this endpoint cannot reach**
//!    and is refused with the modelled 404 naming the two Regions that disagree —
//!    `Functions from '<named region>' are not reachable in this region ('<endpoint
//!    region>')` — with no resource named, because the Region is settled before a version
//!    is. The rows span the operation (an invocation, a read, a policy read, a write, a
//!    publish, a listing), the Region named, and the control that the same function IS
//!    reachable through its own Region's endpoint.
//! 3. **A refusal names the resource the request ADDRESSED**: the `FunctionName` and
//!    `Qualifier` pair, with the qualifier only when the request named one; a pair naming
//!    two different qualifiers is refused with the fixed sentence for that disagreement;
//!    and a layer version is named by its `LayerVersionArn`.
//! 4. **A function's environment is refused in AWS's own words**: over the published 4 KB
//!    budget, quoting the string that was measured, and for a key Lambda reserves for the
//!    runtime, naming the reserved keys the request used. The rows span the operation
//!    (`CreateFunction`, `UpdateFunctionConfiguration`), the size class around the budget,
//!    and which reserved key was named.
//!
//! And one property of every answer: it names the request it answered in the
//! `x-amzn-requestid` header, on a success exactly as on a refusal, with a fresh id per
//! call — which is where an SDK reads the `RequestId` of its response metadata from. It is
//! a property of the RESPONSE, not of any execution an invocation started, so what is
//! asserted is its form and that no two answers share one.

use std::io::Write as _;
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

/// The Region this suite's endpoint is in, unless a case names another.
const REGION: &str = "us-east-1";

/// Another Region, to address this endpoint's resources from and its resources through.
const OTHER_REGION: &str = "us-west-2";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The label the unpublished version is stored under.
const LATEST: &str = "$LATEST";

/// The module the handler of every package below lives in.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(100);

/// "For synchronous invocations, the maximum payload size is 6 MB" (the model's own `Invoke`
/// documentation).
const SYNCHRONOUS_QUOTA: usize = 6 * 1024 * 1024;

/// "For asynchronous invocations, the maximum payload size is 1 MB" (the model's own
/// `Invoke` documentation).
const ASYNCHRONOUS_QUOTA: usize = 1024 * 1024;

/// The quota on a handler's RESPONSE, which Lambda states in its own refusal for a response
/// past it (`Response payload size exceeded maximum allowed payload size (<n> bytes).`).
const RESPONSE_QUOTA: usize = 6_291_556;

/// AWS's published quota for all of a function's environment variables combined, which its
/// own refusal for a set past it states as `4KB`, measured on the whole mapping as one
/// serialized JSON object.
const ENVIRONMENT_QUOTA: usize = 4 * 1024;

/// The string the environment budget is measured on, as the service itself renders it —
/// read from the service rather than restated here, so this suite and the handler cannot
/// disagree about what was counted.
fn measured_environment(variables: &Value) -> String {
    let set: std::collections::BTreeMap<String, String> = variables
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(name, value)| {
                    (
                        name.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    service_com_amazonaws_lambda_awsgirapiservice::state::measured_environment(&set)
}

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
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    fn error_code(&self) -> String {
        self.header("x-amzn-errortype")
            .or_else(|| {
                self.json()
                    .get("__type")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| self.text())
    }

    fn message(&self) -> String {
        let document = self.json();
        document
            .get("Message")
            .or_else(|| document.get("message"))
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

/// Dispatch one request to `region`'s endpoint, signed for that Region's scope. Nothing in
/// the signature is verified: the harness turns request authentication off, and the scope is
/// read only for the Region the caller addressed.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    let host = format!("lambda.{region}.amazonaws.com");
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{region}/lambda/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    );
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://{host}{uri}"))
        .header("host", host)
        .header("authorization", authorization)
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

/// [`call_in`] against this suite's own Region.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    call_in(router, REGION, method, uri, body, headers).await
}

/// A .zip deployment package holding `source` as `lambda_function.py`, written in memory
/// the way a caller uploads one.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// The body of a create whose code is `source`.
fn create_body(name: &str, source: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": HANDLER,
        "Timeout": 20,
        "MemorySize": 512,
        "Code": {
            "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
        },
    }))
    .expect("the create body serializes")
}

/// Create a function in `region` whose code is `source` and wait out the provisioning
/// window the create documents.
async fn create_function_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    source: &str,
) {
    let created = call_in(
        router,
        region,
        "POST",
        FUNCTIONS_URI,
        &create_body(name, source),
        &[],
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} in {region} must succeed, got {} {}",
        created.status,
        created.text()
    );
    // "During this time, you can't invoke or modify the function": the move to `Active` is
    // owed by the clock, and this waits past the instant the create made it due.
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// A handler that returns whatever it is told to.
const ECHO_SOURCE: &str = "def lambda_handler(event, context):\n    return event\n";

/// `Invoke` on the function `reference` names, with the `InvocationType` header `kind`.
async fn invoke(
    router: &MultiServiceRouter,
    reference: &str,
    payload: &[u8],
    kind: Option<&str>,
) -> Answer {
    let headers: Vec<(&str, &str)> = kind
        .map(|value| vec![("x-amz-invocation-type", value)])
        .unwrap_or_default();
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{reference}/invocations"),
        payload,
        &headers,
    )
    .await
}

/// RULE 1: a payload past the quota for its invocation type is refused naming that payload,
/// that type and that type's quota — one row per invocation type and size class.
#[tokio::test]
async fn a_payload_past_its_invocation_types_quota_names_the_payload_the_type_and_the_quota() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function_in(&router, REGION, "quota-probe", ECHO_SOURCE).await;

    // Each row is a JSON payload of exactly `bytes` bytes, so the size the refusal states is
    // the size the request carried.
    let cases: &[(&str, Option<&str>, usize, Option<&str>)] = &[
        (
            "no type named, far under the synchronous quota",
            None,
            32,
            None,
        ),
        (
            "no type named, exactly the synchronous quota",
            None,
            SYNCHRONOUS_QUOTA,
            None,
        ),
        (
            "no type named, one byte over the synchronous quota",
            None,
            SYNCHRONOUS_QUOTA + 1,
            Some("RequestResponse"),
        ),
        (
            "RequestResponse, one byte over its quota",
            Some("RequestResponse"),
            SYNCHRONOUS_QUOTA + 1,
            Some("RequestResponse"),
        ),
        (
            "RequestResponse, far over its quota",
            Some("RequestResponse"),
            SYNCHRONOUS_QUOTA + 4_096,
            Some("RequestResponse"),
        ),
        (
            "DryRun, one byte over the synchronous quota",
            Some("DryRun"),
            SYNCHRONOUS_QUOTA + 1,
            Some("DryRun"),
        ),
        (
            "Event, exactly its own quota",
            Some("Event"),
            ASYNCHRONOUS_QUOTA,
            None,
        ),
        (
            "Event, one byte over its own quota",
            Some("Event"),
            ASYNCHRONOUS_QUOTA + 1,
            Some("Event"),
        ),
        (
            "Event, far over its own quota",
            Some("Event"),
            ASYNCHRONOUS_QUOTA + 9_000,
            Some("Event"),
        ),
    ];

    for (case, kind, bytes, refused_for) in cases {
        let payload = json_payload_of(*bytes);
        assert_eq!(
            payload.len(),
            *bytes,
            "case {case:?}: the row's own payload must be {bytes} bytes, got {}",
            payload.len()
        );
        let answer = invoke(&router, "quota-probe", &payload, *kind).await;
        match refused_for {
            None => assert!(
                answer.status < 400,
                "case {case:?}: a payload within the quota must be served, got {} {}",
                answer.status,
                answer.text()
            ),
            Some(invocation_type) => {
                let limit = if *invocation_type == "Event" {
                    ASYNCHRONOUS_QUOTA
                } else {
                    SYNCHRONOUS_QUOTA
                };
                let expected = format!(
                    "{bytes} byte payload is too large for the {invocation_type} invocation \
                     type (limit {limit} bytes)"
                );
                assert_eq!(
                    answer.status,
                    413,
                    "case {case:?}: expected the modelled 413, got {} {}",
                    answer.status,
                    answer.text()
                );
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

/// A JSON payload of exactly `bytes` bytes: a quoted string padded to length, so the body
/// the quota is measured over is the size the row names and is still the JSON an invocation
/// carries.
fn json_payload_of(bytes: usize) -> Vec<u8> {
    let inner = bytes.saturating_sub(2);
    format!("\"{}\"", "p".repeat(inner)).into_bytes()
}

/// RULE 2: a function ARN whose Region is not the endpoint's names a function this endpoint
/// cannot reach — one row per operation, with the same-Region call as the control for each.
///
/// The refusal states the two Regions that disagree and names no resource: the Region is
/// settled BEFORE a version is, so no operation — not even an invoking one, whose refusal
/// for a name of this Region does carry the version it resolved — has a resolved resource to
/// name.
#[tokio::test]
async fn a_function_arn_from_another_region_names_no_function_at_this_endpoint() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function_in(&router, REGION, "region-probe", ECHO_SOURCE).await;
    // A statement on the function's resource policy, so the POLICY read below has a policy
    // of this Region's function to report: what the foreign-ARN rows then show is the
    // Region refusal rather than an absent policy.
    let granted = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/region-probe/policy"),
        &serde_json::to_vec(&json!({
            "StatementId": "region-probe-grant",
            "Action": "lambda:InvokeFunction",
            "Principal": "s3.amazonaws.com",
        }))
        .expect("the permission body serializes"),
        &[],
    )
    .await;
    assert!(
        granted.status < 400,
        "the control grant must be served, got {} {}",
        granted.status,
        granted.text()
    );

    let home = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:region-probe");
    let foreign = format!("arn:aws:lambda:{OTHER_REGION}:{ACCOUNT}:function:region-probe");
    let partial = format!("{ACCOUNT}:function:region-probe");

    /// One operation to drive the Region rule through: how it is addressed and the body it
    /// takes.
    struct RegionOperation {
        operation: &'static str,
        method: &'static str,
        suffix: &'static str,
        body: Option<Vec<u8>>,
    }

    // One row per operation kind — an invocation, a read, a policy read, a write, a publish,
    // a listing — each driven with the ARN of this endpoint's own Region (which must
    // resolve), the bare and partial forms (which name no Region at all, so they resolve
    // too), and another Region's ARN (which this endpoint cannot reach).
    let operations: Vec<RegionOperation> = vec![
        RegionOperation {
            operation: "Invoke",
            method: "POST",
            suffix: "/invocations",
            body: Some(b"{}".to_vec()),
        },
        RegionOperation {
            operation: "GetFunction",
            method: "GET",
            suffix: "",
            body: None,
        },
        RegionOperation {
            operation: "GetFunctionConfiguration",
            method: "GET",
            suffix: "/configuration",
            body: None,
        },
        RegionOperation {
            operation: "GetPolicy",
            method: "GET",
            suffix: "/policy",
            body: None,
        },
        RegionOperation {
            operation: "ListVersionsByFunction",
            method: "GET",
            suffix: "/versions",
            body: None,
        },
        RegionOperation {
            operation: "ListAliases",
            method: "GET",
            suffix: "/aliases",
            body: None,
        },
        RegionOperation {
            operation: "UpdateFunctionConfiguration",
            method: "PUT",
            suffix: "/configuration",
            body: Some(
                serde_json::to_vec(&json!({ "Description": "re-described" }))
                    .expect("the update body serializes"),
            ),
        },
        RegionOperation {
            operation: "PublishVersion",
            method: "POST",
            suffix: "/versions",
            body: Some(b"{}".to_vec()),
        },
    ];

    for row in &operations {
        let RegionOperation {
            operation,
            method,
            suffix,
            body,
        } = row;
        for (case, reference, reachable) in [
            ("the bare name", "region-probe".to_string(), true),
            ("this Region's own ARN", home.clone(), true),
            (
                "the partial ARN, which names no Region",
                partial.clone(),
                true,
            ),
            ("another Region's ARN", foreign.clone(), false),
            (
                "another Region's ARN at the unpublished version",
                format!("{foreign}:{LATEST}"),
                false,
            ),
        ] {
            let uri = format!("{FUNCTIONS_URI}/{reference}{suffix}");
            let answer = call(
                &router,
                method,
                &uri,
                body.as_deref().unwrap_or_default(),
                &[],
            )
            .await;
            // A configuration write leaves the function updating for a window the update
            // documents, and an overlapping write is refused for THAT rather than for the
            // Region; the next row waits past the instant the clock made that window end.
            if *method == "PUT" {
                tokio::time::sleep(UPDATE_DELAY + SLACK).await;
            }
            if reachable {
                assert!(
                    answer.status < 400,
                    "case {operation} with {case}: expected the call to be served, got {} {}",
                    answer.status,
                    answer.text()
                );
                continue;
            }
            assert_eq!(
                answer.status,
                404,
                "case {operation} with {case}: expected the modelled 404, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {operation} with {case}: expected ResourceNotFoundException, got {}",
                answer.error_code()
            );
            // The Region is settled before a version is, so every operation — the
            // invocation included — states the two Regions and names no resource.
            let expected = format!(
                "Functions from '{OTHER_REGION}' are not reachable in this region ('{REGION}')"
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {operation} with {case}: expected {expected:?}, got {:?}",
                answer.message()
            );
        }
    }

    // The ALIAS operations key the store by the function's own name rather than going
    // through the shared resolution, so each applies the Region rule where it parses the
    // reference: the foreign ARN is refused and this Region's own is served, for the whole
    // create/update/delete sequence on one alias.
    for (operation, method, suffix, body) in [
        (
            "CreateAlias",
            "POST",
            "/aliases".to_string(),
            Some(
                serde_json::to_vec(&json!({ "Name": "live", "FunctionVersion": LATEST }))
                    .expect("the alias body serializes"),
            ),
        ),
        (
            "UpdateAlias",
            "PUT",
            "/aliases/live".to_string(),
            Some(
                serde_json::to_vec(&json!({ "FunctionVersion": LATEST }))
                    .expect("the alias body serializes"),
            ),
        ),
        ("DeleteAlias", "DELETE", "/aliases/live".to_string(), None),
    ] {
        let refused = call(
            &router,
            method,
            &format!("{FUNCTIONS_URI}/{foreign}{suffix}"),
            body.as_deref().unwrap_or_default(),
            &[],
        )
        .await;
        assert_eq!(
            refused.status,
            404,
            "case {operation} with another Region's ARN: expected the modelled 404, got {} {}",
            refused.status,
            refused.text()
        );
        let expected = format!(
            "Functions from '{OTHER_REGION}' are not reachable in this region ('{REGION}')"
        );
        assert_eq!(
            refused.message(),
            expected,
            "case {operation} with another Region's ARN: expected {expected:?}, got {:?}",
            refused.message()
        );
        let served = call(
            &router,
            method,
            &format!("{FUNCTIONS_URI}/{home}{suffix}"),
            body.as_deref().unwrap_or_default(),
            &[],
        )
        .await;
        assert!(
            served.status < 400,
            "case {operation} with this Region's own ARN: expected the call to be served, got \
             {} {}",
            served.status,
            served.text()
        );
    }

    // The CONTROL for the whole table: the function is reachable through its OWN Region's
    // endpoint by every form of its name, so the rows above refuse the Region rather than
    // the ARN form.
    let served = call_in(
        &router,
        REGION,
        "GET",
        &format!("{FUNCTIONS_URI}/{home}"),
        b"",
        &[],
    )
    .await;
    assert!(
        served.status < 400,
        "the control read must be served, got {} {}",
        served.status,
        served.text()
    );
    // And a function created at the OTHER Region's endpoint is readable through the ARN
    // that Region mints for it, so the rows above refuse the ARN's Region against the
    // endpoint's rather than refusing one particular Region.
    create_function_in(&router, OTHER_REGION, "region-probe-west", ECHO_SOURCE).await;
    let west_arn = format!("arn:aws:lambda:{OTHER_REGION}:{ACCOUNT}:function:region-probe-west");
    let elsewhere = call_in(
        &router,
        OTHER_REGION,
        "GET",
        &format!("{FUNCTIONS_URI}/{west_arn}"),
        b"",
        &[],
    )
    .await;
    assert!(
        elsewhere.status < 400,
        "the other Region's own ARN must resolve at its own endpoint, got {} {}",
        elsewhere.status,
        elsewhere.text()
    );
    let across = call(&router, "GET", &format!("{FUNCTIONS_URI}/{west_arn}"), b"", &[]).await;
    assert_eq!(
        across.status,
        404,
        "the other Region's ARN must not resolve at this endpoint, got {} {}",
        across.status,
        across.text()
    );
}

/// RULE 3: a `FunctionName` and a `Qualifier` that name two different versions are refused
/// with the fixed sentence for that disagreement — one row per pair of disagreeing
/// qualifiers, with the agreeing pairs as the control.
#[tokio::test]
async fn a_name_and_a_qualifier_that_disagree_are_refused_with_the_fixed_sentence() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function_in(&router, REGION, "qualifier-probe", ECHO_SOURCE).await;

    /// The sentence AWS gives the disagreement, which states the requirement and no part of
    /// the request.
    const EXPECTED: &str =
        "The derived qualifier from the function name does not match the specified qualifier.";

    for (case, reference, qualifier, disagree) in [
        ("a version against the unpublished one", "qualifier-probe:1", LATEST, true),
        ("the unpublished one against a version", "qualifier-probe:$LATEST", "1", true),
        ("two different versions", "qualifier-probe:1", "2", true),
        ("a version against an alias", "qualifier-probe:1", "live", true),
        ("the same version twice", "qualifier-probe:1", "1", false),
        ("the same alias twice", "qualifier-probe:live", "live", false),
    ] {
        let answer = call(
            &router,
            "GET",
            &format!("{FUNCTIONS_URI}/{reference}/configuration?Qualifier={qualifier}"),
            b"",
            &[],
        )
        .await;
        if !disagree {
            // An agreeing pair names one resource; whether that resource EXISTS is another
            // rule, so the control only asserts it is not refused for disagreeing.
            assert_ne!(
                answer.message(),
                EXPECTED,
                "case {case:?}: an agreeing pair must not be refused for disagreeing, got {}",
                answer.text()
            );
            continue;
        }
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected the modelled 400, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.message(),
            EXPECTED,
            "case {case:?}: expected {EXPECTED:?}, got {:?}",
            answer.message()
        );
    }
}

/// RULE 3, for a LAYER version: the refusal names the version by its `LayerVersionArn` —
/// one row per way the pair names no stored version, and per documented form of the layer
/// name, with the stored version as the control.
#[tokio::test]
async fn a_layer_version_refusal_names_the_layer_version_arn() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let published = call(
        &router,
        "POST",
        &format!("{LAYERS_URI}/held-layer/versions"),
        &serde_json::to_vec(&json!({
            "Content": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package("x = 1\n")),
            },
        }))
        .expect("the publish body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        published.status,
        201,
        "PublishLayerVersion must succeed, got {} {}",
        published.status,
        published.text()
    );

    let layer_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:held-layer");
    let absent_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:absent-layer");
    for (case, name, version, expected) in [
        (
            "a version the held layer has not published, addressed by name",
            "held-layer".to_string(),
            9,
            Some(format!("{layer_arn}:9")),
        ),
        (
            "a version the held layer has not published, addressed by layer ARN",
            layer_arn.clone(),
            12,
            Some(format!("{layer_arn}:12")),
        ),
        (
            "a layer this account does not hold, addressed by name",
            "absent-layer".to_string(),
            1,
            Some(format!("{absent_arn}:1")),
        ),
        (
            "a layer this account does not hold, addressed by layer ARN",
            absent_arn.clone(),
            4,
            Some(format!("{absent_arn}:4")),
        ),
        (
            "the version that IS held",
            "held-layer".to_string(),
            1,
            None,
        ),
    ] {
        let answer = call(
            &router,
            "GET",
            &format!("{LAYERS_URI}/{name}/versions/{version}/policy"),
            b"",
            &[],
        )
        .await;
        match expected {
            // The CONTROL: the held version resolves, so the read gets as far as reporting
            // that it holds no policy — a different refusal, with a different sentence.
            None => assert_ne!(
                answer.message(),
                format!("Layer version {layer_arn}:{version} does not exist."),
                "case {case:?}: the held version must resolve, got {}",
                answer.text()
            ),
            Some(version_arn) => {
                let wanted = format!("Layer version {version_arn} does not exist.");
                assert_eq!(
                    answer.status,
                    404,
                    "case {case:?}: expected the modelled 404, got {} {}",
                    answer.status,
                    answer.text()
                );
                assert_eq!(
                    answer.message(),
                    wanted,
                    "case {case:?}: expected {wanted:?}, got {:?}",
                    answer.message()
                );
            }
        }
    }
}

/// RULE 4: a function's environment is refused in AWS's own words — one row per size class
/// around the published budget and per reserved key, across both operations that accept an
/// `Environment`.
#[tokio::test]
async fn an_environment_is_refused_for_its_budget_and_for_a_reserved_key() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function_in(&router, REGION, "env-probe", ECHO_SOURCE).await;

    /// The fixed opening of both sentences.
    const OPENING: &str =
        "Lambda was unable to configure your environment variables because the environment \
         variables you have provided";

    // Each row is one variable set and the message it must be refused with, or `None` for a
    // set that must be accepted. The budget is measured on the whole mapping as ONE
    // serialized JSON object, which is the string the refusal reports it measured — so a
    // row's punctuation counts as much as its names and values: `{"BULK":"…"}` is eleven
    // bytes of frame around the value.
    let frame = measured_environment(&json!({ "BULK": "" })).len();
    let at_budget = "v".repeat(ENVIRONMENT_QUOTA - frame);
    let over_budget = "v".repeat(ENVIRONMENT_QUOTA - frame + 1);
    let two_over = json!({ "A": over_budget.clone(), "B": over_budget.clone() });
    let rows: Vec<(&str, Value, Option<String>)> = vec![
        ("an empty set", json!({}), None),
        (
            "one small variable",
            json!({ "STAGE": "beta" }),
            None,
        ),
        (
            "exactly the budget",
            json!({ "BULK": at_budget.clone() }),
            None,
        ),
        (
            "one byte over the budget",
            json!({ "BULK": over_budget.clone() }),
            Some(format!(
                "{OPENING} exceeded the 4KB limit. String measured: {}",
                measured_environment(&json!({ "BULK": over_budget.clone() }))
            )),
        ),
        (
            "far over the budget, spread over two variables",
            two_over.clone(),
            Some(format!(
                "{OPENING} exceeded the 4KB limit. String measured: {}",
                measured_environment(&two_over)
            )),
        ),
        (
            "the Region the runtime sets",
            json!({ "AWS_REGION": "us-east-1" }),
            Some(format!(
                "{OPENING} contains reserved keys that are currently not supported for \
                 modification. Reserved keys used in this request: AWS_REGION"
            )),
        ),
        (
            "the handler the runtime sets",
            json!({ "_HANDLER": "other.handler" }),
            Some(format!(
                "{OPENING} contains reserved keys that are currently not supported for \
                 modification. Reserved keys used in this request: _HANDLER"
            )),
        ),
        (
            "a credential the runtime sets",
            json!({ "AWS_SESSION_TOKEN": "token", "STAGE": "beta" }),
            Some(format!(
                "{OPENING} contains reserved keys that are currently not supported for \
                 modification. Reserved keys used in this request: AWS_SESSION_TOKEN"
            )),
        ),
        (
            "two reserved keys at once",
            json!({ "LAMBDA_TASK_ROOT": "/tmp", "AWS_LAMBDA_FUNCTION_NAME": "other" }),
            Some(format!(
                "{OPENING} contains reserved keys that are currently not supported for \
                 modification. Reserved keys used in this request: \
                 AWS_LAMBDA_FUNCTION_NAME, LAMBDA_TASK_ROOT"
            )),
        ),
        (
            "a key that only LOOKS reserved",
            json!({ "AWS_REGION_OVERRIDE": "us-west-2" }),
            None,
        ),
    ];

    for (index, (case, variables, expected)) in rows.iter().enumerate() {
        // The same set is offered to both operations that accept an `Environment`, so the
        // rule is the configuration's rather than one handler's.
        let created = call(
            &router,
            "POST",
            FUNCTIONS_URI,
            &serde_json::to_vec(&json!({
                "FunctionName": format!("env-row-{index}"),
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": HANDLER,
                "Environment": { "Variables": variables },
                "Code": {
                    "ZipFile": base64::engine::general_purpose::STANDARD
                        .encode(package(ECHO_SOURCE)),
                },
            }))
            .expect("the create body serializes"),
            &[],
        )
        .await;
        let updated = call(
            &router,
            "PUT",
            &format!("{FUNCTIONS_URI}/env-probe/configuration"),
            &serde_json::to_vec(&json!({ "Environment": { "Variables": variables } }))
                .expect("the update body serializes"),
            &[],
        )
        .await;
        // An accepted configuration write leaves the function updating for the window the
        // update documents, so the next row waits past the instant the clock made that
        // window end rather than being refused for the overlap.
        tokio::time::sleep(UPDATE_DELAY + SLACK).await;
        for (operation, answer) in [("CreateFunction", created), ("UpdateFunctionConfiguration", updated)] {
            match expected {
                None => assert!(
                    answer.status < 400,
                    "case {case:?} on {operation}: expected the set to be accepted, got {} {}",
                    answer.status,
                    answer.text()
                ),
                Some(wanted) => {
                    assert_eq!(
                        answer.status,
                        400,
                        "case {case:?} on {operation}: expected the modelled 400, got {} {}",
                        answer.status,
                        answer.text()
                    );
                    assert_eq!(
                        answer.message(),
                        *wanted,
                        "case {case:?} on {operation}: expected {wanted:?}, got {:?}",
                        answer.message()
                    );
                }
            }
        }
    }
}

/// Every answer names the request it answered in `x-amzn-requestid`, with a fresh id per
/// call — one row per answer class (a create, a read, a refusal, an unroutable path).
#[tokio::test]
async fn every_answer_names_the_request_it_answered() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function_in(&router, REGION, "id-probe", ECHO_SOURCE).await;

    let mut seen: Vec<String> = Vec::new();
    for (case, method, uri, body) in [
        ("a served read", "GET", format!("{FUNCTIONS_URI}/id-probe"), b"".to_vec()),
        (
            "a served invocation",
            "POST",
            format!("{FUNCTIONS_URI}/id-probe/invocations"),
            b"{}".to_vec(),
        ),
        (
            "a modelled refusal",
            "GET",
            format!("{FUNCTIONS_URI}/absent-probe"),
            b"".to_vec(),
        ),
        (
            "a listing",
            "GET",
            FUNCTIONS_URI.to_string(),
            b"".to_vec(),
        ),
        (
            "a path this model binds no operation to",
            "GET",
            "/2015-03-31/no-such-thing".to_string(),
            b"".to_vec(),
        ),
    ] {
        let answer = call(&router, method, &uri, &body, &[]).await;
        let reported = answer.header("x-amzn-requestid");
        let id = reported.unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the answer to name the request it answered, got \
                 headers {:?}",
                answer.headers
            )
        });
        // The id is a UUID, which is the form AWS reports: five `-`-separated fields of
        // hexadecimal.
        let fields: Vec<&str> = id.split('-').collect();
        assert_eq!(
            fields.len(),
            5,
            "case {case:?}: expected a UUID-shaped id, got {id:?}"
        );
        assert!(
            fields
                .iter()
                .all(|field| !field.is_empty()
                    && field.bytes().all(|byte| byte.is_ascii_hexdigit())),
            "case {case:?}: expected a UUID-shaped id, got {id:?}"
        );
        assert!(
            !seen.contains(&id),
            "case {case:?}: expected a fresh id per call, got {id:?} again"
        );
        seen.push(id);
    }
}

/// A response exactly at the quota is delivered whole, and one byte over it is refused as
/// the response-size failure it is — two rows around the one bound, plus a small control.
#[tokio::test]
async fn a_response_at_the_quota_is_delivered_and_one_byte_over_it_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    // The handler returns a string of exactly the length the event asks for, so the response
    // body is that length plus the two quotes JSON wraps a string in.
    create_function_in(
        &router,
        REGION,
        "size-probe",
        "def lambda_handler(event, context):\n    return 'r' * event['length']\n",
    )
    .await;

    for (case, length, delivered) in [
        ("far under the quota", 16_usize, true),
        ("exactly the quota", RESPONSE_QUOTA - 2, true),
        ("one byte over the quota", RESPONSE_QUOTA - 1, false),
    ] {
        let answer = invoke(
            &router,
            "size-probe",
            &serde_json::to_vec(&json!({ "length": length }))
                .expect("the event serializes"),
            Some("RequestResponse"),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: an invocation that ran is still 200, got {} {}",
            answer.status,
            answer.text()
        );
        if delivered {
            assert_eq!(
                answer.header("x-amz-function-error"),
                None,
                "case {case:?}: a response within the quota must be delivered, got {}",
                String::from_utf8_lossy(&answer.body[..answer.body.len().min(300)])
            );
            assert_eq!(
                answer.body.len(),
                length + 2,
                "case {case:?}: expected the whole response back, got {} bytes",
                answer.body.len()
            );
            continue;
        }
        assert_eq!(
            answer.header("x-amz-function-error").as_deref(),
            Some("Unhandled"),
            "case {case:?}: a response past the quota must fail the invocation, got {}",
            String::from_utf8_lossy(&answer.body[..answer.body.len().min(300)])
        );
        let document = answer.json();
        assert_eq!(
            document.get("errorType").and_then(Value::as_str),
            Some("Function.ResponseSizeTooLarge"),
            "case {case:?}: expected the response-size failure, got {}",
            answer.text()
        );
        let expected = format!(
            "Response payload size exceeded maximum allowed payload size ({RESPONSE_QUOTA} \
             bytes)."
        );
        assert_eq!(
            document.get("errorMessage").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {}",
            answer.text()
        );
    }
}
