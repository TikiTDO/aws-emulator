// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING function's own Amazon SQS call is authorized against: the policies in
//! force on the function's execution role.
//!
//! RULE: a call an AWS SDK makes from inside a running function is made as a session of that
//! function's execution role, so the service serving it decides it against the policies in
//! force on that role — a queue operation no identity-based policy in force on the role
//! allows is refused `AccessDenied` (HTTP 403) naming the assumed-role session, the IAM
//! action the operation performs and the queue's ARN, and the refused call leaves the queue
//! exactly as it was; the same call by a role whose documents allow that action on that queue
//! is served and its effect is in the queue afterwards. A refusal the handler does not catch
//! is the FUNCTION's own failure: the invocation is still answered HTTP 200, carrying
//! `X-Amz-Function-Error: Unhandled` and the error document of the run, not an error of the
//! `Invoke` call.
//!
//! "The execution role grants the function permission to use Amazon Web Services services"
//! (`FunctionConfiguration$Role`), and the identity a function's own call speaks for is the
//! session `arn:aws:sts::<account>:assumed-role/<role>/<function>`; with neither an
//! identity-based nor a resource-based allow a request is denied (IAM User Guide, *Policy
//! evaluation logic*). Amazon SQS's model declares no authorization error for these
//! operations, so the refusal is the AWS-common one every service answers for a request its
//! policies do not permit, and it names the action the way this service words a refusal.
//!
//! Five crates have to agree for that to hold — CloudFormation deploys the queue, the role
//! and the relay function together, IAM holds the role and its inline document, STS mints the
//! session the function's calls are signed with, Lambda runs the handler and supplies it the
//! SDK, and Amazon SQS takes the decision — so the suite lives here: a crate-local suite can
//! only ever see one side of it. What the neighbours pin and this one does not repeat: the
//! Amazon SQS crate's own `queue_policy_authorization` drives the decision for a credential a
//! client assumed directly, and `lambda_in_function_dynamodb_authorization` drives the same
//! in-function seam against another service's decision. What only this suite reaches is the
//! decision taken on a message-plane call a FUNCTION made against the role it runs as, which
//! is what a least-privilege relay's execution role is written for.
//!
//! The rows differ along what the rule generalises over: what the role's own document allows
//! (nothing of this service, this operation's action, another operation's action, every
//! action of the service), which resource that statement names (the queue the handler calls,
//! another queue of the account, every resource), WHICH queue operation the handler calls (a
//! send, a receive, a name resolution) — several per role, so one role's document is asked
//! about more than one action — and whether the handler catches the refusal. Each probe
//! asserts both halves: what the handler reports of the call, and what the QUEUE holds
//! afterwards, so a refusal that still enqueued, and a grant that answered without enqueuing,
//! both fail here.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{elements, form_encode, physical_ids_of};

/// The Region every request here is signed for, and so the Region the queues and the
/// functions belong to.
const REGION: &str = "us-east-1";

/// The account this emulator serves, which every queue ARN and every session ARN carries.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The queue every row's handler addresses.
const QUEUE: &str = "xsvc-in-fn-sqs-relay";

/// Another queue of this account, which one row's statement names instead of the queue the
/// handler calls.
const OTHER_QUEUE: &str = "xsvc-in-fn-sqs-elsewhere";

/// The stack the queues, the roles and the relay functions are deployed by.
const STACK: &str = "xsvc-in-fn-sqs-probes";

/// The `VisibilityTimeout` both declared queues carry, in seconds: the longest a queue may be
/// configured for ("an integer from 0 to 43,200 (12 hours)",
/// `SetQueueAttributesRequest$Attributes`). A message a probe's own receive took is then still
/// in flight at every later probe, however long the suite takes, so no assertion about what
/// the queue holds can turn on how fast the host runs. Nothing here tests the timeout itself.
const VISIBILITY_TIMEOUT: u64 = 43_200;

/// The `StackStatus` that stack must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The code and status Amazon SQS refuses a call the policies in force do not permit with.
const ACCESS_DENIED: &str = "AccessDenied";
const ACCESS_DENIED_STATUS: u64 = 403;

