// SPDX-License-Identifier: CC0-1.0

//! An HTTP api route whose integration is a FIRST-CLASS STEP FUNCTIONS ACTION — `AWS_PROXY`
//! naming the `StepFunctions-StartExecution` or `StepFunctions-StartSyncExecution`
//! `IntegrationSubtype` — performed end to end: this service, Step Functions, IAM and the
//! token service on ONE engine, with the run read back where a client reads it.
//!
//! The RULE every table below is one slice of: **an HTTP api route whose integration names a
//! Step Functions start subtype starts that workflow on every matching request, with each of
//! the action's parameters taken from the integration's `RequestParameters` — one mapping
//! expression over the request, the invocation's context or the stage's variables, or a static
//! value — issued as a session of the role its `CredentialsArn` names, so Step Functions
//! decides on that role's own policies whether the run happens; and the caller is answered
//! that action's own JSON document (`200`), or the refusal Step Functions itself stated,
//! relayed.**
//!
//! Nothing asserted here is a value this emulator fills in: every assertion is either on the
//! document Step Functions answered or on what THAT service holds afterwards, read back
//! through its own `DescribeExecution` — which is why it is mounted beside this one.
//!
//! The steps BEFORE the integration (the claim, stage resolution, the deployment gate, route
//! matching, the authorization gate and the payload quota) are owned by
//! `http_api_servable_stage.rs` and its siblings and are not restated here, and so are the
//! mapping-expression catalog and the role-session rules, owned by
//! `http_api_service_action_integration.rs`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_form::form_encode;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The access-key id standing for the account's own published credential: neither the token
/// service nor IAM holds a record of it, so every request this suite makes on its own behalf
/// speaks for the account's ambient identity.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The inline policy name this suite writes a role's grant under.
const INLINE_POLICY: &str = "inline";

/// The role every state machine below acts as, and the service principal it trusts.
const WORKFLOW_ROLE: &str = "step-functions-workflow";
const STATES_PRINCIPAL: &str = "states.amazonaws.com";

/// The service principal an integration role trusts, which is the one API Gateway assumes a
/// `CredentialsArn` role as.
const GATEWAY_PRINCIPAL: &str = "apigateway.amazonaws.com";

/// The two `IntegrationSubtype` values under test, exactly as `Integration$IntegrationSubtype`
/// spells the action each names.
const START_EXECUTION: &str = "StepFunctions-StartExecution";
const START_SYNC_EXECUTION: &str = "StepFunctions-StartSyncExecution";

/// The IAM actions a statement on an integration role names for them
/// (`<arnNamespace>:<OperationName>`).
const START_EXECUTION_ACTION: &str = "states:StartExecution";
const START_SYNC_EXECUTION_ACTION: &str = "states:StartSyncExecution";

/// A definition whose single `Pass` state passes its input straight through, so the output of
/// a run is the input it ran on.
const ECHO_DEFINITION: &str = r#"{"StartAt":"Echo","States":{"Echo":{"Type":"Pass","End":true}}}"#;

/// A definition whose single `Fail` state ends the run with the error and cause it names.
const FAIL_DEFINITION: &str = r#"{"StartAt":"Boom","States":{"Boom":{"Type":"Fail",
    "Error":"OrderRejected","Cause":"the order was rejected"}}}"#;

/// The error and cause [`FAIL_DEFINITION`] ends with.
const FAIL_ERROR: &str = "OrderRejected";
const FAIL_CAUSE: &str = "the order was rejected";

/// The status and media type a performed action's document is answered under.
const PERFORMED: u16 = 200;
const JSON_MEDIA_TYPE: &str = "application/json";

/// The status and sentence an HTTP api answers for an integration that produced no response,
/// and for a request that cannot satisfy what its route's integration maps onto a parameter
/// the action requires.
const INTEGRATION_FAULT: u16 = 500;
const INTEGRATION_FAULT_MESSAGE: &str = "Internal Server Error";
const BAD_REQUEST: u16 = 400;
const BAD_REQUEST_MESSAGE: &str = "Bad Request";

/// The status Step Functions answers every client refusal under, which is what a relayed
/// refusal carries.
const CLIENT_REFUSAL: u16 = 400;

