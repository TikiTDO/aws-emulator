// SPDX-License-Identifier: CC0-1.0

//! What a CloudFormation-deployed `AWS::Pipes::Pipe` DOES once its stack reports it created.
//!
//! Two rules about the execution role a declaration gives a pipe, one per table below.
//!
//! RULE 1 (the delivery table): **a pipe a stack provisions is a LIVE pipe — once the stack
//! has created it and EventBridge Pipes reports it `RUNNING`, that pipe polls the source its
//! declaration named and delivers to the target it named with no client action at all; and
//! every call it makes on the way is authorized against the execution role the declaration
//! gave it, so a pipe whose role may not DELIVER does not deliver: nothing reaches its target
//! and the record stays on its source queue.**
//!
//! RULE 2 (the rollback table): **a pipe that could not POLL its source at all is a pipe
//! EventBridge Pipes cannot create — it settles in `CREATE_FAILED` with the reason
//! (`services/service-com-amazonaws-pipes-pipes/tests/all/pipe_bring_up.rs` has that rule's
//! own table) — so the resource that declared it FAILS and the stack it is in is rolled back
//! rather than reported created.** That is the difference between the two tables: the target
//! side is one execution of a running pipe failing, and the source side is a pipe that never
//! runs.
//!
//! Four crates have to agree for either to hold — CloudFormation makes the provisioning calls,
//! EventBridge Pipes runs the pipe, IAM holds the roles the pipe acts as, and Amazon SQS
//! holds the sources and the targets — so the suite lives here rather than in any one of
//! them. `services/service-com-amazonaws-pipes-pipes/tests/all/pipe_execution_role.rs` asks
//! the authorization half of the same question of an API-created pipe; what is new here is
//! that a pipe the DECLARATIVE path created behaves exactly like it.
//!
//! Every row deploys ONE stack declaring two pipes side by side: one whose execution role
//! admits every call its stages make, and one whose role is the row's. The rows of each table
//! differ along what its rule generalises over — HOW the role withholds the call (no statement
//! about the action at all, a statement naming another queue, an explicit `Deny` over a
//! wildcard allow). The record a withheld pipe cannot move is counted as the queue still
//! HOLDING it, which covers both a record never taken and one taken and not delivered.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{form_encode, opt_element, outputs_of};

/// The Region every row deploys in.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// How long a stack operation is given to reach a terminal status.
const PATIENCE: Duration = Duration::from_secs(30);

/// How long a delivery the pipe has to make is given before a row fails. Well above the
/// poller's own interval and the pipe's settle span, so a row fails only when the pipe did
/// not deliver at all.
const BUDGET: Duration = Duration::from_secs(25);

/// How long an assertion that NOTHING is delivered watches for. A pipe that was going to
/// deliver would have: it is many times the poller's interval.
const QUIET: Duration = Duration::from_secs(3);

/// How often a queue or a stack is looked at again while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// `TargetParameters.InputTemplate`, so each delivered message carries the source record's
/// own body and a delivery can be attributed to the record that caused it: `<$.body>` selects
/// the `body` of the record envelope the target stage is handed.
const BODY_TEMPLATE: &str = "<$.body>";

/// The attributes that together say how many records a queue still HOLDS: the ones waiting on
/// it plus the ones a consumer has taken and not deleted ("messages that are in flight",
/// `ApproximateNumberOfMessagesNotVisible`), which is what a record a pipe could not move is
/// one of.
const HELD_ATTRIBUTES: [&str; 2] = [
    "ApproximateNumberOfMessages",
    "ApproximateNumberOfMessagesNotVisible",
];

