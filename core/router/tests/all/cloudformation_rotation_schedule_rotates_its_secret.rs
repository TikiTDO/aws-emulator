// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack's `AWS::SecretsManager::RotationSchedule` really makes its secret
//! rotate: the rotation function the same stack deploys is driven through the four documented
//! steps, and `AWSCURRENT` ends up on the version that function wrote.
//!
//! The rule under test: **a stack resource that declares a rotation for a secret puts that
//! rotation in force in AWS Secrets Manager itself — the function and the schedule it names
//! become the secret's own rotation configuration — so every rotation of that secret
//! afterwards invokes that function once per documented step, in the order
//! `createSecret`, `setSecret`, `testSecret`, `finishSecret` for one `ClientRequestToken`,
//! and ends with `AWSCURRENT` on the version the function created and `AWSPREVIOUS` on the
//! one it replaced.** Five crates have to agree for that to hold (CloudFormation makes the
//! calls, Secrets Manager holds the configuration and drives the rotation, Lambda runs the
//! function, IAM answers for its execution role and for the grant that lets Secrets Manager
//! invoke it, STS mints the session the function's own calls are signed with), so the suite
//! lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: how the schedule states its rotation
//! rules (`AutomaticallyAfterDays`, a `rate()` schedule expression with a `Duration`, a
//! `cron()` one), and whether the deployment itself starts a rotation
//! (`RotateImmediatelyOnUpdate` left out, stated `true`, stated `false`). Every expected
//! value is computed here from what the row declares and from the value the previous read
//! answered, never read back out of the answer being checked.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element, outputs_of};

/// The Region every request here is signed for, and so the Region the stack's resources live
/// in.
const REGION: &str = "us-west-2";

/// The service principal Secrets Manager invokes a rotation function as ("ensure that the
/// function policy grants access to the principal `secretsmanager.amazonaws.com`", Secrets
/// Manager User Guide, *Permissions for rotation*).
const SERVICE_PRINCIPAL: &str = "secretsmanager.amazonaws.com";

/// The four steps AWS invokes a rotation function with, in order: "Secrets Manager calls the
/// Lambda rotation function four times, each time with a different value for the Step
/// parameter" (Secrets Manager User Guide, *How rotation works*).
const ROTATION_STEPS: [&str; 4] = ["createSecret", "setSecret", "testSecret", "finishSecret"];

/// The value the stack's secret is created with, which every rotation extends.
const INITIAL_VALUE: &str = "{\"username\":\"appuser\",\"password\":\"initial\"}";

/// What the rotation function appends to the value it stages, so the value a read answers
/// says how many rotations wrote it.
const ROTATED_SUFFIX: &str = "-rotated";

/// The tag the rotation function records the steps it was invoked with under, in order — the
/// suite's own window onto the invocation sequence, read back through `DescribeSecret`.
const STEPS_TAG: &str = "RotationSteps";

/// How long one invocation of the rotation function is given, as the function's own
/// `Timeout`: well past what a run needs, because what is under test is the rotation protocol
/// and not how fast an interpreter gets through a handler.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long this suite waits for a condition another service owes, and how often it looks
/// while waiting. Generous: every wait here is for a CONDITION, and the budget is bounded
/// only so a suite cannot hang — a whole rotation runs four interpreted invocations behind
/// each wait, so the budget has to hold on a host that is busy building and running the rest
/// of the workspace rather than only on an idle one.
const POLL: Duration = Duration::from_millis(50);
/// See [`POLL`].
const POLLS: usize = 4_800;

/// The emulator as it is ASSEMBLED, on one engine (`aws_emulator_router::build_router`): the
/// rotation function's own SDK calls are dispatched back into this same router, so the secret
/// it writes is the secret this suite reads.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request and what each handler reads its Region from.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
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