/// The errors Step Functions names for the refusals this suite drives: the AWS-common
/// authorization refusal, the type gate on a synchronous start
/// (`smithy_query(query="operation", operation="StartSyncExecution")` lists
/// `StateMachineTypeNotSupported`), and the read of an execution nothing started
/// (`DescribeExecution` lists `ExecutionDoesNotExist`).
const ACCESS_DENIED: &str = "AccessDeniedException";
const TYPE_NOT_SUPPORTED: &str = "StateMachineTypeNotSupported";
const EXECUTION_DOES_NOT_EXIST: &str = "ExecutionDoesNotExist";

/// Another Region of the same partition, which a row below maps for the one parameter every
/// first-class subtype may map.
const ANOTHER_REGION: &str = "us-west-2";

/// The error Step Functions names for an ARN that is not a resource of the endpoint's Region
/// (`smithy_query(query="operation", operation="StartExecution")` lists `InvalidArn`).
const INVALID_ARN: &str = "InvalidArn";

/// The member a relayed refusal names the error SHAPE under, and the one it states the
/// service's own detail text under.
const SHAPE_MEMBER: &str = "__type";
const DETAIL_MEMBER: &str = "Message";

/// How long a poll waits for a STANDARD run to reach a terminal status.
///
/// Generous on purpose: what is being waited for is the CONDITION (a status that is no longer
/// `RUNNING`), re-read until it holds, so a slow or loaded host only takes more turns of the
/// loop.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long one turn of that poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The status Step Functions reports for a run that has not finished.
const RUNNING: &str = "RUNNING";

