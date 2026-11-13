// SPDX-License-Identifier: CC0-1.0

//! Which PARTITIONS of a table a running function may address: the condition a statement in
//! its execution-role document carries, decided against the call the function itself makes.
//!
//! RULE: a conditioned statement is decided against the condition keys the request carries
//! values for, and a DynamoDB item operation carries the partition-key value(s) it addresses
//! as `dynamodb:LeadingKeys` — the key that "limits the user's access to only the items whose
//! partition key value matches" it (the DynamoDB condition-key list) — so a grant scoped by
//! such a clause serves the function's call for the partitions its condition selects and
//! refuses it for every other with `AccessDeniedException` (HTTP 400, the status the service
//! framework behind DynamoDB answers an authorization refusal with), naming the assumed-role
//! session the function's calls are made as, the IAM action the operation performs and the
//! table's ARN. A refusal the handler does not catch is the FUNCTION's own failure: the
//! invocation is answered HTTP 200 carrying `X-Amz-Function-Error: Unhandled` and the error
//! document of the run, not an error of the `Invoke` call.
//!
//! Four crates have to agree for that to hold — IAM holds the role and its conditioned
//! document, STS mints the session the function's calls are signed with, Lambda runs the
//! handler and supplies it the SDK, and DynamoDB takes the decision — so the suite lives
//! here. What the neighbours pin and this one does not repeat:
//! `lambda_in_function_dynamodb_authorization` drives the table of UNCONDITIONED grants on
//! this same seam, and the DynamoDB crate's own suites drive a conditioned document for a
//! credential a client assumed directly; what only this suite reaches is a condition decided
//! against the call a FUNCTION made, which is what multi-tenant isolation written on
//! `dynamodb:LeadingKeys` rests on.
//!
//! The rows differ along what the rule generalises over: the set qualifier of the clause
//! (`ForAllValues:StringEquals`, `ForAnyValue:StringEquals`, and no condition at all — the
//! qualifiers the IAM policy reference publishes for a multi-valued context key), how many
//! partitions it lists, WHICH partition the call addresses (one the clause selects, one it
//! does not, a second one it selects), which item operation addresses it (a read and a
//! write), and whether the handler catches the refusal. Each probe asserts both halves — what
//! the handler reports of the call, and what the TABLE holds afterwards — so a refusal that
//! still wrote, and a grant that answered without writing, both fail here.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{elements, form_encode, opt_element, physical_ids_of};

/// The Region every request here is signed for, and so the Region the table and the
/// functions belong to.
const REGION: &str = "us-east-1";

/// The account this emulator serves, which the table's ARN and every session's ARN carry.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The table every row acts on: one item per tenant partition.
const TABLE: &str = "xsvc-in-function-partitions";

/// The partitions the table holds, one per tenant. A row's clause names some of these and a
/// probe addresses one of them.
const PARTITIONS: [&str; 3] = ["tenant-a", "tenant-b", "tenant-c"];

/// The `StackStatus` the functions' stack must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The stack the functions are declared in.
const STACK: &str = "xsvc-in-fn-ddb-conditions";

/// The code and status DynamoDB refuses an unauthorized item operation with.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u64 = 400;

/// The signal an invocation whose handler raised carries, and the status such an invocation
/// is still answered on: the run failed, the call did not.
const UNHANDLED: &str = "Unhandled";

/// The body planted under every partition before a probe, so a refusal differs from an
/// absent item and a write that was refused leaves this text in place.
const PLANTED: &str = "the body planted before the invocation";

/// How long one invocation is given, as the function's own `Timeout`: well past what a run
/// needs on any host, because what is under test is the authorization decision and not how
/// fast an interpreter gets through the handler.
const FUNCTION_TIMEOUT: u64 = 120;

/// The handler every row runs. It performs the one item operation the event names with the
/// SDK the runtime supplies, and either reports the refusal DynamoDB answered — its code, its
/// sentence and the HTTP status it came back on — or leaves it to propagate, which is what
/// makes the invocation report a function error.
const PROBE_HANDLER: &str = r##"
import boto3
from botocore.exceptions import ClientError


def call(client, event):
    table = event["table"]
    key = {"pk": {"S": event["pk"]}}
    if event["op"] == "put":
        client.put_item(
            TableName=table,
            Item={"pk": {"S": event["pk"]}, "body": {"S": event["body"]}},
        )
        return {"ok": True}
    read = client.get_item(TableName=table, Key=key, ConsistentRead=True)
    return {"ok": True, "body": read["Item"]["body"]["S"]}


def handler(event, context):
    client = boto3.client("dynamodb")
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

/// The ARN of the table every statement names and every refusal reports.
fn table_arn() -> String {
    format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{TABLE}")
}

/// WHICH item operation one probe's handler calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Put,
    Get,
}

