// SPDX-License-Identifier: CC0-1.0

//! What `Invoke` says it could not find: `POST
//! /2015-03-31/functions/{FunctionName}/invocations`.
//!
//! The rule under test: **the resource an INVOCATION addresses is a function AT a version —
//! `Qualifier` "specif[ies] a version or alias to invoke a published version of the
//! function", and an invocation naming none runs the unpublished version, which the answer
//! reports as `$LATEST` — so the `ResourceNotFoundException` for a request that resolves to
//! nothing names the ARN the invocation RESOLVED, qualifier included:
//! `Function not found: arn:aws:lambda:<region>:<account>:function:<name>:<qualifier>`.** The
//! fixed text is `Function not found: `; the variable part is built from the request — the
//! function the name addresses, the Region the name named when it carried one, and the
//! qualifier it resolved, whether that qualifier was named in the name, in the `Qualifier`
//! parameter, or by neither.
//!
//! The rows differ along what the rule generalises over: the documented `FunctionName` form
//! the request addresses the function by (the bare name, the full ARN, the partial ARN, the
//! `function:` prefix form, a name carrying its own qualifier), where the qualifier is named
//! (the name, the `Qualifier` parameter, nowhere), which qualifier it is (the unpublished
//! version, a published version number, an alias name), what is missing (the function
//! itself, a version of a function that does exist, a Region this endpoint does not serve),
//! and the `InvocationType` the request carries — the resolution happens before any of the
//! three is acted on, so each row is driven through all of them. Two controls keep the table
//! honest: a request that resolves is NOT refused, and the refusal for a function that
//! exists under a version that does not carries that version rather than `$LATEST`. A Region
//! this endpoint does not serve is settled BEFORE a version is, so its refusal resolves no
//! version to name and states the two Regions that disagree instead:
//! `Functions from '<named region>' are not reachable in this region ('<endpoint region>')`.

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

/// Another Region, to address this endpoint's own function through: a function is regional,
/// so an ARN naming this Region's function from another Region names nothing here.
const OTHER_REGION: &str = "us-west-2";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The label the unpublished version is stored under, which is also the qualifier an
/// invocation that names none resolves.
const LATEST: &str = "$LATEST";

/// The `StatusCode` a `DryRun` invocation answers with: "For the DryRun invocation type, the
/// status code is 204."
const DRY_RUN_STATUS: u16 = 204;

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
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
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

