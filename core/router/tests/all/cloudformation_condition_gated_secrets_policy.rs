// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack whose `AWS::IAM::Policy` is gated on a `Condition` settles to a
//! TERMINAL status, publishes its `Outputs`, and the gated grant decides what the stack's own
//! Lambda function may read out of AWS Secrets Manager — end to end over the emulator's own
//! assembled service set.
//!
//! The rule pinned here: **a template's `Conditions` section decides what the stack HAS — "the
//! optional `Conditions` section … controls whether certain resources are created" — so a
//! resource whose `Condition` evaluates false is simply not created, and the deployment still
//! settles to `CREATE_COMPLETE` with an `OutputValue` for every output whose own `Condition`
//! holds. Nothing else about the stack changes: the resources the condition does NOT gate are
//! created either way, and what the gated resource would have done is the only observable
//! difference.** Where the gated resource is the inline policy a role carries, that difference
//! is a permission: a function running as that role is refused `secretsmanager:GetSecretValue`
//! with the AWS-common `AccessDeniedException` when the policy was not created, and is served
//! the secret's `SecretString` when it was.
//!
//! Five crates have to agree for that to hold — CloudFormation evaluates the conditions and
//! makes the calls, Secrets Manager holds the secret and takes the authorization decision, IAM
//! holds the role and the inline document the decision is read from, Lambda holds and RUNS the
//! function, STS vends the role credentials the handler's own call presents — so the suite
//! lives here rather than in any one of them. Every observation is made where a client makes
//! one: `DescribeStacks`, `DescribeStackResources`, `Invoke` and Secrets Manager's own
//! `GetSecretValue`, never an emulator internal.
//!
//! The rows differ along exactly what the rule generalises over: the FORM the condition is
//! written in (`Fn::Equals`, its operands the other way round, `Fn::Not` over the negative,
//! `Fn::And` over two conditions), WHERE the parameter's value comes from (supplied by the
//! deployment or taken from the declaration's `Default`), the VALUE it takes, whether the
//! condition therefore holds, whether the function is declared with a name of its own or left
//! for CloudFormation to name, and the shape of the `SecretString` the stack deploys. The
//! negative and the positive branch are both exercised through each of the two `Fn::Equals`
//! spellings, so no row's verdict rests on the condition being written one particular way.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::stack_status;
use crate::query_wire::{elements, form_encode, outputs_of, physical_ids_of};

/// The Region every request here is signed for, and so the Region the stack's resources
/// belong to.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The terminal `StackStatus` a create that succeeded settles to.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The statuses a `CreateStack` can settle to, as `DescribeStacks` reports them: the suite
/// waits for a TERMINAL one and then says which it was, so a stack that rolled back fails
/// here with its own status rather than by timing out.
const TERMINAL_STATUSES: [&str; 4] = [
    CREATE_COMPLETE,
    "CREATE_FAILED",
    "ROLLBACK_COMPLETE",
    "ROLLBACK_FAILED",
];

/// The template parameter the condition reads, and the logical ids the template declares.
const PARAMETER: &str = "AllowSecretsAccess";
const SECRET: &str = "Secret";
const ROLE: &str = "Role";
const GATED_POLICY: &str = "SecretsPolicy";
const FUNCTION: &str = "Fn";

/// The condition the gated policy and the gated output both name.
const CONDITION: &str = "GrantSecretsPolicy";

/// The output keys the template declares: one unconditional, one gated on the same condition
/// as the policy.
const FUNCTION_NAME_OUTPUT: &str = "FunctionName";
const SECRET_ARN_OUTPUT: &str = "SecretArn";
const GRANT_OUTPUT: &str = "GrantedPolicyName";

/// The runtime and handler every row's function declares. What the runtime supplies and how an
/// inline package is archived is pinned by `crate::cloudformation_sam_function_writes_its_table`;
/// here they are held fixed so the condition is the only thing that varies.
const RUNTIME: &str = "python3.12";
const HANDLER: &str = "index.handler";

