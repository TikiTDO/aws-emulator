// SPDX-License-Identifier: CC0-1.0

//! The harness the plan-execution suites of this crate share: one emulator over one
//! shared engine, the wire call every case makes, and the IAM/STS records a case about WHO
//! is calling seeds.
//!
//! Nothing here asserts a rule of its own — each suite does that. It exists so the suites
//! about starting an execution ([`crate::start_plan_execution_authorization`],
//! [`crate::plan_execution_event_bus`], [`crate::execution_start_event`],
//! [`crate::execution_refusal_messages`], [`crate::plan_execution_role_evaluation`]) drive
//! the emulator through the same `MultiServiceRouter::dispatch` the router serves on a
//! socket, so what they observe is what an SDK observes: `awsJson1_0`, `POST /`,
//! `X-Amz-Target: ArcRegionSwitch.<Operation>`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The canonical account id every ARN a generated emulator mints carries
/// (`engine::emulator_account_id`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// The Region every call below is signed for.
pub(crate) const REGION: &str = "us-east-1";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
pub(crate) const ACCOUNT_KEY_ID: &str = "AKIDTEST";

/// The access key id of the assumed-role session the authorization cases sign with.
pub(crate) const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The role that session is assumed from, and the session's own name.
pub(crate) const CALLER_ROLE: &str = "switch-starter";
pub(crate) const CALLER_SESSION: &str = "switch-session";

/// The name of the inline policy a case puts in force on that role.
const POLICY_NAME: &str = "inline";

/// How long a seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// How long a case polls for something another service has to deliver before it gives up.
///
/// Generous on purpose: it bounds a POLL LOOP, which ends as soon as the condition holds,
/// so a fast machine never waits for it and a loaded one is not failed by it.
pub(crate) const DELIVERY_DEADLINE: Duration = Duration::from_secs(30);

/// How long a poll loop waits between looks.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// This service on a fresh shared engine, beside whichever other services a suite needs,
/// plus the engine itself — so a case about the clock can ask the store to apply what it
/// would owe at a later instant instead of sleeping for an hour, and a case about WHO is
/// calling can seed the records IAM and STS hold.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    /// This one service.
    pub(crate) fn new() -> Self {
        Self::with(false, false)
    }

    /// This service beside the IAM and STS services, so the policies and the session a
    /// case seeds are the ones those services hold.
    pub(crate) fn with_identity() -> Self {
        Self::with(true, false)
    }

    /// This service beside EventBridge and Amazon SQS, so a rule's target really receives
    /// what this service publishes.
    pub(crate) fn with_events() -> Self {
        Self::with(false, true)
    }

    fn with(identity: bool, events: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These suites exercise operation behaviour, not request signing.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_arcregionswitch_arcregionswitch::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        if identity {
            router.register_service(
                service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                    Arc::clone(&engine),
                ),
            );
            router.register_service(
                service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                    Arc::clone(&engine),
                ),
            );
        }
        if events {
            router.register_service(
                service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(
                    &engine,
                )),
            );
            router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
                Arc::clone(&engine),
            ));
        }
        Self { router, engine }
    }

    /// Call one operation of this service as the account's own credential.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_as(ACCOUNT_KEY_ID, operation, body).await
    }

    /// Call one operation of this service signed with `access_key_id` — what names the
    /// principal the request speaks for.
    pub(crate) async fn call_as(
        &self,
        access_key_id: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        self.rpc(
            access_key_id,
            "arc-region-switch",
            "ArcRegionSwitch",
            "1.0",
            operation,
            body,
        )
        .await
    }

    /// Call one operation of any mounted awsJson service the way an SDK would, in the
    /// Region this suite's requests are scoped to.
    pub(crate) async fn rpc(
        &self,
        access_key_id: &str,
        signing_name: &str,
        target: &str,
        json_version: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        self.rpc_in_region(
            REGION,
            access_key_id,
            signing_name,
            target,
            json_version,
            operation,
            body,
        )
        .await
    }

    /// The same, with the request scoped to `region` — what a caller reaching that
    /// Region's endpoint sends, which is what decides which Region's resources it
    /// addresses.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn rpc_in_region(
        &self,
        region: &str,
        access_key_id: &str,
        signing_name: &str,
        target: &str,
        json_version: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header(
                "content-type",
                format!("application/x-amz-json-{json_version}"),
            )
            .header("x-amz-target", format!("{target}.{operation}"))
            // The router keys on the signing name in the SigV4 credential scope.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential={access_key_id}/20260101/{region}/{signing_name}/aws4_request, \
                     SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One EventBridge call, on the bus of `region`.
    pub(crate) async fn events(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        self.rpc_in_region(
            region,
            ACCOUNT_KEY_ID,
            "events",
            "AWSEvents",
            "1.1",
            operation,
            body,
        )
        .await
    }

    /// One Amazon SQS call, in `region`.
    pub(crate) async fn sqs(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        self.rpc_in_region(
            region,
            ACCOUNT_KEY_ID,
            "sqs",
            "AmazonSQS",
            "1.0",
            operation,
            body,
        )
        .await
    }
}