impl Op {
    /// The word the event names it by, which the handler reads.
    fn event(self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::Get => "get",
        }
    }

    /// The IAM action the operation performs, which is the action a refusal names: DynamoDB's
    /// action list carries one `dynamodb:<OperationName>` per item operation.
    fn action(self) -> &'static str {
        match self {
            Self::Put => "dynamodb:PutItem",
            Self::Get => "dynamodb:GetItem",
        }
    }
}

/// One probe of a row's role: the operation the handler calls, the partition it addresses,
/// whether the row's clause selects that partition, and whether the handler catches a
/// refusal.
struct Probe {
    case: &'static str,
    op: Op,
    partition: &'static str,
    allowed: bool,
    catches: bool,
}

/// One row: the clause the execution role's grant carries, and the probes taken against it.
struct Row {
    /// The set qualifier and operator its `Condition` states, or `None` for a grant stating
    /// no condition at all.
    operator: Option<&'static str>,
    /// The partitions its `dynamodb:LeadingKeys` clause lists.
    leading: &'static [&'static str],
    probes: &'static [Probe],
}

const ROWS: &[Row] = &[
    // A grant scoped to ONE partition: the tenant the role belongs to.
    Row {
        operator: Some("ForAllValues:StringEquals"),
        leading: &["tenant-a"],
        probes: &[
            Probe {
                case: "the read of its own partition is served",
                op: Op::Get,
                partition: "tenant-a",
                allowed: true,
                catches: true,
            },
            Probe {
                case: "the read of ANOTHER tenant's partition is refused",
                op: Op::Get,
                partition: "tenant-b",
                allowed: false,
                catches: true,
            },
            Probe {
                case: "the write of its own partition is served",
                op: Op::Put,
                partition: "tenant-a",
                allowed: true,
                catches: true,
            },
            Probe {
                case: "the write of another tenant's partition is refused",
                op: Op::Put,
                partition: "tenant-b",
                allowed: false,
                catches: true,
            },
            Probe {
                case: "a refusal the handler does not catch is the function's own error",
                op: Op::Put,
                partition: "tenant-b",
                allowed: false,
                catches: false,
            },
        ],
    },
    // The MIRROR of it, so no partition is privileged by anything but the clause naming it.
    Row {
        operator: Some("ForAllValues:StringEquals"),
        leading: &["tenant-b"],
        probes: &[
            Probe {
                case: "the mirrored role reads the partition its own clause names",
                op: Op::Get,
                partition: "tenant-b",
                allowed: true,
                catches: true,
            },
            Probe {
                case: "and is refused the partition the other role was scoped to",
                op: Op::Get,
                partition: "tenant-a",
                allowed: false,
                catches: true,
            },
        ],
    },
    // A clause listing TWO partitions under the other set qualifier.
    Row {
        operator: Some("ForAnyValue:StringEquals"),
        leading: &["tenant-a", "tenant-c"],
        probes: &[
            Probe {
                case: "the first of the two partitions the clause lists is served",
                op: Op::Get,
                partition: "tenant-a",
                allowed: true,
                catches: true,
            },
            Probe {
                case: "the second of them is served too",
                op: Op::Get,
                partition: "tenant-c",
                allowed: true,
                catches: true,
            },
            Probe {
                case: "a partition the clause lists neither of is refused",
                op: Op::Get,
                partition: "tenant-b",
                allowed: false,
                catches: true,
            },
        ],
    },
    // The control: the SAME grant with no condition at all reaches every partition, so what
    // scoped the rows above is the clause and not the action or the resource.
    Row {
        operator: None,
        leading: &[],
        probes: &[
            Probe {
                case: "an unconditioned grant reads one partition",
                op: Op::Get,
                partition: "tenant-a",
                allowed: true,
                catches: true,
            },
            Probe {
                case: "an unconditioned grant reads another",
                op: Op::Get,
                partition: "tenant-b",
                allowed: true,
                catches: true,
            },
            Probe {
                case: "an unconditioned grant writes another",
                op: Op::Put,
                partition: "tenant-c",
                allowed: true,
                catches: true,
            },
        ],
    },
];

/// The role one row runs as, and the function it runs in — named here because the refusal's
/// own sentence names both: the identity a function's call is signed as is the session
/// `arn:aws:sts::<account>:assumed-role/<role>/<function>`.
fn role_name(index: usize) -> String {
    format!("xsvc-in-fn-ddb-cond-role-{index}")
}
fn function_name(index: usize) -> String {
    format!("xsvc-in-fn-ddb-cond-probe-{index}")
}