/// This service, Step Functions, IAM and the token service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the action an integration performs is dispatched back
/// into this same router, so the state machine it starts is the one this suite created.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the Region and the caller's access key id.
        // The INTEGRATION's own call is signed by this emulator, with the session it minted.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// A SigV4 `Authorization` whose credential scope names `service`, which is what routes
    /// the request. The signature is never checked here.
    fn authorization(service: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{service}/\
             aws4_request, SignedHeaders=host, Signature=not-verified"
        )
    }

    /// One Step Functions call (`awsJson1_0`, `AWSStepFunctions.<Operation>`), as its status
    /// and parsed body.
    async fn sfn(&self, operation: &str, body: &Value) -> (u16, Value) {
        let host = format!("states.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", host)
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("AWSStepFunctions.{operation}"))
            .header("authorization", Self::authorization("states"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, parsed(&response.into_body()))
    }

    /// One Step Functions call that must be served.
    async fn sfn_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answered) = self.sfn(operation, body).await;
        assert_eq!(
            status, PERFORMED,
            "case {case:?}: expected {operation} to succeed, got {status} with {answered}"
        );
        answered
    }

    /// One awsQuery call to IAM, as the account's own credential, which must be served.
    async fn iam(&self, case: &str, action: &str, params: &[(&str, String)]) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("https://iam.amazonaws.com/")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", Self::authorization("iam"))
            .body(bytes::Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, PERFORMED,
            "case {case:?}: IAM {action} must succeed, got {status} {answered}"
        );
    }

    /// One control-plane call against this service (`restJson1` under `/v2`), as its status
    /// and parsed body.
    async fn control(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(value.to_string()),
            None => bytes::Bytes::new(),
        };
        let host = format!("apigateway.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header("authorization", Self::authorization("apigateway"))
            .body(payload)
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, parsed(&response.into_body()))
    }

    /// An api carrying one integration per `(route key, integration)` pair and the `$default`
    /// stage with auto-deploy on; answers the host its `ApiEndpoint` names.
    async fn api(&self, case: &str, routes: &[(&str, Value)]) -> String {
        let (status, api) = self
            .control(
                "POST",
                "/v2/apis",
                Some(json!({ "name": "workflow-api", "protocolType": "HTTP" })),
            )
            .await;
        assert_eq!(status, 201, "case {case:?}: CreateApi answered {status} {api}");
        let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
        let host = api["apiEndpoint"]
            .as_str()
            .unwrap_or_default()
            .split_once("://")
            .map(|(_scheme, rest)| rest.to_string())
            .unwrap_or_default();
        for (route_key, integration) in routes {
            let (status, created) = self
                .control(
                    "POST",
                    &format!("/v2/apis/{api_id}/integrations"),
                    Some(integration.clone()),
                )
                .await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreateIntegration answered {status} {created}"
            );
            let integration_id = created["integrationId"].as_str().unwrap_or_default();
            // The subtype must read back: it is what decides which action the route performs.
            assert_eq!(
                created["integrationSubtype"],
                integration["integrationSubtype"],
                "case {case:?}: expected the subtype to be recorded, got {created}"
            );
            let (status, route) = self
                .control(
                    "POST",
                    &format!("/v2/apis/{api_id}/routes"),
                    Some(json!({
                        "routeKey": route_key,
                        "target": format!("integrations/{integration_id}"),
                    })),
                )
                .await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreateRoute answered {status} {route}"
            );
        }
        let (status, stage) = self
            .control(
                "POST",
                &format!("/v2/apis/{api_id}/stages"),
                Some(json!({ "stageName": "$default", "autoDeploy": true })),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateStage answered {status} {stage}"
        );
        host
    }

    /// One client request to a route of the deployed stage, carrying `body`: the invoke the
    /// integration is performed for. No credential — the route's `AuthorizationType` is the
    /// default `NONE`.
    async fn invoke(
        &self,
        host: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, String, Value) {
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let media_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        (status, media_type, parsed(&response.into_body()))
    }

    /// A role trusting `principal`, carrying `statements` as its inline identity policy —
    /// written with IAM's own operations, so the role a session is minted of is the record
    /// `iam:CreateRole` wrote. Answers its ARN.
    async fn role(&self, case: &str, name: &str, principal: &str, statements: Option<Value>) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"Service": principal},
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        self.iam(
            case,
            "CreateRole",
            &[
                ("RoleName", name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        if let Some(statements) = statements {
            let document = json!({ "Version": "2012-10-17", "Statement": statements }).to_string();
            self.iam(
                case,
                "PutRolePolicy",
                &[
                    ("RoleName", name.to_string()),
                    ("PolicyName", INLINE_POLICY.to_string()),
                    ("PolicyDocument", document),
                ],
            )
            .await;
        }
        role_arn(name)
    }

    /// The role every state machine below acts as, created once per emulator.
    async fn workflow_role(&self, case: &str) -> String {
        self.role(case, WORKFLOW_ROLE, STATES_PRINCIPAL, None).await
    }

    /// A state machine of `kind` carrying `definition`; answers its ARN.
    async fn state_machine(&self, case: &str, name: &str, kind: &str, definition: &str) -> String {
        let role = self.workflow_role(case).await;
        let created = self
            .sfn_ok(
                case,
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": definition,
                    "roleArn": role,
                    "type": kind,
                }),
            )
            .await;
        created["stateMachineArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no stateMachineArn in {created}"))
            .to_string()
    }

    /// The execution `execution_arn` names, read back through Step Functions' own
    /// `DescribeExecution` once the run has reached a terminal status.
    ///
    /// The wait is for the CONDITION: the record is re-read until its status is no longer
    /// `RUNNING`, or until a deadline far beyond any span this emulator owes the transition
    /// has passed.
    async fn settled(&self, case: &str, execution_arn: &str) -> Value {
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let described = self
                .sfn_ok(
                    case,
                    "DescribeExecution",
                    &json!({ "executionArn": execution_arn }),
                )
                .await;
            if described["status"] != json!(RUNNING) {
                return described;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected {execution_arn} to reach a terminal status, got \
                 {described}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// One response body as the JSON it is, or the text it is for a body that is not JSON.
fn parsed(body: &[u8]) -> Value {
    if body.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(body).to_string()))
}

/// The ARN of the role `name`, as IAM holds it.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// The integration an api below carries: the first-class subtype `subtype`, performed under
/// `credentials_arn`, with `request_parameters` mapping the action's parameters.
fn subtype_integration(
    subtype: &str,
    credentials_arn: Option<&str>,
    request_parameters: Value,
) -> Value {
    let mut integration = json!({
        "integrationType": "AWS_PROXY",
        "integrationSubtype": subtype,
        "payloadFormatVersion": "1.0",
        "requestParameters": request_parameters,
    });
    if let Some(credentials_arn) = credentials_arn {
        integration["credentialsArn"] = json!(credentials_arn);
    }
    integration
}

/// A statement granting `action` on every resource, which is what an integration's role needs
/// to perform the action the subtype names.
fn grant(action: &str) -> Value {
    json!([{ "Effect": "Allow", "Action": action, "Resource": "*" }])
}

/// The JSON document the string member `member` of `body` carries.
fn document(case: &str, body: &Value, member: &str) -> Value {
    let text = body
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member:?} in {body}"));
    serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("case {case:?}: expected {member:?} to be JSON: {error}"))
}

