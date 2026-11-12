// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING function's own DynamoDB call is authorized against: the policies in force
//! on the function's execution role.
//!
//! RULE: a call an AWS SDK makes from inside a running function is made as a session of that
//! function's execution role, so the service serving it decides it against the policies in
//! force on that role — an item operation no identity-based policy in force on the role
//! allows is refused `AccessDeniedException` (HTTP 400, the status the service framework
//! behind DynamoDB answers an authorization refusal with) naming the assumed-role session,
//! the IAM action the operation performs and the table's ARN, and the refused call changes
//! nothing in the table; the same call by a role whose documents allow the action is served
//! and its effect is in the table afterwards.
//!
//! "The execution role grants the function permission to use Amazon Web Services services"
//! (`FunctionConfiguration$Role`), and the identity a function's own call speaks for is the
//! session `arn:aws:sts::<account>:assumed-role/<role>/<function>`; with neither an
//! identity-based nor a resource-based allow a request is denied (IAM User Guide, *Policy
//! evaluation logic*).
//!
//! Four crates have to agree for that to hold — IAM holds the role and its inline document,
//! STS mints the session the function's calls are signed with, Lambda runs the handler and
//! supplies it the SDK, and DynamoDB takes the decision — so the suite lives here: a
//! crate-local suite can only ever see one side of it. The DynamoDB crate's own
//! `dynamodb_data_plane_authorization` suite pins the decision for a credential a client
//! assumed directly, and `cloudformation_sam_function_writes_its_table` pins the GRANTED
//! in-function write; what only this suite can reach is the decision taken on a call a
//! FUNCTION made against the role it runs as, which is the identity a least-privilege
//! execution role is written for.
//!
//! The rows differ along what the rule generalises over: what the role's own document allows
//! (nothing of this service, this operation's action, another operation's action, every
//! action of the service), which resource that statement names (this table, another table of
//! the account, every resource), and WHICH item operation the handler calls (a write, a read,
//! an update) — several per role, so one role's document is asked about more than one action.
//! Each probe asserts both halves — the handler's own report of what DynamoDB answered, and
//! what the TABLE holds afterwards — so a refusal that still wrote, and a grant that answered
//! without writing, both fail here.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::stack_status;
use crate::query_wire::{elements, form_encode, opt_element, physical_ids_of};

/// The Region every request here is signed for, and so the Region the table and the
/// functions belong to.
const REGION: &str = "us-east-1";

/// The account this emulator serves, which the table's ARN and every session's ARN carry.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The table every row acts on.
const TABLE: &str = "xsvc-in-function-items";

/// The NAME of another table of this account, which one row's statement names instead of the
/// table the handler calls.
const OTHER_TABLE: &str = "xsvc-in-function-elsewhere";

/// The `StackStatus` the functions' stack must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The code and status DynamoDB refuses an unauthorized item operation with.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 400;

/// The body planted under a probe's key before the invocation, for a probe whose operation
/// reads or updates an item: a refusal then differs from an absent item, and an update that
/// was refused leaves this text in place.
const PLANTED: &str = "the body planted before the invocation";

/// How long one invocation is given, as the function's own `Timeout`: well past what a run
/// needs on any host, because what is under test is the authorization decision and not how
/// fast an interpreter gets through the handler.
const FUNCTION_TIMEOUT: u64 = 120;

/// How often to look while waiting for work the clock owes — a created table reaching
/// `ACTIVE`, a stack reaching `CREATE_COMPLETE`, a created function reaching `Active` — and
/// how many looks to take. The spans are those services' own, so no fixed sleep names the
/// instant one ends, and the deadline is generous so a loaded host cannot decide the outcome.
const LOOK: Duration = Duration::from_millis(100);
const LOOKS: usize = 1_800;

/// The handler every row runs: it builds a DynamoDB client with the SDK the runtime supplies
/// and performs the one item operation the event names, reporting either that the call was
/// served or the refusal DynamoDB answered it with — its code, its sentence and the HTTP
/// status it came back on.
const PROBE_HANDLER: &str = r##"
import boto3
from botocore.exceptions import ClientError


