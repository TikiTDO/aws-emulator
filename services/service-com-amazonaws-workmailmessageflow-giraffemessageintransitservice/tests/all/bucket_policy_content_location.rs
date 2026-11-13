// SPDX-License-Identifier: CC0-1.0

//! `PutRawMessageContent` reads the object only when the BUCKET'S OWN POLICY grants
//! WorkMail that read.
//!
//! RULE: a service that fetches a customer-owned S3 object as its own service
//! principal may read it only while the bucket's resource policy allows that
//! principal the object read on that object's ARN — a bucket carrying no policy, or
//! one whose statements say nothing about that read or explicitly deny it, makes the
//! object unreadable — so an update naming such an object answers the operation's
//! documented content-location error and the message keeps the content it had. The
//! model states the criterion: `InvalidContentLocation` covers "you have an
//! incomplete or missing S3 bucket policy"
//! (`smithy_query(query="doc", shape="InvalidContentLocation")`), and `S3Reference`
//! requires that "for WorkMail to process an S3 object, it must have permission to
//! access that object" (`smithy_query(query="doc", shape="S3Reference")`).
//!
//! Every row stages the object through AMAZON S3's own seams — the bucket, the object
//! and the bucket policy are S3's, written the way `CreateBucket`, `PutObject` and
//! `PutBucketPolicy` leave them — and then asks the question an SDK client asks: "I
//! uploaded this and attached that policy; does the update install it?" The rows
//! differ along what the rule generalises over: whether the bucket carries a policy
//! at all, which principal the grant names and in which form, which action it covers,
//! which resource it covers, whether the effect is `Allow` or `Deny`, and whether a
//! condition holds for the read.
//!
//! The decision is read out of S3's store on every call, which
//! [`a_policy_takes_effect_on_the_next_update`] pins by taking the same update from
//! refused to accepted and back with nothing but a bucket-policy write in between.

use serde_json::{json, Value};
use service_com_amazonaws_workmailmessageflow_giraffemessageintransitservice::state::S3Ref;

use crate::raw_message_content::{
    mime, workmail_read_policy, Emulator, StagedObject, StagedPolicy, BUCKET, REGION,
};

/// The key every row stages its object at. Inside `S3Reference$key`'s
/// `^[a-zA-Z0-9\-/]*$` (`smithy_query(query="shape", shape="S3Reference")`).
const KEY: &str = "updated/message-mime";

/// Another region's code, for the regional spelling of a service principal that is
/// not the one the update below is made in.
const OTHER_REGION: &str = "eu-west-1";

/// An AWS account id that is not the one this emulator serves, for a confused-deputy
/// condition that does not hold.
const OTHER_ACCOUNT: &str = "999999999999";

/// A whole policy document carrying `statements`.
fn document(statements: Vec<Value>) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// One statement: its effect, its `Principal` element, its `Action` element and its
/// `Resource` element.
fn statement(effect: &str, principal: Value, action: Value, resource: Value) -> Value {
    json!({
        "Effect": effect,
        "Principal": principal,
        "Action": action,
        "Resource": resource,
    })
}

/// The same statement, carrying `condition` as its `Condition` block.
fn conditioned(mut statement: Value, condition: Value) -> Value {
    statement["Condition"] = condition;
    statement
}

/// A `Principal` element naming the service principal `name`.
fn service(name: &str) -> Value {
    json!({ "Service": name })
}

/// Every key of [`BUCKET`], as a bucket policy writes the resource element.
fn every_key() -> Value {
    json!(format!("arn:aws:s3:::{BUCKET}/*"))
}

/// The one object the update names, as a bucket policy writes the resource element.
fn this_object() -> Value {
    json!(format!("arn:aws:s3:::{BUCKET}/{KEY}"))
}

/// The account this emulator serves — the account the content is installed for, and
/// so the value of the confused-deputy condition key a grant to a service principal
/// is usually scoped by.
fn this_account() -> String {
    engine::emulator_account_id().to_string()
}

