// SPDX-License-Identifier: CC0-1.0

//! A backup job's state change reaches an EventBridge rule's LAMBDA target, and what the
//! invoked function wrote is readable in that function's own log group.
//!
//! The RULE pinned here: **every state an AWS Backup job MOVES INTO after the call that
//! started it is published once onto the account's DEFAULT EventBridge bus as `source`
//! `aws.backup` / `detail-type` `Backup Job State Change`, carrying that job's own stored
//! members as the event `detail` — its id, the state it moved into, the vault, the protected
//! resource and that resource's Backup type — and the recovery point it wrote in `resources`;
//! EventBridge routes the envelope by its CONTENT, so a rule whose `EventPattern` matches
//! hands it UNCHANGED to every one of that rule's targets, and a Lambda target is invoked
//! with it as the invocation's payload only where that FUNCTION's own resource-based policy
//! admits `lambda:InvokeFunction` to the `events.amazonaws.com` principal ("for Lambda
//! functions … EventBridge relies on resource-based policies", `PutTargets`), whose run's
//! output then lands in the function's default log group "`/aws/lambda/<function name>`"
//! (`LoggingConfig$LogGroup`) where `FilterLogEvents` reports it.** The state the START call
//! stored is not a move — it is what that call's own response reports — so the envelopes the
//! bus is owed are the transitions that follow it.
//!
//! Source for the envelope: the AWS Backup EventBridge notifications, which carry
//! `"source": "aws.backup"` with `"detail-type": "Backup Job State Change"`; the backup
//! Smithy model describes the control plane, not the bus, so the `detail` members are
//! asserted against what the SAME job's control-plane response reports for them, and the
//! states a job can take against `smithy_query(query="shape",
//! shape="com.amazonaws.backup#BackupJobState")` (`CREATED`/`PENDING`/`RUNNING`/`COMPLETED`/
//! `FAILED`/`ABORTED`/…).
//!
//! Seven crates have to agree for that to hold (the service that OWNS the protected resource
//! holds it, AWS Backup publishes the job's move, CloudFormation archives the inline handler
//! into the function's deployment package, EventBridge matches and hands over, Lambda decides
//! the invocation against the function's policy and then RUNS the handler, CloudWatch Logs
//! holds what the handler wrote, and IAM answers both for the job's backup role and for the
//! function's execution role), so the suite lives here rather than in any one crate, and it
//! runs over the WHOLE `build_router` service set the binary serves — so the in-process
//! publication is claimed and routed by whichever mounted model a client's own `events`-scoped
//! call would reach.
//!
//! The rows differ along exactly what the rule generalises over: WHICH dimension of the
//! envelope the rule's pattern filters on (the source and detail type alone, the list of early
//! states inside `detail`, ONE state inside it), WHICH Backup resource type the job protects
//! — and so which `detail.resourceType` the envelope carries — WHICH terminal state the job's
//! own backup role leads it to (`COMPLETED`, and the `FAILED` of a role whose permissions
//! cannot create the recovery point), and whether the delivery is admitted at all (a pattern
//! naming a state this job never takes, and a function whose resource policy admits
//! EventBridge nothing) — the two rows whose function must therefore never run. Those two
//! rows carry a POSITIVE CONTROL rather than a fixed quiet window: a second rule on the same
//! bus, matching the source and detail type alone, with its own granted function, so the
//! control's line is what says the envelope really was published and dispatched, and the row's
//! own function holding nothing by then is the refusal rather than a look taken too early.
//! Every protected resource is really held by the service that owns it: each row creates it
//! through that service's own API and backs up the ARN that service answered with.
//!
//! What is NOT asserted here, because it is asserted elsewhere:
//! `services/service-com-amazonaws-backup-cryocontrollerusermanager/tests/all/job_state_change_events.rs`
//! pins the published `detail` of every job KIND against the control plane's own reads,
//! `crate::events_lambda_target_resource_policy` pins the shapes of document that admit or
//! withhold a function target's invocation, and
//! `crate::events_sns_lambda_delivery_observed_in_logs` pins what a CloudFormation-deployed
//! function leaves in its log group for an event a CLIENT published. What only this suite can
//! reach is the whole path the issue describes: a REAL backup job's own transition, routed by
//! content, running a function and leaving the job's id where a client polls for it.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The shared stack reader of this test crate, and the awsQuery callers and the XML readers it
// writes ONCE rather than per suite. They sign for the Region below, which is why this suite
// signs for it too.
use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{element, outputs_of};
use crate::sns_lambda_subscription_permission_boundary::{cloudformation, iam, CREATE_COMPLETE};

