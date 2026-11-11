// SPDX-License-Identifier: CC0-1.0

//! WHICH of a handler's own Amazon S3 reads a bucket policy's explicit `Deny` refuses, end to
//! end: Lambda, Amazon S3, IAM and the token service on ONE engine, a `python3.x` function
//! whose code does `import boto3` and reads several keys of one bucket, and every policy
//! written with those services' own operations.
//!
//! The rule pinned here: a resource-based policy's `Deny` statement refuses exactly the
//! requests its own `Principal`, `Action` and `Resource` elements select — "an explicit deny
//! in any policy overrides any allow", so each of those requests is refused `AccessDenied`
//! (HTTP 403) with the sentence IAM publishes for a deny in a resource-based policy, whatever
//! an identity-based policy granted — while every OTHER request the identity policy allows
//! against that same bucket keeps being served.
//!
//! One identity policy grants `s3:GetObject` over two prefixes of every bucket below, and
//! every bucket holds the same three keys, so what a row varies is the DENY alone: the span of
//! keys its `Resource` names (one prefix, one key, every object), the action it names, the
//! principal it names, and its effect. Every row is read twice — once before its policy is
//! written, where the identity grant alone must serve all three keys, and once after — so a
//! row that refuses nothing is distinguished from a baseline that never worked, and the bytes
//! of each served read are asserted to be the ones that key holds.

use serde_json::{json, Value};

use crate::handler_s3_object_access::{
    document, role_arn, session_arn, Emulator, ACCESS_DENIED, ACCESS_DENIED_STATUS, READER_SOURCE,
};
use crate::invoke_request_id::one_invocation_stream_at_a_time;

/// The buckets this table reads, one per row: a row's own bucket policy is then the only
/// resource policy in force on its reads.
const BUCKET_PREFIX: &str = "handler-deny-scope";

/// The two prefixes of those buckets the execution role's own policy grants `s3:GetObject`
/// over — the shape of grant a function that reads two folders of one bucket carries.
const GRANTED_PREFIXES: [&str; 2] = ["allowed/", "denied/"];

/// The keys every one of those buckets holds and the bytes each holds, one under the first
/// granted prefix and two under the second: a deny scoped to a prefix covers both of the
/// latter, and one scoped to a key covers only its own.
const HELD: [(&str, &str); 3] = [
    ("allowed/report.csv", "allowed,report\n1,10\n"),
    ("denied/report.csv", "denied,report\n2,20\n"),
    ("denied/second.csv", "denied,second\n3,30\n"),
];

/// Another role of the same account, which a row's bucket policy names where the deny is
/// written about someone other than this function's role. A bucket policy may only name a
/// principal this account holds, so it is a role IAM holds too — which is what makes the row
/// about the principal the deny selects and not about an ARN that resolves to nothing.
const OTHER_ROLE: &str = "deny-scope-bystander";

/// The reason clause IAM words a refusal by an explicit deny in a resource-based policy with.
const RESOURCE_DENY: &str = "with an explicit deny in a resource-based policy";

/// One row: the bucket it reads, the policy that bucket is given after the baseline, and the
/// keys of that bucket the policy must refuse. Every key of [`HELD`] that a row does not name
/// must still be served.
struct Row {
    case: &'static str,
    bucket: String,
    policy: String,
    refused: &'static [&'static str],
}