/// The signal an invocation whose handler raised carries, and the status such an invocation
/// is still answered on: the run failed, the call did not.
const UNHANDLED: &str = "Unhandled";

/// How long one invocation is given, as the function's own `Timeout`: well past what a run
/// needs on any host, because what is under test is the authorization decision and not how
/// fast an interpreter gets through the handler.
const FUNCTION_TIMEOUT: u64 = 120;

/// How often to look while waiting for work the clock owes — a stack reaching
/// `CREATE_COMPLETE`, a created function reaching `Active`, a message the queue is owed — and
/// how many looks to take. The spans are those services' own, so no fixed sleep names the
/// instant one ends, and the deadline is generous so a loaded host cannot decide the outcome.
const LOOK: Duration = Duration::from_millis(100);
const LOOKS: usize = 1_800;

/// The handler every row runs. It performs the one queue operation the event names with the
/// SDK the runtime supplies, and either reports the refusal Amazon SQS answered — its code,
/// its sentence and the HTTP status it came back on — or leaves it to propagate, which is what
/// makes the invocation report a function error.
const PROBE_HANDLER: &str = r##"
import boto3
from botocore.exceptions import ClientError


def call(client, event):
    if event["op"] == "send":
        answered = client.send_message(
            QueueUrl=event["url"], MessageBody=event["body"]
        )
        return {"ok": True, "id": bool(answered.get("MessageId"))}
    if event["op"] == "receive":
        answered = client.receive_message(
            QueueUrl=event["url"], MaxNumberOfMessages=10
        )
        bodies = [message["Body"] for message in answered.get("Messages", [])]
        return {"ok": True, "bodies": bodies}
    answered = client.get_queue_url(QueueName=event["name"])
    return {"ok": True, "url": answered["QueueUrl"]}


def handler(event, context):
    client = boto3.client("sqs")
    if not event["catch"]:
        return call(client, event)
    try:
        return call(client, event)
    except ClientError as refused:
        return {
            "ok": False,
            "code": refused.response["Error"]["Code"],
            "message": refused.response["Error"]["Message"],
            "status": refused.response["ResponseMetadata"]["HTTPStatusCode"],
        }
"##;

/// The emulator as it is ASSEMBLED, on one engine: a handler's own SDK call is dispatched
/// back into this same router, so the queue it addresses is the queue this suite reads.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. Its credential is the long-term key every emulator publishes, so
/// these calls are the account's own and not any row's role's.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One awsQuery call of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
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

/// One Amazon SQS operation over its `awsJson1_0` wire, made as the account's own credential
/// — the queue's owner, whose calls no row's grant decides.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("authorization", authorization("sqs"))
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
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

/// The ARN a queue of this account in this Region carries, which is what a statement names
/// and what a refusal reports.
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{name}")
}

/// WHICH queue operation one probe's handler calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Send,
    Receive,
    Url,
}

impl Op {
    /// The word the event names it by, which the handler reads.
    fn event(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Receive => "receive",
            Self::Url => "url",
        }
    }

    /// The IAM action the operation performs, which is the action a statement grants and a
    /// refusal names: this service's action list carries one `sqs:<OperationName>` per
    /// operation.
    fn action(self) -> &'static str {
        match self {
            Self::Send => "sqs:SendMessage",
            Self::Receive => "sqs:ReceiveMessage",
            Self::Url => "sqs:GetQueueUrl",
        }
    }

    /// Whether the queue has to hold a message before the invocation: a receive reports one,
    /// and a receive that is refused has to leave it in the queue.
    fn needs_a_message(self) -> bool {
        matches!(self, Self::Receive)
    }
}

/// WHICH resource a row's statement names.
#[derive(Debug, Clone, Copy)]
enum Scope {
    /// The queue the handler calls.
    Queue,
    /// Another queue of the same account.
    OtherQueue,
    /// Every resource, which the bare `*` wildcard stands for.
    Every,
}

impl Scope {
    fn resource(self) -> String {
        match self {
            Self::Queue => queue_arn(QUEUE),
            Self::OtherQueue => queue_arn(OTHER_QUEUE),
            Self::Every => "*".to_string(),
        }
    }
}

