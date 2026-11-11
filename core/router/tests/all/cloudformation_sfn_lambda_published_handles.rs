// SPDX-License-Identifier: CC0-1.0

//! What a CloudFormation stack PUBLISHES for the AWS Step Functions and AWS Lambda resources
//! it provisions, and what the owning service's refusal of one does to the stack.
//!
//! The rules under test:
//!
//! 1. **A provisioned resource's physical resource id — the value `DescribeStackResources`
//!    reports and `Ref` resolves to — is the identifier the service that OWNS the resource
//!    answers to**, never a name only CloudFormation knows it by: "Ref returns the ARN of the
//!    state machine" for an `AWS::StepFunctions::StateMachine`, "Ref returns the event source
//!    mapping's ID" for an `AWS::Lambda::EventSourceMapping`. So every operation of the owning
//!    service that takes that identifier resolves the resource under the published value, and
//!    the resource is really there: `DescribeStateMachine` and `StartSyncExecution` take an
//!    `Arn`-typed, `@required` `stateMachineArn` and answer `InvalidArn` (HTTP 400) for a value
//!    that is not one
//!    (`smithy_query(query="operation",operation="DescribeStateMachine,StartSyncExecution")`),
//!    which is what the machine's NAME — the separate `Fn::GetAtt … Name` attribute — gets.
//! 2. **Whatever the owning service answers short of success fails the resource**: a
//!    declaration Step Functions refuses leaves the stack `ROLLBACK_COMPLETE` naming the
//!    resource that failed, and what becomes of each PHYSICAL resource the rollback and the
//!    stack's own later deletion reach is that resource's own `DeletionPolicy` — so a sibling
//!    declared `Retain` is still held by Step Functions after both.
//!
//! Four crates have to agree for these to hold (CloudFormation computes the identity and makes
//! the create, Step Functions and Lambda serve it and judge it, IAM answers for the function's
//! execution role and Amazon SQS holds the event source), so the suite lives here rather than
//! in any one of them. `cloudformation_provisions_sfn_state_machines.rs` and
//! `cloudformation_provisions_lambda_event_source_mappings.rs` are the sibling suites over the
//! resources themselves; this one is over the HANDLE each stack publishes, read through
//! `DescribeStackResources`, and over what a refused declaration leaves behind.
//!
//! The rows differ along what the rules generalise over. For the state machines: which FORM
//! the declaration states the definition in (`DefinitionString`, the template object
//! `Definition`, and a `DefinitionString` whose placeholders `DefinitionSubstitutions` fills
//! in), whether the template NAMES the machine or leaves CloudFormation to generate its name,
//! and which `StateMachineType` it declares — which is also what decides whether a client runs
//! the published handle with `StartSyncExecution` or `StartExecution`. For the refusals: which
//! transition of the definition names a state nothing declares. For the mappings: the batch
//! size, whether polling is asked for, whether the declaration names the function by `Ref` (its
//! name) or by `Fn::GetAtt … Arn`, and whether the source queue redrives to a dead-letter queue
//! the same template declares.
//!
//! Every expected value is computed here from what the row declares, never read back out of the
//! answer being checked — except the UUID AWS Lambda assigns a mapping, which is that service's
//! to mint: what is checked about it is its form, that Lambda resolves it, and that the stack
//! published that same one.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder, the XML element readers and the keyed-list readers this test
// crate writes ONCE (`tests/all/query_wire.rs`): "Nothing new should write another one — call
// this module." The two stack waits are the crate's own as well — the one for a status a stack
// reaches, and the one for a deletion that has finished — as are the readers of a mapping AWS
// Lambda holds, and the SigV4 header builder signs for the same Region this suite's hosts name.
use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::cloudformation_provisions_lambda_event_source_mappings::{
    is_mapping_uuid, mapping_until_settled, mappings_of_source, MAPPING_STATE_DISABLED,
    MAPPING_STATE_ENABLED,
};
use crate::cloudformation_provisions_logs_log_groups::wait_for_stack_gone;
use crate::query_wire::{elements, form_encode, members_of, outputs_of, physical_ids_of};
use crate::rds_docdb_neptune_signing_name_routing::authorization;