/// The ARN Step Functions gives one execution of a STANDARD state machine, spelled by the
/// owner of that form rather than by a second copy of it here.
fn standard_execution_arn(machine: &str, execution: &str) -> String {
    service_com_amazonaws_sfn_awsstepfunctions::state::execution_arn(REGION, machine, execution)
}

/// RULE: a route whose integration names `StepFunctions-StartExecution` starts an execution of
/// the state machine its `RequestParameters` map, named and fed by the same mapping, and
/// answers that action's own document — `executionArn` and `startDate` — so what the client
/// holds afterwards resolves to the run Step Functions kept.
#[tokio::test]
async fn a_start_execution_subtype_route_starts_the_run_its_request_parameters_map() {
    /// One case: the route, the request made to it, the parameters the integration maps, the
    /// execution name the mapping names, and the input the run must have been given.
    struct Row {
        case: &'static str,
        machine: &'static str,
        route_key: &'static str,
        path: &'static str,
        headers: &'static [(&'static str, &'static str)],
        body: &'static str,
        parameters: fn(&str) -> Value,
        name: &'static str,
        input: &'static str,
    }
    // Rows differ along WHERE each parameter comes from and WHAT the posted body carries: a
    // header-named run over a non-ASCII JSON body, a statically named run over a member
    // selected out of the body, and a run named from a path variable over an empty document.
    let rows: &[Row] = &[
        Row {
            case: "the name from a header and the whole body as the input",
            machine: "orders",
            route_key: "POST /start",
            path: "/start",
            headers: &[("x-execution-name", "run-from-header")],
            body: r#"{"orderId":"o-1","lines":[1,2,3],"note":"café","amount":42.5}"#,
            parameters: |machine| {
                json!({
                    "StateMachineArn": machine,
                    "Name": "$request.header.x-execution-name",
                    "Input": "$request.body",
                })
            },
            name: "run-from-header",
            input: r#"{"orderId":"o-1","lines":[1,2,3],"note":"café","amount":42.5}"#,
        },
        Row {
            case: "a static name and a member of the body as the input",
            machine: "invoices",
            route_key: "POST /invoices",
            path: "/invoices",
            headers: &[],
            body: r#"{"kind":"raised","payload":{"total":12,"tags":["a","b"]}}"#,
            parameters: |machine| {
                json!({
                    "StateMachineArn": machine,
                    "Name": "fixed-run",
                    "Input": "$request.body.payload",
                })
            },
            name: "fixed-run",
            input: r#"{"total":12,"tags":["a","b"]}"#,
        },
        Row {
            case: "a name from a path variable and an empty document as the input",
            machine: "shipments",
            route_key: "POST /shipments/{label}",
            path: "/shipments/run-7",
            headers: &[],
            body: "{}",
            parameters: |machine| {
                json!({
                    "StateMachineArn": machine,
                    "Name": "$request.path.label",
                    "Input": "$request.body",
                })
            },
            name: "run-7",
            input: "{}",
        },
    ];
    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        let machine_arn = emulator
            .state_machine(case, row.machine, "STANDARD", ECHO_DEFINITION)
            .await;
        let role = emulator
            .role(
                case,
                "apigateway-starts-executions",
                GATEWAY_PRINCIPAL,
                Some(grant(START_EXECUTION_ACTION)),
            )
            .await;
        let integration = subtype_integration(
            START_EXECUTION,
            Some(&role),
            (row.parameters)(&machine_arn),
        );
        let host = emulator
            .api(case, &[(row.route_key, integration)])
            .await;
        let (status, media_type, answered) =
            emulator.invoke(&host, row.path, row.headers, row.body).await;
        assert_eq!(
            status, PERFORMED,
            "case {case:?}: expected the start's own answer, got {status} with {answered}"
        );
        assert_eq!(
            media_type, JSON_MEDIA_TYPE,
            "case {case:?}: expected the action's own media type, got {media_type:?}"
        );
        let expected_arn = standard_execution_arn(row.machine, row.name);
        assert_eq!(
            answered["executionArn"],
            json!(expected_arn),
            "case {case:?}: expected the ARN of the named run, got {answered}"
        );
        assert!(
            answered["startDate"].is_number(),
            "case {case:?}: expected a startDate stated as epoch seconds, got {answered}"
        );
        // What the client holds resolves to the run Step Functions kept, with the input the
        // mapping handed it and the output that run computed from it.
        let settled = emulator.settled(case, &expected_arn).await;
        assert_eq!(
            settled["status"],
            json!("SUCCEEDED"),
            "case {case:?}: expected the run to succeed, got {settled}"
        );
        assert_eq!(
            settled["input"],
            json!(row.input),
            "case {case:?}: expected the mapped input, got {settled}"
        );
        assert_eq!(
            settled["name"],
            json!(row.name),
            "case {case:?}: expected the mapped name, got {settled}"
        );
        let expected_output: Value = serde_json::from_str(row.input)
            .unwrap_or_else(|error| panic!("case {case:?}: the row's input must be JSON: {error}"));
        assert_eq!(
            document(case, &settled, "output"),
            expected_output,
            "case {case:?}: expected the echoed input as the output, got {settled}"
        );
    }
}