/// One probe of a row's role: the operation the handler calls, whether the policies in force
/// let it through, and whether the handler catches a refusal.
struct Probe {
    case: &'static str,
    op: Op,
    allowed: bool,
    catches: bool,
}

/// One row: the document the execution role carries, and the probes taken against it.
struct Row {
    /// The actions the role's inline document allows besides the log actions every execution
    /// role carries; empty for a role granting none of this service's.
    actions: &'static [&'static str],
    /// The resource that statement names.
    scope: Scope,
    probes: &'static [Probe],
}

const ROWS: &[Row] = &[
    // The case the rule is stated about: a least-privilege role granting only the log actions
    // a function needs is granted no queue operation at all, so the relay's own send is
    // refused and — left uncaught — fails the run.
    Row {
        actions: &[],
        scope: Scope::Queue,
        probes: &[
            Probe {
                case: "a role granting only the log actions sends nothing",
                op: Op::Send,
                allowed: false,
                catches: true,
            },
            Probe {
                case: "the same role is granted no receive either",
                op: Op::Receive,
                allowed: false,
                catches: true,
            },
            Probe {
                case: "a refusal the relay does not catch is the function's own error",
                op: Op::Send,
                allowed: false,
                catches: false,
            },
        ],
    },
    Row {
        actions: &["sqs:SendMessage"],
        scope: Scope::Queue,
        probes: &[
            Probe {
                case: "a role granted the send on the queue sends the message",
                op: Op::Send,
                allowed: true,
                catches: true,
            },
            Probe {
                case: "a served send leaves the invocation clean",
                op: Op::Send,
                allowed: true,
                catches: false,
            },
            Probe {
                case: "the same role is granted no receive by it",
                op: Op::Receive,
                allowed: false,
                catches: true,
            },
            Probe {
                case: "the same role is granted no name resolution by it",
                op: Op::Url,
                allowed: false,
                catches: true,
            },
        ],
    },
    Row {
        actions: &["sqs:SendMessage"],
        scope: Scope::OtherQueue,
        probes: &[Probe {
            case: "a role granted the send on ANOTHER queue sends nothing",
            op: Op::Send,
            allowed: false,
            catches: true,
        }],
    },
    Row {
        actions: &["sqs:*"],
        scope: Scope::Queue,
        probes: &[
            Probe {
                case: "a role granted every action of the service on the queue sends",
                op: Op::Send,
                allowed: true,
                catches: true,
            },
            Probe {
                case: "and receives by the same statement",
                op: Op::Receive,
                allowed: true,
                catches: true,
            },
        ],
    },
    Row {
        actions: &["sqs:ReceiveMessage"],
        scope: Scope::Every,
        probes: &[
            Probe {
                case: "a role granted the receive on every resource receives",
                op: Op::Receive,
                allowed: true,
                catches: true,
            },
            Probe {
                case: "the same role is granted no send by it",
                op: Op::Send,
                allowed: false,
                catches: true,
            },
        ],
    },
    Row {
        actions: &["sqs:GetQueueUrl"],
        scope: Scope::Queue,
        probes: &[
            Probe {
                case: "a role granted the name resolution resolves the queue",
                op: Op::Url,
                allowed: true,
                catches: true,
            },
            Probe {
                case: "the same role is granted no send by it",
                op: Op::Send,
                allowed: false,
                catches: true,
            },
        ],
    },
];

/// The role one row runs as, and the function it runs in — named here because the refusal's
/// own sentence names both: the identity a function's call is signed as is the session
/// `arn:aws:sts::<account>:assumed-role/<role>/<function>`.
fn role_name(index: usize) -> String {
    format!("xsvc-in-fn-sqs-role-{index}")
}
fn function_name(index: usize) -> String {
    format!("xsvc-in-fn-sqs-relay-{index}")
}

/// The sentence Amazon SQS refuses a call nothing in force allows with. Written out here
/// rather than built from the service's own helper, so the wording has something to be wrong
/// about.
///
/// The action is named case-folded, which is how this service words a refusal: `sqs:*` is one
/// action list and the case a policy or the model spells a name in is not part of the
/// sentence.
fn refusal_message(index: usize, action: &str) -> String {
    let named = action.to_lowercase();
    format!(
        "User: arn:aws:sts::{ACCOUNT}:assumed-role/{}/{} is not authorized to perform: \
         {named} on resource: {} because no identity-based policy allows the {named} action",
        role_name(index),
        function_name(index),
        queue_arn(QUEUE),
    )
}