/// One row: the policy the bucket carries, and whether WorkMail may read the object
/// under it.
struct Row {
    case: &'static str,
    policy: StagedPolicy,
    grants: bool,
}

impl Row {
    /// A row whose bucket carries `document`.
    fn new(case: &'static str, document: String, grants: bool) -> Self {
        Self {
            case,
            policy: StagedPolicy::Document(document),
            grants,
        }
    }
}

/// The table: the whole class of bucket policies, spanning presence, principal,
/// action, resource, effect and condition.
fn rows() -> Vec<Row> {
    vec![
        // Presence. A freshly created bucket carries no policy at all, which is the
        // "missing S3 bucket policy" the error names.
        Row {
            case: "a bucket carrying no policy at all",
            policy: StagedPolicy::Absent,
            grants: false,
        },
        // The grant WorkMail's own instructions tell a client to attach.
        Row {
            case: "the documented grant to WorkMail's service principal",
            policy: StagedPolicy::WorkMailRead,
            grants: true,
        },
        // Principal. A service principal is named globally or per region, and the
        // two spell the same identity; every other principal is someone else.
        Row::new(
            "the grant naming the regional spelling of that service principal",
            document(vec![statement(
                "Allow",
                service(&format!("workmail.{REGION}.amazonaws.com")),
                json!("s3:GetObject"),
                every_key(),
            )]),
            true,
        ),
        Row::new(
            "a grant to every principal",
            document(vec![statement(
                "Allow",
                json!("*"),
                json!("s3:GetObject"),
                every_key(),
            )]),
            true,
        ),
        Row::new(
            "a grant naming the service principal among several principals",
            document(vec![statement(
                "Allow",
                json!({ "Service": ["ses.amazonaws.com", "workmail.amazonaws.com"] }),
                json!("s3:GetObject"),
                every_key(),
            )]),
            true,
        ),
        Row::new(
            "a grant to another service principal",
            document(vec![statement(
                "Allow",
                service("ses.amazonaws.com"),
                json!("s3:GetObject"),
                every_key(),
            )]),
            false,
        ),
        Row::new(
            "a grant to the regional spelling of another region",
            document(vec![statement(
                "Allow",
                service(&format!("workmail.{OTHER_REGION}.amazonaws.com")),
                json!("s3:GetObject"),
                every_key(),
            )]),
            false,
        ),
        Row::new(
            "a grant to the calling account's own root rather than to the service",
            document(vec![statement(
                "Allow",
                json!({ "AWS": format!("arn:aws:iam::{}:root", this_account()) }),
                json!("s3:GetObject"),
                every_key(),
            )]),
            false,
        ),
        // Action. The read is authorized under the object-read action, whatever form
        // the element names it in.
        Row::new(
            "a grant whose action element is the service wildcard",
            document(vec![statement(
                "Allow",
                service("workmail.amazonaws.com"),
                json!("s3:*"),
                every_key(),
            )]),
            true,
        ),
        Row::new(
            "a grant whose action list carries the read beside another action",
            document(vec![statement(
                "Allow",
                service("workmail.amazonaws.com"),
                json!(["s3:ListBucket", "s3:GetObject"]),
                every_key(),
            )]),
            true,
        ),
        Row::new(
            "a grant of the object WRITE only",
            document(vec![statement(
                "Allow",
                service("workmail.amazonaws.com"),
                json!("s3:PutObject"),
                every_key(),
            )]),
            false,
        ),
        Row::new(
            "a grant of a bucket-level action only",
            document(vec![statement(
                "Allow",
                service("workmail.amazonaws.com"),
                json!("s3:ListBucket"),
                json!(format!("arn:aws:s3:::{BUCKET}")),
            )]),
            false,
        ),
        // Resource. The ARN a statement has to cover is the OBJECT's.
        Row::new(
            "a grant naming the one object rather than a key prefix",
            document(vec![statement(
                "Allow",
                service("workmail.amazonaws.com"),
                json!("s3:GetObject"),
                this_object(),
            )]),
            true,
        ),
        Row::new(
            "a grant over another key prefix of the same bucket",
            document(vec![statement(
                "Allow",
                service("workmail.amazonaws.com"),
                json!("s3:GetObject"),
                json!(format!("arn:aws:s3:::{BUCKET}/archive/*")),
            )]),
            false,
        ),
        Row::new(
            "a grant over another bucket's objects",
            document(vec![statement(
                "Allow",
                service("workmail.amazonaws.com"),
                json!("s3:GetObject"),
                json!("arn:aws:s3:::otherbucket/*"),
            )]),
            false,
        ),
        Row::new(
            "a grant whose resource is the bucket rather than its objects",
            document(vec![statement(
                "Allow",
                service("workmail.amazonaws.com"),
                json!("s3:GetObject"),
                json!(format!("arn:aws:s3:::{BUCKET}")),
            )]),
            false,
        ),
        // Effect. An explicit deny about the read withholds it however many grants
        // stand beside it.
        Row::new(
            "a grant the same document explicitly denies",
            document(vec![
                statement(
                    "Allow",
                    service("workmail.amazonaws.com"),
                    json!("s3:GetObject"),
                    every_key(),
                ),
                statement(
                    "Deny",
                    service("workmail.amazonaws.com"),
                    json!("s3:GetObject"),
                    every_key(),
                ),
            ]),
            false,
        ),
        Row::new(
            "a deny of another principal beside the grant",
            document(vec![
                statement(
                    "Allow",
                    service("workmail.amazonaws.com"),
                    json!("s3:GetObject"),
                    every_key(),
                ),
                statement(
                    "Deny",
                    service("ses.amazonaws.com"),
                    json!("s3:GetObject"),
                    every_key(),
                ),
            ]),
            true,
        ),
        Row::new(
            "a document that says nothing about the object read",
            document(vec![statement(
                "Allow",
                service("workmail.amazonaws.com"),
                json!("s3:GetBucketLocation"),
                json!(format!("arn:aws:s3:::{BUCKET}")),
            )]),
            false,
        ),
        // Conditions. The emulator serves every endpoint over TLS and installs the
        // content for the account it serves, so a clause over either of those is
        // decided rather than left open.
        Row::new(
            "a grant conditioned on the secure transport the emulator serves",
            document(vec![conditioned(
                statement(
                    "Allow",
                    service("workmail.amazonaws.com"),
                    json!("s3:GetObject"),
                    every_key(),
                ),
                json!({ "Bool": { "aws:SecureTransport": "true" } }),
            )]),
            true,
        ),
        Row::new(
            "a grant conditioned on the account the content is installed for",
            document(vec![conditioned(
                statement(
                    "Allow",
                    service("workmail.amazonaws.com"),
                    json!("s3:GetObject"),
                    every_key(),
                ),
                json!({ "StringEquals": { "aws:SourceAccount": this_account() } }),
            )]),
            true,
        ),
        Row::new(
            "a grant conditioned on another account",
            document(vec![conditioned(
                statement(
                    "Allow",
                    service("workmail.amazonaws.com"),
                    json!("s3:GetObject"),
                    every_key(),
                ),
                json!({ "StringEquals": { "aws:SourceAccount": OTHER_ACCOUNT } }),
            )]),
            false,
        ),
        Row::new(
            "the grant beside the guardrail that denies only an insecure read",
            document(vec![
                statement(
                    "Allow",
                    service("workmail.amazonaws.com"),
                    json!("s3:GetObject"),
                    every_key(),
                ),
                conditioned(
                    statement("Deny", json!("*"), json!("s3:*"), every_key()),
                    json!({ "Bool": { "aws:SecureTransport": "false" } }),
                ),
            ]),
            true,
        ),
        Row::new(
            "a deny of every read that did not arrive over a secure transport, \
             stated so that it denies this one",
            document(vec![
                statement(
                    "Allow",
                    service("workmail.amazonaws.com"),
                    json!("s3:GetObject"),
                    every_key(),
                ),
                conditioned(
                    statement("Deny", json!("*"), json!("s3:*"), every_key()),
                    json!({ "Bool": { "aws:SecureTransport": "true" } }),
                ),
            ]),
            false,
        ),
    ]
}

