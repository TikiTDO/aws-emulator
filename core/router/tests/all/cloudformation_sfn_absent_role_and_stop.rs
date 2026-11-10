// SPDX-License-Identifier: CC0-1.0

//! What a CLIENT of a CloudFormation-deployed state machine gets out of it: the handle the
//! stack publishes, what an execution whose execution role cannot be assumed fails with, and
//! what a stop leaves a running execution in.
//!
//! Three rules hold here, none of them inside one crate, which is why the suite lives in this
//! test target with CloudFormation, Step Functions and IAM mounted on ONE engine exactly as
//! `core/router/src/lib.rs` mounts them:
//!
//! 1. **The reference a stack publishes for a resource whose published return values document
//!    `Ref` as an ARN is that ARN** — for a state machine, the one the owning service minted
//!    (`arn:<partition>:states:<region>:<account>:stateMachine:<name>`), so the exported value
//!    ends in the name the stack publishes as that resource's separate `Name` attribute and is
//!    accepted wherever Step Functions takes a state-machine ARN.
//! 2. **Step Functions performs a `Task` state's work ACTING AS the state machine's execution
//!    role**, so a role this account's IAM answers cannot be assumed by either of that
//!    service's principals fails the TASK rather than the interpreter: the execution ends
//!    `FAILED` with `States.TaskFailed`, its history records a `TaskFailed` event, and the work
//!    the state named never happens — while the CONTROL plane created the machine all the same,
//!    because it does not resolve the role.
//! 3. **`StopExecution` settles the execution it names**: a running execution reaches the
//!    terminal `ABORTED` status carrying the `error` and `cause` the call supplied, its history
//!    ends on an `ExecutionAborted` event carrying them, and nothing is appended to it
//!    afterwards — the outcome the run was heading for never lands.
//!
//! The rows differ along what each rule generalises over: for rules 1 and 2, what IAM ANSWERS
//! about the role the template declares (this account holds no such role, a role whose trust
//! policy names another service principal, a role trusting this service's regional principal, a
//! role of another account IAM here cannot be asked about), which intrinsic publishes the
//! reference, whether the template NAMES the machine, and which Region it deploys in; for rule
//! 3, which of `error` and `cause` the stop supplies and whether the run it stops would
//! otherwise have finished inside the test.
//!
//! `all/cloudformation_sfn_reference_starts_execution.rs` is the sibling suite over what a start
//! against the published reference answers; this one is over the two FAILURE paths a client of
//! such a machine runs into, and over the identity the stack exports for them.
//!
//! The Step Functions crate asks the data-plane halves of rules 2 and 3 of a machine its OWN
//! client created — `tests/all/execution_role_assumption.rs` for the role, and
//! `tests/all/execution_history_outcomes.rs` for the abort event and the history it closes. What
//! is asked here is the CloudFormation-deployed machine: the handle a stack published is what the
//! start and the stop are made against, and the role is the one a TEMPLATE declared, so the two
//! crates have to agree for any of it to hold. Neither suite's harness can be called from here —
//! a crate's `tests/` target is not importable by another crate's — so the rows are restated over
//! this suite's own three-service router rather than shared; what is shared is what CAN be: the
//! awsQuery form encoder and XML readers this test crate owns (`tests/all/query_wire.rs`) and the
//! Step Functions crate's own event-type and error-name constants.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::state::integration::TASK_FAILED_ERROR;
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    EVENT_EXECUTION_ABORTED, EVENT_EXECUTION_STARTED, EVENT_TASK_FAILED,
};

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{element, form_encode, opt_element, outputs_of};

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The Region IAM's own requests are signed for. IAM is a global service, so the roles it holds
/// are the same records whichever Region a state machine's execution asks about them from.
const IAM_REGION: &str = "us-east-1";

/// How long a row waits for a status the emulator owes — a deployed stack, a terminal
/// execution. Every span here is sub-second, so this is slack rather than a budget.
const PATIENCE: Duration = Duration::from_secs(20);

/// How often a row asks again while it waits.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A role ARN of this account that IAM holds nothing under.
const NOT_HELD: &str = "arn:aws:iam::000000000000:role/NotHeldHere";