/// Call one AWS Secrets Manager operation over its awsJson1_1 wire.
async fn secretsmanager(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("secretsmanager.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", authorization("secretsmanager"))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The same, requiring the 200 the operation answers.
async fn secretsmanager_ok(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
) -> Value {
    let (status, answered) = secretsmanager(router, operation, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {status} with body {answered}"
    );
    answered
}

/// Poll `stack` until CloudFormation first reports `wanted`, and answer the last status it
/// reported with the `DescribeStacks` answer that status was read from.
async fn wait_for_stack_status(
    router: &MultiServiceRouter,
    stack: &str,
    wanted: &str,
) -> (String, String) {
    let mut last = (String::new(), String::new());
    for _ in 0..POLLS {
        let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            status, 200,
            "DescribeStacks {stack:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = opt_element(&body, "StackStatus").unwrap_or_default();
        last = (reported, body);
        if last.0 == wanted {
            return last;
        }
        tokio::time::sleep(POLL).await;
    }
    last
}

/// Why each resource of `stack` ended where it did, for a report about a stack that did not
/// reach the status a case expected.
async fn failure_reasons(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (_, events) = cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
    elements(&events, "ResourceStatusReason")
}

/// The rotation function every row deploys: the four-step rotation AWS documents, written the
/// way the AWS rotation templates are. It reads the secret's own metadata, refuses a step
/// whose version is not staged, records the step it was invoked with, stages its new value on
/// `createSecret`, proves that value is readable on `testSecret` and moves `AWSCURRENT` onto
/// it on `finishSecret` — all through this service's own operations, so every version the
/// rotation produces is one the function itself wrote.
const ROTATION_FUNCTION: &str = r#"
import boto3

SUFFIX = "-rotated"
STEPS_TAG = "RotationSteps"


def record(client, arn, tags, step):
    recorded = ""
    for tag in tags or []:
        if tag["Key"] == STEPS_TAG:
            recorded = tag["Value"]
    recorded = recorded + "," + step if recorded else step
    client.tag_resource(SecretId=arn, Tags=[{"Key": STEPS_TAG, "Value": recorded}])


def lambda_handler(event, context):
    arn = event["SecretId"]
    token = event["ClientRequestToken"]
    step = event["Step"]
    client = boto3.client("secretsmanager")
    metadata = client.describe_secret(SecretId=arn)
    if not metadata.get("RotationEnabled"):
        raise ValueError("Secret %s is not enabled for rotation" % arn)
    versions = metadata["VersionIdsToStages"]
    if token not in versions:
        raise ValueError(
            "Secret version %s has no stage for rotation of secret %s." % (token, arn))
    staged = versions[token]
    if "AWSPENDING" not in staged and "AWSCURRENT" not in staged:
        raise ValueError(
            "Secret version %s not set as AWSPENDING for rotation of secret %s." % (token, arn))
    record(client, arn, metadata.get("Tags"), step)
    if step == "createSecret":
        current = client.get_secret_value(SecretId=arn, VersionStage="AWSCURRENT")
        client.put_secret_value(
            SecretId=arn,
            ClientRequestToken=token,
            SecretString=current["SecretString"] + SUFFIX,
            VersionStages=["AWSPENDING"],
        )
    elif step == "setSecret":
        pass
    elif step == "testSecret":
        pending = client.get_secret_value(SecretId=arn, VersionId=token)
        if not pending["SecretString"].endswith(SUFFIX):
            raise ValueError("the staged value was not the one createSecret wrote")
    elif step == "finishSecret":
        previous = None
        for version, stages in versions.items():
            if "AWSCURRENT" in stages:
                previous = version
        client.update_secret_version_stage(
            SecretId=arn,
            VersionStage="AWSCURRENT",
            MoveToVersionId=token,
            RemoveFromVersionId=previous,
        )
    else:
        raise ValueError("Invalid step parameter %s" % step)
    return {"step": step}
"#;

/// One row of the table: how the schedule is declared, and what the rule says the deployment
/// must then leave behind.
struct Row {
    case: &'static str,
    /// The `RotationRules` the schedule declares, which `DescribeSecret` must then report
    /// unchanged.
    rules: fn() -> Value,
    /// What the declaration states for `RotateImmediatelyOnUpdate`, or `None` where it states
    /// nothing (AWS documents the default as rotating immediately).
    rotate_immediately: Option<bool>,
    /// How many rotations the DEPLOYMENT itself must have completed by the time the stack
    /// reports complete.
    rotations_on_deploy: usize,
}

const ROWS: &[Row] = &[
    Row {
        case: "a day-interval schedule, with the start of the rotation left to the default",
        rules: || json!({ "AutomaticallyAfterDays": 30 }),
        rotate_immediately: None,
        rotations_on_deploy: 1,
    },
    Row {
        case: "a rate() schedule expression with a rotation window, rotating immediately",
        rules: || json!({ "ScheduleExpression": "rate(12 hours)", "Duration": "2h" }),
        rotate_immediately: Some(true),
        rotations_on_deploy: 1,
    },
    Row {
        case: "a cron() schedule expression with the start of the rotation held back",
        rules: || json!({ "ScheduleExpression": "cron(0 8 1 * ? *)" }),
        rotate_immediately: Some(false),
        rotations_on_deploy: 0,
    },
];

/// The policy that lets AWS Lambda assume the rotation function's execution role.
fn assume_role_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "lambda.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
}