/// RULE: a route whose integration names `StepFunctions-StartSyncExecution` answers the
/// SYNCHRONOUS start's own document — what the Express run came to, whether it succeeded or
/// failed — and the run is persisted under the ARN that document names.
#[tokio::test]
async fn a_start_sync_execution_subtype_route_answers_what_the_express_run_came_to() {
    /// One case: the definition the Express workflow runs, and the terminal outcome the
    /// answer must state.
    struct Row {
        case: &'static str,
        machine: &'static str,
        definition: &'static str,
        body: &'static str,
        status: &'static str,
        output: Option<&'static str>,
        error: Option<&'static str>,
        cause: Option<&'static str>,
    }
    // Rows differ along the OUTCOME the walk reaches: a run that succeeds with an output, and
    // one that fails with an error and a cause.
    let rows: &[Row] = &[
        Row {
            case: "a run that succeeds",
            machine: "priced-orders",
            definition: ECHO_DEFINITION,
            body: r#"{"orderId":"o-2","note":"café","amount":42.5}"#,
            status: "SUCCEEDED",
            output: Some(r#"{"orderId":"o-2","note":"café","amount":42.5}"#),
            error: None,
            cause: None,
        },
        Row {
            case: "a run that fails",
            machine: "rejected-orders",
            definition: FAIL_DEFINITION,
            body: r#"{"orderId":"o-3"}"#,
            status: "FAILED",
            output: None,
            error: Some(FAIL_ERROR),
            cause: Some(FAIL_CAUSE),
        },
    ];
    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        let machine_arn = emulator
            .state_machine(case, row.machine, "EXPRESS", row.definition)
            .await;
        let role = emulator
            .role(
                case,
                "apigateway-starts-sync-executions",
                GATEWAY_PRINCIPAL,
                Some(grant(START_SYNC_EXECUTION_ACTION)),
            )
            .await;
        let integration = subtype_integration(
            START_SYNC_EXECUTION,
            Some(&role),
            json!({
                "StateMachineArn": machine_arn,
                "Name": "$request.header.x-execution-name",
                "Input": "$request.body",
            }),
        );
        let host = emulator.api(case, &[("POST /start", integration)]).await;
        let (status, media_type, answered) = emulator
            .invoke(
                &host,
                "/start",
                &[("x-execution-name", "sync-run")],
                row.body,
            )
            .await;
        // "StartSyncExecution will return a 200 OK response, even if your execution fails":
        // the gateway relays the action's own answer, so a failed run is a 200 too.
        assert_eq!(
            status, PERFORMED,
            "case {case:?}: expected the synchronous start's own answer, got {status} with \
             {answered}"
        );
        assert_eq!(
            media_type, JSON_MEDIA_TYPE,
            "case {case:?}: expected the action's own media type, got {media_type:?}"
        );
        assert_eq!(
            answered["status"],
            json!(row.status),
            "case {case:?}: expected the run's terminal status, got {answered}"
        );
        assert_eq!(
            answered["name"],
            json!("sync-run"),
            "case {case:?}: expected the mapped name, got {answered}"
        );
        assert_eq!(
            answered["input"],
            json!(row.body),
            "case {case:?}: expected the mapped input, got {answered}"
        );
        for (member, expected) in [("error", row.error), ("cause", row.cause)] {
            assert_eq!(
                answered.get(member).and_then(Value::as_str),
                expected,
                "case {case:?}: expected {member:?} to be {expected:?}, got {answered}"
            );
        }
        match row.output {
            Some(output) => {
                let expected: Value = serde_json::from_str(output)
                    .unwrap_or_else(|error| panic!("case {case:?}: the output must be JSON: {error}"));
                assert_eq!(
                    document(case, &answered, "output"),
                    expected,
                    "case {case:?}: expected the run's output, got {answered}"
                );
            }
            None => assert!(
                answered.get("output").is_none_or(Value::is_null),
                "case {case:?}: expected no output for a failed run, got {answered}"
            ),
        }
        for member in ["startDate", "stopDate"] {
            assert!(
                answered[member].is_number(),
                "case {case:?}: expected {member} stated as epoch seconds, got {answered}"
            );
        }
        // The run is named as an Express execution of the state machine it ran, which is the
        // ARN form Step Functions mints for a synchronous start; it is not readable through
        // `DescribeExecution` ("not supported by EXPRESS state machines"), so what is asserted
        // is the identity the answer carries.
        let execution_arn = answered["executionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no executionArn in {answered}"))
            .to_string();
        let express_prefix = format!(
            "arn:aws:states:{REGION}:{}:express:{}:sync-run:",
            engine::emulator_account_id(),
            row.machine
        );
        assert!(
            execution_arn.starts_with(&express_prefix),
            "case {case:?}: expected an Express execution of {} named sync-run, got \
             {execution_arn:?}",
            row.machine
        );
        assert_eq!(
            answered["stateMachineArn"],
            json!(machine_arn),
            "case {case:?}: expected the run to belong to the mapped state machine, got \
             {answered}"
        );
    }
}