/// The rows: what the deny's own elements select, varied one element at a time.
fn rows(role: &str) -> Vec<Row> {
    let bucket = |row: usize| format!("{BUCKET_PREFIX}-{row}");
    let object = |row: usize, key: &str| format!("arn:aws:s3:::{}/{key}", bucket(row));
    let every_object = |row: usize| format!("arn:aws:s3:::{}/*", bucket(row));
    let prefix = |row: usize, prefix: &str| format!("arn:aws:s3:::{}/{prefix}*", bucket(row));
    let bystander = role_arn(OTHER_ROLE);
    vec![
        Row {
            case: "a deny over one granted prefix refuses that prefix's keys alone",
            bucket: bucket(0),
            policy: document(
                "Deny",
                "s3:GetObject",
                &prefix(0, GRANTED_PREFIXES[1]),
                Some(role),
            ),
            refused: &["denied/report.csv", "denied/second.csv"],
        },
        Row {
            case: "a deny over one key refuses that key alone, not its neighbour",
            bucket: bucket(1),
            policy: document(
                "Deny",
                "s3:GetObject",
                &object(1, "denied/report.csv"),
                Some(role),
            ),
            refused: &["denied/report.csv"],
        },
        Row {
            case: "a deny over every object of the bucket refuses both prefixes",
            bucket: bucket(2),
            policy: document("Deny", "s3:GetObject", &every_object(2), Some(role)),
            refused: &["allowed/report.csv", "denied/report.csv", "denied/second.csv"],
        },
        Row {
            case: "a deny naming another action refuses none of these reads",
            bucket: bucket(3),
            policy: document("Deny", "s3:PutObject", &every_object(3), Some(role)),
            refused: &[],
        },
        Row {
            case: "a deny naming another principal refuses none of these reads",
            bucket: bucket(4),
            policy: document("Deny", "s3:GetObject", &every_object(4), Some(&bystander)),
            refused: &[],
        },
        Row {
            case: "a bucket policy that only allows refuses none of these reads",
            bucket: bucket(5),
            policy: document(
                "Allow",
                "s3:GetObject",
                &prefix(5, GRANTED_PREFIXES[0]),
                Some(role),
            ),
            refused: &[],
        },
    ]
}

/// The identity policy in force on the execution role throughout: `s3:GetObject` over both
/// granted prefixes of every bucket of this table, which is what makes each row about the
/// bucket policy alone.
fn identity_policy() -> String {
    let statements: Vec<Value> = GRANTED_PREFIXES
        .iter()
        .map(|prefix| {
            json!({
                "Effect": "Allow",
                "Action": "s3:GetObject",
                "Resource": format!("arn:aws:s3:::{BUCKET_PREFIX}-*/{prefix}*"),
            })
        })
        .collect();
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// What one read of the table is judged by: which phase it was made in, which row it belongs
/// to, the address it addressed and the bytes that address holds, whether the policy in force
/// must refuse it, and the session the call was made as.
struct Expect<'a> {
    phase: &'a str,
    case: &'a str,
    bucket: &'a str,
    key: &'a str,
    body: &'a str,
    refused: bool,
    session: &'a str,
}

/// What one read must come to: the bytes that key holds, or S3's own refusal naming the
/// session the call was made as, the action, the object and the policy that refused it.
fn assert_read(expect: &Expect<'_>, answer: &Value) {
    let Expect {
        phase,
        case,
        bucket,
        key,
        body,
        refused,
        session,
    } = *expect;
    if !refused {
        assert_eq!(
            answer["read"],
            json!(true),
            "case {case:?} ({phase}): expected the read of {bucket}/{key} to be served, got \
             {answer}"
        );
        assert_eq!(
            answer["body"],
            json!(body),
            "case {case:?} ({phase}): expected {bucket}/{key} to read back the bytes it \
             holds, got {answer}"
        );
        assert_eq!(
            answer["status"],
            json!(200),
            "case {case:?} ({phase}): expected S3's own 200 for {bucket}/{key}, got {answer}"
        );
        return;
    }
    assert_eq!(
        answer["read"],
        json!(false),
        "case {case:?} ({phase}): expected the read of {bucket}/{key} to be refused, got \
         {answer}"
    );
    assert_eq!(
        answer["code"],
        json!(ACCESS_DENIED),
        "case {case:?} ({phase}): expected S3's own refusal code for {bucket}/{key}, got \
         {answer}"
    );
    assert_eq!(
        answer["status"],
        json!(ACCESS_DENIED_STATUS),
        "case {case:?} ({phase}): expected S3's own 403 for {bucket}/{key}, got {answer}"
    );
    let expected = format!(
        "User: {session} is not authorized to perform: s3:GetObject on resource: \
         \"arn:aws:s3:::{bucket}/{key}\" {RESOURCE_DENY}"
    );
    assert_eq!(
        answer["message"],
        json!(expected),
        "case {case:?} ({phase}): expected the refusal {expected:?}, got {answer}"
    );
}