def handler(event, context):
    client = boto3.client("dynamodb")
    table = event["table"]
    key = {"pk": {"S": event["pk"]}}
    try:
        if event["op"] == "put":
            client.put_item(
                TableName=table,
                Item={"pk": {"S": event["pk"]}, "body": {"S": event["body"]}},
            )
            return {"ok": True}
        if event["op"] == "get":
            read = client.get_item(TableName=table, Key=key, ConsistentRead=True)
            return {"ok": True, "body": read["Item"]["body"]["S"]}
        answered = client.update_item(
            TableName=table,
            Key=key,
            UpdateExpression="SET #b = :b",
            ExpressionAttributeNames={"#b": "body"},
            ExpressionAttributeValues={":b": {"S": event["body"]}},
            ReturnValues="ALL_NEW",
        )
        return {"ok": True, "body": answered["Attributes"]["body"]["S"]}
    except ClientError as refused:
        return {
            "ok": False,
            "code": refused.response["Error"]["Code"],
            "message": refused.response["Error"]["Message"],
            "status": refused.response["ResponseMetadata"]["HTTPStatusCode"],
        }
"##;

/// The emulator as it is ASSEMBLED, on one engine: a handler's own SDK call is dispatched
/// back into this same router, so the table it addresses is the table this suite reads.
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

/// One DynamoDB operation over its `awsJson1_0` wire, made as the account's own credential.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("dynamodb.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("authorization", authorization("dynamodb"))
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
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

/// The ARN of a table of this account in this Region.
fn table_arn(table: &str) -> String {
    format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{table}")
}

/// WHICH item operation one probe's handler calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Put,
    Get,
    Update,
}

impl Op {
    /// The word the event names it by, which the handler reads.
    fn event(self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::Get => "get",
            Self::Update => "update",
        }
    }

    /// The IAM action the operation performs, which is the action a refusal names: DynamoDB's
    /// action list carries one `dynamodb:<OperationName>` per item operation.
    fn action(self) -> &'static str {
        match self {
            Self::Put => "dynamodb:PutItem",
            Self::Get => "dynamodb:GetItem",
            Self::Update => "dynamodb:UpdateItem",
        }
    }

    /// Whether a probe's key has to hold an item before the invocation: a read reports one,
    /// and an update that is refused has to leave one in place.
    fn plants_an_item(self) -> bool {
        matches!(self, Self::Get | Self::Update)
    }
}

/// WHICH resource a row's statement names.
#[derive(Debug, Clone, Copy)]
enum Scope {
    /// The table the handler calls.
    Table,
    /// Another table of the same account.
    OtherTable,
    /// Every resource, which the bare `*` wildcard stands for.
    Every,
}

impl Scope {
    fn resource(self) -> String {
        match self {
            Self::Table => table_arn(TABLE),
            Self::OtherTable => table_arn(OTHER_TABLE),
            Self::Every => "*".to_string(),
        }
    }
}

/// One probe of a row's role: the operation the handler calls, and whether the policies in
/// force let it through.
struct Probe {
    case: &'static str,
    op: Op,
    allowed: bool,
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
    // a function needs is granted no item operation at all.
    Row {
        actions: &[],
        scope: Scope::Table,
        probes: &[
            Probe {
                case: "a role granting only the log actions writes nothing",
                op: Op::Put,
                allowed: false,
            },
            Probe {
                case: "a role granting only the log actions reads nothing",
                op: Op::Get,
                allowed: false,
            },
        ],
    },
    Row {
        actions: &["dynamodb:PutItem"],
        scope: Scope::Table,
        probes: &[
            Probe {
                case: "a role granting the write on the table writes it",
                op: Op::Put,
                allowed: true,
            },
            Probe {
                case: "the same role is granted no read by it",
                op: Op::Get,
                allowed: false,
            },
            Probe {
                case: "the same role is granted no update by it",
                op: Op::Update,
                allowed: false,
            },
        ],
    },
    Row {
        actions: &["dynamodb:PutItem"],
        scope: Scope::OtherTable,
        probes: &[Probe {
            case: "a role granting the write on ANOTHER table writes nothing",
            op: Op::Put,
            allowed: false,
        }],
    },
    Row {
        actions: &["dynamodb:*"],
        scope: Scope::Table,
        probes: &[
            Probe {
                case: "a role granting every action of the service on the table writes it",
                op: Op::Put,
                allowed: true,
            },
            Probe {
                case: "and is granted the read by the same statement",
                op: Op::Get,
                allowed: true,
            },
        ],
    },
    Row {
        actions: &["dynamodb:GetItem"],
        scope: Scope::Table,
        probes: &[
            Probe {
                case: "a role granting the read on the table reads the item",
                op: Op::Get,
                allowed: true,
            },
            Probe {
                case: "the same role is granted no write by it",
                op: Op::Put,
                allowed: false,
            },
        ],
    },
    Row {
        actions: &["dynamodb:UpdateItem"],
        scope: Scope::Every,
        probes: &[
            Probe {
                case: "a role granting the update on every resource updates the item",
                op: Op::Update,
                allowed: true,
            },
            Probe {
                case: "the same role is granted no read by it",
                op: Op::Get,
                allowed: false,
            },
        ],
    },
];