/// RULE: Step Functions decides on the integration role's own policies whether the run
/// happens, so a route whose role does not hold the action is answered that service's own
/// refusal and starts nothing — while a sibling route whose role does hold it keeps starting
/// runs, before and after.
#[tokio::test]
async fn a_role_without_the_action_is_refused_by_step_functions_and_starts_nothing() {
    let case = "a route whose role holds no start permission beside one that does";
    let emulator = Emulator::new();
    let machine_arn = emulator
        .state_machine(case, "orders", "STANDARD", ECHO_DEFINITION)
        .await;
    let permitted = emulator
        .role(
            case,
            "apigateway-may-start",
            GATEWAY_PRINCIPAL,
            Some(grant(START_EXECUTION_ACTION)),
        )
        .await;
    // The refused role holds a grant of ANOTHER action of the same service, so what refuses
    // the call is the missing permission and not a role nothing is known about.
    let refused = emulator
        .role(
            case,
            "apigateway-may-not-start",
            GATEWAY_PRINCIPAL,
            Some(grant("states:DescribeExecution")),
        )
        .await;
    let parameters = |name: &str| {
        json!({
            "StateMachineArn": machine_arn,
            "Name": name,
            "Input": "$request.body",
        })
    };
    let host = emulator
        .api(
            case,
            &[
                (
                    "POST /permitted/{name}",
                    subtype_integration(
                        START_EXECUTION,
                        Some(&permitted),
                        parameters("$request.path.name"),
                    ),
                ),
                (
                    "POST /refused",
                    subtype_integration(
                        START_EXECUTION,
                        Some(&refused),
                        parameters("refused-run"),
                    ),
                ),
            ],
        )
        .await;
    // Before: the permitted route starts a run.
    let (status, _media_type, answered) = emulator
        .invoke(&host, "/permitted/before", &[], r#"{"n":1}"#)
        .await;
    assert_eq!(
        status, PERFORMED,
        "case {case:?}: expected the permitted route to start a run, got {status} {answered}"
    );
    // The refused route is answered Step Functions' own refusal, relayed.
    let (status, media_type, relayed) = emulator
        .invoke(&host, "/refused", &[], r#"{"n":2}"#)
        .await;
    assert_eq!(
        status, CLIENT_REFUSAL,
        "case {case:?}: expected the service's own status, got {status} {relayed}"
    );
    assert_eq!(
        media_type, JSON_MEDIA_TYPE,
        "case {case:?}: expected a JSON refusal document, got {media_type:?}"
    );
    assert_eq!(
        relayed[SHAPE_MEMBER],
        json!(ACCESS_DENIED),
        "case {case:?}: expected the error the service named, got {relayed}"
    );
    let detail = relayed[DETAIL_MEMBER].as_str().unwrap_or_default();
    assert!(
        detail.contains(START_EXECUTION_ACTION) && detail.contains(&machine_arn),
        "case {case:?}: expected the service's own sentence about the refused action, got \
         {relayed}"
    );
    // Nothing was started: the execution the mapping named does not exist.
    let (status, described) = emulator
        .sfn(
            "DescribeExecution",
            &json!({ "executionArn": standard_execution_arn("orders", "refused-run") }),
        )
        .await;
    assert_eq!(
        status, CLIENT_REFUSAL,
        "case {case:?}: expected no such execution, got {status} {described}"
    );
    let named = described[SHAPE_MEMBER].as_str().unwrap_or_default();
    assert!(
        named.ends_with(EXECUTION_DOES_NOT_EXIST),
        "case {case:?}: expected {EXECUTION_DOES_NOT_EXIST}, got {described}"
    );
    // After: the permitted route is unaffected by its sibling's refusal.
    let (status, _media_type, answered) = emulator
        .invoke(&host, "/permitted/after", &[], r#"{"n":3}"#)
        .await;
    assert_eq!(
        status, PERFORMED,
        "case {case:?}: expected the permitted route to keep starting runs, got {status} \
         {answered}"
    );
    for name in ["before", "after"] {
        let settled = emulator
            .settled(case, &standard_execution_arn("orders", name))
            .await;
        assert_eq!(
            settled["status"],
            json!("SUCCEEDED"),
            "case {case:?}: expected the run {name:?} to have run, got {settled}"
        );
    }
}

/// RULE: what the integration mapped decides whether the action is called at all — a mapping
/// the action cannot be called with is an integration that produced no response, a mapping
/// expression THIS REQUEST carries no value for is the request's own fault, and a call Step
/// Functions itself refuses is that refusal relayed.
#[tokio::test]
async fn a_mapping_the_action_cannot_be_called_with_starts_nothing() {
    /// One case: the subtype, the workflow type behind it, the parameters mapped, the request
    /// made, and the status and body member the route must answer.
    struct Row {
        case: &'static str,
        subtype: &'static str,
        kind: &'static str,
        parameters: fn(&str) -> Value,
        headers: &'static [(&'static str, &'static str)],
        status: u16,
        /// The `message` an answer of this gateway's own states, or the error a relayed
        /// refusal names.
        expected: &'static str,
    }
    // Rows differ along WHAT is wrong: the required parameter mapped to an expression the
    // request does not carry, nothing mapped for it at all, a parameter outside the action,
    // and a synchronous start of a workflow type that action does not serve.
    let rows: &[Row] = &[
        Row {
            case: "an expression the request carries no value for",
            subtype: START_EXECUTION,
            kind: "STANDARD",
            parameters: |_machine| json!({ "StateMachineArn": "$request.header.x-machine" }),
            headers: &[],
            status: BAD_REQUEST,
            expected: BAD_REQUEST_MESSAGE,
        },
        Row {
            case: "nothing mapped for the required parameter",
            subtype: START_EXECUTION,
            kind: "STANDARD",
            parameters: |_machine| json!({ "Name": "unnamed-run" }),
            headers: &[],
            status: INTEGRATION_FAULT,
            expected: INTEGRATION_FAULT_MESSAGE,
        },
        Row {
            case: "a parameter this emulator does not carry into the action",
            subtype: START_EXECUTION,
            kind: "STANDARD",
            parameters: |machine| {
                json!({ "StateMachineArn": machine, "IncludedData": "METADATA_ONLY" })
            },
            headers: &[],
            status: INTEGRATION_FAULT,
            expected: INTEGRATION_FAULT_MESSAGE,
        },
        Row {
            case: "a synchronous start of a STANDARD workflow",
            subtype: START_SYNC_EXECUTION,
            kind: "STANDARD",
            parameters: |machine| json!({ "StateMachineArn": machine }),
            headers: &[],
            status: CLIENT_REFUSAL,
            expected: TYPE_NOT_SUPPORTED,
        },
    ];
    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        let machine_arn = emulator
            .state_machine(case, "orders", row.kind, ECHO_DEFINITION)
            .await;
        let role = emulator
            .role(
                case,
                "apigateway-starts-anything",
                GATEWAY_PRINCIPAL,
                Some(json!([{
                    "Effect": "Allow",
                    "Action": [START_EXECUTION_ACTION, START_SYNC_EXECUTION_ACTION],
                    "Resource": "*",
                }])),
            )
            .await;
        let integration =
            subtype_integration(row.subtype, Some(&role), (row.parameters)(&machine_arn));
        let host = emulator.api(case, &[("POST /start", integration)]).await;
        let (status, _media_type, answered) = emulator
            .invoke(&host, "/start", row.headers, r#"{"n":1}"#)
            .await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected {}, got {status} with {answered}",
            row.status
        );
        let stated = [SHAPE_MEMBER, DETAIL_MEMBER, "message"]
            .iter()
            .filter_map(|member| answered.get(*member).and_then(Value::as_str))
            .any(|text| text.contains(row.expected));
        assert!(
            stated,
            "case {case:?}: expected {:?} in the answer, got {answered}",
            row.expected
        );
    }
}

/// RULE: `Region` is the one parameter every first-class subtype integration may map that no
/// action's own request shape declares — it names the Region API Gateway sends the call to —
/// and an integration that maps none has the action called in the api's OWN Region.
#[tokio::test]
async fn a_mapped_region_is_the_region_the_action_is_called_in() {
    /// One case: the value the integration maps for `Region`, and whether the start then
    /// reaches the state machine this suite created in the api's own Region.
    struct Row {
        case: &'static str,
        region: Option<&'static str>,
        reaches_the_machine: bool,
    }
    // Rows differ along what is mapped for `Region`: nothing at all, the api's own Region,
    // another Region of the same partition, and a mapping expression this request carries no
    // value for.
    let rows: &[Row] = &[
        Row {
            case: "nothing mapped",
            region: None,
            reaches_the_machine: true,
        },
        Row {
            case: "the api's own Region",
            region: Some(REGION),
            reaches_the_machine: true,
        },
        Row {
            case: "another Region",
            region: Some(ANOTHER_REGION),
            reaches_the_machine: false,
        },
        Row {
            case: "an expression the request carries no value for",
            region: Some("$request.header.x-region"),
            reaches_the_machine: true,
        },
    ];
    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        let machine_arn = emulator
            .state_machine(case, "orders", "STANDARD", ECHO_DEFINITION)
            .await;
        let role = emulator
            .role(
                case,
                "apigateway-starts-across-regions",
                GATEWAY_PRINCIPAL,
                Some(grant(START_EXECUTION_ACTION)),
            )
            .await;
        let mut parameters = json!({
            "StateMachineArn": machine_arn,
            "Name": "region-run",
            "Input": "$request.body",
        });
        if let Some(region) = row.region {
            parameters["Region"] = json!(region);
        }
        let integration = subtype_integration(START_EXECUTION, Some(&role), parameters);
        let host = emulator.api(case, &[("POST /start", integration)]).await;
        let (status, _media_type, answered) = emulator
            .invoke(&host, "/start", &[], r#"{"n":1}"#)
            .await;
        let execution_arn = standard_execution_arn("orders", "region-run");
        if row.reaches_the_machine {
            assert_eq!(
                status, PERFORMED,
                "case {case:?}: expected the start to reach the state machine, got {status} \
                 with {answered}"
            );
            let settled = emulator.settled(case, &execution_arn).await;
            assert_eq!(
                settled["status"],
                json!("SUCCEEDED"),
                "case {case:?}: expected the run to have happened, got {settled}"
            );
            continue;
        }
        // The call really went to the mapped Region: a regional endpoint serves only its own
        // Region's resources, so the state machine's ARN is not one it can be asked about,
        // and Step Functions' own refusal says which Region it was asked in.
        assert_eq!(
            status, CLIENT_REFUSAL,
            "case {case:?}: expected the service's own status, got {status} with {answered}"
        );
        let named = answered[SHAPE_MEMBER].as_str().unwrap_or_default();
        assert!(
            named.ends_with(INVALID_ARN),
            "case {case:?}: expected {INVALID_ARN}, got {answered}"
        );
        let detail = answered[DETAIL_MEMBER].as_str().unwrap_or_default();
        assert!(
            detail.contains(ANOTHER_REGION),
            "case {case:?}: expected the refusal to name the Region the call was made in, got \
             {answered}"
        );
        // Nothing was started in the api's own Region either.
        let (status, described) = emulator
            .sfn(
                "DescribeExecution",
                &json!({ "executionArn": execution_arn }),
            )
            .await;
        assert_eq!(
            status, CLIENT_REFUSAL,
            "case {case:?}: expected no such execution, got {status} {described}"
        );
        let named = described[SHAPE_MEMBER].as_str().unwrap_or_default();
        assert!(
            named.ends_with(EXECUTION_DOES_NOT_EXIST),
            "case {case:?}: expected {EXECUTION_DOES_NOT_EXIST}, got {described}"
        );
    }
}