/// The rule, over the whole table: the update installs the object's bytes exactly
/// when the bucket's policy grants WorkMail the read, and otherwise answers
/// `InvalidContentLocation` with the message's content left as it was.
#[tokio::test]
async fn an_update_resolves_only_what_the_bucket_policy_grants_workmail() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();
        let message_id = "00000000-0000-0000-0000-000000000001";
        let original = mime("original", "body");
        emulator.seed_message(message_id, &original);
        let uploaded = mime("rewritten through S3", "body");
        let s3_reference = emulator.seed_object(
            &S3Ref::new(BUCKET, KEY, None),
            StagedObject::new(uploaded.clone()).with_bucket_policy(row.policy),
        );

        let put = emulator
            .put_raw_message_content(REGION, message_id, &s3_reference)
            .await;
        let after = emulator.get_raw_message_content(REGION, message_id).await;

        if row.grants {
            assert_eq!(
                put.status, 200,
                "case {case:?}: expected HTTP 200 from an update the bucket policy grants, got \
                 {} ({})",
                put.status,
                put.summary()
            );
            assert_eq!(
                after.payload.as_ref(),
                uploaded.as_slice(),
                "case {case:?}: expected the {} octet(s) S3 holds for the object, got {}",
                uploaded.len(),
                after.summary()
            );
        } else {
            assert_eq!(
                put.status, 400,
                "case {case:?}: expected HTTP 400 from an update the bucket policy does not \
                 grant, got {} ({})",
                put.status,
                put.summary()
            );
            assert_eq!(
                put.error_type, "InvalidContentLocation",
                "case {case:?}: expected InvalidContentLocation, got {:?} ({})",
                put.error_type,
                put.summary()
            );
            assert_eq!(
                after.payload.as_ref(),
                original.as_slice(),
                "case {case:?}: the refused update changed the stored content: {}",
                after.summary()
            );
        }
    }
}

