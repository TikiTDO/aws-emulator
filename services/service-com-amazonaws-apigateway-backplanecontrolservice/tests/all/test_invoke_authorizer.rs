// SPDX-License-Identifier: CC0-1.0

//! `TestInvokeAuthorizer`: what simulating one stored `Authorizer` of an api reports.
//!
//! The rule under test, one level above the recorded case:
//!
//! > A test invocation EXECUTES the authorizer the request names, by the same steps a
//! > request to a deployed stage executes it by — the identity source read off the simulated
//! > request, the Lambda function the authorizer names invoked with the event its own type
//! > documents, and the policy it returned judged against the ARN of the method the
//! > simulation addresses (the api's own, under the stage AWS reserves for a simulated
//! > authorizer execution) — and reports the decision: `clientStatus` 0 for every authorizer
//! > that PRODUCED a decision, whichever way its policy decided, and otherwise the status of
//! > the gateway response the client would have been answered through, beside the principal
//! > the authorizer named, the policy it returned and the values it published. `claims` is a
//! > user-pool authorizer's own member, so a Lambda-backed authorizer's report carries none
//! > of it. The execution log reports what the authorizer RETURNED — the principal it named
//! > and every value it published — and never the identity the request presented. An
//! > identifier no record carries is `NotFoundException`.
//!
//! The rows differ along what the rule generalises over:
//!
//! * the DECISION the returned policy takes about the simulated method — an allow naming
//!   that method, an allow naming every method, an explicit deny, and an allow naming
//!   another method: each of the four is an authorizer that RAN, so each is reported at
//!   `clientStatus` 0 with the document it returned;
//! * each way a call earns no decision at all — an identity source the simulated request
//!   does not carry (the function is not invoked), the `Unauthorized` error API Gateway
//!   documents a refusal by, a function that fails otherwise, a function that returns no
//!   authorizer output, and an authorizer whose function grants API Gateway no invocation of
//!   it;
//! * both Lambda authorizer TYPES, `TOKEN` and `REQUEST`, which differ in the event the
//!   function is handed — every row reads the event's own `type` back out of the published
//!   values, so each asserts which event its function saw;
//! * WHERE the identity travels: a header, and a query-string parameter of
//!   `pathWithQueryString`;
//! * the PATH the simulation addresses, which is what the method ARN the policy is judged
//!   against names.
//!
//! Only a cross-service suite can pin this: the api and the authorizer record are API
//! Gateway's, while the function, its code and its resource policy are AWS Lambda's — both on
//! ONE engine, as `core/router/src/lib.rs` mounts them.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for and every ARN names.
const REGION: &str = "us-east-1";

/// The function every authorizer of the api names, and the role a created function carries.
const FUNCTION: &str = "simulated-authorizer";
const ROLE: &str = "arn:aws:iam::000000000000:role/lambda-exec";
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long this suite waits for Lambda's own clock-owed transition (a created function
/// becoming `Active`), and how often it looks.
const WAIT: Duration = Duration::from_secs(20);
const LOOK: Duration = Duration::from_millis(100);

/// The header the header-sourced authorizers declare as their identity source, and the
/// query-string parameter the query-sourced one declares.
const IDENTITY_HEADER: &str = "Authorization";
const IDENTITY_PARAMETER: &str = "access_token";

/// `Authorizer$authorizerResultTtlInSeconds` the authorizers are created with: long enough
/// that a simulation reusing a held result would be observable.
const HELD_TTL: u32 = 300;

/// The stage segment of the method ARN a simulated authorizer execution synthesizes: AWS's
/// reserved test-invoke stage, as `TestInvokeAuthorizer` spells it in the `methodArn` it hands
/// the authorizer.
const TEST_INVOKE_ARN_STAGE: &str = "ESTestInvoke-stage";

/// The one authorizer function every row is decided by: it reads the token out of whichever
/// event it was handed, chooses what to return from that token, and publishes a value that is
/// different on every invocation — which is what tells one invocation from another.
const HANDLER_SOURCE: &str = r#"
import time