/// The Region every request here is signed for — the Region the shared `wait_for_stack_status`
/// polls in — and so the Region the stacks' resources belong to.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The `StackStatus` values the rules are stated in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const ROLLBACK_COMPLETE: &str = "ROLLBACK_COMPLETE";

/// The `ResourceStatus` a resource whose create the owning service refused is recorded in.
const CREATE_FAILED: &str = "CREATE_FAILED";

/// The execution role every state machine declares. Nothing is known about it in IAM for the
/// state-machine rows, so the create is served — the documented reading of a role provisioned
/// outside the client that creates the resource.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/StatesExecutionRole";

/// The two `StateMachineType` values, from that member's own enum
/// (`smithy_query(query="shape", shape="StateMachineType")`).
const STANDARD: &str = "STANDARD";
const EXPRESS: &str = "EXPRESS";

/// What `StartSyncExecution` reports for a run that reached a terminal success
/// (`SyncExecutionStatus`: SUCCEEDED | FAILED | TIMED_OUT).
const SUCCEEDED: &str = "SUCCEEDED";

/// The longest name a state machine may carry, from the modelled `Name` (`@length(min: 1,
/// max: 80)`, `smithy_query(query="operation",operation="CreateStateMachine")`) — which the
/// name CloudFormation GENERATES for a declaration that states none must also respect.
const MAX_MACHINE_NAME: usize = 80;

