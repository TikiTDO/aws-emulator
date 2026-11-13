// SPDX-License-Identifier: CC0-1.0

//! What a WILDCARD grant on a function's execution role lets a running handler's own Amazon
//! S3 calls do: Lambda, Amazon S3, IAM and the token service on ONE engine, a `python3.x`
//! function whose code does `import boto3` and writes and reads objects, and every bucket and
//! policy written with those services' own operations.
//!
//! The rule pinned here: a handler's SDK call is authorized by matching the request's action
//! and resource against the elements of the policies in force on its execution role, with
//! IAM's two wildcards admitted in each element — so a statement whose `Action` and `Resource`
//! cover the call grants it on ANY bucket of the account, including a bucket created
//! independently that no policy names, while a wildcard that does NOT cover the call (an
//! action prefix that excludes it, an ARN pattern that excludes its bucket) leaves it refused
//! and an explicit `Deny` still overrides the grant. Nothing about the grant is keyed to a
//! particular bucket ARN: the breadth of the wildcard is what decides, not which name it was
//! written with.
//!
//! The rows differ along the FORM and BREADTH of the grant the role carries — the bare
//! resource wildcard with a service-wide action, the wildcard over every action of every
//! service, the resource wildcard spelled as an ARN pattern, the two actions named one by one,
//! an action prefix that covers only the write, a service-wide action scoped to one bucket's
//! objects, and a wildcard grant an equally wide deny overrides. Every row makes the same
//! three calls on two buckets of its own, neither of which appears in any policy except where
//! the row is about a scoped resource: a write, the read back of that write, and the read of a
//! second bucket written out of band. Every row's write address is seeded out of band with
//! other bytes first, and read back through S3's own API after the invocation, so a served
//! write is the bytes the bucket then holds and a refused one stored nothing.

use serde_json::{json, Value};

use crate::handler_s3_object_access::{
    session_arn, Emulator, ACCESS_DENIED, ACCESS_DENIED_STATUS,
};
use crate::invoke_request_id::one_invocation_stream_at_a_time;

/// The buckets this table uses, two per row: a row's calls address buckets of its own, so the
/// only policy in force on them is the identity policy the row writes on the execution role.
const BUCKET_PREFIX: &str = "handler-wildcard-grant";

/// The object the handler writes, and the bytes it writes there.
const WRITE_KEY: &str = "written/a.txt";
const WRITE_BODY: &str = "wildcard,write\n1,10\n";

/// The bytes that address holds BEFORE any invocation, written out of band: distinct from
/// what the handler writes, so a served write is told apart from a refused one by the bytes
/// alone, and a read of the address is a read of an object that exists either way.
const PRIOR_BODY: &str = "prior,write\n0,0\n";

/// The object written out of band in the second bucket, and the bytes it holds.
const READ_KEY: &str = "held/b.txt";
const READ_BODY: &str = "wildcard,read\n2,20\n";

/// The reason clause IAM words a refusal with when no identity-based policy allows the
/// action, and the one it words a refusal by an explicit deny in an identity policy with.
const NO_GRANT: &str = "because no identity-based policy allows the";
const IDENTITY_DENY: &str = "with an explicit deny in an identity-based policy";

/// The handler every row runs: it makes ONE call per entry the event names and reports what
/// each answered — `{"ok": true}` for a write, `{"ok": true, "body": …}` for a read, and the
/// code, sentence and status of the refusal otherwise — so the invocation's payload IS what
/// the SDK gave the guest.
const SOURCE: &str = r#"
import boto3
import botocore.exceptions


def lambda_handler(event, context):
    client = boto3.client("s3")
    results = []
    for call in event["calls"]:
        try:
            if call["action"] == "put":
                client.put_object(
                    Bucket=call["bucket"],
                    Key=call["key"],
                    Body=call["content"].encode("utf-8"),
                )
                results.append({"ok": True})
            else:
                answered = client.get_object(Bucket=call["bucket"], Key=call["key"])
                results.append({
                    "ok": True,
                    "body": answered["Body"].read().decode("utf-8"),
                })
        except botocore.exceptions.ClientError as error:
            results.append({
                "ok": False,
                "error": error.response["Error"]["Code"],
                "message": error.response["Error"]["Message"],
                "status": error.response["ResponseMetadata"]["HTTPStatusCode"],
            })
    return {"results": results}
"#;

/// What one call of a row must come to: served, or refused by the policy in force with the
/// reason clause IAM words that refusal with.
#[derive(Debug)]
enum Expect {
    Served,
    Refused(Reason),
}

/// Which refusal sentence a refused call must carry.
#[derive(Debug)]
enum Reason {
    /// Nothing in the identity policy allows the action.
    NoGrant,
    /// The identity policy allows it and also denies it.
    Deny,
}

