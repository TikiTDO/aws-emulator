// SPDX-License-Identifier: CC0-1.0

//! Whether the bucket holding an update's object GRANTS WorkMail the read — the third
//! documented `InvalidContentLocation` reason, and the only one that is a property of the
//! bucket's own resource policy rather than of its Region or its owner.
//!
//! The object is the CUSTOMER's and the read is not the customer's: WorkMail fetches the
//! MIME content itself, as its own service principal, so the only document that can
//! authorize that fetch is the bucket's resource-based policy — a service principal has no
//! identity-based policy in the calling account to grant it. That is why the model's
//! `InvalidContentLocation` documentation
//! (`smithy_query(query="doc", shape="InvalidContentLocation")`) lists "you have an
//! incomplete or missing S3 bucket policy" beside the Region and the owner, and why
//! `S3Reference`'s own documentation states that "for WorkMail to process an S3 object, it
//! must have permission to access that object"
//! (`smithy_query(query="doc", shape="S3Reference")`): a freshly created bucket carries no
//! policy at all, so nothing grants the fetch and the object is unreadable.
//!
//! The decision is the tree's ONE evaluation of IAM policy documents
//! ([`::service_common::iam_authorization`]), asked the way every other emulated service
//! asks about a resource-based policy, so two services cannot read one document
//! differently: a grant needs a statement whose `Action` covers the object read, whose
//! `Resource` covers the object's ARN and whose `Principal` names this reader, and an
//! explicit `Deny` about that request withholds it however many `Allow`s stand beside it.
//! Nothing here reaches a store: the document is read off the bucket by the caller
//! ([`super::referenced_object`]), which is the one place that holds Amazon S3's guard.

use ::service_common::iam_authorization::{resource_decision, Decision, Request};

/// The AWS service that READS the object: WorkMail, which the error's own text names
/// ("WorkMail could not access the updated email content"). Its message flow is one of its
/// data planes, not a service of its own, so the principal that fetches the content is
/// WorkMail's.
const READER_SERVICE: &str = "workmail";

/// Every spelling of [`READER_SERVICE`]'s service principal a bucket policy may name it by,
/// for a read made in `region`.
///
/// A service principal is written either globally, `<service>.amazonaws.com`, or regionally,
/// `<service>.<region>.amazonaws.com` (AWS General Reference, *AWS service principals*), and
/// the two name the same identity — so a policy that grants either of them grants this read,
/// and the regional one is the region the read is made in. Both are compared literally,
/// which is what the IAM grammar gives a `Principal` element: it admits no partial wildcard,
/// only the whole-element `"*"`, which the evaluation itself reads as every principal.
fn reader_principals(region: &str) -> Vec<String> {
    vec![
        format!("{READER_SERVICE}.amazonaws.com"),
        format!("{READER_SERVICE}.{region}.amazonaws.com"),
    ]
}

/// The condition key saying the fetch arrived over a secure transport (AWS General
/// Reference, *Global condition context keys*), which this emulator ANSWERS rather than
/// leaves open: it serves every endpoint over TLS, so the guardrail clause AWS publishes for
/// a bucket policy — a `Deny` of everything that did not arrive that way — withholds nothing
/// from a read made here.
const SECURE_TRANSPORT_KEY: &str = "aws:SecureTransport";

/// That key's value, as IAM's boolean literals spell it.
const SECURE_TRANSPORT_VALUE: &str = "true";

/// The condition key naming the account the service principal is making the call FOR (AWS
/// General Reference, *Global condition context keys*) — the confused-deputy guard a grant to
/// a service principal is usually scoped by. This emulator serves one account, and the
/// message flow the content is installed into is that account's.
const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";

/// The condition keys a read judged here carries a value for, and so DECLARES it models.
/// A clause over any other key leaves its statement undecided, which never grants.
const PUBLISHED_CONDITION_KEYS: &[&str] = &[SECURE_TRANSPORT_KEY, SOURCE_ACCOUNT_KEY];

/// Whether `policy` — the document the bucket carries, `None` for a bucket carrying none —
/// grants WorkMail the read of `key` in `bucket` for a read made in `region`.
///
/// A bucket carrying no policy grants nothing: there is no document to grant from, which is
/// the "missing S3 bucket policy" the error names. A document that grants nothing about this
/// read ("incomplete"), or that explicitly denies it, grants nothing either — only a
/// statement that allows this principal this action on this object does.
pub fn read_is_granted(policy: Option<&str>, bucket: &str, key: &str, region: &str) -> bool {
    let Some(policy) = policy else {
        return false;
    };
    let principals = reader_principals(region);
    // The resource a statement has to cover is the OBJECT's ARN, rendered by Amazon S3's own
    // one place that states it, so a policy written for S3 is matched against the ARN S3
    // itself judges that object by.
    let resource = ::service_com_amazonaws_s3_amazons3::state::object_arn(bucket, key);
    let context = [
        (
            SECURE_TRANSPORT_KEY.to_string(),
            SECURE_TRANSPORT_VALUE.to_string(),
        ),
        (
            SOURCE_ACCOUNT_KEY.to_string(),
            ::engine::emulator_account_id().to_string(),
        ),
    ];
    // The action is the one AMAZON S3 publishes for a read of an object, taken from S3's own
    // statement of it rather than spelled again here.
    let decision = resource_decision(
        policy,
        &Request::new(
            ::service_com_amazonaws_s3_amazons3::state::GET_OBJECT_ACTION,
            &resource,
            &principals,
        )
        .with_context(&context)
        .with_published_keys(PUBLISHED_CONDITION_KEYS),
    );
    decision == Decision::Allowed
}