/// The wire error code of an error response: the `__type` member's shape name.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` of an error response.
pub(crate) fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The ARN the account holds a role under.
pub(crate) fn role_arn(role_name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{role_name}")
}

/// The principal ARN an assumed-role session of [`CALLER_ROLE`] speaks for — what an
/// authorization refusal names.
pub(crate) fn session_arn() -> String {
    format!("arn:aws:sts::{ACCOUNT}:assumed-role/{CALLER_ROLE}/{CALLER_SESSION}")
}

/// Record the role `role_name` in the IAM store of the shared engine, as
/// `iam:CreateRole` records one.
pub(crate) fn given_role(engine: &Arc<engine::InMemoryEngine>, role_name: &str) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    store.lock().roles.insert(
        iam::name_key(role_name),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: role_name.to_string(),
            role_id: format!("AROAEXAMPLE{}", role_name.to_uppercase()),
            arn: role_arn(role_name),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
}

/// Take the role `role_name` out of the IAM store again, as `iam:DeleteRole` does.
pub(crate) fn given_no_role(engine: &Arc<engine::InMemoryEngine>, role_name: &str) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    store.lock().roles.remove(&iam::name_key(role_name));
}

/// Record [`CALLER_ROLE`] and the assumed-role session [`SESSION_KEY_ID`] speaks for, as
/// `iam:CreateRole` and `sts:AssumeRole` record them.
pub(crate) fn given_session(engine: &Arc<engine::InMemoryEngine>) {
    given_role(engine, CALLER_ROLE);
    let role_arn = role_arn(CALLER_ROLE);
    let vend = sts::NewSession {
        role_arn: Some(role_arn.clone()),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role_arn),
                account: ACCOUNT.to_string(),
                arn: session_arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let store = sts::service_state(&**engine).expect("the STS store");
    store
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, SystemTime::now()))
        .expect("the session is recorded");
}

/// Put `document` in force on [`CALLER_ROLE`] as its inline policy, replacing whatever it
/// carried; `None` takes every policy off it, as `iam:DeleteRolePolicy` does.
pub(crate) fn given_role_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.role_inline_policies.remove(&iam::name_key(CALLER_ROLE));
    if let Some(document) = document {
        store.role_inline_policies.insert(
            iam::name_key(CALLER_ROLE),
            BTreeMap::from([(
                iam::name_key(POLICY_NAME),
                iam::InlinePolicy {
                    policy_name: POLICY_NAME.to_string(),
                    policy_document: document.to_string(),
                },
            )]),
        );
    }
}

/// A policy allowing `actions` on `resource`.
pub(crate) fn allow(actions: Value, resource: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": resource }]
    })
}

/// One step carrying a manual-approval execution block — the one block type whose work is
/// inside this service — with the window its configuration gives the approval.
///
/// Members from `smithy_query(query="shape", shape="ExecutionApprovalConfiguration",
/// model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`:
/// `approvalRole` is required and `timeoutMinutes` is `@range(min: 1)` with `@default: 60`.
pub(crate) fn approval_step(name: &str) -> Value {
    json!({
        "name": name,
        "executionBlockType": "ManualApproval",
        "executionBlockConfiguration": {
            "executionApprovalConfig": {
                "approvalRole": role_arn("Approver"),
                "timeoutMinutes": 60,
            }
        }
    })
}