/// The template one row deploys: the secret, the rotation function with its execution role,
/// the grant that lets Secrets Manager invoke it, and the schedule that binds the two.
fn template_for(row: &Row, stack: &str) -> String {
    let mut schedule = json!({
        "SecretId": { "Ref": "AppSecret" },
        "RotationLambdaARN": { "Fn::GetAtt": ["RotationFunction", "Arn"] },
        "RotationRules": (row.rules)()
    });
    if let Some(immediately) = row.rotate_immediately {
        schedule["RotateImmediatelyOnUpdate"] = json!(immediately);
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a secret whose credential a rotation function rotates",
        "Resources": {
            "RotationRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": assume_role_policy(),
                    "Policies": [{
                        "PolicyName": "rotate-the-secret",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": "secretsmanager:*",
                                "Resource": "*"
                            }]
                        }
                    }]
                }
            },
            "RotationFunction": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "Runtime": "python3.12",
                    "Handler": "index.lambda_handler",
                    "Role": { "Fn::GetAtt": ["RotationRole", "Arn"] },
                    "Timeout": FUNCTION_TIMEOUT,
                    "Code": { "ZipFile": ROTATION_FUNCTION }
                }
            },
            "InvokePermission": {
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "Action": "lambda:InvokeFunction",
                    "FunctionName": { "Fn::GetAtt": ["RotationFunction", "Arn"] },
                    "Principal": SERVICE_PRINCIPAL
                }
            },
            "AppSecret": {
                "Type": "AWS::SecretsManager::Secret",
                "Properties": {
                    "Name": format!("{stack}-credentials"),
                    "SecretString": INITIAL_VALUE
                }
            },
            // The grant has to be in force before the rotation the schedule starts invokes
            // the function, which is what `DependsOn` states.
            "Schedule": {
                "Type": "AWS::SecretsManager::RotationSchedule",
                "DependsOn": "InvokePermission",
                "Properties": schedule
            }
        },
        "Outputs": {
            "SecretArn": { "Value": { "Ref": "AppSecret" } },
            "FunctionArn": { "Value": { "Fn::GetAtt": ["RotationFunction", "Arn"] } },
            "ScheduleRef": { "Value": { "Ref": "Schedule" } }
        }
    })
    .to_string()
}

/// Create the stack `stack` from `template`, acknowledging the role it declares.
async fn create_stack(router: &MultiServiceRouter, case: &str, stack: &str, template: &str) {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", template),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
}

/// Deploy `template` over the stack `stack` and wait for the update to complete.
async fn update_stack(router: &MultiServiceRouter, case: &str, stack: &str, template: &str) {
    let (status, answered) = cloudformation(
        router,
        "UpdateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", template),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {answered}"
    );
    let (reached, body) = wait_for_stack_status(router, stack, "UPDATE_COMPLETE").await;
    assert_eq!(
        reached, "UPDATE_COMPLETE",
        "case {case:?}: expected the update to complete, got {reached:?} with {body} and \
         resource failures {:?}",
        failure_reasons(router, stack).await
    );
}