def lambda_handler(event, context):
    kind = event["type"]
    token = event.get("authorizationToken")
    if token is None:
        headers = event.get("headers") or {}
        token = headers.get("Authorization") or ""
        if not token:
            query = event.get("queryStringParameters") or {}
            token = query.get("access_token") or ""
    if token == "raise-unauthorized":
        raise Exception("Unauthorized")
    if token == "raise-other":
        raise Exception("the authorizer itself failed")
    if token == "malformed":
        return {"what": "this is not an authorizer output"}
    resource = event["methodArn"]
    effect = "Allow"
    if token.startswith("deny"):
        effect = "Deny"
    if token.startswith("other"):
        resource = resource.rsplit("/", 1)[0] + "/elsewhere"
    if token.startswith("star"):
        resource = "*"
    return {
        "principalId": "principal-" + token,
        "policyDocument": {
            "Version": "2012-10-17",
            "Statement": [
                {"Action": "execute-api:Invoke", "Effect": effect, "Resource": resource}
            ],
        },
        "context": {"ran": str(time.time_ns()), "kind": kind},
    }
"#;

/// One answered request: its status and its body verbatim.
struct Answered {
    status: u16,
    body: Bytes,
}

impl Answered {
    /// The body as text.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The body as JSON, `Null` when it is not JSON at all.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// The two services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts
/// them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite exercises the services, not SigV4: the calls carry a credential scope so the
    // router can name the target service, and the signature itself is never checked here.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
        .register_service(service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(
            engine,
        ));
    router
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    signing_name: &str,
    body: Bytes,
) -> Answered {
    let builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", authorization(signing_name));
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answered {
        status: parts.status.as_u16(),
        body,
    }
}