/// The policy is read out of Amazon S3 on every call: the SAME update goes from
/// refused to accepted and back again with nothing changing but the bucket's own
/// policy — which is also the sequence a client hits when it forgets the policy,
/// attaches it, and later takes it off.
#[tokio::test]
async fn a_policy_takes_effect_on_the_next_update() {
    let emulator = Emulator::new();
    let message_id = "policy-lifecycle-0";
    let original = mime("original", "body");
    emulator.seed_message(message_id, &original);
    let uploaded = mime("rewritten through S3", "body");
    let s3_reference = emulator.seed_object(
        &S3Ref::new(BUCKET, KEY, None),
        StagedObject::new(uploaded.clone()).with_bucket_policy(StagedPolicy::Absent),
    );

    // (case, the policy in force before the update, whether it grants the read)
    let grant = workmail_read_policy(BUCKET);
    let steps: [(&str, Option<&str>, bool); 4] = [
        ("before any policy is attached", None, false),
        ("once the grant is attached", Some(grant.as_str()), true),
        ("once the policy is taken off again", None, false),
        ("once the grant is attached a second time", Some(grant.as_str()), true),
    ];

    for (case, policy, grants) in steps {
        // Each step starts from the pre-update content, so what the step's own
        // answer did to the message is what is asserted.
        emulator.seed_message(message_id, &original);
        emulator.put_bucket_policy(BUCKET, policy);

        let put = emulator
            .put_raw_message_content(REGION, message_id, &s3_reference)
            .await;
        let after = emulator.get_raw_message_content(REGION, message_id).await;
        let expected_status = if grants { 200 } else { 400 };
        assert_eq!(
            put.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} ({})",
            put.status,
            put.summary()
        );
        let expected_content = if grants { &uploaded } else { &original };
        assert_eq!(
            after.payload.as_ref(),
            expected_content.as_slice(),
            "case {case:?}: expected the {} octet(s) the step's answer leaves in place, got {}",
            expected_content.len(),
            after.summary()
        );
    }
}