/// A role ARN of ANOTHER account, whose IAM this emulator is not the one to ask.
const ANOTHER_ACCOUNT: &str = "arn:aws:iam::210987654321:role/Elsewhere";

/// The integration a parent's `Task` names: it starts a child execution, which is work a row can
/// then look for independently of what the parent reports.
const FIRE_AND_FORGET: &str = "arn:aws:states:::states:startExecution";

/// The three services this question spans, over ONE shared engine.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which is
/// what routes the request and what each handler reads its Region from. The signature is never
/// checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one Step Functions operation (`sigv4Name: states`, service shape `AWSStepFunctions`,
/// `awsJson1_0`) over its own wire, signed for `region`.
async fn sfn(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("states.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSStepFunctions.{operation}"))
        .header("authorization", authorization("states", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// [`sfn`] where the call must be served.
async fn sfn_ok(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
    case: &str,
) -> Value {
    let (status, payload) = sfn(router, region, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {status} with body {payload}"
    );
    payload
}

/// One awsQuery call to IAM, which must be served, answered with the XML body.
async fn iam(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, String)],
    case: &str,
) -> String {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("iam", IAM_REGION))
        .body(Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from IAM {action}, got {status} with {text}"
    );
    text
}

/// The text a member of a JSON response carries.
fn string(body: &Value, name: &str, case: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} in {body}"))
        .to_string()
}

/// A trust policy with one statement: `effect` for `principal` over `action`.
fn trust(effect: &str, principal: &str, action: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "Service": principal },
            "Action": action,
        }],
    })
}

/// A policy allowing every Step Functions action, so a row whose role this service CAN assume is
/// not then refused on what that role may do.
fn allow_every_states_action() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": "states:*", "Resource": "*" }],
    })
}

/// A role carrying `document` as its trust policy and [`allow_every_states_action`] inline,
/// answered by its own ARN — written through IAM's own `CreateRole`/`PutRolePolicy`, so the
/// records a task reads are the ones that service wrote.
async fn role_trusting(
    router: &MultiServiceRouter,
    name: &str,
    document: &Value,
    case: &str,
) -> String {
    let created = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", name.to_string()),
            ("AssumeRolePolicyDocument", document.to_string()),
        ],
        case,
    )
    .await;
    let arn = element(&created, "Arn");
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name.to_string()),
            ("PolicyName", "inline".to_string()),
            ("PolicyDocument", allow_every_states_action().to_string()),
        ],
        case,
    )
    .await;
    arn
}

