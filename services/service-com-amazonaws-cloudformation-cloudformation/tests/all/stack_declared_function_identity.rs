// SPDX-License-Identifier: CC0-1.0

//! The `AWS::Lambda::Function` a completed stack declares, read back through AWS Lambda's
//! own `GetFunction`.
//!
//! RULE: a stack reaches a successful terminal status only once every resource it declares
//! really exists in the service that OWNS it, so the owning service's own read resolves that
//! resource under the identity the stack published — for an `AWS::Lambda::Function` that read
//! is `GetFunction`, which answers 200 for EVERY identifier form its `FunctionName` member
//! accepts (name-only, function ARN, partial ARN) and reports the function out of its
//! provisioning window, with the configuration the declaration stated.
//!
//! A client that deploys a function by template and then waits on it — `GetFunction`, or the
//! `FunctionActiveV2` waiter built on it — must find it there: a stack that reports success
//! while AWS Lambda answers `ResourceNotFoundException` fails every downstream step far from
//! its cause.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! function or leaves CloudFormation to name it, the runtime family the declaration states
//! (so the inline source is archived under a different handler file), how many functions one
//! stack declares, which optional configuration members the declaration carries (and so
//! whether the published default or the declared value is reported), and whether
//! the stack also declares the `AWS::Logs::LogGroup` at the function's own log-group name —
//! the shape a deployment that manages its function's logs itself has.
//!
//! Every expected value is computed here from what the row declares plus the identifiers the
//! stack publishes through its outputs; nothing is read out of the `GetFunction` answer being
//! checked.

use std::time::Duration;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::hotswapped_function_code::lambda;
use crate::stack_lifecycle::{
    acknowledging_iam, create_stack, describe_stack, form_encode, output_value, router, xml_text,
    ACCOUNT_ID, REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// How often a poll looks again, and how many times it looks: 400 looks 25 ms apart is ten
/// seconds, far past the span any transition here is owed, so the deadline is generous on a
/// loaded host while the loop still notices the settled state as soon as it holds.
const LOOK_AGAIN: Duration = Duration::from_millis(25);
const LOOKS: usize = 400;

/// The logical ids every template below declares its shared resources under.
const ROLE_LOGICAL_ID: &str = "ExecutionRole";
const GROUP_LOGICAL_ID: &str = "LogGroup";

/// The output key every template publishes the execution role's ARN under.
const ROLE_ARN_OUTPUT: &str = "RoleArn";

/// An AWS-managed policy ARN: its account segment is `aws`, so it is AWS's own resource and
/// IAM's `AttachRolePolicy` takes it as present — the policy a Lambda execution role is
/// normally given.
const MANAGED_POLICY_ARN: &str = "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole";

/// The `State` AWS Lambda reports for a function it takes calls on. Source: the model's
/// `State` enum (`Pending | Active | Inactive | Failed`) and its documentation — "the
/// function is ready to be invoked" only in `Active`.
const STATE_ACTIVE: &str = "Active";

/// What AWS Lambda reports for the two members a declaration may leave out. Source: the
/// members' own documentation on `CreateFunction` — `MemorySize` "The default value is 128
/// MB", `Timeout` "The default is 3 seconds".
const DEFAULT_MEMORY_SIZE_MB: i64 = 128;
const DEFAULT_TIMEOUT_SECONDS: i64 = 3;

/// The trust policy a function's execution role is declared with: the service principal AWS
/// Lambda assumes the role as.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "lambda.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
}

/// The inline source one row's function is declared with, for the runtime family it names —
/// so the declared text is a handler of that language rather than one language's text under
/// every runtime.
fn inline_source(runtime: &str) -> &'static str {
    if runtime.starts_with("nodejs") {
        "export const handler = async (event) => ({ ok: true });\n"
    } else if runtime.starts_with("ruby") {
        "def handler(event:, context:)\n  { ok: true }\nend\n"
    } else {
        "def handler(event, context):\n    return {'ok': True}\n"
    }
}

/// One function a row's stack declares, and what the declaration states about it.
struct Declared<'a> {
    logical_id: &'a str,
    /// The `FunctionName` the declaration states, or `None` to leave the naming to
    /// CloudFormation — the physical id the stack then publishes through `Ref`.
    declared_name: Option<&'a str>,
    runtime: &'a str,
    handler: &'a str,
    /// The optional configuration members the declaration carries, each as AWS Lambda must
    /// report it back.
    memory_size: Option<i64>,
    timeout: Option<i64>,
    description: Option<&'a str>,
    /// The output keys the stack publishes this function's ARN and its `Ref` under.
    arn_output: &'a str,
    name_output: &'a str,
}