/// The emulator with the four services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process calls CloudFormation and the pipe make are signed either way
/// (the credential scope is what routes them).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_pipes_pipes::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in [`REGION`], which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
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
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// One Amazon SQS call over its `awsJson1.0` wire.
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
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `DescribePipe` over EventBridge Pipes' own `restJson1` wire, on the URI that operation's
/// `@http` binding declares (`GET /v1/pipes/{Name}`).
async fn describe_pipe(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/v1/pipes/{name}"))
        .header("host", format!("pipes.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("pipes"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The stack as it reads once its operation has reached a TERMINAL status, polled rather than
/// slept for: the pipes' own creations hold the deployment open, so how many spans it takes is
/// the deployment's to decide and not this suite's to assume.
async fn settled_stack(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let began = Instant::now();
    loop {
        let (status, described) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStacks answered {status} {described}"
        );
        let reported = opt_element(&described, "StackStatus").unwrap_or_default();
        if !reported.ends_with("_IN_PROGRESS") || began.elapsed() >= PATIENCE {
            return described;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Send `body` to the queue at `url`.
async fn send(router: &MultiServiceRouter, case: &str, url: &str, body: &str) {
    let (status, answered) = sqs(
        router,
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": body }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SendMessage answered {status} {answered}"
    );
}

/// One `ReceiveMessage` of up to ten messages at `url`, as bodies, left on the queue:
/// `VisibilityTimeout: 0` takes no checkout, so a later look sees them again and nothing this
/// suite reads is hidden from the next assertion.
async fn receive_now(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<String> {
    let (status, answered) = sqs(
        router,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10, "VisibilityTimeout": 0 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ReceiveMessage answered {status} {answered}"
    );
    answered["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every message body seen at `url` until `wanted` has been seen among them, or `budget`
/// lapses.
async fn collected_until(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    wanted: &str,
    budget: Duration,
) -> BTreeSet<String> {
    let deadline = Instant::now() + budget;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    loop {
        seen.extend(receive_now(router, case, url).await);
        if seen.contains(wanted) || Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// How many records the queue at `url` still HOLDS ([`HELD_ATTRIBUTES`]).
async fn held(router: &MultiServiceRouter, case: &str, url: &str) -> i64 {
    let (status, answered) = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": HELD_ATTRIBUTES }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueueAttributes answered {status} {answered}"
    );
    HELD_ATTRIBUTES
        .iter()
        .map(|attribute| {
            answered["Attributes"][attribute]
                .as_str()
                .unwrap_or_default()
                .parse()
                .unwrap_or(-1)
        })
        .sum()
}

/// Wait until the queue at `url` holds `want` records, answering what it held last.
async fn wait_for_held(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    want: i64,
    budget: Duration,
) -> i64 {
    let deadline = Instant::now() + budget;
    loop {
        let counted = held(router, case, url).await;
        if counted == want || Instant::now() >= deadline {
            return counted;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The trust policy every execution role here carries: EventBridge Pipes acts as the pipe's
/// `RoleArn`, so a role a stack creates for a pipe has to admit that service principal or the
/// pipe's creation cannot be completed at all.
fn pipes_trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "pipes.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
}

/// A role declaration carrying `statements` as its one inline policy.
fn role(statements: Value) -> Value {
    json!({
        "Type": "AWS::IAM::Role",
        "Properties": {
            "AssumeRolePolicyDocument": pipes_trust_policy(),
            "Policies": [{
                "PolicyName": "move-records",
                "PolicyDocument": { "Version": "2012-10-17", "Statement": statements },
            }],
        },
    })
}

/// The statements a pipe's execution role carries when it may make every call its stages
/// make: the calls an Amazon SQS source's stage makes on the source, and the one an Amazon SQS
/// target's stage makes on the target.
fn admits_every_call() -> Value {
    json!([{
        "Effect": "Allow",
        "Action": [
            "sqs:ReceiveMessage",
            "sqs:DeleteMessage",
            "sqs:GetQueueAttributes",
            "sqs:SendMessage",
        ],
        "Resource": "*",
    }])
}

/// One pipe declaration: the source and target queues named by their logical ids, under the
/// role named by its own, with the template that makes each delivery carry the record's body.
fn pipe(name: &str, source: &str, target: &str, pipe_role: &str) -> Value {
    json!({
        "Type": "AWS::Pipes::Pipe",
        "Properties": {
            "Name": name,
            "Source": { "Fn::GetAtt": [source, "Arn"] },
            "Target": { "Fn::GetAtt": [target, "Arn"] },
            "RoleArn": { "Fn::GetAtt": [pipe_role, "Arn"] },
            "SourceParameters": { "SqsQueueParameters": { "BatchSize": 1 } },
            "TargetParameters": { "InputTemplate": BODY_TEMPLATE },
        },
    })
}

/// One row of the table: the statements the WITHHELD pipe's role carries.
struct Row {
    case: &'static str,
    /// The prefix every name this row's template declares carries, so two rows never share a
    /// resource.
    prefix: &'static str,
    /// The statements of the role the row's second pipe acts as — built from the row's own
    /// queue names, because what the rule turns on is the (action, RESOURCE) pair each call is
    /// made on.
    withheld: fn(&Row) -> Value,
}

impl Row {
    fn stack(&self) -> String {
        format!("{}-stack", self.prefix)
    }

    fn open_source(&self) -> String {
        format!("{}-open-source", self.prefix)
    }

    fn withheld_source(&self) -> String {
        format!("{}-withheld-source", self.prefix)
    }

    fn open_target(&self) -> String {
        format!("{}-open-target", self.prefix)
    }

    fn withheld_target(&self) -> String {
        format!("{}-withheld-target", self.prefix)
    }

    fn open_pipe(&self) -> String {
        format!("{}-open-pipe", self.prefix)
    }

    fn withheld_pipe(&self) -> String {
        format!("{}-withheld-pipe", self.prefix)
    }

    /// The ARN of the queue called `name`: the same total function of the Region and the name
    /// Amazon SQS mints one from, so a statement here names the very resource the pipe's call
    /// is made on.
    fn queue_arn(name: &str) -> String {
        format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{name}")
    }

    /// The one template this row deploys: four queues, two roles and two pipes, with each
    /// queue's url published as an output (`Ref` on `AWS::SQS::Queue` returns the queue URL).
    fn template(&self) -> String {
        json!({
            "Resources": {
                "OpenSource": {
                    "Type": "AWS::SQS::Queue",
                    "Properties": { "QueueName": self.open_source() },
                },
                "WithheldSource": {
                    "Type": "AWS::SQS::Queue",
                    "Properties": { "QueueName": self.withheld_source() },
                },
                "OpenTarget": {
                    "Type": "AWS::SQS::Queue",
                    "Properties": { "QueueName": self.open_target() },
                },
                "WithheldTarget": {
                    "Type": "AWS::SQS::Queue",
                    "Properties": { "QueueName": self.withheld_target() },
                },
                "OpenRole": role(admits_every_call()),
                "WithheldRole": role((self.withheld)(self)),
                "OpenPipe": pipe(&self.open_pipe(), "OpenSource", "OpenTarget", "OpenRole"),
                "WithheldPipe": pipe(
                    &self.withheld_pipe(),
                    "WithheldSource",
                    "WithheldTarget",
                    "WithheldRole",
                ),
            },
            "Outputs": {
                "OpenSourceUrl": { "Value": { "Ref": "OpenSource" } },
                "WithheldSourceUrl": { "Value": { "Ref": "WithheldSource" } },
                "OpenTargetUrl": { "Value": { "Ref": "OpenTarget" } },
                "WithheldTargetUrl": { "Value": { "Ref": "WithheldTarget" } },
            },
        })
        .to_string()
    }
}

/// The rows whose withheld pipe CAN be brought up: its role may take the source's records,
/// and what it may not do is DELIVER them. They differ in how the delivery is withheld.
const DELIVERY_ROWS: &[Row] = &[
    Row {
        case: "a role that may poll its source but not deliver to its target",
        prefix: "pipe-withheld-send",
        withheld: |row| {
            json!([{
                "Effect": "Allow",
                "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"],
                "Resource": Row::queue_arn(&row.withheld_source()),
            }])
        },
    },
    Row {
        case: "a role whose send grant names ANOTHER queue than its target",
        prefix: "pipe-withheld-send-other",
        withheld: |row| {
            json!([
                {
                    "Effect": "Allow",
                    "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"],
                    "Resource": Row::queue_arn(&row.withheld_source()),
                },
                {
                    "Effect": "Allow",
                    "Action": ["sqs:SendMessage"],
                    "Resource": Row::queue_arn(&row.open_target()),
                },
            ])
        },
    },
    Row {
        case: "a role whose wildcard grant is overridden by a deny on the target send",
        prefix: "pipe-withheld-send-deny",
        withheld: |row| {
            json!([
                { "Effect": "Allow", "Action": ["sqs:*"], "Resource": "*" },
                {
                    "Effect": "Deny",
                    "Action": ["sqs:SendMessage"],
                    "Resource": Row::queue_arn(&row.withheld_target()),
                },
            ])
        },
    },
];

/// The rows whose withheld pipe could not POLL its source at all, so its creation cannot be
/// completed and the stack that declared it is rolled back. They differ in how the poll is
/// withheld — the same three ways the delivery table withholds a delivery.
const ROLLBACK_ROWS: &[Row] = &[
    Row {
        case: "a role that states nothing at all about polling its source",
        prefix: "pipe-withheld-unstated",
        withheld: |_| {
            json!([{
                "Effect": "Allow",
                "Action": ["sqs:SendMessage", "sqs:GetQueueAttributes"],
                "Resource": "*",
            }])
        },
    },
    Row {
        case: "a role whose poll grant names ANOTHER queue than its source",
        prefix: "pipe-withheld-other",
        withheld: |row| {
            json!([{
                "Effect": "Allow",
                "Action": ["sqs:*"],
                "Resource": [
                    Row::queue_arn(&row.open_source()),
                    Row::queue_arn(&row.withheld_target()),
                ],
            }])
        },
    },
    Row {
        case: "a role whose wildcard grant is overridden by a deny on the source receive",
        prefix: "pipe-withheld-deny",
        withheld: |row| {
            json!([
                { "Effect": "Allow", "Action": ["sqs:*"], "Resource": "*" },
                {
                    "Effect": "Deny",
                    "Action": ["sqs:ReceiveMessage"],
                    "Resource": Row::queue_arn(&row.withheld_source()),
                },
            ])
        },
    },
];

/// RULE 1, every row: the stack creates both pipes, EventBridge Pipes reports both `RUNNING`,
/// the pipe whose role admits every call delivers its record to its target with no client
/// action, and the sibling whose role withholds the DELIVERY delivers nothing — its record is
/// still its source queue's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deployed_pipe_delivers_only_what_its_execution_role_allows() {
    let router = build_router();

    for row in DELIVERY_ROWS {
        let case = row.case;
        let stack = row.stack();

        let (status, created) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack),
                ("TemplateBody", &row.template()),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStack answered {status} {created}"
        );

        let described = settled_stack(&router, case, &stack).await;
        let reported = opt_element(&described, "StackStatus").unwrap_or_default();
        assert_eq!(
            reported, "CREATE_COMPLETE",
            "case {case:?}: expected the stack CREATE_COMPLETE, got {reported:?} in {described}"
        );

        let outputs = outputs_of(&described);
        let url = |key: &str| {
            outputs
                .get(key)
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {described}"))
        };
        let open_source = url("OpenSourceUrl");
        let withheld_source = url("WithheldSourceUrl");
        let open_target = url("OpenTargetUrl");
        let withheld_target = url("WithheldTargetUrl");

        // Both pipes are the live pipes the stack says it created: a role that withholds a
        // stage's call does not change the state the pipe reports.
        for name in [row.open_pipe(), row.withheld_pipe()] {
            let (status, answered) = describe_pipe(&router, &name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribePipe {name} answered {status} {answered}"
            );
            let state = answered["CurrentState"].as_str().unwrap_or_default();
            assert_eq!(
                state, "RUNNING",
                "case {case:?}: expected the deployed pipe {name} RUNNING, got {state:?}"
            );
        }

        // One record per source, each naming the leg it was sent on.
        let delivered = format!("{}-delivered", row.prefix);
        let withheld = format!("{}-withheld", row.prefix);
        send(&router, case, &open_source, &delivered).await;
        send(&router, case, &withheld_source, &withheld).await;

        let seen = collected_until(&router, case, &open_target, &delivered, BUDGET).await;
        assert!(
            seen.contains(&delivered),
            "case {case:?}: expected the pipe whose role admits every call to deliver \
             {delivered:?} to its target, saw {seen:?}"
        );
        let drained = wait_for_held(&router, case, &open_source, 0, BUDGET).await;
        assert_eq!(
            drained, 0,
            "case {case:?}: expected the delivered record to have left its source, it holds \
             {drained}"
        );

        // The withheld leg: nothing of it ever reaches its target, however long the open leg's
        // own delivery took.
        tokio::time::sleep(QUIET).await;
        let seen = receive_now(&router, case, &withheld_target).await;
        assert!(
            seen.is_empty(),
            "case {case:?}: expected the target of the pipe whose role withholds a stage's \
             call to hold nothing, saw {seen:?}"
        );

        // And its record is still its source's — never taken, or taken and not moved.
        let kept = wait_for_held(&router, case, &withheld_source, 1, BUDGET).await;
        assert_eq!(
            kept, 1,
            "case {case:?}: expected the source of the withheld pipe to still hold its one \
             record, it holds {kept}"
        );
    }
}

/// RULE 2, every row: a pipe whose execution role may not take its SOURCE's records could
/// never poll it, so EventBridge Pipes cannot complete that pipe's creation — the resource
/// that declared it fails, and the stack reaches `ROLLBACK_COMPLETE` naming that resource
/// instead of reporting a deployment that did not happen. The rollback takes the pipe back
/// out, so EventBridge Pipes holds no pipe under that name afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stack_whose_pipe_cannot_poll_its_source_is_rolled_back() {
    let router = build_router();

    for row in ROLLBACK_ROWS {
        let case = row.case;
        let stack = row.stack();

        let (status, created) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack),
                ("TemplateBody", &row.template()),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStack answered {status} {created}"
        );

        let described = settled_stack(&router, case, &stack).await;
        let reported = opt_element(&described, "StackStatus").unwrap_or_default();
        assert_eq!(
            reported, "ROLLBACK_COMPLETE",
            "case {case:?}: expected the stack whose pipe cannot poll its source to roll \
             back, got {reported:?} in {described}"
        );
        let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
        assert!(
            reason.contains("WithheldPipe"),
            "case {case:?}: expected the stack's reason to name the resource that failed, got \
             {reason:?}"
        );

        // The pipe that could not be brought up is not a live pipe: the rollback deleted the
        // pipe the resource had created, so EventBridge Pipes holds none under that name.
        let name = row.withheld_pipe();
        let (status, answered) = describe_pipe(&router, &name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected EventBridge Pipes to hold no pipe {name} after the \
             rollback, it answered {status} {answered}"
        );
    }
}