/// Every row read before any bucket policy is written, then again with each row's policy in
/// force: the baseline control the identity policy alone must serve, and the reads each deny
/// selects.
#[tokio::test]
async fn a_bucket_policy_deny_refuses_only_the_handler_reads_it_names() {
    let _stream = one_invocation_stream_at_a_time().await;
    let setup = "the function, its role and the buckets of the deny-scope table";
    let role_name = "lambda-deny-scope-reader";
    let function_name = "s3-deny-scope-reader";
    let emulator = Emulator::new();
    let role = emulator
        .execution_role(setup, role_name, Some(&identity_policy()))
        .await;
    emulator.execution_role(setup, OTHER_ROLE, None).await;
    emulator
        .function(setup, function_name, &role, READER_SOURCE)
        .await;
    let rows = rows(&role);
    for row in &rows {
        emulator
            .s3_ok(row.case, "PUT", &format!("/{}", row.bucket), &[])
            .await;
        for (key, body) in HELD {
            emulator
                .s3_ok(
                    row.case,
                    "PUT",
                    &format!("/{}/{key}", row.bucket),
                    body.as_bytes(),
                )
                .await;
        }
    }
    // Every key of every bucket, read in ONE invocation: the handler reports what each read
    // answered, in the order the event names them.
    let reads: Vec<Value> = rows
        .iter()
        .flat_map(|row| {
            HELD.iter()
                .map(move |(key, _)| json!({"bucket": row.bucket, "key": key}))
        })
        .collect();
    let session = session_arn(role_name, function_name);

    // The baseline: with no bucket policy anywhere, the identity grant alone serves all of it.
    let phase = "before any bucket policy";
    let answered = read_all(&emulator, function_name, phase, &reads).await;
    for (row, answers) in rows.iter().zip(answered.chunks(HELD.len())) {
        for ((key, body), answer) in HELD.iter().zip(answers) {
            assert_read(
                &Expect {
                    phase,
                    case: row.case,
                    bucket: &row.bucket,
                    key,
                    body,
                    refused: false,
                    session: &session,
                },
                answer,
            );
        }
    }

    for row in &rows {
        emulator
            .s3_ok(
                row.case,
                "PUT",
                &format!("/{}?policy", row.bucket),
                row.policy.as_bytes(),
            )
            .await;
    }
    let phase = "with the policy in force";
    let answered = read_all(&emulator, function_name, phase, &reads).await;
    for (row, answers) in rows.iter().zip(answered.chunks(HELD.len())) {
        for ((key, body), answer) in HELD.iter().zip(answers) {
            assert_read(
                &Expect {
                    phase,
                    case: row.case,
                    bucket: &row.bucket,
                    key,
                    body,
                    refused: row.refused.contains(key),
                    session: &session,
                },
                answer,
            );
        }
    }
}

/// One invocation reading every address of `reads`, as the answers the handler reported.
async fn read_all(
    emulator: &Emulator,
    function_name: &str,
    phase: &str,
    reads: &[Value],
) -> Vec<Value> {
    let (signal, payload) = emulator
        .invoke(phase, function_name, &json!({ "reads": reads }))
        .await;
    assert_eq!(
        signal, None,
        "{phase}: the handler must run to completion, got the function error {signal:?} with \
         {payload}"
    );
    let answered = payload["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        answered.len(),
        reads.len(),
        "{phase}: expected one answer per read, got {payload}"
    );
    answered
}