/// One step carrying a Lambda execution block: it invokes another service's function, so
/// this emulator cannot perform it and the step fails.
pub(crate) fn lambda_step(name: &str) -> Value {
    json!({
        "name": name,
        "executionBlockType": "CustomActionLambda",
        "executionBlockConfiguration": {
            "customActionLambdaConfig": {
                "timeoutMinutes": 60,
                "retryIntervalMinutes": 1.0,
                "regionToRun": "activatingRegion",
                "lambdas": [{"arn": format!("arn:aws:lambda:us-west-2:{ACCOUNT}:function:switch")}],
            }
        }
    })
}

/// A `CreatePlan` request for a plan covering `regions`, whose workflows are `workflows`,
/// performed as the role `execution_role`.
pub(crate) fn plan_request(
    name: &str,
    regions: [&str; 2],
    execution_role: &str,
    workflows: Value,
) -> Value {
    json!({
        "name": name,
        "regions": regions,
        "recoveryApproach": "activePassive",
        "executionRole": execution_role,
        "workflows": workflows,
    })
}

/// One workflow performing `action` with `steps`.
pub(crate) fn workflow_of(action: &str, steps: Vec<Value>) -> Value {
    json!({
        "workflowTargetAction": action,
        "workflowDescription": format!("the {action} workflow"),
        "steps": steps,
    })
}

/// Create a plan whose `activate` workflow runs `steps`, and return its ARN.
pub(crate) async fn plan_with_steps(
    emulator: &Emulator,
    case: &str,
    name: &str,
    steps: Vec<Value>,
) -> String {
    create_plan(
        emulator,
        case,
        &plan_request(
            name,
            ["us-east-1", "us-west-2"],
            &role_arn("ArcRegionSwitchRole"),
            json!([workflow_of("activate", steps)]),
        ),
    )
    .await
}

/// Create the plan `request` describes and return its ARN.
pub(crate) async fn create_plan(emulator: &Emulator, case: &str, request: &Value) -> String {
    let (status, created) = emulator.call("CreatePlan", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreatePlan expected 200, got {status} with {created}"
    );
    created["plan"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Start an `activate` execution of `plan_arn` targeting `target_region`, and return its
/// id.
pub(crate) async fn start_execution(
    emulator: &Emulator,
    case: &str,
    plan_arn: &str,
    target_region: &str,
) -> String {
    let (status, started) = emulator
        .call(
            "StartPlanExecution",
            &json!({"planArn": plan_arn, "targetRegion": target_region, "action": "activate"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartPlanExecution expected 200, got {status} with {started}"
    );
    started["executionId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Read one execution back through `GetPlanExecution`.
pub(crate) async fn get_execution(
    emulator: &Emulator,
    case: &str,
    plan_arn: &str,
    execution_id: &str,
) -> Value {
    let (status, got) = emulator
        .call(
            "GetPlanExecution",
            &json!({"planArn": plan_arn, "executionId": execution_id}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetPlanExecution expected 200, got {status} with {got}"
    );
    got
}

/// Every event `ListPlanExecutionEvents` reports for one execution, oldest first.
pub(crate) async fn events_of(
    emulator: &Emulator,
    case: &str,
    plan_arn: &str,
    execution_id: &str,
) -> Vec<Value> {
    let (status, page) = emulator
        .call(
            "ListPlanExecutionEvents",
            &json!({"planArn": plan_arn, "executionId": execution_id, "maxResults": 100}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPlanExecutionEvents expected 200, got {status} with {page}"
    );
    page["items"].as_array().cloned().unwrap_or_default()
}

/// Poll `GetPlanExecution` until the execution reports one of `states`, or the generous
/// [`DELIVERY_DEADLINE`] passes; answers the execution as last read.
///
/// Waits for the CONDITION rather than for a duration, so nothing here depends on how
/// fast the host runs.
pub(crate) async fn poll_until_state(
    emulator: &Emulator,
    case: &str,
    plan_arn: &str,
    execution_id: &str,
    states: &[&str],
) -> Value {
    let deadline = std::time::Instant::now() + DELIVERY_DEADLINE;
    loop {
        let execution = get_execution(emulator, case, plan_arn, execution_id).await;
        let state = execution["executionState"].as_str().unwrap_or_default();
        if states.contains(&state) {
            return execution;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected the execution to reach one of {states:?}, it reports \
             {state:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