/// The trust policy a Lambda execution role needs: Lambda has to be able to assume it for the
/// function to be handed it at all.
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

/// The inline document one row's role carries: the log actions every execution role needs,
/// and the row's own statement where it has one.
fn role_document(row: &Row) -> Value {
    let mut statements = vec![json!({
        "Effect": "Allow",
        "Action": ["logs:CreateLogGroup", "logs:CreateLogStream", "logs:PutLogEvents"],
        "Resource": "*",
    })];
    if !row.actions.is_empty() {
        statements.push(json!({
            "Effect": "Allow",
            "Action": row.actions,
            "Resource": row.scope.resource(),
        }));
    }
    json!({"Version": "2012-10-17", "Statement": statements})
}

/// The template the suite deploys: the two queues, one execution role per row carrying that
/// row's document, and one relay function per row running as that role. The inline source is
/// archived into the deployment package by CloudFormation, which is how a function whose
/// handler is written here comes to exist at all.
fn stack_template() -> String {
    let mut resources = serde_json::Map::new();
    for (name, logical) in [(QUEUE, "Relay"), (OTHER_QUEUE, "Elsewhere")] {
        resources.insert(
            logical.to_string(),
            json!({
                "Type": "AWS::SQS::Queue",
                "Properties": {
                    "QueueName": name,
                    "VisibilityTimeout": VISIBILITY_TIMEOUT,
                },
            }),
        );
    }
    for (index, row) in ROWS.iter().enumerate() {
        let role_logical = format!("Role{index}");
        resources.insert(
            role_logical.clone(),
            json!({
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": role_name(index),
                    "AssumeRolePolicyDocument": trust_policy(),
                    "Policies": [{
                        "PolicyName": "relay-grants",
                        "PolicyDocument": role_document(row),
                    }],
                },
            }),
        );
        resources.insert(
            format!("Probe{index}"),
            json!({
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": function_name(index),
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": {"Fn::GetAtt": [role_logical, "Arn"]},
                    "Timeout": FUNCTION_TIMEOUT,
                    "Code": {"ZipFile": PROBE_HANDLER},
                },
            }),
        );
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": Value::Object(resources),
    })
    .to_string()
}

/// Deploy the stack, wait for `CREATE_COMPLETE` and answer the physical id of each declared
/// resource.
async fn deploy(router: &MultiServiceRouter) -> BTreeMap<String, String> {
    let template = stack_template();
    let (status, answered) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "CreateStack",
        &[
            ("StackName", STACK),
            ("TemplateBody", &template),
            // The template declares named IAM roles, which a create acknowledges before
            // CloudFormation provisions them.
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateStack({STACK:?}) must be accepted, got {status} with {answered}"
    );
    // The stack's completion is owed from the instant the owning services report its
    // resources ready, which no fixed sleep can name: this looks until it reports it, on the
    // shared wait whose deadline is generous enough that a loaded host does not decide the
    // outcome.
    let reported = wait_for_stack_status(router, STACK, CREATE_COMPLETE).await;
    let failures = if reported.0 == CREATE_COMPLETE {
        Vec::new()
    } else {
        let (_, events) = query(
            router,
            "cloudformation",
            "2010-05-15",
            "DescribeStackEvents",
            &[("StackName", STACK)],
        )
        .await;
        elements(&events, "ResourceStatusReason")
    };
    assert_eq!(
        reported.0, CREATE_COMPLETE,
        "setup: the stack must reach {CREATE_COMPLETE}, got {} with resource failures \
         {failures:?}",
        reported.1
    );
    let (status, resources) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "DescribeStackResources",
        &[("StackName", STACK)],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: DescribeStackResources({STACK:?}) must succeed, got {status} with {resources}"
    );
    physical_ids_of(&resources)
}