/// One row of the table.
struct Case<'a> {
    case: &'a str,
    stack: &'a str,
    /// The log-group name the row's template declares an `AWS::Logs::LogGroup` at, which
    /// every function in the row then `DependsOn`; `None` for a row that declares none.
    declared_group: Option<&'a str>,
    functions: &'a [Declared<'a>],
}

/// The template a row deploys: the execution role, the log group the row declares, and each
/// declared function — with the outputs that publish the identifiers the stack decides.
fn template(case: &Case<'_>) -> String {
    let mut resources = Map::new();
    resources.insert(
        ROLE_LOGICAL_ID.to_string(),
        json!({
            "Type": "AWS::IAM::Role",
            "Properties": {
                "AssumeRolePolicyDocument": trust_policy(),
                "ManagedPolicyArns": [MANAGED_POLICY_ARN],
            },
        }),
    );
    let mut outputs = Map::new();
    outputs.insert(
        ROLE_ARN_OUTPUT.to_string(),
        json!({"Value": {"Fn::GetAtt": [ROLE_LOGICAL_ID, "Arn"]}}),
    );
    if let Some(group) = case.declared_group {
        resources.insert(
            GROUP_LOGICAL_ID.to_string(),
            json!({
                "Type": "AWS::Logs::LogGroup",
                "Properties": {"LogGroupName": group, "RetentionInDays": 3},
            }),
        );
    }
    for function in case.functions {
        let mut properties = json!({
            "Runtime": function.runtime,
            "Handler": function.handler,
            "Role": {"Fn::GetAtt": [ROLE_LOGICAL_ID, "Arn"]},
            "Code": {"ZipFile": inline_source(function.runtime)},
        });
        if let Some(name) = function.declared_name {
            properties["FunctionName"] = json!(name);
        }
        if let Some(memory) = function.memory_size {
            properties["MemorySize"] = json!(memory);
        }
        if let Some(timeout) = function.timeout {
            properties["Timeout"] = json!(timeout);
        }
        if let Some(description) = function.description {
            properties["Description"] = json!(description);
        }
        let mut declaration = json!({
            "Type": "AWS::Lambda::Function",
            "Properties": properties,
        });
        if case.declared_group.is_some() {
            declaration["DependsOn"] = json!(GROUP_LOGICAL_ID);
        }
        resources.insert(function.logical_id.to_string(), declaration);
        outputs.insert(
            function.arn_output.to_string(),
            json!({"Value": {"Fn::GetAtt": [function.logical_id, "Arn"]}}),
        );
        outputs.insert(
            function.name_output.to_string(),
            json!({"Value": {"Ref": function.logical_id}}),
        );
    }
    json!({"Resources": Value::Object(resources), "Outputs": Value::Object(outputs)}).to_string()
}