/// One row: the statements the execution role's inline identity policy carries, and what each
/// of the row's three calls must come to under them.
struct Row {
    case: &'static str,
    statements: Vec<Value>,
    /// The write of the row's first bucket.
    put: Expect,
    /// The read back of what that write stored.
    read_written: Expect,
    /// The read of the row's second bucket, written out of band.
    read_held: Expect,
}

/// The first bucket of row `row`: the one the handler writes to and reads back.
fn write_bucket(row: usize) -> String {
    format!("{BUCKET_PREFIX}-{row}-write")
}

/// The second bucket of row `row`: the one written out of band and only read.
fn read_bucket(row: usize) -> String {
    format!("{BUCKET_PREFIX}-{row}-read")
}

/// One `Allow` statement over `action` and `resource`.
fn allow(action: Value, resource: &str) -> Value {
    json!({"Effect": "Allow", "Action": action, "Resource": resource})
}

/// The rows: the grant's form and breadth, varied one element at a time.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a service-wide action over the bare resource wildcard",
            statements: vec![allow(json!("s3:*"), "*")],
            put: Expect::Served,
            read_written: Expect::Served,
            read_held: Expect::Served,
        },
        Row {
            case: "the wildcard over every action of every service",
            statements: vec![allow(json!("*"), "*")],
            put: Expect::Served,
            read_written: Expect::Served,
            read_held: Expect::Served,
        },
        Row {
            case: "a service-wide action over every resource of this service, as an ARN pattern",
            statements: vec![allow(json!("s3:*"), "arn:aws:s3:::*")],
            put: Expect::Served,
            read_written: Expect::Served,
            read_held: Expect::Served,
        },
        Row {
            case: "both actions named one by one over the bare resource wildcard",
            statements: vec![allow(json!(["s3:PutObject", "s3:GetObject"]), "*")],
            put: Expect::Served,
            read_written: Expect::Served,
            read_held: Expect::Served,
        },
        Row {
            case: "an action prefix that covers the write alone, over every resource",
            statements: vec![allow(json!("s3:Put*"), "*")],
            put: Expect::Served,
            read_written: Expect::Refused(Reason::NoGrant),
            read_held: Expect::Refused(Reason::NoGrant),
        },
        Row {
            case: "a service-wide action scoped to one bucket's objects",
            statements: vec![allow(
                json!("s3:*"),
                &format!("arn:aws:s3:::{}/*", write_bucket(5)),
            )],
            put: Expect::Served,
            read_written: Expect::Served,
            read_held: Expect::Refused(Reason::NoGrant),
        },
        Row {
            case: "an equally wide deny beside the wildcard grant",
            statements: vec![
                allow(json!("s3:*"), "*"),
                json!({"Effect": "Deny", "Action": "s3:*", "Resource": "*"}),
            ],
            put: Expect::Refused(Reason::Deny),
            read_written: Expect::Refused(Reason::Deny),
            read_held: Expect::Refused(Reason::Deny),
        },
    ]
}

/// One call of a row: what the handler is asked to do, and what it must come to.
struct Call<'a> {
    label: &'static str,
    /// `"put"` or `"get"`, as the handler reads it.
    action: &'static str,
    bucket: String,
    key: &'static str,
    /// The bytes a write stores, or the bytes a served read must answer.
    content: &'static str,
    expect: &'a Expect,
}

impl Call<'_> {
    /// The IAM action name this call is judged under (Amazon S3 "Actions": one action per
    /// object operation).
    fn iam_action(&self) -> &'static str {
        if self.action == "put" {
            "s3:PutObject"
        } else {
            "s3:GetObject"
        }
    }

    /// This call as the event entry the handler reads.
    fn event_entry(&self) -> Value {
        json!({
            "action": self.action,
            "bucket": self.bucket,
            "key": self.key,
            "content": self.content,
        })
    }
}

/// The three calls of row `row`: the write of a bucket no policy names, the read back of it,
/// and the read of a second bucket no policy names.
fn calls(row: usize, expectations: &Row) -> Vec<Call<'_>> {
    vec![
        Call {
            label: "the write",
            action: "put",
            bucket: write_bucket(row),
            key: WRITE_KEY,
            content: WRITE_BODY,
            expect: &expectations.put,
        },
        Call {
            label: "the read back of the write",
            action: "get",
            bucket: write_bucket(row),
            key: WRITE_KEY,
            content: WRITE_BODY,
            expect: &expectations.read_written,
        },
        Call {
            label: "the read of a second bucket",
            action: "get",
            bucket: read_bucket(row),
            key: READ_KEY,
            content: READ_BODY,
            expect: &expectations.read_held,
        },
    ]
}