/// The poller's own reads of a queue, which the function's execution role has to be granted
/// before Lambda will create a mapping onto one. Source: the actions AWS's own
/// `AWSLambdaSQSQueueExecutionRole` grants for an SQS event source.
const QUEUE_READS: &[&str] = &[
    "sqs:ReceiveMessage",
    "sqs:DeleteMessage",
    "sqs:GetQueueAttributes",
];

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other suite
/// here; the in-process callback CloudFormation makes signs its request either way (the
/// credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
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
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one Step Functions operation (`sigv4Name: states`, service shape `AWSStepFunctions`,
/// `awsJson1_0`). The `sync-` host prefix the model declares on `StartSyncExecution` is a DNS
/// detail of the real endpoint; what routes a request here is its credential scope, so one
/// caller serves every operation.
async fn sfn(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("states.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSStepFunctions.{operation}"))
        .header("authorization", authorization("states"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// One Amazon SQS call over its awsJson1_0 wire.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The ARN a state machine of `name` carries: the total function of the Region, the account and
/// the name both services compute it from.
fn state_machine_arn(name: &str) -> String {
    format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:{name}")
}

/// The prefix of the ARN one STANDARD execution of `machine` carries.
fn execution_arn_prefix(machine: &str) -> String {
    format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:{machine}:")
}

/// The ARN a queue of `name` carries.
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{name}")
}

/// The ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The error code an answer carries, as the awsJson envelope states it (`__type`, which may be
/// qualified by the shape's namespace).
fn error_code(answer: &Value) -> String {
    answer
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The physical id the stack published for each of its logical resources, read through
/// `DescribeStackResources` — the operation a client reads a deployed resource's handle with.
async fn published_ids(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> std::collections::BTreeMap<String, String> {
    let (status, body) = cloudformation(router, "DescribeStackResources", &[("StackName", stack)])
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with body \
         {body}"
    );
    physical_ids_of(&body)
}

/// One output of a stack that has completed, by key.
fn output(case: &str, described: &str, key: &str) -> String {
    let outputs = outputs_of(described);
    outputs
        .get(key)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an output {key:?} in {outputs:?}"))
}

// ---------------------------------------------------------------------------
// Rule 1, for AWS::StepFunctions::StateMachine
// ---------------------------------------------------------------------------

/// The placeholder one row's declaration carries, and the key `DefinitionSubstitutions` states
/// its value under: "specify values for the placeholder variables in your state machine
/// definition".
const PLACEHOLDER: &str = "${Outcome}";

/// How a declaration states the workflow. The type publishes three forms and the API takes
/// one, so all three have to arrive at the same definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefinitionForm {
    /// `DefinitionString`: the Amazon States Language document as TEXT.
    Text,
    /// `Definition`: that same document as a template OBJECT.
    Object,
    /// `DefinitionString` carrying a placeholder, with `DefinitionSubstitutions` stating the
    /// value for it.
    Substituted,
}

/// One row of the state-machine table: what the template declares, and what the handle the
/// stack publishes for it must then resolve to.
struct MachineRow {
    case: &'static str,
    stack: &'static str,
    /// The name the template declares, or `None` for a template that names nothing — whose
    /// machine carries the name CloudFormation generated.
    declared_name: Option<&'static str>,
    /// The `StateMachineType` the template declares, or `None` for a declaration that leaves it
    /// out — whose machine is the documented default, `STANDARD`.
    declared_type: Option<&'static str>,
    /// Which form the definition is stated in.
    form: DefinitionForm,
    /// The `Result` the definition's one state ends with, which is what a run of the published
    /// handle has to answer.
    result: &'static str,
}

const MACHINE_ROWS: &[MachineRow] = &[
    MachineRow {
        case: "a named STANDARD machine whose definition is DefinitionString",
        stack: "handle-standard-named",
        declared_name: Some("orders-standard"),
        declared_type: Some(STANDARD),
        form: DefinitionForm::Text,
        result: "standard",
    },
    MachineRow {
        case: "a machine CloudFormation names, declaring no type, whose definition is the \
               template object Definition",
        stack: "handle-default-generated",
        declared_name: None,
        declared_type: None,
        form: DefinitionForm::Object,
        result: "generated",
    },
    MachineRow {
        case: "a named EXPRESS machine whose definition carries DefinitionSubstitutions",
        stack: "handle-express-named",
        declared_name: Some("orders-express"),
        declared_type: Some(EXPRESS),
        form: DefinitionForm::Substituted,
        result: "expressed",
    },
    MachineRow {
        case: "an EXPRESS machine CloudFormation names",
        stack: "handle-express-generated",
        declared_name: None,
        declared_type: Some(EXPRESS),
        form: DefinitionForm::Text,
        result: "express-generated",
    },
];

/// A one-state definition that ends immediately, as a row states it.
fn pass_definition(result: &str) -> Value {
    json!({
        "StartAt": "Done",
        "States": { "Done": { "Type": "Pass", "Result": result, "End": true } },
    })
}

/// The template one state-machine row deploys: the machine it declares, and the two outputs a
/// client reads its handle and its name from.
fn machine_template(row: &MachineRow) -> String {
    let mut properties = json!({ "RoleArn": ROLE_ARN });
    if let Some(name) = row.declared_name {
        properties["StateMachineName"] = json!(name);
    }
    if let Some(kind) = row.declared_type {
        properties["StateMachineType"] = json!(kind);
    }
    match row.form {
        DefinitionForm::Text => {
            properties["DefinitionString"] = json!(pass_definition(row.result).to_string());
        }
        DefinitionForm::Object => {
            properties["Definition"] = pass_definition(row.result);
        }
        DefinitionForm::Substituted => {
            properties["DefinitionString"] = json!(pass_definition(PLACEHOLDER).to_string());
            properties["DefinitionSubstitutions"] = json!({ "Outcome": row.result });
        }
    }
    json!({
        "Resources": {
            "Machine": { "Type": "AWS::StepFunctions::StateMachine", "Properties": properties },
        },
        "Outputs": {
            "MachineRef": { "Value": { "Ref": "Machine" } },
            "MachineName": { "Value": { "Fn::GetAtt": ["Machine", "Name"] } },
        },
    })
    .to_string()
}

/// Every row: the physical resource id the stack publishes for its state machine is the ARN
/// Step Functions answers to, `Ref` resolves to that same value, the machine really holds the
/// definition the declaration stated, a run started against the published handle is served, and
/// the machine's NAME — a separate attribute, never the handle — is refused as `InvalidArn` by
/// the operations that take the ARN.
#[tokio::test]
async fn a_stack_publishes_the_state_machine_arn_step_functions_answers_to() {
    for row in MACHINE_ROWS {
        let router = build_router();
        let case = row.case;
        let template = machine_template(row);

        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[("StackName", row.stack), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        let (reached, described) = wait_for_stack_status(&router, row.stack, CREATE_COMPLETE).await;
        assert_eq!(
            reached, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described}"
        );

        // The machine's NAME, through the attribute the type publishes it as — so the name half
        // of the rule is known independently of the handle under test, including for the rows
        // whose name CloudFormation generated.
        let name = output(case, &described, "MachineName");
        match row.declared_name {
            Some(declared) => assert_eq!(
                name, declared,
                "case {case:?}: expected the Name attribute to be the declared name \
                 {declared:?}, got {name:?}"
            ),
            // A declaration that names nothing gets CloudFormation's generated
            // `<stack>-<logical id>-<unique id>` name, so the handle under test is known to be
            // built on a name of that form rather than on whatever the read answered.
            None => {
                let prefix = format!("{}-Machine-", row.stack);
                assert!(
                    name.starts_with(&prefix) && name.len() > prefix.len(),
                    "case {case:?}: expected a generated name under {prefix:?} with a unique \
                     segment, got {name:?}"
                );
                assert!(
                    name.len() <= MAX_MACHINE_NAME,
                    "case {case:?}: expected the generated name to stay inside the modelled \
                     1..{MAX_MACHINE_NAME}, got {name:?}"
                );
            }
        }

        // "Ref returns the ARN of the state machine": both the per-resource read and the
        // reference resolve to that ARN.
        let expected = state_machine_arn(&name);
        let published = published_ids(&router, case, row.stack).await;
        assert_eq!(
            published.get("Machine").map(String::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected DescribeStackResources to publish the machine's ARN \
             {expected:?}, got {published:?}"
        );
        let referenced = output(case, &described, "MachineRef");
        assert_eq!(
            referenced, expected,
            "case {case:?}: expected Ref to resolve to the machine's ARN {expected:?}, got \
             {referenced:?}"
        );

        // And the resource is really there, under exactly that handle.
        let (status, machine) = sfn(
            &router,
            "DescribeStateMachine",
            &json!({ "stateMachineArn": referenced }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeStateMachine to resolve the published handle, got \
             {status} with body {machine}"
        );
        assert_eq!(
            machine.get("name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected the machine to carry the name {name:?}, got {machine}"
        );
        let expected_type = row.declared_type.unwrap_or(STANDARD);
        assert_eq!(
            machine.get("type").and_then(Value::as_str),
            Some(expected_type),
            "case {case:?}: expected the machine to be a {expected_type} workflow, got {machine}"
        );
        let reported: Value = machine
            .get("definition")
            .and_then(Value::as_str)
            .map(|text| serde_json::from_str(text).unwrap_or(Value::Null))
            .unwrap_or(Value::Null);
        assert_eq!(
            reported,
            pass_definition(row.result),
            "case {case:?}: expected the definition the declaration stated, with its \
             placeholders filled in, got {reported}"
        );

        // A client RUNS the workflow with the published handle, through the operation its type
        // is served by: `StartSyncExecution` "is not available for STANDARD workflows".
        if expected_type == EXPRESS {
            let (status, run) = sfn(
                &router,
                "StartSyncExecution",
                &json!({ "stateMachineArn": referenced, "input": "{}" }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected StartSyncExecution to run the published handle, got \
                 {status} with body {run}"
            );
            assert_eq!(
                run.get("status").and_then(Value::as_str),
                Some(SUCCEEDED),
                "case {case:?}: expected the synchronous run to report {SUCCEEDED}, got {run}"
            );
            let answered: Value = run
                .get("output")
                .and_then(Value::as_str)
                .map(|text| serde_json::from_str(text).unwrap_or(Value::Null))
                .unwrap_or(Value::Null);
            assert_eq!(
                answered,
                json!(row.result),
                "case {case:?}: expected the run to answer the definition's own result, got {run}"
            );
        } else {
            let (status, run) = sfn(
                &router,
                "StartExecution",
                &json!({ "stateMachineArn": referenced, "input": "{}" }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected StartExecution to run the published handle, got {status} \
                 with body {run}"
            );
            let started = run
                .get("executionArn")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let prefix = execution_arn_prefix(&name);
            assert!(
                started.starts_with(&prefix),
                "case {case:?}: expected an execution ARN under {prefix:?}, got {started:?}"
            );
        }

        // The other half of the rule: the NAME is not the handle, and the operations that take
        // the ARN refuse it — which is what a stack publishing a name instead of an ARN costs
        // every consumer of the machine.
        // Each request carries only the members its own operation models: the read takes the
        // ARN alone, the start takes an input beside it, so what the refusal is about is
        // unambiguous.
        let refusals = [
            (
                "DescribeStateMachine",
                json!({ "stateMachineArn": name }),
            ),
            (
                "StartSyncExecution",
                json!({ "stateMachineArn": name, "input": "{}" }),
            ),
        ];
        for (operation, request) in &refusals {
            let (status, refused) = sfn(&router, operation, request).await;
            let code = error_code(&refused);
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidArn"),
                "case {case:?}: expected {operation} to refuse the bare name {name:?} with 400 \
                 InvalidArn, got {status} with body {refused}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a refused declaration, and what the retained sibling is left as
// ---------------------------------------------------------------------------

/// One row of the refusal table: a definition Step Functions will not take, stated as the
/// transition that names a state nothing declares.
struct RefusedRow {
    case: &'static str,
    stack: &'static str,
    /// The definition the failing resource declares.
    definition: fn() -> Value,
}

/// A state name no definition below declares, which is what makes each row's transition
/// dangle.
const UNDECLARED: &str = "NoSuchState";

const REFUSED_ROWS: &[RefusedRow] = &[
    RefusedRow {
        // The recorded shape: the entry point names a state the definition does not declare.
        case: "a StartAt naming a state the definition does not declare",
        stack: "refused-start-at",
        definition: || {
            json!({
                "StartAt": UNDECLARED,
                "States": { "Done": { "Type": "Pass", "End": true } },
            })
        },
    },
    RefusedRow {
        case: "a Next naming a state the definition does not declare",
        stack: "refused-next",
        definition: || {
            json!({
                "StartAt": "First",
                "States": { "First": { "Type": "Pass", "Next": UNDECLARED } },
            })
        },
    },
    RefusedRow {
        case: "a Choice whose Default names a state the definition does not declare",
        stack: "refused-choice-default",
        definition: || {
            json!({
                "StartAt": "Pick",
                "States": {
                    "Pick": {
                        "Type": "Choice",
                        "Choices": [{ "Variable": "$.x", "NumericEquals": 1, "Next": "Done" }],
                        "Default": UNDECLARED,
                    },
                    "Done": { "Type": "Succeed" },
                },
            })
        },
    },
];

/// The name of the machine each refusal row KEEPS, and of the one its stack fails on.
fn kept_machine(stack: &str) -> String {
    format!("{stack}-kept")
}
/// See [`kept_machine`].
fn doomed_machine(stack: &str) -> String {
    format!("{stack}-doomed")
}

/// The template one refusal row deploys: a machine whose `DeletionPolicy` is `Retain`, and
/// behind it the machine Step Functions refuses — which depends on the retained one, so the
/// walk really creates that one before it fails.
fn refused_template(row: &RefusedRow) -> String {
    json!({
        "Resources": {
            "Kept": {
                "Type": "AWS::StepFunctions::StateMachine",
                "DeletionPolicy": "Retain",
                "Properties": {
                    "StateMachineName": kept_machine(row.stack),
                    "RoleArn": ROLE_ARN,
                    "DefinitionString": pass_definition("kept").to_string(),
                },
            },
            "Doomed": {
                "Type": "AWS::StepFunctions::StateMachine",
                "DependsOn": ["Kept"],
                "Properties": {
                    "StateMachineName": doomed_machine(row.stack),
                    "RoleArn": ROLE_ARN,
                    "DefinitionString": (row.definition)().to_string(),
                },
            },
        },
    })
    .to_string()
}

/// Whether Step Functions still holds the machine of `name`, with the answer it gave.
async fn machine_is_held(router: &MultiServiceRouter, name: &str) -> (bool, Value) {
    let (status, body) = sfn(
        router,
        "DescribeStateMachine",
        &json!({ "stateMachineArn": state_machine_arn(name) }),
    )
    .await;
    (status == 200, body)
}

/// Every row: the declaration Step Functions refuses fails its resource, so the stack rolls
/// back naming it and nothing answers under the ARN that resource would have published — while
/// the sibling the template declared `Retain` is still held by Step Functions after the
/// rollback AND after the stack's own deletion.
#[tokio::test]
async fn a_refused_declaration_rolls_the_stack_back_and_keeps_a_retained_sibling() {
    for row in REFUSED_ROWS {
        let router = build_router();
        let case = row.case;
        let template = refused_template(row);

        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[("StackName", row.stack), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        let (reached, described) =
            wait_for_stack_status(&router, row.stack, ROLLBACK_COMPLETE).await;
        assert_eq!(
            reached, ROLLBACK_COMPLETE,
            "case {case:?}: expected the refused resource to roll the stack back to \
             {ROLLBACK_COMPLETE}, got {described}"
        );

        // The failure names the resource it was about: each event's status is read out of the
        // event that carries it, so a record missing one of the two members cannot misalign the
        // rest.
        let (status, events) =
            cloudformation(&router, "DescribeStackEvents", &[("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackEvents, got {status} with body \
             {events}"
        );
        let failed: Vec<String> = members_of(&events)
            .into_iter()
            .filter_map(|event| {
                let recorded = elements(event, "ResourceStatus").into_iter().next()?;
                let logical_id = elements(event, "LogicalResourceId").into_iter().next()?;
                (recorded == CREATE_FAILED).then_some(logical_id)
            })
            .collect();
        assert!(
            failed.iter().any(|logical_id| logical_id == "Doomed"),
            "case {case:?}: expected the refused resource to be recorded CREATE_FAILED, got \
             {failed:?} in {events}"
        );

        // Nothing answers under the ARN the failed resource would have published.
        let (held, answer) = machine_is_held(&router, &doomed_machine(row.stack)).await;
        assert!(
            !held,
            "case {case:?}: expected nothing to answer for the refused machine's ARN, got \
             {answer}"
        );

        // The retained sibling survives the rollback: what becomes of each physical resource is
        // its own `DeletionPolicy`.
        let (held, answer) = machine_is_held(&router, &kept_machine(row.stack)).await;
        assert!(
            held,
            "case {case:?}: expected the retained machine to survive the rollback, got {answer}"
        );

        // …and the stack's own deletion, which is the next thing a client does with a stack it
        // cannot deploy.
        let (status, deleted) =
            cloudformation(&router, "DeleteStack", &[("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {deleted}"
        );
        let (gone, last) = wait_for_stack_gone(&router, row.stack).await;
        assert!(
            gone,
            "case {case:?}: expected the stack's deletion to finish, last read {last}"
        );
        let (held, answer) = machine_is_held(&router, &kept_machine(row.stack)).await;
        assert!(
            held,
            "case {case:?}: expected the retained machine to survive the stack deletion, got \
             {answer}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 1, for AWS::Lambda::EventSourceMapping
// ---------------------------------------------------------------------------

/// The `maxReceiveCount` the row that declares a redrive policy states: how many receives a
/// message is moved to the dead-letter queue after.
const MAX_RECEIVE_COUNT: i64 = 3;

/// One row of the mapping table: what the template declares, and what Lambda must then hold
/// under the id the stack published.
struct MappingRow {
    case: &'static str,
    stack: &'static str,
    /// The source queue's name, which the row computes the source ARN from.
    queue: &'static str,
    /// The dead-letter queue the source redrives to, for a row that declares one.
    dead_letter: Option<&'static str>,
    /// The batch size the declaration states.
    batch_size: i64,
    /// `Enabled`, as the declaration states it, or `None` for one that leaves it out — whose
    /// mapping polls, which is the documented default.
    enabled: Option<bool>,
    /// Whether the declaration names the function by `Fn::GetAtt … Arn` rather than by `Ref`
    /// (its NAME): `EventSourceMapping$FunctionName` is "the name or ARN of the Lambda
    /// function", so both have to reach the same function.
    function_by_arn: bool,
}

const MAPPING_ROWS: &[MappingRow] = &[
    MappingRow {
        case: "a mapping naming its function by Ref, onto a queue redriving to a dead-letter \
               queue the same template declares",
        stack: "mapping-redrive",
        queue: "orders-inbox",
        dead_letter: Some("orders-failures"),
        batch_size: 10,
        enabled: Some(true),
        function_by_arn: false,
    },
    MappingRow {
        case: "a mapping naming its function by Fn::GetAtt Arn, declaring no Enabled",
        stack: "mapping-by-arn",
        queue: "orders-direct",
        dead_letter: None,
        batch_size: 1,
        enabled: None,
        function_by_arn: true,
    },
    MappingRow {
        case: "a mapping the declaration asks not to poll",
        stack: "mapping-paused",
        queue: "orders-paused",
        dead_letter: None,
        batch_size: 5,
        enabled: Some(false),
        function_by_arn: false,
    },
];

/// The name of the function one mapping row declares, and of the role it runs as.
fn worker_function(stack: &str) -> String {
    format!("{stack}-worker")
}
/// See [`worker_function`].
fn worker_role(stack: &str) -> String {
    format!("{stack}-exec")
}

/// The template one mapping row deploys: the source queue (with its dead-letter queue where the
/// row declares one), the execution role granted the poller's reads of that queue, the function
/// the mapping invokes, and the mapping itself.
fn mapping_template(row: &MappingRow) -> String {
    let function = worker_function(row.stack);
    let mut source = json!({ "QueueName": row.queue });
    if row.dead_letter.is_some() {
        source["RedrivePolicy"] = json!({
            "deadLetterTargetArn": { "Fn::GetAtt": ["DeadLetters", "Arn"] },
            "maxReceiveCount": MAX_RECEIVE_COUNT,
        });
    }
    let named_function = if row.function_by_arn {
        json!({ "Fn::GetAtt": ["Worker", "Arn"] })
    } else {
        json!({ "Ref": "Worker" })
    };
    let mut mapping = json!({
        "EventSourceArn": { "Fn::GetAtt": ["Source", "Arn"] },
        "FunctionName": named_function,
        "BatchSize": row.batch_size,
    });
    if let Some(enabled) = row.enabled {
        mapping["Enabled"] = json!(enabled);
    }
    let mut resources = json!({
        "Source": { "Type": "AWS::SQS::Queue", "Properties": source },
        "ExecRole": {
            "Type": "AWS::IAM::Role",
            "Properties": {
                "RoleName": worker_role(row.stack),
                "AssumeRolePolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": "lambda.amazonaws.com" },
                        "Action": "sts:AssumeRole",
                    }],
                },
                "Policies": [{
                    "PolicyName": "queue-reads",
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Action": QUEUE_READS,
                            "Resource": queue_arn(row.queue),
                        }],
                    },
                }],
            },
        },
        "Worker": {
            "Type": "AWS::Lambda::Function",
            "Properties": {
                "FunctionName": function,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Role": { "Fn::GetAtt": ["ExecRole", "Arn"] },
                "Code": {
                    "ZipFile": "def handler(event, context):\n    return len(event['Records'])\n",
                },
            },
        },
        "Mapping": {
            "Type": "AWS::Lambda::EventSourceMapping",
            "Properties": mapping,
        },
    });
    if let Some(dead_letter) = row.dead_letter {
        resources["DeadLetters"] =
            json!({ "Type": "AWS::SQS::Queue", "Properties": { "QueueName": dead_letter } });
        resources["Source"]["DependsOn"] = json!(["DeadLetters"]);
    }
    json!({
        "Resources": resources,
        "Outputs": {
            "MappingRef": { "Value": { "Ref": "Mapping" } },
            "SourceQueueUrl": { "Value": { "Ref": "Source" } },
        },
    })
    .to_string()
}

/// Every row: the physical resource id the stack publishes for its mapping is the UUID Lambda
/// minted, `Ref` resolves to that same value, Lambda holds the mapping under it reading the
/// source the template named and invoking the function the stack created, and the mapping is
/// the one `ListEventSourceMappings` reports for that source.
#[tokio::test]
async fn a_stack_publishes_the_uuid_lambda_minted_for_its_event_source_mapping() {
    for row in MAPPING_ROWS {
        let router = build_router();
        let case = row.case;
        let template = mapping_template(row);
        let source = queue_arn(row.queue);
        let function = worker_function(row.stack);

        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", &template),
                ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        let (reached, described) = wait_for_stack_status(&router, row.stack, CREATE_COMPLETE).await;
        assert_eq!(
            reached, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described}"
        );

        // "Ref returns the event source mapping's ID": the per-resource read and the reference
        // agree, and what they carry is an id of the form Lambda mints.
        let published = published_ids(&router, case, row.stack).await;
        let uuid = published.get("Mapping").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: expected a physical id for the mapping, got {published:?}")
        });
        assert!(
            is_mapping_uuid(&uuid),
            "case {case:?}: expected the published id to be the UUID Lambda assigns a mapping, \
             got {uuid:?}"
        );
        let referenced = output(case, &described, "MappingRef");
        assert_eq!(
            referenced, uuid,
            "case {case:?}: expected Ref to resolve to the published id {uuid:?}, got \
             {referenced:?}"
        );

        // Lambda holds the mapping under exactly that id, wired as the template declared it.
        let settled = mapping_until_settled(&router, case, &uuid).await;
        assert_eq!(
            settled.get("UUID").and_then(Value::as_str),
            Some(uuid.as_str()),
            "case {case:?}: expected the mapping to carry the published id, got {settled}"
        );
        assert_eq!(
            settled.get("EventSourceArn").and_then(Value::as_str),
            Some(source.as_str()),
            "case {case:?}: expected the mapping to read the source the template named, got \
             {settled}"
        );
        assert_eq!(
            settled.get("FunctionArn").and_then(Value::as_str),
            Some(function_arn(&function).as_str()),
            "case {case:?}: expected the mapping to invoke the function the stack created, got \
             {settled}"
        );
        assert_eq!(
            settled.get("BatchSize").and_then(Value::as_i64),
            Some(row.batch_size),
            "case {case:?}: expected the declared batch size {}, got {settled}",
            row.batch_size
        );
        let expected_state = if row.enabled == Some(false) {
            MAPPING_STATE_DISABLED
        } else {
            MAPPING_STATE_ENABLED
        };
        assert_eq!(
            settled.get("State").and_then(Value::as_str),
            Some(expected_state),
            "case {case:?}: expected the mapping to settle at {expected_state}, got {settled}"
        );

        // And it is the mapping Lambda reports FOR that source, which is how a client reaches a
        // deployed trigger it has only the queue's ARN for.
        let held = mappings_of_source(&router, case, &source).await;
        let ids: Vec<String> = held
            .iter()
            .filter_map(|mapping| mapping.get("UUID").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        assert_eq!(
            ids,
            vec![uuid.clone()],
            "case {case:?}: expected the source to be read by exactly the mapping the stack \
             published, got {held:?}"
        );

        // The row that declares a dead-letter queue really wired one: the source queue's own
        // redrive policy names it, as the attribute a template's property IS.
        if let Some(dead_letter) = row.dead_letter {
            let queue_url = output(case, &described, "SourceQueueUrl");
            let (status, attributes) = sqs(
                &router,
                "GetQueueAttributes",
                &json!({ "QueueUrl": queue_url, "AttributeNames": ["RedrivePolicy"] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetQueueAttributes, got {status} with body \
                 {attributes}"
            );
            let declared: Value = attributes
                .get("Attributes")
                .and_then(|map| map.get("RedrivePolicy"))
                .and_then(Value::as_str)
                .map(|text| serde_json::from_str(text).unwrap_or(Value::Null))
                .unwrap_or(Value::Null);
            assert_eq!(
                declared,
                json!({
                    "deadLetterTargetArn": queue_arn(dead_letter),
                    "maxReceiveCount": MAX_RECEIVE_COUNT,
                }),
                "case {case:?}: expected the source queue to redrive to the dead-letter queue \
                 the template declared, got {attributes}"
            );
        }
    }
}