/// The Region every request below is signed for — the one the shared stack reader and the
/// shared awsQuery callers sign for.
const REGION: &str = "us-east-1";

/// The `source` and `detail-type` AWS Backup publishes a job's state changes under, spelled
/// out here rather than imported from the service crate, so a wrong constant in the
/// implementation cannot make this suite pass.
const BACKUP_SOURCE: &str = "aws.backup";
const BACKUP_JOB_STATE_CHANGE: &str = "Backup Job State Change";

/// The service principal EventBridge invokes a resource-policy-governed target as, and the
/// action such an invocation of a function performs.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The two terminal states a backup job of this emulator reaches, and a state it never takes,
/// as the model's `BackupJobState` spells them.
const COMPLETED: &str = "COMPLETED";
const FAILED: &str = "FAILED";
const ABORTED: &str = "ABORTED";

/// The prefix the handler writes its payload behind, so one line of the function's log is
/// recognisably the envelope it was handed.
const MARKER: &str = "RECEIVED_EVENT::";

/// The handler every row deploys inline: it writes the WHOLE event it was handed behind
/// [`MARKER`], so the envelope a client reads out of the log group is the one EventBridge
/// delivered rather than anything this suite put there. The prefix is interpolated from that
/// one constant, so the line the handler writes and the line this suite parses cannot drift
/// apart. `index.handler` and `index.py` are what a template's inline source is archived as
/// for a Python runtime.
fn handler_source() -> String {
    format!(
        "\nimport json\n\n\ndef handler(event, context):\n\
         \x20   print(\"{MARKER}\" + json.dumps(event))\n\
         \x20   return {{\"handled\": True}}\n"
    )
}

/// How long a row waits for the invocation it must see, how long a delivered row then
/// watches on for a SECOND envelope, and how often either looks.
const DELIVERY_BUDGET: Duration = Duration::from_secs(30);
const SECOND_LOOK: Duration = Duration::from_millis(750);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// How many times a row asks Lambda whether the deployed function is invocable yet.
const POLLS: usize = 600;