/// Deploy `template` as `stack_name` in `region` and answer the stack's `Outputs` once it has
/// reached `CREATE_COMPLETE`, which is when the resources it declares exist in their own
/// services.
async fn deployed_outputs(
    router: &MultiServiceRouter,
    region: &str,
    stack_name: &str,
    template: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let (status, body) = cloudformation(
        router,
        region,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );

    let began = Instant::now();
    loop {
        let (status, described) =
            cloudformation(router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        match opt_element(&described, "StackStatus").as_deref() {
            Some("CREATE_COMPLETE") => return outputs_of(&described),
            Some("CREATE_IN_PROGRESS") => {}
            other => panic!(
                "case {case:?}: expected the stack to be creating or complete, got {other:?} in \
                 {described}"
            ),
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected CREATE_COMPLETE within {PATIENCE:?}, got {described}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The output `key` a deployed stack publishes.
fn output(outputs: &BTreeMap<String, String>, key: &str, case: &str) -> String {
    outputs
        .get(key)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an output {key:?} in {outputs:?}"))
}

/// The ARN a state machine of `name` carries in `region`: the total function of the Region, the
/// account and the name both services compute the identity from.
fn state_machine_arn(region: &str, name: &str) -> String {
    format!("arn:aws:states:{region}:{ACCOUNT}:stateMachine:{name}")
}

/// `DescribeExecution` once the execution has left `RUNNING`.
async fn settled(
    router: &MultiServiceRouter,
    region: &str,
    execution_arn: &str,
    case: &str,
) -> Value {
    let began = Instant::now();
    loop {
        let described = sfn_ok(
            router,
            region,
            "DescribeExecution",
            &json!({ "executionArn": execution_arn }),
            case,
        )
        .await;
        if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
            return described;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected a terminal status within {PATIENCE:?}, got {described}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every event of an execution's history, in order.
async fn history(
    router: &MultiServiceRouter,
    region: &str,
    execution_arn: &str,
    case: &str,
) -> Vec<Value> {
    let answered = sfn_ok(
        router,
        region,
        "GetExecutionHistory",
        &json!({ "executionArn": execution_arn, "maxResults": 1000 }),
        case,
    )
    .await;
    answered
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `type` of each event, in the order they were answered.
fn types(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| event.get("type").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// How many executions `ListExecutions` reports for `machine_arn`.
async fn started_count(
    router: &MultiServiceRouter,
    region: &str,
    machine_arn: &str,
    case: &str,
) -> usize {
    let listed = sfn_ok(
        router,
        region,
        "ListExecutions",
        &json!({ "stateMachineArn": machine_arn, "maxResults": 100 }),
        case,
    )
    .await;
    listed
        .get("executions")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: the published identity, and the role the data plane has to assume
// ---------------------------------------------------------------------------

/// What IAM answers about the role a row's template declares.
enum RoleAnswer {
    /// A role ARN IAM is asked about and holds nothing under.
    NotHeld,
    /// A role IAM holds, carrying the trust document `(effect, principal, action)` states.
    Held {
        name: &'static str,
        effect: &'static str,
        /// `{region}` is replaced with the row's Region, so a row can name this service's
        /// REGIONAL principal.
        principal: &'static str,
        action: &'static str,
    },
    /// A role of another account, which this emulator's IAM cannot be asked about.
    OtherAccount,
}

/// How a row's template publishes the reference to the machine it declares.
enum Reference {
    Ref,
    GetAttArn,
    SubLogicalId,
}

impl Reference {
    /// The intrinsic, over the logical id `logical`.
    fn intrinsic(&self, logical: &str) -> Value {
        match self {
            Self::Ref => json!({ "Ref": logical }),
            Self::GetAttArn => json!({ "Fn::GetAtt": [logical, "Arn"] }),
            Self::SubLogicalId => json!({ "Fn::Sub": format!("${{{logical}}}") }),
        }
    }
}

/// One row: a stack declaring a state machine, and what an execution of it must then do.
struct RoleRow {
    case: &'static str,
    region: &'static str,
    answer: RoleAnswer,
    reference: Reference,
    /// The name the template declares, or `None` to leave CloudFormation to generate one.
    declared_name: Option<&'static str>,
    /// Whether this service can act as the role, which is whether the `Task` state's work
    /// happens at all.
    assumable: bool,
}

const ROLE_ROWS: &[RoleRow] = &[
    RoleRow {
        case: "a role this account's IAM holds nothing under",
        region: "us-east-1",
        answer: RoleAnswer::NotHeld,
        reference: Reference::Ref,
        declared_name: Some("absent-role-workflow"),
        assumable: false,
    },
    RoleRow {
        case: "a role whose trust policy names another service principal",
        region: "us-west-2",
        answer: RoleAnswer::Held {
            name: "LambdaTrustedWorkflowRole",
            effect: "Allow",
            principal: "lambda.amazonaws.com",
            action: "sts:AssumeRole",
        },
        reference: Reference::GetAttArn,
        declared_name: None,
        assumable: false,
    },
    RoleRow {
        case: "a role whose trust policy explicitly denies the assume",
        region: "eu-west-1",
        answer: RoleAnswer::Held {
            name: "DeniedTrustWorkflowRole",
            effect: "Deny",
            principal: "states.amazonaws.com",
            action: "sts:AssumeRole",
        },
        reference: Reference::SubLogicalId,
        declared_name: Some("denied-trust-workflow"),
        assumable: false,
    },
    RoleRow {
        // The service assumes the role as either of its principals, so a role trusting the
        // Region's own is one it can act as.
        case: "a role trusting this service's regional principal",
        region: "eu-central-1",
        answer: RoleAnswer::Held {
            name: "RegionalTrustWorkflowRole",
            effect: "Allow",
            principal: "states.{region}.amazonaws.com",
            action: "sts:AssumeRole",
        },
        reference: Reference::Ref,
        declared_name: None,
        assumable: true,
    },
    RoleRow {
        // Another account's roles are not this emulator's IAM to answer for, so nothing is
        // known and nothing is refused.
        case: "a role of another account",
        region: "ap-southeast-2",
        answer: RoleAnswer::OtherAccount,
        reference: Reference::GetAttArn,
        declared_name: None,
        assumable: true,
    },
];

/// The stack publishes its state machine's ARN, and an execution of a machine whose execution
/// role this service cannot assume fails AT THE TASK with `States.TaskFailed`, having done none
/// of the work the task named.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stack_publishes_the_machine_arn_and_an_unassumable_role_fails_the_task() {
    let router = build_router();

    for (index, row) in ROLE_ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;

        // The role ARN the template declares, resolved from what IAM is to answer about it.
        let role_arn = match &row.answer {
            RoleAnswer::NotHeld => NOT_HELD.to_string(),
            RoleAnswer::OtherAccount => ANOTHER_ACCOUNT.to_string(),
            RoleAnswer::Held {
                name,
                effect,
                principal,
                action,
            } => {
                let principal = principal.replace("{region}", region);
                role_trusting(&router, name, &trust(effect, &principal, action), case).await
            }
        };

        // The work the parent's `Task` names: a child state machine, created out of band, whose
        // executions are what says whether that work happened. Its own definition finishes at
        // once and makes no call, so nothing about its role is ever asked.
        let child_name = format!("task-child-{index}");
        let child = sfn_ok(
            &router,
            region,
            "CreateStateMachine",
            &json!({
                "name": child_name,
                "definition": json!({
                    "StartAt": "Done",
                    "States": { "Done": { "Type": "Pass", "End": true } },
                })
                .to_string(),
                "roleArn": ANOTHER_ACCOUNT,
            }),
            case,
        )
        .await;
        let child_arn = string(&child, "stateMachineArn", case);

        // The stack: one state machine whose single `Task` starts the child, published through
        // the row's intrinsic, beside the machine's own `Name` attribute — so the name half of
        // rule 1 is known INDEPENDENTLY of the reference under test.
        let logical = "Workflow";
        let mut properties = json!({
            "RoleArn": role_arn,
            "DefinitionString": json!({
                "StartAt": "CallChild",
                "States": {
                    "CallChild": {
                        "Type": "Task",
                        "Resource": FIRE_AND_FORGET,
                        "Parameters": { "StateMachineArn": child_arn },
                        "End": true,
                    },
                },
            })
            .to_string(),
        });
        if let Some(name) = row.declared_name {
            properties["StateMachineName"] = json!(name);
        }
        let template = json!({
            "AWSTemplateFormatVersion": "2010-09-09",
            "Resources": {
                logical: {
                    "Type": "AWS::StepFunctions::StateMachine",
                    "Properties": properties,
                },
            },
            "Outputs": {
                "StateMachineArn": { "Value": row.reference.intrinsic(logical) },
                "StateMachineName": { "Value": { "Fn::GetAtt": [logical, "Name"] } },
            },
        })
        .to_string();

        let stack_name = format!("absent-role-stack-{index}");
        let outputs = deployed_outputs(&router, region, &stack_name, &template, case).await;
        let reference = output(&outputs, "StateMachineArn", case);
        let machine_name = output(&outputs, "StateMachineName", case);
        if let Some(declared) = row.declared_name {
            assert_eq!(
                machine_name, declared,
                "case {case:?}: expected the Name attribute to be the declared name \
                 {declared:?}, got {machine_name:?}"
            );
        }

        // Rule 1: the published reference is the machine's ARN, which ends in the name the
        // stack publishes beside it — whichever intrinsic resolved it, and whether the name was
        // declared or generated.
        let expected = state_machine_arn(region, &machine_name);
        assert_eq!(
            reference, expected,
            "case {case:?}: expected the stack's reference to be the machine's ARN {expected:?}, \
             got {reference:?}"
        );
        let suffix = format!(":stateMachine:{machine_name}");
        assert!(
            reference.ends_with(&suffix),
            "case {case:?}: expected the reference to end in {suffix:?}, got {reference:?}"
        );
        // And Step Functions answers for it: the machine the stack reported really exists under
        // the handle it published.
        let described_machine = sfn_ok(
            &router,
            region,
            "DescribeStateMachine",
            &json!({ "stateMachineArn": reference }),
            case,
        )
        .await;
        assert_eq!(
            string(&described_machine, "name", case),
            machine_name,
            "case {case:?}: expected the machine under the published ARN to carry the published \
             name, got {described_machine}"
        );
        assert_eq!(
            string(&described_machine, "roleArn", case),
            role_arn,
            "case {case:?}: expected the machine to carry the role the template declared, got \
             {described_machine}"
        );

        // Rule 2: the DATA plane is where the role is resolved.
        let before = started_count(&router, region, &child_arn, case).await;
        let started = sfn_ok(
            &router,
            region,
            "StartExecution",
            &json!({ "stateMachineArn": reference, "name": "only", "input": "{}" }),
            case,
        )
        .await;
        let execution_arn = string(&started, "executionArn", case);
        let described = settled(&router, region, &execution_arn, case).await;

        if row.assumable {
            assert_eq!(
                described.get("status").and_then(Value::as_str),
                Some("SUCCEEDED"),
                "case {case:?}: expected the execution to run its task, got {described}"
            );
            assert_eq!(
                started_count(&router, region, &child_arn, case).await,
                before + 1,
                "case {case:?}: expected the child execution the task started"
            );
            continue;
        }

        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some(TASK_FAILED_ERROR)),
            "case {case:?}: expected FAILED with {TASK_FAILED_ERROR}, got {described}"
        );
        // The cause names the role reference an operator has to act on, and both of the
        // principals the assume was attempted as.
        let cause = string(&described, "cause", case);
        for fragment in [
            "is not authorized to assume the provided role",
            "states.amazonaws.com",
            &format!("states.{region}.amazonaws.com"),
            role_arn.as_str(),
        ] {
            assert!(
                cause.contains(fragment),
                "case {case:?}: expected the cause to carry {fragment:?}, got {cause:?}"
            );
        }
        // The TASK reported the failure as its own, and the work it named never happened.
        let events = history(&router, region, &execution_arn, case).await;
        let task_failures = types(&events)
            .iter()
            .filter(|event_type| event_type.as_str() == EVENT_TASK_FAILED)
            .count();
        assert_eq!(
            task_failures,
            1,
            "case {case:?}: expected exactly one {EVENT_TASK_FAILED}, got {:?}",
            types(&events)
        );
        assert_eq!(
            started_count(&router, region, &child_arn, case).await,
            before,
            "case {case:?}: expected no child execution for a role that cannot be assumed"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a stop settles the execution it names
// ---------------------------------------------------------------------------

/// The Region the stop rows deploy in.
const STOP_REGION: &str = "us-west-2";

/// How many seconds the SHORT row's `Wait` state waits: long enough that the execution is still
/// `RUNNING` when it is stopped, and short enough that a run nothing stopped WOULD have reached
/// its `Succeed` state inside [`PAST_THE_SHORT_RUN`] — which is what makes "the history is
/// truncated at the abort" an assertion rather than a tautology.
const SHORT_WAIT_SECONDS: u64 = 2;

/// Long enough for a run of the SHORT machine to have finished had nothing stopped it: its
/// `Wait`, the clock-owed span an execution settles after, and slack.
const PAST_THE_SHORT_RUN: Duration = Duration::from_millis(3_400);

/// How long a row that stopped a LONG run waits before reading the record again: past the span
/// the service settles an execution after, so a stop that only looked settled has had its
/// chance to move.
const PAST_THE_SETTLE_SPAN: Duration = Duration::from_millis(900);

/// A definition that waits `seconds` and then succeeds.
fn wait_definition(seconds: u64) -> Value {
    json!({
        "StartAt": "Hold",
        "States": {
            "Hold": { "Type": "Wait", "Seconds": seconds, "Next": "Done" },
            "Done": { "Type": "Succeed" },
        },
    })
}

/// One stop row: which of the stack's two machines it runs, and what the stop supplies.
struct StopRow {
    case: &'static str,
    /// The output key carrying the machine's ARN.
    reference_output: &'static str,
    /// The output key carrying that machine's name.
    name_output: &'static str,
    /// The execution name the row starts under.
    execution_name: &'static str,
    error: Option<&'static str>,
    cause: Option<&'static str>,
    /// How long the row waits before reading the record a second time.
    settle_wait: Duration,
}

const STOP_ROWS: &[StopRow] = &[
    StopRow {
        case: "a stop carrying an error and a cause, of a run that would have succeeded",
        reference_output: "ShortWaitArn",
        name_output: "ShortWaitName",
        execution_name: "stopped-with-both",
        error: Some("Cancelled"),
        cause: Some("the operator asked"),
        settle_wait: PAST_THE_SHORT_RUN,
    },
    StopRow {
        case: "a stop carrying neither, of a long run",
        reference_output: "LongWaitArn",
        name_output: "LongWaitName",
        execution_name: "stopped-with-neither",
        error: None,
        cause: None,
        settle_wait: PAST_THE_SETTLE_SPAN,
    },
    StopRow {
        case: "a stop carrying only a cause, of a long run",
        reference_output: "LongWaitArn",
        name_output: "LongWaitName",
        execution_name: "stopped-with-a-cause",
        error: None,
        cause: Some("the operator asked"),
        settle_wait: PAST_THE_SETTLE_SPAN,
    },
    StopRow {
        case: "a stop carrying only an error, of a long run",
        reference_output: "LongWaitArn",
        name_output: "LongWaitName",
        execution_name: "stopped-with-an-error",
        error: Some("Cancelled"),
        cause: None,
        settle_wait: PAST_THE_SETTLE_SPAN,
    },
];

/// Stopping a running execution of a stack-deployed machine settles it: the record reaches
/// `ABORTED` with the error and cause the call supplied, the history ends on `ExecutionAborted`
/// carrying them, and nothing is appended afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopping_a_running_execution_settles_it_as_aborted() {
    let router = build_router();
    let case = "the stack declaring the two waiting machines";

    // The two machines a row picks between: one whose run would finish inside the test, one that
    // would still be waiting when it ends. The template names one and leaves the other's name to
    // CloudFormation, and publishes each machine's reference beside its `Name`.
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "ShortWait": {
                "Type": "AWS::StepFunctions::StateMachine",
                "Properties": {
                    "StateMachineName": "short-wait-workflow",
                    "RoleArn": ANOTHER_ACCOUNT,
                    "DefinitionString": wait_definition(SHORT_WAIT_SECONDS).to_string(),
                },
            },
            "LongWait": {
                "Type": "AWS::StepFunctions::StateMachine",
                "Properties": {
                    "RoleArn": ANOTHER_ACCOUNT,
                    "Definition": wait_definition(600),
                },
            },
        },
        "Outputs": {
            "ShortWaitArn": { "Value": { "Ref": "ShortWait" } },
            "ShortWaitName": { "Value": { "Fn::GetAtt": ["ShortWait", "Name"] } },
            "LongWaitArn": { "Value": { "Fn::GetAtt": ["LongWait", "Arn"] } },
            "LongWaitName": { "Value": { "Fn::GetAtt": ["LongWait", "Name"] } },
        },
    })
    .to_string();

    let outputs = deployed_outputs(&router, STOP_REGION, "stop-abort-stack", &template, case).await;

    for row in STOP_ROWS {
        let case = row.case;
        let machine_arn = output(&outputs, row.reference_output, case);
        let machine_name = output(&outputs, row.name_output, case);
        // Each row's machine is reached through the handle the stack published, which is that
        // machine's ARN (rule 1 again, for the machine this row runs).
        assert_eq!(
            machine_arn,
            state_machine_arn(STOP_REGION, &machine_name),
            "case {case:?}: expected the stack's reference to be the machine's ARN, got \
             {machine_arn:?}"
        );

        let started = sfn_ok(
            &router,
            STOP_REGION,
            "StartExecution",
            &json!({
                "stateMachineArn": machine_arn,
                "name": row.execution_name,
                "input": "{}",
            }),
            case,
        )
        .await;
        let execution_arn = string(&started, "executionArn", case);
        let running = sfn_ok(
            &router,
            STOP_REGION,
            "DescribeExecution",
            &json!({ "executionArn": execution_arn }),
            case,
        )
        .await;
        assert_eq!(
            running.get("status").and_then(Value::as_str),
            Some("RUNNING"),
            "case {case:?}: expected the waiting execution to be RUNNING before the stop, got \
             {running}"
        );

        let mut request = json!({ "executionArn": execution_arn });
        if let Some(error) = row.error {
            request["error"] = json!(error);
        }
        if let Some(cause) = row.cause {
            request["cause"] = json!(cause);
        }
        let stopped = sfn_ok(&router, STOP_REGION, "StopExecution", &request, case).await;
        let stop_date = stopped
            .get("stopDate")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a stopDate in {stopped}"));

        // The stop SETTLES the execution: the next read of it reports a terminal status rather
        // than a run still in flight.
        let described = settled(&router, STOP_REGION, &execution_arn, case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("ABORTED"),
            "case {case:?}: expected a stopped execution to be ABORTED, got {described}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            row.error,
            "case {case:?}: expected the error {:?} the stop supplied, got {described}",
            row.error
        );
        assert_eq!(
            described.get("cause").and_then(Value::as_str),
            row.cause,
            "case {case:?}: expected the cause {:?} the stop supplied, got {described}",
            row.cause
        );
        assert_eq!(
            described.get("stopDate"),
            Some(&stop_date),
            "case {case:?}: expected the stopDate the stop reported, got {described}"
        );

        // The history closes on the abort, carrying what the stop supplied.
        let events = history(&router, STOP_REGION, &execution_arn, case).await;
        let event_types = types(&events);
        assert_eq!(
            event_types.first().map(String::as_str),
            Some(EVENT_EXECUTION_STARTED),
            "case {case:?}: expected the history to open on {EVENT_EXECUTION_STARTED}, got \
             {event_types:?}"
        );
        assert_eq!(
            event_types.last().map(String::as_str),
            Some(EVENT_EXECUTION_ABORTED),
            "case {case:?}: expected the history to end on {EVENT_EXECUTION_ABORTED}, got \
             {event_types:?}"
        );
        let aborted = events
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: expected a history with events in it"));
        assert_eq!(
            aborted.get("id").and_then(Value::as_i64),
            Some(i64::try_from(events.len()).expect("the event count fits")),
            "case {case:?}: expected the abort to be the last of {} events, got {aborted}",
            events.len()
        );
        let details = aborted
            .get("executionAbortedEventDetails")
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected executionAbortedEventDetails in {aborted}")
            });
        assert_eq!(
            details.get("error").and_then(Value::as_str),
            row.error,
            "case {case:?}: expected the abort event to carry the error {:?}, got {details}",
            row.error
        );
        assert_eq!(
            details.get("cause").and_then(Value::as_str),
            row.cause,
            "case {case:?}: expected the abort event to carry the cause {:?}, got {details}",
            row.cause
        );

        // And the history is TRUNCATED there: the outcome the run was heading for never lands,
        // even once the instant it was due at has passed.
        ::tokio::time::sleep(row.settle_wait).await;
        let after = sfn_ok(
            &router,
            STOP_REGION,
            "DescribeExecution",
            &json!({ "executionArn": execution_arn }),
            case,
        )
        .await;
        assert_eq!(
            after.get("status").and_then(Value::as_str),
            Some("ABORTED"),
            "case {case:?}: expected a stopped execution to stay ABORTED, got {after}"
        );
        assert!(
            after.get("output").is_none_or(Value::is_null),
            "case {case:?}: expected a stopped execution to have no output, got {after}"
        );
        let later_types = types(&history(&router, STOP_REGION, &execution_arn, case).await);
        assert_eq!(
            later_types, event_types,
            "case {case:?}: expected nothing to be appended after the abort, got {later_types:?}"
        );
        for terminal in ["ExecutionSucceeded", "ExecutionFailed"] {
            assert!(
                !later_types.iter().any(|event_type| event_type == terminal),
                "case {case:?}: expected no {terminal} after the abort, got {later_types:?}"
            );
        }
    }
}