/// `Invoke` on the function `reference` names.
async fn invoke(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
    invocation_type: Option<&str>,
) -> Answer {
    let query = qualifier.map_or_else(String::new, |value| format!("?Qualifier={value}"));
    let uri = format!("{FUNCTIONS_URI}/{reference}/invocations{query}");
    let headers: Vec<(&str, &str)> = invocation_type
        .map(|value| vec![("x-amz-invocation-type", value)])
        .unwrap_or_default();
    call(router, "POST", &uri, b"{}", &headers).await
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

/// One row: how the request addresses a function that cannot be resolved, and the ARN the
/// refusal must name.
struct NotFoundRow {
    case: &'static str,
    /// The `FunctionName` path label, built from the name of the function that EXISTS.
    reference: fn(&str) -> String,
    /// The `Qualifier` query parameter the request carries, if any.
    qualifier: Option<&'static str>,
    /// The qualifier the invocation resolves, which the refused ARN must carry — absent
    /// for a name no version is resolved for at all, which is an ARN of another Region:
    /// that is refused on the Region, before a version is resolved.
    resolved_qualifier: Option<&'static str>,
    /// The Region the refusal must name: the one the reference itself named when it
    /// carried one, else this endpoint's.
    addressed_region: &'static str,
    /// The function the refused ARN must name — the one the request addressed, which for the
    /// version rows is the function that exists.
    function: &'static str,
}

/// The function every row is written against: it EXISTS, with version `1` and the alias
/// `live`, so a row that is refused is refused for the version it named rather than for the
/// function.
const HELD: &str = "invoke-held";

/// A name no function of this account is held under.
const ABSENT: &str = "invoke-absent";

fn not_found_rows() -> Vec<NotFoundRow> {
    vec![
        NotFoundRow {
            case: "a function that does not exist, addressed by the bare name",
            reference: |_| ABSENT.to_string(),
            qualifier: None,
            resolved_qualifier: Some(LATEST),
            addressed_region: REGION,
            function: ABSENT,
        },
        NotFoundRow {
            case: "a function arn that names no function",
            reference: |_| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{ABSENT}"),
            qualifier: None,
            resolved_qualifier: Some(LATEST),
            addressed_region: REGION,
            function: ABSENT,
        },
        NotFoundRow {
            case: "a partial arn that names no function",
            reference: |_| format!("{ACCOUNT}:function:{ABSENT}"),
            qualifier: None,
            resolved_qualifier: Some(LATEST),
            addressed_region: REGION,
            function: ABSENT,
        },
        NotFoundRow {
            case: "the function: prefix form naming no function",
            reference: |_| format!("function:{ABSENT}"),
            qualifier: None,
            resolved_qualifier: Some(LATEST),
            addressed_region: REGION,
            function: ABSENT,
        },
        NotFoundRow {
            case: "a missing function with the qualifier named in the Qualifier parameter",
            reference: |_| ABSENT.to_string(),
            qualifier: Some("7"),
            resolved_qualifier: Some("7"),
            addressed_region: REGION,
            function: ABSENT,
        },
        NotFoundRow {
            case: "a missing function with the qualifier named in the name",
            reference: |_| format!("{ABSENT}:live"),
            qualifier: None,
            resolved_qualifier: Some("live"),
            addressed_region: REGION,
            function: ABSENT,
        },
        NotFoundRow {
            case: "a version a function that exists has not published",
            reference: |held| held.to_string(),
            qualifier: Some("9"),
            resolved_qualifier: Some("9"),
            addressed_region: REGION,
            function: HELD,
        },
        NotFoundRow {
            case: "an alias a function that exists does not hold",
            reference: |held| held.to_string(),
            qualifier: Some("absent-alias"),
            resolved_qualifier: Some("absent-alias"),
            addressed_region: REGION,
            function: HELD,
        },
        NotFoundRow {
            case: "a version named in the function name that does not exist",
            reference: |held| format!("{held}:9"),
            qualifier: None,
            resolved_qualifier: Some("9"),
            addressed_region: REGION,
            function: HELD,
        },
        // A function this endpoint DOES hold, addressed through another Region's ARN: the
        // function is regional, so it is not reachable from here. The Region is settled
        // before a version is, so no version is resolved and the refusal states the two
        // Regions instead of naming a resolved ARN.
        NotFoundRow {
            case: "a held function addressed through another Region",
            reference: |held| format!("arn:aws:lambda:{OTHER_REGION}:{ACCOUNT}:function:{held}"),
            qualifier: None,
            resolved_qualifier: None,
            addressed_region: OTHER_REGION,
            function: HELD,
        },
        NotFoundRow {
            case: "a held function addressed through another Region at a version",
            reference: |held| {
                format!("arn:aws:lambda:{OTHER_REGION}:{ACCOUNT}:function:{held}:1")
            },
            qualifier: None,
            resolved_qualifier: None,
            addressed_region: OTHER_REGION,
            function: HELD,
        },
    ]
}

/// RULE: `Invoke`'s `ResourceNotFoundException` names the ARN the invocation resolved,
/// qualifier included.
#[tokio::test]
async fn the_invoke_refusal_names_the_qualified_arn_the_invocation_resolved() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, HELD).await;

    for row in not_found_rows() {
        let case = row.case;
        let reference = (row.reference)(HELD);
        // A reference naming another Region is refused on the Region before any version is
        // resolved, so there is no resolved ARN to name: those rows resolve no qualifier
        // and are refused on the two Regions that disagree instead.
        let expected = match row.resolved_qualifier {
            Some(qualifier) => format!(
                "Function not found: arn:aws:lambda:{}:{ACCOUNT}:function:{}:{qualifier}",
                row.addressed_region, row.function
            ),
            None => format!(
                "Functions from '{}' are not reachable in this region ('{REGION}')",
                row.addressed_region
            ),
        };
        // The resolution happens before the invocation type is acted on, so every type is
        // refused the same way — including the asynchronous one, which never queues an
        // event for a resource that does not exist.
        for invocation_type in [None, Some("DryRun"), Some("Event"), Some("RequestResponse")] {
            let answer = invoke(&router, &reference, row.qualifier, invocation_type).await;
            assert_eq!(
                answer.status,
                404,
                "case {case:?} ({invocation_type:?}): expected the modelled 404, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {case:?} ({invocation_type:?}): expected ResourceNotFoundException, got \
                 {}",
                answer.error_code()
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?} ({invocation_type:?}): expected {expected:?}, got {:?}",
                answer.message()
            );
        }
    }
}

/// The CONTROL for the table above: an invocation that RESOLVES is not refused at all, in
/// every form the rows address a function by — so no row passes against a handler that
/// refuses everything.
#[tokio::test]
async fn an_invocation_that_resolves_is_not_refused_as_not_found() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, HELD).await;

    for (case, reference, qualifier) in [
        ("the bare name", HELD.to_string(), None),
        (
            "the full function arn",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{HELD}"),
            None,
        ),
        (
            "the partial arn",
            format!("{ACCOUNT}:function:{HELD}"),
            None,
        ),
        ("a published version", HELD.to_string(), Some("1")),
        ("an alias", HELD.to_string(), Some("live")),
        ("a version named in the name", format!("{HELD}:1"), None),
    ] {
        // `DryRun` "validate[s] parameter values and verif[ies] that the user or role has
        // permission to invoke the function" and runs nothing, so it is the invocation type
        // whose answer depends only on the resolution.
        let answer = invoke(&router, &reference, qualifier, Some("DryRun")).await;
        assert_eq!(
            answer.status,
            DRY_RUN_STATUS,
            "case {case:?}: expected the dry run to be served, got {} {}",
            answer.status,
            answer.text()
        );
    }
}