/// The whole emulator over one engine, exactly as `core/router/src/lib.rs` assembles it for
/// the binary: the log delivery the function's own run makes is dispatched back into this same
/// router, so the group this suite reads is the group that run wrote.
fn router() -> MultiServiceRouter {
    let mut router = aws_emulator_router::build_router(aws_emulator_router::new_engine());
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked: the router above runs with request authentication
/// off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// The status and parsed body of one response (`Value::Null` for an empty or unparseable
/// body).
fn answered(response: &http::Response<Bytes>) -> (u16, Value) {
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One `X-Amz-Target` RPC call against the service whose signing name is `signing_name`.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    json_version: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", target)
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    answered(&router.dispatch(request).await)
}

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`), which must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(
        router,
        "events",
        &format!("AWSEvents.{operation}"),
        "1.1",
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One Amazon DynamoDB call (`awsJson1_0`, `DynamoDB_20120810.<Operation>`), which must answer
/// 200.
async fn dynamodb(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(
        router,
        "dynamodb",
        &format!("DynamoDB_20120810.{operation}"),
        "1.0",
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DynamoDB {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One CloudWatch Logs call (`awsJson1_1`, `Logs_20140328.<Operation>`), with whatever status
/// it answered: a group no delivery has created yet is a refusal this suite reads THROUGH
/// rather than asserts on — it is exactly what a client polling for a line that never came
/// sees.
async fn logs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(
        router,
        "logs",
        &format!("Logs_20140328.{operation}"),
        "1.1",
        body,
    )
    .await
}

/// One `restJson1` call against the service whose signing name is `signing_name`, as its
/// status and parsed body.
async fn rest(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |body| Bytes::from(body.to_string()));
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name));
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(payload)
        .expect("the test request is well formed");
    answered(&router.dispatch(request).await)
}

/// One AWS Backup call, which must answer 200.
async fn backup(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> Value {
    let (status, parsed) = rest(router, "backup", method, uri, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Backup {method} {uri}, got {status} with {parsed}"
    );
    parsed
}

/// A string member of a response, or a panic naming the case and what came back.
fn member(case: &str, body: &Value, pointer: &str) -> String {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected {pointer} in {body}"))
        .to_string()
}

/// Every message the log group `group` holds right now, as a client reads them.
async fn log_messages(router: &MultiServiceRouter, group: &str) -> Vec<String> {
    let body = json!({ "logGroupName": group });
    let (status, answered) = logs(router, "FilterLogEvents", &body).await;
    if status != 200 {
        return Vec::new();
    }
    answered["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|event| event["message"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Poll `function`'s default log group until a line carries `needle`, then watch
/// [`SECOND_LOOK`] longer and answer every message the group then held — so a SECOND envelope
/// delivered a moment later is in the snapshot the "published once" half of the rule is
/// asserted against. Answers whatever had arrived if [`DELIVERY_BUDGET`] passes with no such
/// line at all, which is what a client polling for a line that never came sees.
///
/// `needle` is the id of THIS row's job, not merely [`MARKER`]: a log group a shared control
/// function has already written to holds an earlier row's envelope, and waiting for the
/// marker alone would answer with that one.
async fn awaited_log_messages(
    router: &MultiServiceRouter,
    function: &str,
    needle: &str,
) -> Vec<String> {
    let group = format!("/aws/lambda/{function}");
    let deadline = Instant::now() + DELIVERY_BUDGET;
    loop {
        let seen = log_messages(router, &group).await;
        if seen.iter().any(|line| line.contains(needle)) {
            tokio::time::sleep(SECOND_LOOK).await;
            return log_messages(router, &group).await;
        }
        if Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// An identity policy allowing `actions` on every resource: what a row's backup role, and
/// every function's execution role, carries.
fn allowing(actions: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": actions,
            "Resource": "*",
        }],
    })
    .to_string()
}

/// The trust policy a role handed to `service` carries.
fn trusting(service: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": service },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// Create the role `name` where IAM holds it — trusted by `service`, granting `actions` — and
/// answer the ARN IAM ITSELF reports for it, so a value this suite hands another service is
/// the owning service's own answer rather than an ARN format written out here.
async fn role(
    router: &MultiServiceRouter,
    name: &str,
    service: &str,
    actions: &[&str],
) -> String {
    let created = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", &trusting(service)),
        ],
    )
    .await;
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name),
            ("PolicyName", "granted"),
            ("PolicyDocument", &allowing(actions)),
        ],
    )
    .await;
    element(&created, "Arn")
}

/// Deploy a stack holding ONE function with the inline handler above, running as
/// `execution_role`, and answer its `(name, arn)` once Lambda reports it invocable.
///
/// The function comes from a template because that is what archives inline source into a
/// deployment package this emulator can run: `Code.ZipFile` in a template is the function's
/// SOURCE TEXT, while the API's member of that name is the archive's bytes. Everything the
/// rule is about — the rule, its target and the grant — is then wired through each service's
/// OWN API below, the way a client wires it.
async fn deployed_function(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    execution_role: &str,
) -> (String, String) {
    let name = format!("{stack}-handler");
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Handler": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": &name,
                    "Role": execution_role,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Timeout": 10,
                    "Code": { "ZipFile": handler_source() },
                },
            },
        },
        "Outputs": {
            "FunctionArn": { "Value": { "Fn::GetAtt": ["Handler", "Arn"] } },
        },
    })
    .to_string();
    cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", &template)],
    )
    .await;
    let (status, described) = wait_for_stack_status(router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "case {case:?}: expected the handler stack to deploy, got {status} with {described}"
    );
    let arn = outputs_of(&described)
        .get("FunctionArn")
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the stack to publish a FunctionArn, got {described}")
        });

    // The function has to be invocable before anything is delivered to it: "During this time,
    // you can't invoke or modify the function" (`CreateFunction`), so a handover withheld
    // there would be the lifecycle's refusal rather than the rule under test.
    for _ in 0..POLLS {
        let (_, reported) = rest(
            router,
            "lambda",
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        if reported.get("State").and_then(Value::as_str) == Some("Active") {
            return (name, arn);
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("case {case:?}: Lambda never reported {name} Active");
}

/// Which kind of resource a row's job protects.
///
/// AWS Backup names a protected resource's type after the SERVICE that owns its ARN, so each
/// row creates the resource through that service's own API and backs up the ARN that service
/// answered with — the type the envelope reports is then derived from a resource something
/// really holds.
#[derive(Debug, Clone, Copy)]
enum Protected {
    /// An Amazon DynamoDB table.
    DynamoDbTable,
    /// An Amazon EFS file system.
    EfsFileSystem,
}

impl Protected {
    /// The Backup resource type of such a resource, as the Backup API's own `ByResourceType`
    /// catalog spells it (`ListBackupJobs`).
    fn resource_type(self) -> &'static str {
        match self {
            Self::DynamoDbTable => "DynamoDB",
            Self::EfsFileSystem => "EFS",
        }
    }

    /// Create one under `name` and answer the ARN its owning service reports for it.
    async fn created(self, router: &MultiServiceRouter, case: &str, name: &str) -> String {
        match self {
            Self::DynamoDbTable => {
                let created = dynamodb(
                    router,
                    case,
                    "CreateTable",
                    &json!({
                        "TableName": name,
                        "AttributeDefinitions": [
                            { "AttributeName": "id", "AttributeType": "S" },
                        ],
                        "KeySchema": [{ "AttributeName": "id", "KeyType": "HASH" }],
                        "BillingMode": "PAY_PER_REQUEST",
                    }),
                )
                .await;
                member(case, &created, "/TableDescription/TableArn")
            }
            Self::EfsFileSystem => {
                let (status, created) = rest(
                    router,
                    "elasticfilesystem",
                    "POST",
                    "/2015-02-01/file-systems",
                    Some(&json!({ "CreationToken": name })),
                )
                .await;
                assert_eq!(
                    status, 201,
                    "case {case:?}: expected HTTP 201 from CreateFileSystem, got {status} with \
                     {created}"
                );
                member(case, &created, "/FileSystemArn")
            }
        }
    }
}

/// What one row filters on, protects, and must therefore see.
struct Row {
    case: &'static str,
    /// The stack the row's function is deployed from, which also names the function.
    stack: &'static str,
    /// The `detail` clause the rule's pattern carries beyond the source and the detail type.
    /// `None` filters on those two alone.
    detail: Option<fn() -> Value>,
    /// The resource the job backs up.
    protected: Protected,
    /// Whether the job's own backup role is granted the permissions a backup needs. A role
    /// whose permissions allow nothing of the kind cannot create the recovery point, so the
    /// job's terminal state is `FAILED`.
    permitted: bool,
    /// Whether the function's resource policy admits `lambda:InvokeFunction` to EventBridge.
    admitted: bool,
    /// The `detail.state` the function must be handed, or `None` when nothing may reach it at
    /// all.
    expected: Option<&'static str>,
}

/// The `detail` clauses the rows filter on, each a pattern a client really writes.
fn early_states() -> Value {
    json!({ "state": ["RUNNING", "PENDING", "CREATED", COMPLETED] })
}
fn completed_only() -> Value {
    json!({ "state": [COMPLETED] })
}
fn failed_only() -> Value {
    json!({ "state": [FAILED] })
}
fn aborted_only() -> Value {
    json!({ "state": [ABORTED] })
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the source and detail type alone, backing up a DynamoDB table",
            stack: "backup-events-source-only",
            detail: None,
            protected: Protected::DynamoDbTable,
            permitted: true,
            admitted: true,
            expected: Some(COMPLETED),
        },
        Row {
            case: "the early states the job may take, listed inside the detail",
            stack: "backup-events-early-states",
            detail: Some(early_states),
            protected: Protected::DynamoDbTable,
            permitted: true,
            admitted: true,
            expected: Some(COMPLETED),
        },
        Row {
            case: "one state inside the detail, backing up an EFS file system",
            stack: "backup-events-completed-only",
            detail: Some(completed_only),
            protected: Protected::EfsFileSystem,
            permitted: true,
            admitted: true,
            expected: Some(COMPLETED),
        },
        Row {
            case: "a job whose role cannot create the recovery point, on a FAILED pattern",
            stack: "backup-events-failed-only",
            detail: Some(failed_only),
            protected: Protected::DynamoDbTable,
            permitted: false,
            admitted: true,
            expected: Some(FAILED),
        },
        Row {
            case: "a pattern naming a state this job never takes",
            stack: "backup-events-aborted-only",
            detail: Some(aborted_only),
            protected: Protected::DynamoDbTable,
            permitted: true,
            admitted: true,
            expected: None,
        },
        Row {
            case: "a function whose resource policy admits EventBridge nothing",
            stack: "backup-events-ungranted",
            detail: None,
            protected: Protected::DynamoDbTable,
            permitted: true,
            admitted: false,
            expected: None,
        },
    ]
}

/// RULE: a backup job's every move is published on the default bus and handed to the matching
/// rule's Lambda target, whose run leaves the envelope in that function's own log group — and
/// a rule that cannot match, or a function that admits EventBridge nothing, runs nothing.
#[tokio::test]
async fn a_backup_jobs_state_change_invokes_the_rules_lambda_target() {
    // ONE emulator for the whole table: every row names its own vault, protected resource,
    // roles, rule and function, so no row can see another's resources — and a rule an earlier
    // row left on the bus only ever delivers to that row's own function, which has already
    // been read by the time the next row starts. Assembling the whole service set once rather
    // than per row keeps this suite's cost in the shared test binary to one emulator.
    let router = router();
    // The one positive control the rows that must see NOTHING are read against, deployed when
    // the first such row needs it and then shared.
    let mut control: Option<String> = None;
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;

        // The execution role the run's own log delivery is judged against: Lambda writes a
        // run's log ACTING AS that role, so the role grants the three CloudWatch Logs calls
        // that delivery makes — the grants AWS's own basic execution role carries.
        let execution_role = role(
            &router,
            &format!("backup-events-exec-{index}"),
            "lambda.amazonaws.com",
            &[
                "logs:CreateLogGroup",
                "logs:CreateLogStream",
                "logs:PutLogEvents",
            ],
        )
        .await;
        let (function_name, function_arn) =
            deployed_function(&router, case, row.stack, &execution_role).await;

        // The rule, on the default bus, filtering this row's own dimension of the envelope.
        let mut pattern = json!({
            "source": [BACKUP_SOURCE],
            "detail-type": [BACKUP_JOB_STATE_CHANGE],
        });
        if let Some(clause) = row.detail {
            pattern["detail"] = clause();
        }
        let rule_name = format!("backup-job-rule-{index}");
        let put = events(
            &router,
            case,
            "PutRule",
            &json!({
                "Name": &rule_name,
                "EventPattern": pattern.to_string(),
                "State": "ENABLED",
            }),
        )
        .await;
        let rule_arn = member(case, &put, "/RuleArn");

        // The grant a function target's invocation is decided by, scoped to that rule.
        if row.admitted {
            let (status, granted) = rest(
                &router,
                "lambda",
                "POST",
                &format!("/2015-03-31/functions/{function_name}/policy"),
                Some(&json!({
                    "StatementId": "events-invoke",
                    "Action": INVOKE_ACTION,
                    "Principal": EVENTS_PRINCIPAL,
                    "SourceArn": &rule_arn,
                })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected HTTP 201 from AddPermission, got {status} with {granted}"
            );
        }
        let wired = events(
            &router,
            case,
            "PutTargets",
            &json!({
                "Rule": &rule_name,
                "Targets": [{ "Id": "watcher", "Arn": &function_arn }],
            }),
        )
        .await;
        assert_eq!(
            wired["FailedEntryCount"],
            json!(0),
            "case {case:?}: every target must be accepted, got {wired}"
        );

        // A row that must see NOTHING is read against a POSITIVE CONTROL: one rule on the
        // same bus, matching the source and detail type alone, with its own granted function.
        // The one publication reaches both, so the control's line is what says the envelope
        // was published and dispatched at all — and the row's own function holding nothing
        // once that line is there is evidence of the refusal rather than of having looked too
        // early. ONE control serves every such row: its pattern matches every job, and each
        // row looks for its OWN job's id in it.
        if row.expected.is_none() && control.is_none() {
            let (control_name, control_arn) =
                deployed_function(&router, case, "backup-events-control", &execution_role).await;
            let control_rule = "backup-job-control-rule";
            let put = events(
                &router,
                case,
                "PutRule",
                &json!({
                    "Name": control_rule,
                    "EventPattern": json!({
                        "source": [BACKUP_SOURCE],
                        "detail-type": [BACKUP_JOB_STATE_CHANGE],
                    })
                    .to_string(),
                    "State": "ENABLED",
                }),
            )
            .await;
            let control_rule_arn = member(case, &put, "/RuleArn");
            let (status, granted) = rest(
                &router,
                "lambda",
                "POST",
                &format!("/2015-03-31/functions/{control_name}/policy"),
                Some(&json!({
                    "StatementId": "events-invoke",
                    "Action": INVOKE_ACTION,
                    "Principal": EVENTS_PRINCIPAL,
                    "SourceArn": &control_rule_arn,
                })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected HTTP 201 from the control's AddPermission, got \
                 {status} with {granted}"
            );
            let wired = events(
                &router,
                case,
                "PutTargets",
                &json!({
                    "Rule": control_rule,
                    "Targets": [{ "Id": "control", "Arn": &control_arn }],
                }),
            )
            .await;
            assert_eq!(
                wired["FailedEntryCount"],
                json!(0),
                "case {case:?}: the control's target must be accepted, got {wired}"
            );
            control = Some(control_name);
        }

        // The resource the job protects, held by the service that owns it; the vault the
        // backup is taken into; and the role the backup is performed under — a role whose
        // permissions allow the backup leads the job to COMPLETED, one that allows nothing of
        // the kind to FAILED.
        let resource_arn = row
            .protected
            .created(&router, case, &format!("backup-events-protected-{index}"))
            .await;
        let vault_name = format!("backup-events-vault-{index}");
        backup(
            &router,
            case,
            "PUT",
            &format!("/backup-vaults/{vault_name}"),
            Some(&json!({})),
        )
        .await;
        let backup_role = role(
            &router,
            &format!("backup-events-role-{index}"),
            "backup.amazonaws.com",
            if row.permitted {
                &["*"]
            } else {
                &["s3:GetObject"]
            },
        )
        .await;

        let started = backup(
            &router,
            case,
            "PUT",
            "/backup-jobs",
            Some(&json!({
                "BackupVaultName": &vault_name,
                "ResourceArn": &resource_arn,
                "IamRoleArn": &backup_role,
            })),
        )
        .await;
        let job_id = member(case, &started, "/BackupJobId");
        let recovery_point_arn = member(case, &started, "/RecoveryPointArn");

        // What a client sees of the delivery: the function's own output in its default log
        // group. No further AWS Backup call is made — the transition the clock owes is
        // published by the service itself.
        let Some(expected_state) = row.expected else {
            // The control proves the publication and the dispatch happened; after one further
            // settle window — longer than the span between the two dispatches of one
            // publication — this row's own function is read, and it must hold nothing of the
            // job.
            let control = control.as_deref().expect("the control is deployed for such a row");
            let delivered = awaited_log_messages(&router, control, &job_id).await;
            assert!(
                delivered.iter().any(|line| line.contains(&job_id)),
                "case {case:?}: the control function must receive the job's envelope, got \
                 {delivered:?} for job {job_id:?}"
            );
            tokio::time::sleep(SECOND_LOOK).await;
            let seen = log_messages(&router, &format!("/aws/lambda/{function_name}")).await;
            assert!(
                !seen.iter().any(|line| line.contains(&job_id)),
                "case {case:?}: nothing may invoke the function, yet its log group holds \
                 {seen:?} for job {job_id:?}"
            );
            continue;
        };
        let seen = awaited_log_messages(&router, &function_name, &job_id).await;
        let envelopes: Vec<Value> = seen
            .iter()
            .filter_map(|line| line.split_once(MARKER).map(|(_, rest)| rest.trim()))
            .map(|payload| {
                serde_json::from_str(payload).unwrap_or_else(|error| {
                    panic!(
                        "case {case:?}: the handler's line must carry the envelope it was \
                         handed ({error}), got {payload:?}"
                    )
                })
            })
            .collect();
        // Each state the job moves into is published ONCE, so no state reaches the target
        // twice however many transitions the pattern admits — and exactly one of the
        // envelopes carries the state this row's pattern asked for.
        let mut states: Vec<String> = envelopes
            .iter()
            .map(|envelope| {
                envelope["detail"]["state"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        states.sort();
        assert!(
            states.windows(2).all(|pair| pair[0] != pair[1]),
            "case {case:?}: a state is published once, so none may reach the target twice, \
             got {states:?} from {seen:?}"
        );
        let matching: Vec<&Value> = envelopes
            .iter()
            .filter(|envelope| envelope["detail"]["state"] == json!(expected_state))
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {case:?}: expected exactly one envelope carrying {expected_state:?}, got \
             {states:?} from {seen:?}"
        );
        let envelope = matching[0].clone();

        // The envelope is the whole one AWS hands a target, and its detail is this job's own
        // stored state.
        for (pointer, expect) in [
            ("/source", json!(BACKUP_SOURCE)),
            ("/detail-type", json!(BACKUP_JOB_STATE_CHANGE)),
            ("/account", json!(engine::EMULATOR_ACCOUNT_ID)),
            ("/region", json!(REGION)),
            ("/resources", json!([&recovery_point_arn])),
            ("/detail/backupJobId", json!(&job_id)),
            ("/detail/state", json!(expected_state)),
            ("/detail/backupVaultName", json!(&vault_name)),
            ("/detail/resourceArn", json!(&resource_arn)),
            ("/detail/resourceType", json!(row.protected.resource_type())),
            ("/detail/iamRoleArn", json!(&backup_role)),
        ] {
            let actual = envelope.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expect,
                "case {case:?}: expected {pointer} to be {expect}, got {actual} in {envelope}"
            );
        }
        assert!(
            envelope["id"].as_str().is_some_and(|id| !id.is_empty()),
            "case {case:?}: expected the envelope to carry an event id, got {envelope}"
        );
        assert!(
            envelope["time"].as_str().is_some_and(|at| at.ends_with('Z')),
            "case {case:?}: expected the envelope to state when the job moved, got {envelope}"
        );
        // A job that could not be performed says why; one that was performed has nothing to
        // report.
        let status_message = envelope
            .pointer("/detail/statusMessage")
            .and_then(Value::as_str);
        if expected_state == FAILED {
            assert!(
                status_message.is_some_and(|message| !message.is_empty()),
                "case {case:?}: a FAILED job's envelope must say why, got {envelope}"
            );
        } else {
            assert_eq!(
                status_message, None,
                "case {case:?}: a performed backup reports no status message, got {envelope}"
            );
        }

        // The same state the control plane reports for the same job: the envelope is the
        // job's own stored state and not a second, invented copy of it.
        let described = backup(
            &router,
            case,
            "GET",
            &format!("/backup-jobs/{job_id}"),
            None,
        )
        .await;
        let reported = member(case, &described, "/State");
        assert_eq!(
            reported, expected_state,
            "case {case:?}: expected DescribeBackupJob to report {expected_state:?} for the \
             state the envelope carried, got {reported:?} in {described}"
        );
    }
}