/// How long one invocation of this handler is given, as the function's own `Timeout`: well
/// past what a run needs, because what is under test is the authorization decision and not how
/// fast an interpreter gets through the handler.
const FUNCTION_TIMEOUT: u64 = 30;

/// The IAM action the gated policy grants, which is the one the handler's read performs
/// ("Actions defined by AWS Secrets Manager").
const READ_ACTION: &str = "secretsmanager:GetSecretValue";

/// The error code AWS answers a call no policy in force allows with — the AWS-common
/// `AccessDeniedException`, which is what the handler reports back as its own `error_code`.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// How long to wait between polls of the function's state, and how many polls to make.
const POLL: Duration = Duration::from_millis(25);
const POLLS: usize = 400;

/// The emulator as it is ASSEMBLED, on one engine (`aws_emulator_router::build_router`): the
/// handler's own SDK call is dispatched back into this same router, so the secret it reads is
/// the secret this stack deployed.
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
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
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

/// One Lambda `restJson1` call, as its status, its headers and its parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::from(body.to_vec()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// Call one Secrets Manager operation over its `awsJson1_1` wire.
async fn secretsmanager(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("secretsmanager.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", authorization("secretsmanager"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active` — "the
/// function is ready to be invoked" (`FunctionConfiguration$State`) — and answer the
/// configuration it reported then.
///
/// The span a created function spends `Pending` is that service's own, so no fixed sleep names
/// the instant it ends.
async fn wait_until_active(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let mut last = Value::Null;
    for _ in 0..POLLS {
        let (status, _, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            &[],
        )
        .await;
        // A refused poll is not a function that is merely still coming up, so it is reported
        // as the refusal it is rather than as a state that never became Active.
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetFunctionConfiguration on {name:?}, got \
             {status} with {configuration}"
        );
        if configuration.get("State").and_then(Value::as_str) == Some("Active") {
            return configuration;
        }
        last = configuration;
        tokio::time::sleep(POLL).await;
    }
    last
}

/// Poll `DescribeStacks` until the stack first reports a TERMINAL status, and answer it with
/// the answer it was read from.
///
/// One deadline over the module's own `stack_status` read, rather than a wait for one named
/// status: a create that settles anywhere but `CREATE_COMPLETE` is answered the instant it
/// settles, and a stack that never settles — the symptom this suite pins — fails once, here,
/// instead of being waited out again.
async fn wait_for_terminal_status(router: &MultiServiceRouter, stack: &str) -> (String, String) {
    let mut last = stack_status(router, stack).await;
    for _ in 0..POLLS {
        if TERMINAL_STATUSES.contains(&last.0.as_str()) {
            return last;
        }
        tokio::time::sleep(POLL).await;
        last = stack_status(router, stack).await;
    }
    last
}

/// The handler every row's function declares inline: it reads the secret the stack named it
/// through its environment, and reports either the value or the error code the service refused
/// the read with. The source carries nothing the deployment had to compute — the secret is
/// addressed by the identifier the template passed in.
const READER_SOURCE: &str = r#"
import os

import boto3
import botocore.exceptions


def handler(event, context):
    client = boto3.client("secretsmanager")
    try:
        answered = client.get_secret_value(SecretId=os.environ["SECRET_ID"])
    except botocore.exceptions.ClientError as error:
        return {"ok": False, "error_code": error.response["Error"]["Code"]}
    return {"ok": True, "secret": answered["SecretString"]}
"#;

/// How a row writes the condition that gates the policy. Every form is one of the condition
/// functions CloudFormation evaluates in the `Conditions` section ("Fn::And, Fn::Equals,
/// Fn::If, Fn::Not, Fn::Or"), over the same parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// `Fn::Equals: [{Ref: parameter}, "true"]`.
    Equals,
    /// The same comparison with its operands the other way round.
    EqualsReversed,
    /// `Fn::Not` over the comparison against the other allowed value.
    NotOfTheNegative,
    /// `Fn::And` over the comparison and a second condition that always holds.
    AndOfTwo,
}

/// One row: how the condition is written, where its parameter's value comes from, and what
/// therefore becomes of the gated grant.
struct Row {
    case: &'static str,
    form: Form,
    /// The value the deployment SUPPLIES for the parameter, or `None` to leave it to the
    /// declaration's `Default`.
    supplied: Option<&'static str>,
    /// The `Default` the parameter is declared with, when it is declared with one.
    default: Option<&'static str>,
    /// Whether the condition therefore holds, so the gated policy is created and the read is
    /// served.
    granted: bool,
    /// Whether the function is declared with a name of its own rather than left for
    /// CloudFormation to name.
    named_function: bool,
    /// The `SecretString` the stack's secret is deployed with.
    secret: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "the deployment supplies false, so the gated policy is never created and the \
               read is refused",
        form: Form::Equals,
        supplied: Some("false"),
        default: Some("false"),
        granted: false,
        named_function: true,
        secret: "a-plain-secret-string",
    },
    Row {
        case: "the same template with true supplied, so the gated policy is created and the \
               read is served",
        form: Form::Equals,
        supplied: Some("true"),
        default: Some("false"),
        granted: true,
        named_function: true,
        secret: "a-plain-secret-string",
    },
    Row {
        case: "the value comes from the parameter's own Default rather than the deployment",
        form: Form::Equals,
        supplied: None,
        default: Some("false"),
        granted: false,
        named_function: true,
        secret: r#"{"endpoint":"db.internal","port":5432}"#,
    },
    Row {
        case: "the comparison is written with its operands the other way round, and holds",
        form: Form::EqualsReversed,
        supplied: Some("true"),
        default: None,
        granted: true,
        named_function: false,
        secret: r#"{"endpoint":"db.internal","port":5432}"#,
    },
    Row {
        case: "the comparison is written with its operands the other way round, and does not \
               hold",
        form: Form::EqualsReversed,
        supplied: Some("false"),
        default: None,
        granted: false,
        named_function: false,
        // A value whose shape is neither plain nor JSON: `SecretString` is free text, so it
        // carries newlines as written. (Its only constraint is a length of at least one —
        // `smithy_query(query="shape", shape="SecretStringType")`.)
        secret: "line one\nline two\nline three\n",
    },
    Row {
        case: "the condition is Fn::Not over the comparison against the other allowed value",
        form: Form::NotOfTheNegative,
        supplied: Some("false"),
        default: None,
        granted: false,
        named_function: false,
        secret: "a-secret-with-unicode-£-and-a-space",
    },
    Row {
        case: "the condition is Fn::And over the comparison and one that always holds",
        form: Form::AndOfTwo,
        supplied: Some("true"),
        default: None,
        granted: true,
        named_function: true,
        secret: "a-secret-with-unicode-£-and-a-space",
    },
];

/// The condition declaration one row's `Conditions` section carries.
fn condition(form: Form) -> Value {
    let parameter = json!({ "Ref": PARAMETER });
    match form {
        Form::Equals => json!({ "Fn::Equals": [parameter, "true"] }),
        Form::EqualsReversed => json!({ "Fn::Equals": ["true", parameter] }),
        Form::NotOfTheNegative => {
            json!({ "Fn::Not": [{ "Fn::Equals": [parameter, "false"] }] })
        }
        Form::AndOfTwo => json!({
            "Fn::And": [
                { "Fn::Equals": [parameter, "true"] },
                { "Fn::Equals": ["1", "1"] },
            ]
        }),
    }
}

/// The trust policy the stack's role carries, so Lambda can be handed it.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "lambda.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
}

/// The template a row deploys: the secret, the execution role, the `Condition`-gated inline
/// policy granting the read on that secret, the function that performs it, and the outputs a
/// client reads the deployment's identifiers out of.
fn template(row: &Row, stack: &str) -> String {
    let mut parameter = json!({ "Type": "String", "AllowedValues": ["true", "false"] });
    if let Some(default) = row.default {
        parameter["Default"] = json!(default);
    }
    let mut function = json!({
        "Runtime": RUNTIME,
        "Handler": HANDLER,
        "Timeout": FUNCTION_TIMEOUT,
        "Role": { "Fn::GetAtt": [ROLE, "Arn"] },
        "Environment": { "Variables": { "SECRET_ID": { "Ref": SECRET } } },
        "Code": { "ZipFile": READER_SOURCE },
    });
    if row.named_function {
        function["FunctionName"] = json!(format!("{stack}-fn"));
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Parameters": { PARAMETER: parameter },
        "Conditions": { CONDITION: condition(row.form) },
        "Resources": {
            SECRET: {
                "Type": "AWS::SecretsManager::Secret",
                "Properties": {
                    "Name": format!("{stack}-secret"),
                    "SecretString": row.secret,
                },
            },
            ROLE: {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": format!("{stack}-role"),
                    "AssumeRolePolicyDocument": trust_policy(),
                },
            },
            GATED_POLICY: {
                "Type": "AWS::IAM::Policy",
                "Condition": CONDITION,
                "Properties": {
                    "PolicyName": format!("{stack}-read-the-secret"),
                    "Roles": [{ "Ref": ROLE }],
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Action": READ_ACTION,
                            "Resource": { "Ref": SECRET },
                        }],
                    },
                },
            },
            FUNCTION: {
                "Type": "AWS::Lambda::Function",
                "Properties": function,
            },
        },
        "Outputs": {
            FUNCTION_NAME_OUTPUT: { "Value": { "Ref": FUNCTION } },
            SECRET_ARN_OUTPUT: { "Value": { "Ref": SECRET } },
            GRANT_OUTPUT: {
                "Condition": CONDITION,
                "Value": { "Ref": GATED_POLICY },
            },
        },
    })
    .to_string()
}

/// What the handler must answer for a row: the secret's own value when the gated grant was
/// created, and the refusal's error code when it was not.
fn expected_payload(row: &Row) -> Value {
    if row.granted {
        json!({ "ok": true, "secret": row.secret })
    } else {
        json!({ "ok": false, "error_code": ACCESS_DENIED })
    }
}

/// Deploy one row's stack, and answer `(the outputs, the logical -> physical id map)`.
async fn deploy(
    router: &MultiServiceRouter,
    row: &Row,
    stack: &str,
) -> (
    std::collections::BTreeMap<String, String>,
    std::collections::BTreeMap<String, String>,
) {
    let case = row.case;
    let body = template(row, stack);
    let mut members = vec![
        ("StackName", stack.to_string()),
        ("TemplateBody", body),
        // The template declares IAM resources, one of them NAMED, which is what these two
        // capabilities acknowledge.
        ("Capabilities.member.1", "CAPABILITY_IAM".to_string()),
        ("Capabilities.member.2", "CAPABILITY_NAMED_IAM".to_string()),
    ];
    if let Some(supplied) = row.supplied {
        members.push(("Parameters.member.1.ParameterKey", PARAMETER.to_string()));
        members.push((
            "Parameters.member.1.ParameterValue",
            supplied.to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = members
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    let (status, answered) = cloudformation(router, "CreateStack", &borrowed).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {answered}"
    );

    // The stack must settle to a TERMINAL status at all — the half of the rule a client that
    // waits on `DescribeStacks` depends on — and the one it settles to must be the success.
    let (reached, described) = wait_for_terminal_status(router, stack).await;
    assert!(
        TERMINAL_STATUSES.contains(&reached.as_str()),
        "case {case:?}: expected a terminal StackStatus, got {reached:?} with {described}"
    );
    let reasons = if reached == CREATE_COMPLETE {
        Vec::new()
    } else {
        let (_, events) =
            cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
        elements(&events, "ResourceStatusReason")
    };
    assert_eq!(
        reached, CREATE_COMPLETE,
        "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described} with \
         resource failures {reasons:?}"
    );

    let (status, resources) =
        cloudformation(router, "DescribeStackResources", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with body \
         {resources}"
    );
    (outputs_of(&described), physical_ids_of(&resources))
}

/// One deploy, one invocation and one read-back per row.
#[tokio::test]
async fn a_condition_gated_policy_decides_what_the_stacks_function_reads() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let router = build_router();
        let stack = format!("cfn-gated-secret-{index}");
        let (outputs, physical) = deploy(&router, row, &stack).await;

        // What the stack HAS: every ungated resource either way, and the gated policy exactly
        // when its condition holds.
        for logical in [SECRET, ROLE, FUNCTION] {
            assert!(
                physical.contains_key(logical),
                "case {case:?}: expected a physical id for {logical}, got {physical:?}"
            );
        }
        assert_eq!(
            physical.contains_key(GATED_POLICY),
            row.granted,
            "case {case:?}: expected the stack to hold {GATED_POLICY} == {}, got {physical:?}",
            row.granted
        );

        // What the stack PUBLISHES: a value for each output whose condition holds, and nothing
        // for the one whose does not.
        let function = outputs
            .get(FUNCTION_NAME_OUTPUT)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no {FUNCTION_NAME_OUTPUT} output in {outputs:?}"));
        assert_eq!(
            Some(&function),
            physical.get(FUNCTION),
            "case {case:?}: expected {FUNCTION_NAME_OUTPUT} to carry the function's own \
             physical id {:?}, got {function:?}",
            physical.get(FUNCTION)
        );
        let secret_arn = outputs
            .get(SECRET_ARN_OUTPUT)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no {SECRET_ARN_OUTPUT} output in {outputs:?}"));
        // A secret's ARN is `arn:aws:secretsmanager:<region>:<account>:secret:<name>-<six
        // characters>`, and the name is the one this row's template declared — so the whole
        // ARN but the service's own six-character suffix is checked.
        let expected_prefix =
            format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:{stack}-secret-");
        assert!(
            secret_arn.starts_with(&expected_prefix)
                && secret_arn.len() == expected_prefix.len() + 6,
            "case {case:?}: expected {SECRET_ARN_OUTPUT} to carry {expected_prefix:?} plus six \
             characters, got {secret_arn:?}"
        );
        assert_eq!(
            outputs.contains_key(GRANT_OUTPUT),
            row.granted,
            "case {case:?}: expected the {GRANT_OUTPUT} output to be reported == {}, got \
             {outputs:?}",
            row.granted
        );

        // The secret the stack reported as created is one Secrets Manager really holds, read
        // through that service's own API on the ARN the stack published.
        let (status, held) = secretsmanager(
            &router,
            "GetSecretValue",
            &json!({ "SecretId": secret_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetSecretValue on the published ARN, got \
             {status} with {held}"
        );
        assert_eq!(
            held.get("SecretString").and_then(Value::as_str),
            Some(row.secret),
            "case {case:?}: expected the deployed SecretString {:?}, got {held}",
            row.secret
        );

        // What the gated grant MEANS: the function reads the secret, or is refused.
        let configuration = wait_until_active(&router, case, &function).await;
        assert_eq!(
            configuration.get("State").and_then(Value::as_str),
            Some("Active"),
            "case {case:?}: expected Lambda to report {function} Active, got {configuration}"
        );
        let (status, headers, payload) = lambda(
            &router,
            "POST",
            &format!("/2015-03-31/functions/{function}/invocations"),
            b"{}",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from Invoke, got {status} with payload {payload}"
        );
        let signal = headers
            .get("x-amz-function-error")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        assert_eq!(
            signal, None,
            "case {case:?}: expected no function error, got {signal:?} with payload {payload}"
        );
        let expected = expected_payload(row);
        assert_eq!(
            payload, expected,
            "case {case:?}: expected the handler's own result {expected}, got {payload}"
        );
    }
}