/// What one call must come to: the handler's own success, or S3's own refusal naming the
/// session the call was made as, the action, the object and the policy that refused it.
fn assert_call(case: &str, session: &str, call: &Call<'_>, answer: &Value) {
    let label = call.label;
    let bucket = &call.bucket;
    let key = call.key;
    match call.expect {
        Expect::Served => {
            assert_eq!(
                answer["ok"],
                json!(true),
                "case {case:?}: expected {label} of {bucket}/{key} to be served, got {answer}"
            );
            if call.action == "get" {
                assert_eq!(
                    answer["body"],
                    json!(call.content),
                    "case {case:?}: expected {label} to answer the bytes {bucket}/{key} \
                     holds, got {answer}"
                );
            }
        }
        Expect::Refused(reason) => {
            assert_eq!(
                answer["ok"],
                json!(false),
                "case {case:?}: expected {label} of {bucket}/{key} to be refused, got {answer}"
            );
            assert_eq!(
                answer["error"],
                json!(ACCESS_DENIED),
                "case {case:?}: expected S3's own refusal code for {label}, got {answer}"
            );
            assert_eq!(
                answer["status"],
                json!(ACCESS_DENIED_STATUS),
                "case {case:?}: expected S3's own 403 for {label}, got {answer}"
            );
            let action = call.iam_action();
            let clause = match reason {
                Reason::NoGrant => format!("{NO_GRANT} {action} action"),
                Reason::Deny => IDENTITY_DENY.to_string(),
            };
            let expected = format!(
                "User: {session} is not authorized to perform: {action} on resource: \
                 \"arn:aws:s3:::{bucket}/{key}\" {clause}"
            );
            assert_eq!(
                answer["message"],
                json!(expected),
                "case {case:?}: expected the refusal {expected:?} for {label}, got {answer}"
            );
        }
    }
}

/// One invocation per grant, each writing and reading buckets the grant does not name: what
/// the handler was served, and — for a served write — what the bucket itself then holds.
///
/// The role's inline policy is rewritten BETWEEN invocations rather than per emulator, so the
/// rows also pin that each call is judged against the document in force when it is made.
#[tokio::test]
async fn a_wildcard_grant_on_the_execution_role_serves_a_handlers_s3_calls_on_any_bucket() {
    let _stream = one_invocation_stream_at_a_time().await;
    let setup = "the function, its role and the buckets of the wildcard-grant table";
    let role_name = "lambda-wildcard-grant";
    let function_name = "s3-wildcard-grant";
    let emulator = Emulator::new();
    let role = emulator.execution_role(setup, role_name, None).await;
    emulator.function(setup, function_name, &role, SOURCE).await;
    let rows = rows();
    // Every bucket is created on its own, with its own operations and no policy of its own:
    // what a row's calls are judged by is the identity policy alone.
    for (index, row) in rows.iter().enumerate() {
        for bucket in [write_bucket(index), read_bucket(index)] {
            emulator
                .s3_ok(row.case, "PUT", &format!("/{bucket}"), &[])
                .await;
        }
        emulator
            .s3_ok(
                row.case,
                "PUT",
                &format!("/{}/{READ_KEY}", read_bucket(index)),
                READ_BODY.as_bytes(),
            )
            .await;
        emulator
            .s3_ok(
                row.case,
                "PUT",
                &format!("/{}/{WRITE_KEY}", write_bucket(index)),
                PRIOR_BODY.as_bytes(),
            )
            .await;
    }
    let session = session_arn(role_name, function_name);
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let document = json!({"Version": "2012-10-17", "Statement": row.statements}).to_string();
        emulator
            .role_policy(case, role_name, Some(&document))
            .await;
        let calls = calls(index, row);
        let event = json!({
            "calls": calls.iter().map(Call::event_entry).collect::<Vec<Value>>(),
        });
        let (signal, payload) = emulator.invoke(case, function_name, &event).await;
        assert_eq!(
            signal, None,
            "case {case:?}: the handler must run to completion, got the function error \
             {signal:?} with {payload}"
        );
        let answered = payload["results"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            answered.len(),
            calls.len(),
            "case {case:?}: expected one answer per call, got {payload}"
        );
        for (call, answer) in calls.iter().zip(&answered) {
            assert_call(case, &session, call, answer);
        }
        // What the BUCKET holds: a write the handler reported as served is the bytes a client
        // reads back through S3's own API, and a refused one left the address as it was.
        let served = matches!(row.put, Expect::Served);
        let expected = if served { WRITE_BODY } else { PRIOR_BODY };
        let held = emulator
            .s3_ok(
                case,
                "GET",
                &format!("/{}/{WRITE_KEY}", write_bucket(index)),
                &[],
            )
            .await;
        assert_eq!(
            held, expected,
            "case {case:?}: expected the bucket to hold {expected:?} after a write that was \
             served={served}, got {held:?}"
        );
    }
}