/// The role one row runs as, and the function it runs in — named here because the refusal's
/// own sentence names both: the identity a function's call is signed as is the session
/// `arn:aws:sts::<account>:assumed-role/<role>/<function>`.
fn role_name(index: usize) -> String {
    format!("xsvc-in-fn-ddb-role-{index}")
}
fn function_name(index: usize) -> String {
    format!("xsvc-in-fn-ddb-probe-{index}")
}

/// The sentence DynamoDB refuses an item operation nothing in force allows with. Written out
/// here rather than built from the service's own helper, so the wording has something to be
/// wrong about.
fn refusal_message(index: usize, action: &str) -> String {
    format!(
        "User: arn:aws:sts::{ACCOUNT}:assumed-role/{}/{} is not authorized to perform: \
         {action} on resource: {} because no identity-based policy allows the {action} action",
        role_name(index),
        function_name(index),
        table_arn(TABLE),
    )
}

/// The trust policy a Lambda execution role needs.
fn trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "lambda.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// The inline document one row's role carries: the log actions every execution role needs,
/// and the row's own statement where it has one.
fn role_document(row: &Row) -> String {
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
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// Create the role one row runs as, with the document [`role_document`] states, and answer
/// the ARN IAM minted for it.
async fn create_role(router: &MultiServiceRouter, index: usize, row: &Row) -> String {
    let name = role_name(index);
    let (status, created) = query(
        router,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", &name),
            ("AssumeRolePolicyDocument", &trust_policy()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateRole({name:?}) must succeed, got {status} with {created}"
    );
    let arn = opt_element(&created, "Arn").unwrap_or_else(|| {
        panic!("setup: CreateRole({name:?}) must answer the role's ARN, got {created}")
    });
    let (status, embedded) = query(
        router,
        "iam",
        "2010-05-08",
        "PutRolePolicy",
        &[
            ("RoleName", &name),
            ("PolicyName", "probe-grants"),
            ("PolicyDocument", &role_document(row)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: PutRolePolicy({name:?}) must succeed, got {status} with {embedded}"
    );
    arn
}

/// Create the table every row acts on and wait until DynamoDB reports it `ACTIVE` — "you can
/// perform read and write operations only on an ACTIVE table" — asserting the ARN it reports
/// is the one the statements and the refusals name.
async fn create_table(router: &MultiServiceRouter) {
    let (status, created) = dynamodb(
        router,
        "CreateTable",
        &json!({
            "TableName": TABLE,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateTable({TABLE:?}) must succeed, got {status} with {created}"
    );
    let mut described = Value::Null;
    for _ in 0..LOOKS {
        let (_, answered) = dynamodb(router, "DescribeTable", &json!({"TableName": TABLE})).await;
        described = answered;
        if described["Table"]["TableStatus"].as_str() == Some("ACTIVE") {
            break;
        }
        tokio::time::sleep(LOOK).await;
    }
    assert_eq!(
        described["Table"]["TableStatus"].as_str(),
        Some("ACTIVE"),
        "setup: DynamoDB must report {TABLE:?} ACTIVE, got {described}"
    );
    assert_eq!(
        described["Table"]["TableArn"].as_str(),
        Some(table_arn(TABLE).as_str()),
        "setup: the table's ARN is what a statement and a refusal name, got {described}"
    );
}

/// The stack declaring one function per row, each running as that row's role: the inline
/// source is archived into the deployment package by CloudFormation, which is how a function
/// whose handler is written here comes to exist at all.
fn function_stack(role_arns: &[String]) -> String {
    let mut resources = serde_json::Map::new();
    for (index, role_arn) in role_arns.iter().enumerate() {
        resources.insert(
            format!("Probe{index}"),
            json!({
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": function_name(index),
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": role_arn,
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

/// Deploy the functions' stack, wait for `CREATE_COMPLETE` and answer the physical id of each
/// declared function.
async fn deploy_functions(
    router: &MultiServiceRouter,
    stack: &str,
    role_arns: &[String],
) -> BTreeMap<String, String> {
    let template = function_stack(role_arns);
    let (status, answered) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateStack({stack:?}) must be accepted, got {status} with {answered}"
    );
    // The stack's completion is owed from the instant Lambda reports its functions created,
    // which no fixed sleep can name: this looks until it reports it, with a deadline generous
    // enough that a loaded host does not decide the outcome.
    let mut reported = stack_status(router, stack).await;
    for _ in 0..LOOKS {
        if reported.0 == CREATE_COMPLETE {
            break;
        }
        tokio::time::sleep(LOOK).await;
        reported = stack_status(router, stack).await;
    }
    let failures = if reported.0 == CREATE_COMPLETE {
        Vec::new()
    } else {
        let (_, events) = query(
            router,
            "cloudformation",
            "2010-05-15",
            "DescribeStackEvents",
            &[("StackName", stack)],
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
        &[("StackName", stack)],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: DescribeStackResources({stack:?}) must succeed, got {status} with {resources}"
    );
    physical_ids_of(&resources)
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

/// Write one item under `pk` as the ACCOUNT's own credential, which a probe's refusal cannot
/// be confused with.
async fn plant_item(router: &MultiServiceRouter, case: &str, pk: &str) {
    let (status, put) = dynamodb(
        router,
        "PutItem",
        &json!({
            "TableName": TABLE,
            "Item": {"pk": {"S": pk}, "body": {"S": PLANTED}},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: planting the item under {pk:?} must succeed, got {status} with {put}"
    );
}

/// The item the table holds under `pk`, read back through DynamoDB's own `GetItem` as the
/// account's own credential.
async fn held_item(router: &MultiServiceRouter, case: &str, pk: &str) -> Option<Value> {
    let (status, answered) = dynamodb(
        router,
        "GetItem",
        &json!({
            "TableName": TABLE,
            "Key": {"pk": {"S": pk}},
            "ConsistentRead": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetItem({pk:?}) must succeed, got {status} with {answered}"
    );
    answered.get("Item").cloned()
}

/// Every probe: the function's own item operation is decided against the policies in force on
/// its execution role, and what the table holds afterwards follows that decision.
#[tokio::test]
async fn an_in_function_item_operation_is_decided_against_its_execution_role() {
    let router = build_router();
    create_table(&router).await;

    let mut role_arns = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        role_arns.push(create_role(&router, index, row).await);
    }
    let physical = deploy_functions(&router, "xsvc-in-fn-ddb-probes", &role_arns).await;

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
            let pk = format!("row-{index}-probe-{probe_index}");
            let body = format!("written-by-{index}-{probe_index}");
            if probe.op.plants_an_item() {
                plant_item(&router, case, &pk).await;
            }

            let event = json!({
                "op": probe.op.event(),
                "table": TABLE,
                "pk": pk,
                "body": body,
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
            let faulted = headers
                .get("x-amz-function-error")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            assert_eq!(
                faulted, None,
                "case {case:?}: the handler must run to completion, got {faulted:?} with \
                 {payload}"
            );

            // What the handler reports is what DynamoDB answered its call with.
            let expected = if probe.allowed {
                match probe.op {
                    Op::Put => json!({"ok": true}),
                    Op::Get => json!({"ok": true, "body": PLANTED}),
                    Op::Update => json!({"ok": true, "body": body}),
                }
            } else {
                json!({
                    "ok": false,
                    "code": ACCESS_DENIED,
                    "message": refusal_message(index, probe.op.action()),
                    "status": ACCESS_DENIED_STATUS,
                })
            };
            assert_eq!(
                payload, expected,
                "case {case:?}: expected the handler to report {expected}, got {payload}"
            );

            // And what the TABLE holds: the effect of a served call is there, and a refused
            // one left the table exactly as it was.
            let expected_item = match (probe.allowed, probe.op) {
                (true, Op::Put | Op::Update) => {
                    Some(json!({"pk": {"S": pk}, "body": {"S": body}}))
                }
                (true, Op::Get) => Some(json!({"pk": {"S": pk}, "body": {"S": PLANTED}})),
                (false, op) if op.plants_an_item() => {
                    Some(json!({"pk": {"S": pk}, "body": {"S": PLANTED}}))
                }
                (false, _) => None,
            };
            let held = held_item(&router, case, &pk).await;
            assert_eq!(
                held, expected_item,
                "case {case:?}: expected the table to hold {expected_item:?} under {pk:?}, got \
                 {held:?}"
            );
        }
    }
}
