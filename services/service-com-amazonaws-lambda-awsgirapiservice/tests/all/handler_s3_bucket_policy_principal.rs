// SPDX-License-Identifier: CC0-1.0

//! WHO a bucket policy's grant reaches, for the Amazon S3 read a RUNNING handler makes with
//! its own `boto3` client: Lambda, Amazon S3, IAM and the token service on ONE engine, a
//! `python3.x` function whose code does `import boto3`, and every policy written with those
//! services' own operations.
//!
//! The rule pinned here: a resource-based policy's `Principal` element states the IDENTITIES
//! its statement grants to, so a bucket policy authorizes a handler's read exactly when one of
//! the values of that element names the identity the call was made as — the session of the
//! function's execution role, or the role behind it — in whichever of the forms the element
//! admits (a bare string, a list, a map keyed by principal type), because the form is not what
//! decides. A value naming another identity of the same account grants nothing, and a value
//! naming the ACCOUNT delegates the permission to IAM rather than granting it, so a role
//! carrying no identity policy of its own is still refused. Every refusal is Amazon S3's own
//! `AccessDenied` (HTTP 403) naming the identity the call was made as, the action and the
//! object.
//!
//! The phases differ along what the rule generalises over: whether a bucket policy is in force
//! at all, which identity its `Principal` element names (this function's role, another role the
//! account holds, both, the account itself), and which form the element is written in. They run
//! in sequence over ONE bucket and ONE object, each read in its own invocation, so each read is
//! also judged against the policy in force WHEN IT IS MADE: the grant is written, rewritten and
//! finally removed under a function whose execution role carries no identity policy throughout,
//! which is what makes every served read attributable to the bucket's own policy and every
//! refusal falsifiable against the read before it.

use serde_json::{json, Value};

use crate::handler_s3_object_access::{
    role_arn, session_arn, Emulator, ACCESS_DENIED, ACCESS_DENIED_STATUS, READER_SOURCE,
};
use crate::invoke_request_id::one_invocation_stream_at_a_time;

/// The bucket every phase reads, the key it holds and the bytes that key holds.
const BUCKET: &str = "handler-policy-principal";
const KEY: &str = "ledger/day.csv";
const BODY: &str = "day,total\n1,7\n";

/// The execution role the function runs as, and the function's own name — the session a
/// handler's calls are made under is named for both.
const ROLE_NAME: &str = "lambda-policy-principal-reader";
const FUNCTION_NAME: &str = "s3-policy-principal-reader";

/// Another role of the same account, which a phase's policy names instead of or beside this
/// function's. A bucket policy may only name a principal this account holds, so the role a
/// grant is NOT for is a role IAM holds too — which is what makes a phase about the identity
/// the element names and not about an ARN that resolves to nothing.
const BYSTANDER_ROLE: &str = "policy-principal-bystander";

/// The reason clause IAM words a refusal that no policy in force allowed with.
const NO_GRANT: &str = "because no identity-based policy allows the s3:GetObject action";

/// The object every statement below names, as a policy names it.
fn object() -> String {
    format!("arn:aws:s3:::{BUCKET}/{KEY}")
}

/// The spelling a policy names this whole ACCOUNT by: its root
/// (`::service_common::iam_authorization::account_principals` is where the family comes from,
/// and `arn:<partition>:iam::<account>:root` is the one a bucket policy is written with).
fn account_root() -> String {
    format!("arn:aws:iam::{}:root", engine::emulator_account_id())
}

/// One bucket policy granting `s3:GetObject` on [`object`] to `principal`, with the `Principal`
/// element written exactly as `principal` gives it.
fn policy_naming(principal: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": principal,
            "Action": "s3:GetObject",
            "Resource": object(),
        }],
    })
    .to_string()
}

/// What the bucket's policy is for one phase.
enum InForce {
    /// The bucket carries none yet, which is the state it is created in.
    Absent,
    /// This document, written with `PutBucketPolicy` before the phase's read.
    Written(String),
    /// None any more: the document the phase before it wrote, removed with
    /// `DeleteBucketPolicy`.
    Removed,
}

/// One phase: the policy in force for its read, and whether that read must be served.
struct Phase {
    case: &'static str,
    in_force: InForce,
    served: bool,
}