/// The physical id CloudFormation reports for the stack's schedule resource, which for this
/// type is the ARN of the secret its rotation was configured on.
async fn schedule_physical_id(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (status, body) = cloudformation(
        router,
        "DescribeStackResource",
        &[("StackName", stack), ("LogicalResourceId", "Schedule")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResource, got {status} with body \
         {body}"
    );
    opt_element(&body, "PhysicalResourceId")
        .unwrap_or_else(|| panic!("case {case:?}: no PhysicalResourceId in {body}"))
}

/// The `AWSCURRENT` version of `secret_id`, as the pair a client reads: its id and its value.
async fn current_version(
    router: &MultiServiceRouter,
    case: &str,
    secret_id: &str,
) -> (String, String) {
    let read = secretsmanager_ok(
        router,
        case,
        "GetSecretValue",
        &json!({ "SecretId": secret_id, "VersionStage": "AWSCURRENT" }),
    )
    .await;
    (
        read["VersionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no VersionId in {read}"))
            .to_string(),
        read["SecretString"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no SecretString in {read}"))
            .to_string(),
    )
}

/// The steps the rotation function recorded, in the order it was invoked with them.
fn recorded_steps(described: &Value) -> Vec<String> {
    described["Tags"]
        .as_array()
        .and_then(|tags| {
            tags.iter()
                .find(|tag| tag["Key"].as_str() == Some(STEPS_TAG))
                .and_then(|tag| tag["Value"].as_str())
        })
        .map(|recorded| recorded.split(',').map(str::to_string).collect())
        .unwrap_or_default()
}

/// Poll `DescribeSecret` until the function has recorded `wanted` steps, and answer the last
/// record read. The invocations are the rotation's own work, so nothing here sleeps a fixed
/// span on its behalf.
async fn wait_for_steps(
    router: &MultiServiceRouter,
    case: &str,
    secret_id: &str,
    wanted: usize,
) -> Vec<String> {
    let mut last = Vec::new();
    for _ in 0..POLLS {
        let described = secretsmanager_ok(
            router,
            case,
            "DescribeSecret",
            &json!({ "SecretId": secret_id }),
        )
        .await;
        last = recorded_steps(&described);
        if last.len() >= wanted {
            return last;
        }
        tokio::time::sleep(POLL).await;
    }
    last
}

/// Poll `GetSecretValue` until `AWSCURRENT` names a version other than `previous`, and answer
/// what it read then.
async fn wait_for_a_new_current(
    router: &MultiServiceRouter,
    case: &str,
    secret_id: &str,
    previous: &str,
) -> (String, String) {
    let mut last = (String::new(), String::new());
    for _ in 0..POLLS {
        last = current_version(router, case, secret_id).await;
        if last.0 != previous {
            return last;
        }
        tokio::time::sleep(POLL).await;
    }
    last
}

/// The stages `DescribeSecret` reports for each of the secret's versions.
fn stages_of(described: &Value) -> BTreeMap<String, Vec<String>> {
    described["VersionIdsToStages"]
        .as_object()
        .map(|versions| {
            versions
                .iter()
                .map(|(version, stages)| {
                    let labels = stages
                        .as_array()
                        .map(|labels| {
                            labels
                                .iter()
                                .filter_map(|label| label.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    (version.clone(), labels)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The three rules below, asserted in sequence inside ONE test.
///
/// Each of them deploys a stack whose rotation function is a real `python3.12` handler this
/// emulator INTERPRETS, so the three are kept off each other's heels: run as separate tests
/// they would be scheduled in parallel by the test harness and this one suite would contend
/// with itself — and with every other suite of this target — for the host it shares. Nothing
/// about the rules needs them concurrent.
#[tokio::test]
async fn a_stack_declared_rotation_schedule_is_the_secrets_rotation() {
    a_stack_declared_rotation_schedule_really_rotates_its_secret().await;
    a_hosted_rotation_function_is_refused_rather_than_reported_deployed().await;
    removing_the_schedule_turns_the_secrets_rotation_off().await;
}

/// RULE: the schedule a stack declares IS the secret's rotation configuration, and a rotation
/// of that secret drives the declared function through the four documented steps in order,
/// leaving `AWSCURRENT` on the version the function wrote and `AWSPREVIOUS` on the one it
/// replaced.
async fn a_stack_declared_rotation_schedule_really_rotates_its_secret() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let router = build_router();
        let stack = format!("rotationstack{index}");
        let template = template_for(row, &stack);
        create_stack(&router, case, &stack, &template).await;

        let (reached, body) = wait_for_stack_status(&router, &stack, "CREATE_COMPLETE").await;
        assert_eq!(
            reached, "CREATE_COMPLETE",
            "case {case:?}: expected the stack to complete, got {reached:?} with {body} and \
             resource failures {:?}",
            failure_reasons(&router, &stack).await
        );
        let outputs = outputs_of(&body);
        let secret = outputs
            .get("SecretArn")
            .unwrap_or_else(|| panic!("case {case:?}: no SecretArn output in {body}"))
            .clone();
        let function = outputs
            .get("FunctionArn")
            .unwrap_or_else(|| panic!("case {case:?}: no FunctionArn output in {body}"))
            .clone();

        // "Ref returns the ARN of the secret" for the schedule, which is the handle every
        // Secrets Manager call on the rotation takes.
        assert_eq!(
            outputs.get("ScheduleRef"),
            Some(&secret),
            "case {case:?}: expected the schedule's Ref to be the secret's ARN, got {outputs:?}"
        );

        // The CONFIGURATION is in force in the owning service: the function and the rules the
        // declaration stated are what that service reports for the secret.
        let described = secretsmanager_ok(
            &router,
            case,
            "DescribeSecret",
            &json!({ "SecretId": secret }),
        )
        .await;
        assert_eq!(
            described["RotationEnabled"].as_bool(),
            Some(true),
            "case {case:?}: expected the secret to report rotation enabled, got {described}"
        );
        assert_eq!(
            described["RotationLambdaARN"].as_str(),
            Some(function.as_str()),
            "case {case:?}: expected the declared rotation function {function:?}, got \
             {described}"
        );
        assert_eq!(
            described["RotationRules"],
            (row.rules)(),
            "case {case:?}: expected the declared rotation rules, got {described}"
        );

        // What the DEPLOYMENT itself rotated: a schedule that starts a rotation leaves the
        // stack with a value the function wrote, one that holds it back leaves the declared
        // one.
        let on_deploy = row.rotations_on_deploy;
        let mut expected_value = INITIAL_VALUE.to_string();
        for _ in 0..on_deploy {
            expected_value.push_str(ROTATED_SUFFIX);
        }
        let (before_id, before_value) = current_version(&router, case, &secret).await;
        assert_eq!(
            before_value, expected_value,
            "case {case:?}: expected {on_deploy} rotation(s) by the time the stack completed, \
             so the value {expected_value:?}, got {before_value:?}"
        );
        let recorded = recorded_steps(&described);
        assert_eq!(
            recorded.len(),
            on_deploy * ROTATION_STEPS.len(),
            "case {case:?}: expected {on_deploy} rotation(s) of four steps by the time the \
             stack completed, got {recorded:?}"
        );

        // A rotation a CLIENT starts afterwards runs through the same configuration.
        let started = secretsmanager_ok(
            &router,
            case,
            "RotateSecret",
            &json!({ "SecretId": secret }),
        )
        .await;
        let staked = started["VersionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no VersionId in {started}"))
            .to_string();
        assert_ne!(
            staked, before_id,
            "case {case:?}: expected the rotation to stake a new version, got the one in use \
             {before_id:?}"
        );

        let steps = wait_for_steps(
            &router,
            case,
            &secret,
            (on_deploy + 1) * ROTATION_STEPS.len(),
        )
        .await;
        let tail: Vec<String> = steps
            .iter()
            .rev()
            .take(ROTATION_STEPS.len())
            .rev()
            .cloned()
            .collect();
        assert_eq!(
            tail, ROTATION_STEPS,
            "case {case:?}: expected the four documented steps in order, got {steps:?}"
        );
        assert_eq!(
            steps.len() % ROTATION_STEPS.len(),
            0,
            "case {case:?}: expected whole rotations of four steps each, got {steps:?}"
        );

        let (after_id, after_value) =
            wait_for_a_new_current(&router, case, &secret, &before_id).await;
        assert_eq!(
            after_id, staked,
            "case {case:?}: expected AWSCURRENT on the version the rotation staked \
             {staked:?}, got {after_id:?}"
        );
        assert_eq!(
            after_value,
            format!("{before_value}{ROTATED_SUFFIX}"),
            "case {case:?}: expected the value the rotation function wrote, got {after_value:?}"
        );

        // The version the rotation replaced carries `AWSPREVIOUS`, and the one it wrote
        // carries `AWSCURRENT`.
        let described = secretsmanager_ok(
            &router,
            case,
            "DescribeSecret",
            &json!({ "SecretId": secret }),
        )
        .await;
        let stages = stages_of(&described);
        assert!(
            stages
                .get(&before_id)
                .is_some_and(|labels| labels.iter().any(|label| label == "AWSPREVIOUS")),
            "case {case:?}: expected the replaced version {before_id:?} to carry AWSPREVIOUS, \
             got {stages:?}"
        );
        assert!(
            stages
                .get(&after_id)
                .is_some_and(|labels| labels.iter().any(|label| label == "AWSCURRENT")),
            "case {case:?}: expected the rotated version {after_id:?} to carry AWSCURRENT, got \
             {stages:?}"
        );
        // The value under `AWSPREVIOUS` is the one in use before the rotation, so a caller can
        // still read what it replaced.
        let previous = secretsmanager_ok(
            &router,
            case,
            "GetSecretValue",
            &json!({ "SecretId": secret, "VersionStage": "AWSPREVIOUS" }),
        )
        .await;
        assert_eq!(
            previous["SecretString"].as_str(),
            Some(before_value.as_str()),
            "case {case:?}: expected AWSPREVIOUS to answer the replaced value, got {previous}"
        );
    }
}

/// RULE, from the other side: a declaration this emulator cannot put in force is REFUSED
/// rather than reported deployed — a `HostedRotationLambda` asks Secrets Manager to create a
/// rotation function of its own, which nothing here creates, so the resource fails and the
/// stack rolls back instead of publishing a schedule nothing rotates through.
async fn a_hosted_rotation_function_is_refused_rather_than_reported_deployed() {
    let router = build_router();
    let case = "a schedule asking for a Secrets Manager hosted rotation function";
    let stack = "hostedrotationstack";
    let template = json!({
        "Resources": {
            "AppSecret": {
                "Type": "AWS::SecretsManager::Secret",
                "Properties": {
                    "Name": format!("{stack}-credentials"),
                    "SecretString": INITIAL_VALUE
                }
            },
            "Schedule": {
                "Type": "AWS::SecretsManager::RotationSchedule",
                "Properties": {
                    "SecretId": { "Ref": "AppSecret" },
                    "HostedRotationLambda": { "RotationType": "PostgreSQLSingleUser" },
                    "RotationRules": { "AutomaticallyAfterDays": 7 }
                }
            }
        }
    })
    .to_string();
    create_stack(&router, case, stack, &template).await;

    let (reached, body) = wait_for_stack_status(&router, stack, "ROLLBACK_COMPLETE").await;
    assert_eq!(
        reached, "ROLLBACK_COMPLETE",
        "case {case:?}: expected the refused declaration to roll the stack back, got \
         {reached:?} with {body}"
    );
    let reasons = failure_reasons(&router, stack).await;
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("HostedRotationLambda")),
        "case {case:?}: expected a failure naming the property that was not applied, got \
         {reasons:?}"
    );
}

/// RULE: a later deployment that still declares the schedule APPLIES its changed declaration
/// to the rotation that exists — the resource keeps the identifier the owning service answered
/// and the new rules reach `RotateSecret` — and one that drops the schedule turns the rotation
/// off, because the resource IS the secret's rotation configuration and its removal is
/// `CancelRotateSecret`. The secret itself, a resource of its own, stays through both.
async fn removing_the_schedule_turns_the_secrets_rotation_off() {
    let router = build_router();
    let case = "a stack whose schedule is changed and then removed by later deployments";
    let stack = "cancelledrotationstack";
    let row = &ROWS[2];
    let template = template_for(row, stack);
    create_stack(&router, case, stack, &template).await;
    let (reached, body) = wait_for_stack_status(&router, stack, "CREATE_COMPLETE").await;
    assert_eq!(
        reached, "CREATE_COMPLETE",
        "case {case:?}: expected the stack to complete, got {reached:?} with {body} and \
         resource failures {:?}",
        failure_reasons(&router, stack).await
    );
    let secret = outputs_of(&body)
        .get("SecretArn")
        .unwrap_or_else(|| panic!("case {case:?}: no SecretArn output in {body}"))
        .clone();
    let created_id = schedule_physical_id(&router, case, stack).await;
    assert_eq!(
        created_id, secret,
        "case {case:?}: expected the schedule to be known by the secret's ARN, got \
         {created_id:?}"
    );

    // A later deployment that CHANGES the schedule: the same resource, with new rotation
    // rules, which the owning service must then report for the secret.
    let changed_rules = json!({ "ScheduleExpression": "cron(0 17 15 * ? *)", "Duration": "1h" });
    let mut changed: Value = serde_json::from_str(&template).expect("the template is JSON");
    changed["Resources"]["Schedule"]["Properties"]["RotationRules"] = changed_rules.clone();
    update_stack(&router, case, stack, &changed.to_string()).await;
    let described = secretsmanager_ok(
        &router,
        case,
        "DescribeSecret",
        &json!({ "SecretId": secret }),
    )
    .await;
    assert_eq!(
        described["RotationEnabled"].as_bool(),
        Some(true),
        "case {case:?}: expected the changed declaration to leave rotation enabled, got \
         {described}"
    );
    assert_eq!(
        described["RotationRules"], changed_rules,
        "case {case:?}: expected the newly declared rotation rules to be in force, got \
         {described}"
    );
    // The resource was UPDATED, not replaced: a replacement would have removed the old one,
    // and this type's removal turns the rotation off.
    let updated_id = schedule_physical_id(&router, case, stack).await;
    assert_eq!(
        updated_id, created_id,
        "case {case:?}: expected the schedule to keep the identifier it was created under, \
         got {updated_id:?}"
    );

    // The same template without the schedule: everything else the stack declares stays.
    let mut without = changed.clone();
    without["Resources"]
        .as_object_mut()
        .expect("the template declares resources")
        .remove("Schedule");
    without["Outputs"]
        .as_object_mut()
        .expect("the template declares outputs")
        .remove("ScheduleRef");
    update_stack(&router, case, stack, &without.to_string()).await;

    let described = secretsmanager_ok(
        &router,
        case,
        "DescribeSecret",
        &json!({ "SecretId": secret }),
    )
    .await;
    assert_ne!(
        described["RotationEnabled"].as_bool(),
        Some(true),
        "case {case:?}: expected the removed schedule to turn rotation off, got {described}"
    );
    // The secret is still there, with the value the template declared.
    let (_, value) = current_version(&router, case, &secret).await;
    assert_eq!(
        value, INITIAL_VALUE,
        "case {case:?}: expected the secret to keep the declared value, got {value:?}"
    );
}