/// The stack's body once the operation in flight on it is no longer in progress: looked for
/// again until it settles or the budget above runs out, so nothing here depends on how fast
/// the host runs.
async fn settled_stack(router: &MultiServiceRouter, case: &str, stack: &str) -> (String, String) {
    let mut last = (String::new(), String::new());
    for _ in 0..LOOKS {
        let (code, body) = describe_stack(router, stack).await;
        assert_eq!(
            code, 200,
            "case {case:?}: DescribeStacks({stack:?}) must answer 200, got {code}: {body}"
        );
        let reported = xml_text(&body, "StackStatus").unwrap_or_default();
        let settled = !reported.ends_with("_IN_PROGRESS");
        last = (reported, body);
        if settled {
            return last;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    last
}

/// Every identifier form `GetFunction` resolves a function by, each paired with the name of
/// that form.
///
/// Source: the `FunctionName` member's own documentation on `GetFunction` — "Function name –
/// my-function (name-only) … Function ARN –
/// arn:aws:lambda:us-west-2:123456789012:function:my-function … Partial ARN –
/// 123456789012:function:my-function".
fn identifier_forms(name: &str, arn: &str) -> Vec<(&'static str, String)> {
    vec![
        ("name-only", name.to_string()),
        ("function ARN", arn.to_string()),
        ("partial ARN", format!("{ACCOUNT_ID}:function:{name}")),
    ]
}

/// `GetFunction` for one identifier, as AWS Lambda's model binds it: `GET
/// /2015-03-31/functions/{FunctionName}`, with the identifier percent-encoded the way a
/// non-greedy `@httpLabel` is sent.
async fn get_function(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    // The escaping a non-greedy `@httpLabel` gets is the unreserved-set percent-encoding
    // this target already has one spelling of (`stack_lifecycle::form_encode`), so an ARN's
    // colons travel as `%3A` rather than as path separators.
    let answered = lambda(
        router,
        "GET",
        &format!("/2015-03-31/functions/{}", form_encode(identifier)),
        &[],
        &[],
    )
    .await;
    (answered.status, answered.body)
}

/// The text one member of a `GetFunction` answer's `Configuration` carries.
fn reported(configuration: &Value, member: &str) -> Option<String> {
    configuration
        .get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[tokio::test]
async fn a_declared_function_resolves_through_lambdas_own_read_once_the_stack_completes() {
    let cases: &[Case<'_>] = &[
        Case {
            // The issue's own shape: a NAMED function, its execution role, and the log group
            // declared at that function's own log-group name, which the function depends on.
            case: "a named function beside the log group declared at its own name",
            stack: "declared-function-with-its-group",
            declared_group: Some("/aws/lambda/declared-identity-fn"),
            functions: &[Declared {
                logical_id: "Fn",
                declared_name: Some("declared-identity-fn"),
                runtime: "python3.12",
                handler: "index.handler",
                memory_size: None,
                timeout: None,
                description: None,
                arn_output: "FnArn",
                name_output: "FnName",
            }],
        },
        Case {
            // No `FunctionName`: CloudFormation decides the physical id, and the identity
            // the stack publishes is the one AWS Lambda must answer for.
            case: "a function CloudFormation names, with its optional configuration declared",
            stack: "generated-function-name",
            declared_group: None,
            functions: &[Declared {
                logical_id: "Worker",
                declared_name: None,
                runtime: "nodejs20.x",
                handler: "index.handler",
                memory_size: Some(512),
                timeout: Some(45),
                description: Some("reads the queue"),
                arn_output: "WorkerArn",
                name_output: "WorkerName",
            }],
        },
        Case {
            // Three functions in ONE stack, over three runtime families and both naming
            // forms: the rule holds per resource, not per stack.
            case: "three functions of different runtime families in one stack",
            stack: "many-declared-functions",
            declared_group: None,
            functions: &[
                Declared {
                    logical_id: "Reader",
                    declared_name: Some("declared-identity-reader"),
                    runtime: "python3.13",
                    handler: "app.handler",
                    memory_size: Some(128),
                    timeout: Some(3),
                    description: None,
                    arn_output: "ReaderArn",
                    name_output: "ReaderName",
                },
                Declared {
                    logical_id: "Writer",
                    declared_name: None,
                    runtime: "nodejs22.x",
                    handler: "index.handler",
                    memory_size: None,
                    timeout: Some(900),
                    description: Some("writes the ledger"),
                    arn_output: "WriterArn",
                    name_output: "WriterName",
                },
                Declared {
                    logical_id: "Auditor",
                    declared_name: Some("declared-identity-auditor"),
                    runtime: "ruby3.3",
                    handler: "index.handler",
                    memory_size: Some(10240),
                    timeout: None,
                    description: None,
                    arn_output: "AuditorArn",
                    name_output: "AuditorName",
                },
            ],
        },
    ];

    for case in cases {
        let router = router();
        let body = template(case);
        let (code, created) = create_stack(&router, case.stack, &body, &acknowledging_iam(&[])).await;
        assert_eq!(
            code, 200,
            "case {:?}: CreateStack must answer 200, got {code}: {created}",
            case.case
        );

        let (reported_status, described) = settled_stack(&router, case.case, case.stack).await;
        assert_eq!(
            reported_status,
            status::CREATE_COMPLETE,
            "case {:?}: the stack must reach CREATE_COMPLETE, got {reported_status:?} with \
             {described}",
            case.case
        );
        let role_arn = output_value(&described, ROLE_ARN_OUTPUT).unwrap_or_else(|| {
            panic!(
                "case {:?}: the stack must publish the execution role's ARN, got {described}",
                case.case
            )
        });

        for function in case.functions {
            let name = output_value(&described, function.name_output).unwrap_or_else(|| {
                panic!(
                    "case {:?}: the stack must publish {:?}'s name, got {described}",
                    case.case, function.logical_id
                )
            });
            let arn = output_value(&described, function.arn_output).unwrap_or_else(|| {
                panic!(
                    "case {:?}: the stack must publish {:?}'s ARN, got {described}",
                    case.case, function.logical_id
                )
            });
            if let Some(declared) = function.declared_name {
                assert_eq!(
                    name, declared,
                    "case {:?}: the stack must publish the declared name for {:?}, got {name:?}",
                    case.case, function.logical_id
                );
            }
            // The ARN a client builds for the published name, so the identity the stack
            // reports is checked against the form rather than against itself.
            assert_eq!(
                arn,
                format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:{name}"),
                "case {:?}: the ARN the stack publishes for {:?} must be the function ARN of \
                 {name:?}, got {arn:?}",
                case.case,
                function.logical_id
            );

            for (form, identifier) in identifier_forms(&name, &arn) {
                let (code, answered) = get_function(&router, &identifier).await;
                assert_eq!(
                    code, 200,
                    "case {:?}: GetFunction({identifier:?}) by {form} must answer 200 for the \
                     function the stack reports as created, got {code}: {answered}",
                    case.case
                );
                let configuration = answered.get("Configuration").cloned().unwrap_or(Value::Null);
                assert_eq!(
                    reported(&configuration, "FunctionName").as_deref(),
                    Some(name.as_str()),
                    "case {:?}: GetFunction({identifier:?}) by {form} must report the function \
                     {name:?}, got {configuration}",
                    case.case
                );
                assert_eq!(
                    reported(&configuration, "FunctionArn").as_deref(),
                    Some(arn.as_str()),
                    "case {:?}: GetFunction({identifier:?}) by {form} must report the ARN \
                     {arn:?}, got {configuration}",
                    case.case
                );
                assert_eq!(
                    reported(&configuration, "State").as_deref(),
                    Some(STATE_ACTIVE),
                    "case {:?}: GetFunction({identifier:?}) by {form} must report the function \
                     out of its provisioning window once the stack is complete, got \
                     {configuration}",
                    case.case
                );
                assert_eq!(
                    reported(&configuration, "Runtime").as_deref(),
                    Some(function.runtime),
                    "case {:?}: GetFunction({identifier:?}) by {form} must report the declared \
                     runtime, got {configuration}",
                    case.case
                );
                assert_eq!(
                    reported(&configuration, "Handler").as_deref(),
                    Some(function.handler),
                    "case {:?}: GetFunction({identifier:?}) by {form} must report the declared \
                     handler, got {configuration}",
                    case.case
                );
                assert_eq!(
                    reported(&configuration, "Role").as_deref(),
                    Some(role_arn.as_str()),
                    "case {:?}: GetFunction({identifier:?}) by {form} must report the role the \
                     stack published, got {configuration}",
                    case.case
                );
                // A member the declaration left out is reported at the default its own
                // documentation publishes, not left unset.
                let expected_memory = function.memory_size.unwrap_or(DEFAULT_MEMORY_SIZE_MB);
                assert_eq!(
                    configuration.get("MemorySize").and_then(Value::as_i64),
                    Some(expected_memory),
                    "case {:?}: GetFunction({identifier:?}) by {form} must report MemorySize \
                     {expected_memory}, got {configuration}",
                    case.case
                );
                let expected_timeout = function.timeout.unwrap_or(DEFAULT_TIMEOUT_SECONDS);
                assert_eq!(
                    configuration.get("Timeout").and_then(Value::as_i64),
                    Some(expected_timeout),
                    "case {:?}: GetFunction({identifier:?}) by {form} must report Timeout \
                     {expected_timeout}, got {configuration}",
                    case.case
                );
                if let Some(description) = function.description {
                    assert_eq!(
                        reported(&configuration, "Description").as_deref(),
                        Some(description),
                        "case {:?}: GetFunction({identifier:?}) by {form} must report the \
                         declared Description, got {configuration}",
                        case.case
                    );
                }
            }
        }
    }
}