/// The phases, in the order they run: the policy is written, rewritten and removed around a
/// role that never carries an identity policy of its own.
fn phases(role: &str, bystander: &str, session: &str) -> Vec<Phase> {
    vec![
        Phase {
            case: "no bucket policy at all",
            in_force: InForce::Absent,
            served: false,
        },
        Phase {
            case: "the element names this function's role, keyed by principal type",
            in_force: InForce::Written(policy_naming(json!({"AWS": role}))),
            served: true,
        },
        Phase {
            case: "the element names that same role as a bare string",
            in_force: InForce::Written(policy_naming(json!(role))),
            served: true,
        },
        Phase {
            case: "the element names the session the call is made as",
            in_force: InForce::Written(policy_naming(json!({"AWS": session}))),
            served: true,
        },
        Phase {
            case: "a list names this function's role beside another the account holds",
            in_force: InForce::Written(policy_naming(json!({"AWS": [bystander, role]}))),
            served: true,
        },
        Phase {
            case: "a list names only another role the account holds",
            in_force: InForce::Written(policy_naming(json!({"AWS": [bystander]}))),
            served: false,
        },
        Phase {
            case: "the element names the account, which delegates to IAM rather than granting",
            in_force: InForce::Written(policy_naming(json!({"AWS": account_root()}))),
            served: false,
        },
        Phase {
            case: "the grant is taken away again",
            in_force: InForce::Removed,
            served: false,
        },
    ]
}

/// Every phase, each read in its own invocation of one function whose execution role carries no
/// identity policy: what the handler read back is what the bucket's own policy granted it.
#[tokio::test]
async fn a_bucket_policy_grants_a_handlers_read_to_the_principals_its_element_names() {
    let _stream = one_invocation_stream_at_a_time().await;
    let setup = "the function, its role and the bucket of the principal table";
    let emulator = Emulator::new();
    let role = emulator.execution_role(setup, ROLE_NAME, None).await;
    emulator.execution_role(setup, BYSTANDER_ROLE, None).await;
    emulator
        .function(setup, FUNCTION_NAME, &role, READER_SOURCE)
        .await;
    emulator
        .s3_ok(setup, "PUT", &format!("/{BUCKET}"), &[])
        .await;
    emulator
        .s3_ok(setup, "PUT", &format!("/{BUCKET}/{KEY}"), BODY.as_bytes())
        .await;

    let session = session_arn(ROLE_NAME, FUNCTION_NAME);
    let bystander = role_arn(BYSTANDER_ROLE);
    for phase in phases(&role, &bystander, &session) {
        let case = phase.case;
        match &phase.in_force {
            InForce::Absent => {}
            InForce::Written(document) => {
                emulator
                    .s3_ok(
                        case,
                        "PUT",
                        &format!("/{BUCKET}?policy"),
                        document.as_bytes(),
                    )
                    .await;
            }
            InForce::Removed => {
                emulator
                    .s3_ok(case, "DELETE", &format!("/{BUCKET}?policy"), &[])
                    .await;
            }
        }
        let (signal, payload) = emulator
            .invoke(
                case,
                FUNCTION_NAME,
                &json!({ "reads": [{ "bucket": BUCKET, "key": KEY }] }),
            )
            .await;
        assert_eq!(
            signal, None,
            "case {case:?}: the handler must run to completion, got the function error \
             {signal:?} with {payload}"
        );
        let answered = payload["results"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            answered.len(),
            1,
            "case {case:?}: expected one answer for the one read, got {payload}"
        );
        let answer = &answered[0];
        assert_eq!(
            answer["read"],
            json!(phase.served),
            "case {case:?}: expected the read to be served={}, got {answer}",
            phase.served
        );
        if phase.served {
            assert_eq!(
                answer["body"],
                json!(BODY),
                "case {case:?}: expected the handler to read the bytes the bucket holds, got \
                 {answer}"
            );
            assert_eq!(
                answer["length"],
                json!(BODY.len()),
                "case {case:?}: expected the object's own length, got {answer}"
            );
            assert_eq!(
                answer["status"],
                json!(200),
                "case {case:?}: expected S3's own 200, got {answer}"
            );
            continue;
        }
        assert_eq!(
            answer["code"],
            json!(ACCESS_DENIED),
            "case {case:?}: expected S3's own refusal code, got {answer}"
        );
        assert_eq!(
            answer["status"],
            json!(ACCESS_DENIED_STATUS),
            "case {case:?}: expected S3's own 403, got {answer}"
        );
        // The refusal names the identity the call was made as — the session of the function's
        // execution role, named for the function — the action and the object.
        let expected = format!(
            "User: {session} is not authorized to perform: s3:GetObject on resource: \
             \"{}\" {NO_GRANT}",
            object()
        );
        assert_eq!(
            answer["message"],
            json!(expected),
            "case {case:?}: expected the refusal {expected:?}, got {answer}"
        );
    }
}