/// The sentence DynamoDB refuses an item operation nothing in force allows with: a statement
/// whose condition this call does not satisfy allows nothing, so the refusal is the implicit
/// deny's. Written out here rather than built from the service's own helper, so the wording
/// has something to be wrong about.
fn refusal_message(index: usize, action: &str) -> String {
    format!(
        "User: arn:aws:sts::{ACCOUNT}:assumed-role/{}/{} is not authorized to perform: \
         {action} on resource: {} because no identity-based policy allows the {action} action",
        role_name(index),
        function_name(index),
        table_arn(),
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
/// and the row's own grant on the table — conditioned where the row states a clause.
fn role_document(row: &Row) -> String {
    let mut grant = json!({
        "Effect": "Allow",
        "Action": ["dynamodb:GetItem", "dynamodb:PutItem"],
        "Resource": table_arn(),
    });
    if let Some(operator) = row.operator {
        grant["Condition"] = json!({operator: {"dynamodb:LeadingKeys": row.leading}});
    }
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": ["logs:CreateLogGroup", "logs:CreateLogStream", "logs:PutLogEvents"],
                "Resource": "*",
            },
            grant,
        ],
    })
    .to_string()
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
            ("PolicyName", "partition-grants"),
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
        Some(table_arn().as_str()),
        "setup: the table's ARN is what a statement and a refusal name, got {described}"
    );
}

/// How often to look while waiting for work the clock owes — a created table reaching
/// `ACTIVE`, a created function reaching `Active` — and how many looks to take. The spans are
/// those services' own, so no fixed sleep names the instant one ends, and the deadline is
/// generous so a loaded host cannot decide the outcome.
const LOOK: std::time::Duration = std::time::Duration::from_millis(100);
const LOOKS: usize = 1_800;

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
    role_arns: &[String],
) -> BTreeMap<String, String> {
    let template = function_stack(role_arns);
    let (status, answered) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "CreateStack",
        &[("StackName", STACK), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateStack({STACK:?}) must be accepted, got {status} with {answered}"
    );
    // The stack's completion is owed from the instant Lambda reports its functions created,
    // which no fixed sleep can name: this looks until it reports it, on the shared wait with
    // a deadline generous enough that a loaded host does not decide the outcome.
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

/// Write the planted body under every partition as the ACCOUNT's own credential, which no
/// probe's call can be confused with.
async fn plant_items(router: &MultiServiceRouter, case: &str) {
    for partition in PARTITIONS {
        let (status, put) = dynamodb(
            router,
            "PutItem",
            &json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": partition}, "body": {"S": PLANTED}},
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: planting the item under {partition:?} must succeed, got {status} \
             with {put}"
        );
    }
}

/// The item the table holds under `partition`, read back through DynamoDB's own `GetItem` as
/// the account's own credential.
async fn held_item(router: &MultiServiceRouter, case: &str, partition: &str) -> Option<Value> {
    let (status, answered) = dynamodb(
        router,
        "GetItem",
        &json!({
            "TableName": TABLE,
            "Key": {"pk": {"S": partition}},
            "ConsistentRead": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetItem({partition:?}) must succeed, got {status} with {answered}"
    );
    answered.get("Item").cloned()
}

/// The `X-Amz-Function-Error` an invocation carries, which is set exactly when the run
/// failed.
fn function_error(headers: &http::HeaderMap) -> Option<String> {
    headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// Every probe: the partition a function's own item operation may address is the one its
/// execution role's clause selects, and what the table holds afterwards follows that
/// decision.
#[tokio::test]
async fn a_conditioned_execution_role_scopes_the_partitions_its_function_may_address() {
    let router = build_router();
    create_table(&router).await;

    let mut role_arns = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        role_arns.push(create_role(&router, index, row).await);
    }
    let physical = deploy_functions(&router, &role_arns).await;

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
            // Every probe starts from the same table: the planted body under every
            // partition, whatever an earlier probe wrote.
            plant_items(&router, case).await;
            let body = format!("written-by-{index}-{probe_index}");

            let event = json!({
                "op": probe.op.event(),
                "table": TABLE,
                "pk": probe.partition,
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
                    "case {case:?}: a handler that catches the refusal runs to completion, \
                     got {:?} with {payload}",
                    function_error(&headers)
                );
                // What the handler reports is what DynamoDB answered its call with.
                let expected = match (probe.allowed, probe.op) {
                    (true, Op::Put) => json!({"ok": true}),
                    (true, Op::Get) => json!({"ok": true, "body": PLANTED}),
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
                let reported = payload["errorMessage"].as_str().unwrap_or_default();
                assert!(
                    reported.contains(&refusal_message(index, probe.op.action())),
                    "case {case:?}: the error document must carry what DynamoDB refused the \
                     call with ({:?}), got {payload}",
                    refusal_message(index, probe.op.action())
                );
                assert!(
                    payload["errorType"].as_str().is_some_and(|kind| !kind.is_empty()),
                    "case {case:?}: the error document must name the class that was raised, \
                     got {payload}"
                );
            }

            // And what the TABLE holds: the effect of a served write is there, and a refused
            // call left the partition exactly as it was.
            let expected_item = if probe.allowed && probe.op == Op::Put {
                json!({"pk": {"S": probe.partition}, "body": {"S": body}})
            } else {
                json!({"pk": {"S": probe.partition}, "body": {"S": PLANTED}})
            };
            let held = held_item(&router, case, probe.partition).await;
            assert_eq!(
                held,
                Some(expected_item.clone()),
                "case {case:?}: expected the table to hold {expected_item} under {:?}, got \
                 {held:?}",
                probe.partition
            );
        }
    }
}