/// The url Amazon SQS reports for the queue `name`, asserting the ARN it carries is the one
/// every statement and every refusal names.
async fn resolved_queue(router: &MultiServiceRouter, name: &str) -> String {
    let (status, answered) = sqs(router, "GetQueueUrl", &json!({"QueueName": name})).await;
    assert_eq!(
        status, 200,
        "setup: GetQueueUrl({name:?}) must succeed, got {status} with {answered}"
    );
    let url = answered["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: GetQueueUrl({name:?}) must answer a url, got {answered}"))
        .to_string();
    let (status, attributes) = sqs(
        router,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: GetQueueAttributes({name:?}) must succeed, got {status} with {attributes}"
    );
    assert_eq!(
        attributes["Attributes"]["QueueArn"].as_str(),
        Some(queue_arn(name).as_str()),
        "setup: the queue's ARN is what a statement and a refusal name, got {attributes}"
    );
    url
}

/// Wait until Lambda reports `name` `Active`, which is the state an invocation needs.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) {
    let mut last = Value::Null;
    for _ in 0..LOOKS {
        let (_, _, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            &[],
        )
        .await;
        if configuration["State"].as_str() == Some("Active") {
            return;
        }
        last = configuration;
        tokio::time::sleep(LOOK).await;
    }
    panic!("setup: Lambda must report {name:?} Active, last reported {last}");
}