/// One API Gateway control-plane call.
async fn apigw(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answered {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    dispatch(
        router,
        method,
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        "apigateway",
        Bytes::from(payload),
    )
    .await
}

/// One API Gateway control-plane call that has to be accepted with `expected`.
async fn accepted(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let answered = apigw(router, method, path, body).await;
    assert_eq!(
        answered.status,
        expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// One AWS Lambda call over its `restJson1` wire.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answered = dispatch(
        router,
        method,
        &format!("lambda.{REGION}.amazonaws.com"),
        path,
        "lambda",
        Bytes::from(payload),
    )
    .await;
    (answered.status, answered.json())
}

/// A .zip deployment package holding `source` as the module the handler names.
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

/// The member `name` of a JSON object, as text.
fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`.
async fn wait_until_active(router: &MultiServiceRouter) {
    let looks = WAIT.as_millis() / LOOK.as_millis().max(1);
    let mut last = Value::Null;
    for _ in 0..looks {
        let (_, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            None,
        )
        .await;
        if configuration.get("State").and_then(Value::as_str) == Some("Active") {
            return;
        }
        last = configuration;
        tokio::time::sleep(LOOK).await;
    }
    panic!("setup CreateFunction: the function never became Active, last read {last}");
}

/// Which authorizer of the api a row is answered by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    /// A `TOKEN` authorizer sourcing its identity from the header.
    Token,
    /// A `REQUEST` authorizer sourcing its identity from the same header.
    Request,
    /// A `REQUEST` authorizer sourcing its identity from the query string.
    Query,
    /// A `TOKEN` authorizer whose function grants API Gateway no invocation of it.
    Ungranted,
}

/// The api and the four authorizers of it every row below is simulated against.
struct Wired {
    api: String,
    token: String,
    request: String,
    query: String,
    ungranted: String,
}

impl Wired {
    /// The id of the authorizer `which` names.
    fn authorizer(&self, which: Which) -> &str {
        match which {
            Which::Token => &self.token,
            Which::Request => &self.request,
            Which::Query => &self.query,
            Which::Ungranted => &self.ungranted,
        }
    }
}

/// The function, the api and its authorizers: three of them granted an invocation of the
/// function, one of them not.
async fn wired(router: &MultiServiceRouter) -> Wired {
    let (status, function) = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup CreateFunction: expected HTTP 201, got {status} with {function}"
    );
    let function_arn = member("CreateFunction", &function, "FunctionArn");
    let authorizer_uri = format!(
        "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/{function_arn}/invocations"
    );

    let api = accepted(
        router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "simulated-authorizer-api" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &api, "id");

    let mut created: Vec<String> = Vec::new();
    // `(name, type, identitySource, ttl, granted)` — the four authorizers the rows span.
    let declared: &[(&str, &str, String, u32, bool)] = &[
        (
            "header-token",
            "TOKEN",
            format!("method.request.header.{IDENTITY_HEADER}"),
            HELD_TTL,
            true,
        ),
        (
            "header-request",
            "REQUEST",
            format!("method.request.header.{IDENTITY_HEADER}"),
            HELD_TTL,
            true,
        ),
        (
            "query-request",
            "REQUEST",
            format!("method.request.querystring.{IDENTITY_PARAMETER}"),
            0,
            true,
        ),
        (
            "ungranted-token",
            "TOKEN",
            format!("method.request.header.{IDENTITY_HEADER}"),
            HELD_TTL,
            false,
        ),
    ];
    for (name, kind, source, ttl, granted) in declared {
        let authorizer = accepted(
            router,
            name,
            "POST",
            &format!("/restapis/{api}/authorizers"),
            Some(json!({
                "name": name,
                "type": kind,
                "authorizerUri": authorizer_uri,
                "identitySource": source,
                "authorizerResultTtlInSeconds": ttl,
            })),
            201,
        )
        .await;
        let authorizer = member(name, &authorizer, "id");
        // The authorizer is read back through the control plane's own reader, so the record
        // the simulation executes is the one a client can see.
        let read = accepted(
            router,
            name,
            "GET",
            &format!("/restapis/{api}/authorizers/{authorizer}"),
            None,
            200,
        )
        .await;
        assert_eq!(
            member(name, &read, "type"),
            (*kind).to_string(),
            "setup {name:?}: expected the stored authorizer to carry type {kind:?}, got {read}"
        );
        if *granted {
            // "Grants a principal permission to use a function" (`AddPermission`), scoped by
            // `SourceArn` to the AUTHORIZER whose function is invoked — which is what the
            // ungranted authorizer has none of.
            let (status, policy) = lambda(
                router,
                "POST",
                &format!("/2015-03-31/functions/{FUNCTION}/policy"),
                Some(json!({
                    "StatementId": format!("apigateway-invoke-{name}"),
                    "Action": "lambda:InvokeFunction",
                    "Principal": "apigateway.amazonaws.com",
                    "SourceArn": format!(
                        "arn:aws:execute-api:{REGION}:{account}:{api}/authorizers/{authorizer}",
                        account = engine::emulator_account_id(),
                    ),
                })),
            )
            .await;
            assert_eq!(
                status, 201,
                "setup AddPermission({name:?}): expected HTTP 201, got {status} with {policy}"
            );
        }
        created.push(authorizer);
    }
    wait_until_active(router).await;
    Wired {
        api,
        token: created[0].clone(),
        request: created[1].clone(),
        query: created[2].clone(),
        ungranted: created[3].clone(),
    }
}

/// A `headers` map carrying the identity header, as a simulated request carries one.
fn presenting(token: &str) -> Value {
    let mut headers = serde_json::Map::new();
    headers.insert(IDENTITY_HEADER.to_string(), json!(token));
    Value::Object(headers)
}

/// Simulate `which` authorizer of the api with the identity header `header` and `path`.
async fn simulate(
    router: &MultiServiceRouter,
    wired: &Wired,
    which: Which,
    header: Option<&str>,
    path: &str,
) -> Answered {
    let mut body = serde_json::Map::new();
    body.insert("pathWithQueryString".to_string(), json!(path));
    if let Some(token) = header {
        body.insert("headers".to_string(), presenting(token));
    }
    apigw(
        router,
        "POST",
        &format!(
            "/restapis/{api}/authorizers/{authorizer}",
            api = wired.api,
            authorizer = wired.authorizer(which)
        ),
        Some(Value::Object(body)),
    )
    .await
}

/// One row: the authorizer, what the simulated request carries and what the simulation has
/// to report.
struct Row {
    case: &'static str,
    which: Which,
    /// The value the identity header carries, or `None` for a request carrying no header.
    header: Option<&'static str>,
    /// The identity the simulated request presents, whichever member carries it — the
    /// principal the authorizer names is `principal-` and this.
    identity: &'static str,
    /// `pathWithQueryString`.
    path: &'static str,
    /// What the `Resource` of the returned policy names: `{method}` for the ARN of the
    /// method the simulation addresses, `{elsewhere}` for another method of the same api,
    /// and a literal for anything else. `None` where the authorizer returned no policy.
    resource: Option<&'static str>,
    /// The `clientStatus` the simulation reports: `0` for an authorizer that produced a
    /// decision, and the status of the gateway response the client would have met for a call
    /// that earned none.
    client_status: i64,
    /// Whether the authorizer RETURNED a policy, which is reported whichever way it decided.
    returned: bool,
    /// The `type` member of the event the function saw, read back out of the published
    /// values; `None` for a row whose function was not invoked at all.
    event: Option<&'static str>,
    /// A phrase the execution log has to carry, naming why the call earned no decision.
    log: Option<&'static str>,
}

/// Every way a simulation of a Lambda authorizer can go.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a policy allowing the simulated method",
            which: Which::Token,
            header: Some("allow-me"),
            identity: "allow-me",
            path: "/",
            resource: Some("{method}"),
            client_status: 0,
            returned: true,
            event: Some("TOKEN"),
            log: None,
        },
        Row {
            case: "a policy allowing the simulated method on a deeper path",
            which: Which::Token,
            header: Some("allow-me"),
            identity: "allow-me",
            path: "/orders/42",
            resource: Some("{method}"),
            client_status: 0,
            returned: true,
            event: Some("TOKEN"),
            log: None,
        },
        Row {
            case: "a policy allowing every method",
            which: Which::Token,
            header: Some("star-me"),
            identity: "star-me",
            path: "/orders",
            resource: Some("*"),
            client_status: 0,
            returned: true,
            event: Some("TOKEN"),
            log: None,
        },
        Row {
            case: "an explicit deny",
            which: Which::Token,
            header: Some("deny-me"),
            identity: "deny-me",
            path: "/orders",
            resource: Some("{method}"),
            // The function RAN and returned this document, so the authorizer succeeded: the
            // deny is the decision's content, and the log is where the gateway response that
            // call would have met is named.
            client_status: 0,
            returned: true,
            event: Some("TOKEN"),
            log: Some("does not allow"),
        },
        Row {
            case: "a policy naming another method",
            which: Which::Token,
            header: Some("other-me"),
            identity: "other-me",
            path: "/orders",
            resource: Some("{elsewhere}"),
            client_status: 0,
            returned: true,
            event: Some("TOKEN"),
            log: Some("does not allow"),
        },
        Row {
            case: "the function refusing with the Unauthorized error",
            which: Which::Token,
            header: Some("raise-unauthorized"),
            identity: "raise-unauthorized",
            path: "/",
            resource: None,
            client_status: 401,
            returned: false,
            event: None,
            log: Some("Unauthorized"),
        },
        Row {
            case: "the function failing otherwise",
            which: Which::Token,
            header: Some("raise-other"),
            identity: "raise-other",
            path: "/",
            resource: None,
            client_status: 500,
            returned: false,
            event: None,
            log: Some("errored"),
        },
        Row {
            case: "the function returning no authorizer output",
            which: Which::Token,
            header: Some("malformed"),
            identity: "malformed",
            path: "/",
            resource: None,
            client_status: 500,
            returned: false,
            event: None,
            log: Some("returned no principalId member"),
        },
        Row {
            case: "an identity source the simulated request does not carry",
            which: Which::Token,
            header: None,
            identity: "",
            path: "/",
            resource: None,
            client_status: 401,
            returned: false,
            event: None,
            log: Some("is not carried by the simulated request"),
        },
        Row {
            case: "a REQUEST authorizer, which sees the whole request",
            which: Which::Request,
            header: Some("allow-me"),
            identity: "allow-me",
            path: "/orders",
            resource: Some("{method}"),
            client_status: 0,
            returned: true,
            event: Some("REQUEST"),
            log: None,
        },
        Row {
            case: "an identity travelling in the query string",
            which: Which::Query,
            header: None,
            identity: "allow-me",
            path: "/orders?access_token=allow-me",
            resource: Some("{method}"),
            client_status: 0,
            returned: true,
            event: Some("REQUEST"),
            log: None,
        },
        Row {
            case: "a query-string identity source the request does not carry",
            which: Which::Query,
            header: None,
            identity: "",
            path: "/orders",
            resource: None,
            client_status: 401,
            returned: false,
            event: None,
            log: Some("is not carried by the simulated request"),
        },
        Row {
            case: "an authorizer whose function grants API Gateway no invocation",
            which: Which::Ungranted,
            header: Some("allow-me"),
            identity: "allow-me",
            path: "/",
            resource: None,
            client_status: 500,
            returned: false,
            event: None,
            log: Some("no invocation was made"),
        },
    ]
}

/// A simulation executes the authorizer and reports the decision its execution produced.
#[tokio::test]
async fn a_simulation_reports_the_decision_the_authorizer_produced() {
    let router = emulator();
    let wired = wired(&router).await;
    for row in rows() {
        let case = row.case;
        let answered = simulate(&router, &wired, row.which, row.header, row.path).await;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: expected the simulation to be answered HTTP 200, got {} with {:?}",
            answered.status,
            answered.text()
        );
        let reported = answered.json();
        let status = reported.get("clientStatus").and_then(Value::as_i64);
        assert_eq!(
            status,
            Some(row.client_status),
            "case {case:?}: expected clientStatus {}, got {reported}",
            row.client_status
        );

        let policy = reported
            .get("policy")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let principal = reported
            .get("principalId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            (!policy.is_empty(), !principal.is_empty()),
            (row.returned, row.returned),
            "case {case:?}: expected the authorizer's own return value reported={}, got \
             {reported}",
            row.returned
        );
        if row.returned {
            // The ARN the policy was judged against is the one the function was handed: the
            // api's own method ARN under the stage AWS reserves for a test invocation,
            // naming the path the simulation addressed.
            let method_arn = format!(
                "arn:aws:execute-api:{REGION}:{account}:{api}/{TEST_INVOKE_ARN_STAGE}/GET/{path}",
                account = engine::emulator_account_id(),
                api = wired.api,
                path = row
                    .path
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .trim_start_matches('/'),
            );
            let expected = match row.resource.unwrap_or_default() {
                "{method}" => method_arn.clone(),
                "{elsewhere}" => format!(
                    "{head}/elsewhere",
                    head = method_arn
                        .rsplit_once('/')
                        .map(|(head, _tail)| head)
                        .unwrap_or_default()
                ),
                literal => literal.to_string(),
            };
            assert!(
                policy.contains(&expected),
                "case {case:?}: expected the returned policy to name {expected:?}, got \
                 {policy:?}"
            );
            assert_eq!(
                principal,
                format!("principal-{identity}", identity = row.identity),
                "case {case:?}: expected the principal the authorizer named, got {principal:?}"
            );
        }

        // "The authorization response": the values the authorizer published, which is where
        // the event's own `type` is read back from — so each row asserts which event its
        // function saw.
        let authorization = reported.get("authorization").cloned().unwrap_or(Value::Null);
        let event_type = authorization
            .get("kind")
            .and_then(Value::as_array)
            .and_then(|values| values.first())
            .and_then(Value::as_str);
        assert_eq!(
            event_type,
            row.event,
            "case {case:?}: expected the function to have seen a {:?} event, got \
             {authorization}",
            row.event
        );

        let log = reported
            .get("log")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            log.contains("Execution log for request"),
            "case {case:?}: expected an execution log, got {log:?}"
        );
        if let Some(phrase) = row.log {
            assert!(
                log.contains(phrase),
                "case {case:?}: expected the execution log to carry {phrase:?}, got {log:?}"
            );
        }
        // What the authorizer RETURNED is reported in the log: the principal it named and
        // every value it published, each read back out of the report itself — which is what a
        // developer simulating an execution reads its result from.
        if row.returned {
            assert!(
                log.contains(&principal),
                "case {case:?}: expected the execution log to name the principal \
                 {principal:?}, got {log:?}"
            );
            let published = authorization
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: no published values in {reported}"));
            for (name, values) in published {
                let value = values
                    .as_array()
                    .and_then(|values| values.first())
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert!(
                    log.contains(value),
                    "case {case:?}: expected the execution log to carry the published \
                     {name}={value:?}, got {log:?}"
                );
            }
        }
        assert!(
            reported.get("latency").and_then(Value::as_i64).is_some(),
            "case {case:?}: expected a measured latency, got {reported}"
        );
        // `claims` is the member a COGNITO_USER_POOLS authorizer's accepted identity token
        // fills, so every row here — all of them Lambda-backed — carries none of it.
        assert!(
            reported.get("claims").is_none(),
            "case {case:?}: expected no claims member for a Lambda-backed authorizer, got \
             {reported}"
        );
        // A caller's credential never travels into the lines the emulator writes about the
        // REQUEST: those report the header NAMES and not the value presented under one. (The
        // lines reporting what the authorizer RETURNED carry the authorizer's own output,
        // which a function is free to derive from the token it was handed.)
        if let Some(token) = row.header {
            let about_the_request: String = log
                .lines()
                .filter(|line| !line.contains("returned"))
                .collect::<Vec<&str>>()
                .join("\n");
            assert!(
                !about_the_request.contains(token),
                "case {case:?}: expected the presented token to stay out of what the \
                 execution log says about the request, got {about_the_request:?}"
            );
        }
    }
}

/// A simulation holds nothing: the authorizer is executed afresh for every one of them, even
/// where its own `authorizerResultTtlInSeconds` would hold a result for a deployed stage.
#[tokio::test]
async fn a_simulation_holds_no_result_for_a_later_call() {
    let router = emulator();
    let wired = wired(&router).await;
    let mut ran: Vec<String> = Vec::new();
    // The same identity twice, against the authorizer that holds its results for a stage.
    for attempt in ["first", "second"] {
        let answered = simulate(&router, &wired, Which::Token, Some("allow-me"), "/orders").await;
        assert_eq!(
            answered.status,
            200,
            "case {attempt:?}: expected HTTP 200, got {} with {:?}",
            answered.status,
            answered.text()
        );
        let reported = answered.json();
        let marker = reported
            .get("authorization")
            .and_then(|published| published.get("ran"))
            .and_then(Value::as_array)
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {attempt:?}: no per-invocation marker in {reported}"))
            .to_string();
        ran.push(marker);
    }
    assert_ne!(
        ran[0], ran[1],
        "case \"the same identity twice\": expected the authorizer executed afresh for each \
         simulation, got the same invocation marker {:?}",
        ran[0]
    );
}

/// An identifier no record of the account carries is `NotFoundException`, naming the record
/// the call could not resolve.
#[tokio::test]
async fn an_unresolvable_identifier_is_not_found() {
    let router = emulator();
    let wired = wired(&router).await;
    let account = engine::emulator_account_id();
    // Rows differ along WHICH identifier of the two the request addresses resolves to
    // nothing.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "an api of no record",
            format!("/restapis/zzzz999999/authorizers/{id}", id = wired.token),
            format!("Invalid API identifier specified {account}:zzzz999999"),
        ),
        (
            "an authorizer of no record",
            format!("/restapis/{api}/authorizers/zz9999", api = wired.api),
            "Invalid Authorizer identifier specified".to_string(),
        ),
    ];
    for (case, path, expected) in rows {
        let answered = apigw(&router, "POST", &path, Some(json!({ "pathWithQueryString": "/" })))
            .await;
        assert_eq!(
            answered.status,
            404,
            "case {case:?}: expected HTTP 404, got {} with {:?}",
            answered.status,
            answered.text()
        );
        let reported = answered.json();
        let message = reported
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {reported}"
        );
    }
}