/// Send one message as the ACCOUNT's own credential, which no probe's call can be confused
/// with.
async fn plant_message(router: &MultiServiceRouter, case: &str, url: &str, body: &str) {
    let (status, sent) = sqs(
        router,
        "SendMessage",
        &json!({"QueueUrl": url, "MessageBody": body}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: planting {body:?} must succeed, got {status} with {sent}"
    );
}

/// Take everything the queue still hands over, as the account's own credential, deleting each
/// message so the next probe starts from an empty queue — and answer the bodies in the order
/// they came over.
///
/// `wanted` is how many the caller expects: the loop looks again until it has them or the
/// generous deadline passes, so a host that is slow to make a message available cannot decide
/// the outcome, and a caller expecting none takes one look and reports what it found.
async fn drained(router: &MultiServiceRouter, case: &str, url: &str, wanted: usize) -> Vec<String> {
    let mut bodies = Vec::new();
    for look in 0..LOOKS {
        let (status, answered) = sqs(
            router,
            "ReceiveMessage",
            &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: draining the queue must succeed, got {status} with {answered}"
        );
        let handed = answered["Messages"].as_array().cloned().unwrap_or_default();
        for message in &handed {
            bodies.push(
                message["Body"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a message with no body {message}"))
                    .to_string(),
            );
            let (status, deleted) = sqs(
                router,
                "DeleteMessage",
                &json!({
                    "QueueUrl": url,
                    "ReceiptHandle": message["ReceiptHandle"].as_str().unwrap_or_default(),
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: deleting a drained message must succeed, got {status} with \
                 {deleted}"
            );
        }
        if bodies.len() >= wanted || look + 1 == LOOKS {
            break;
        }
        tokio::time::sleep(LOOK).await;
    }
    bodies
}

/// The `X-Amz-Function-Error` an invocation carries, which is set exactly when the run failed.
fn function_error(headers: &http::HeaderMap) -> Option<String> {
    headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// Every probe: the queue operation a deployed relay function may perform is the one the
/// policies in force on its execution role allow, and what the queue holds afterwards follows
/// that decision.
#[tokio::test]
async fn an_in_function_queue_call_is_decided_against_its_execution_role() {
    let router = build_router();
    let physical = deploy(&router).await;

    let url = resolved_queue(&router, QUEUE).await;
    let other = resolved_queue(&router, OTHER_QUEUE).await;
    assert_ne!(
        url, other,
        "setup: the two declared queues must be two queues, got {url:?} for both"
    );
    for (logical, expected) in [("Relay", &url), ("Elsewhere", &other)] {
        let reported = physical.get(logical).cloned().unwrap_or_default();
        assert_eq!(
            &reported,
            expected,
            "setup: the stack must report {logical} as the queue the service holds, got \
             {reported:?}"
        );
    }

    for (index, row) in ROWS.iter().enumerate() {
        let logical = format!("Probe{index}");
        let function = physical.get(&logical).cloned().unwrap_or_else(|| {
            panic!("setup: the stack must report a physical id for {logical}")
        });
        assert_eq!(
            function,
            function_name(index),
            "setup: the stack must name the function {:?}, got {function:?}",
            function_name(index)
        );
        wait_until_active(&router, &function).await;

        for (probe_index, probe) in row.probes.iter().enumerate() {
            let case = probe.case;
            let body = format!("relay-{index}-{probe_index}");
            if probe.op.needs_a_message() {
                plant_message(&router, case, &url, &body).await;
            }

            let event = json!({
                "op": probe.op.event(),
                "url": url,
                "name": QUEUE,
                "body": body,
                "catch": probe.catches,
            });
            let (status, headers, payload) = lambda(
                &router,
                "POST",
                &format!("/2015-03-31/functions/{function}/invocations"),
                event.to_string().as_bytes(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the invocation must be served, got {status} with {payload}"
            );

            if probe.catches {
                assert_eq!(
                    function_error(&headers),
                    None,
                    "case {case:?}: a handler that catches the refusal runs to completion, got \
                     {:?} with {payload}",
                    function_error(&headers)
                );
                // What the handler reports is what Amazon SQS answered its call with.
                let expected = match (probe.allowed, probe.op) {
                    (true, Op::Send) => json!({"ok": true, "id": true}),
                    (true, Op::Receive) => json!({"ok": true, "bodies": [body.clone()]}),
                    (true, Op::Url) => json!({"ok": true, "url": url}),
                    (false, op) => json!({
                        "ok": false,
                        "code": ACCESS_DENIED,
                        "message": refusal_message(index, op.action()),
                        "status": ACCESS_DENIED_STATUS,
                    }),
                };
                assert_eq!(
                    payload, expected,
                    "case {case:?}: expected the handler to report {expected}, got {payload}"
                );
            } else if probe.allowed {
                assert_eq!(
                    function_error(&headers),
                    None,
                    "case {case:?}: a served call fails no run, got {:?} with {payload}",
                    function_error(&headers)
                );
                assert_eq!(
                    payload,
                    json!({"ok": true, "id": true}),
                    "case {case:?}: expected the handler's own report, got {payload}"
                );
            } else {
                // The refusal was left to propagate: the RUN failed, so the invocation
                // carries the function error and the run's own error document.
                assert_eq!(
                    function_error(&headers).as_deref(),
                    Some(UNHANDLED),
                    "case {case:?}: an uncaught refusal must surface as the function's own \
                     error, got {:?} with {payload}",
                    function_error(&headers)
                );
                let wanted = refusal_message(index, probe.op.action());
                let reported = payload["errorMessage"].as_str().unwrap_or_default();
                assert!(
                    reported.contains(&wanted),
                    "case {case:?}: the error document must carry what Amazon SQS refused the \
                     call with ({wanted:?}), got {payload}"
                );
                assert!(
                    payload["errorType"]
                        .as_str()
                        .is_some_and(|kind| !kind.is_empty()),
                    "case {case:?}: the error document must name the class that was raised, \
                     got {payload}"
                );
            }

            // And what the QUEUE holds: a served send is in it, a refused one enqueued
            // nothing, a served receive took the planted message out, and a refused receive
            // left it exactly where it was.
            let expected_bodies: Vec<String> = match (probe.allowed, probe.op) {
                (true, Op::Send) => vec![body.clone()],
                (false, Op::Receive) => vec![body.clone()],
                _ => Vec::new(),
            };
            let held = drained(&router, case, &url, expected_bodies.len()).await;
            assert_eq!(
                held, expected_bodies,
                "case {case:?}: expected the queue to hold {expected_bodies:?}, got {held:?}"
            );
            // Nothing any row did reached the OTHER queue: the statement that names it grants
            // the handler nothing on the queue it actually calls.
            let elsewhere = drained(&router, case, &other, 0).await;
            assert!(
                elsewhere.is_empty(),
                "case {case:?}: expected the other queue to stay empty, got {elsewhere:?}"
            );
        }
    }
}
