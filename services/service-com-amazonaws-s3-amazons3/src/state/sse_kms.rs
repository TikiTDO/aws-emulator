// SPDX-License-Identifier: CC0-1.0

//! Whether the KMS key an object was stored under lets THIS caller have its plaintext.
//!
//! An object stored with server-side encryption using a customer-managed KMS key
//! ([`super::KMS_SSE_ALGORITHMS`], the two `ServerSideEncryption` values a KMS key id
//! belongs to) is readable only THROUGH that key — its plaintext comes of opening the
//! object's data key — so every read that serves such an object's BYTES performs
//! `kms:Decrypt` on that key, as the caller and through S3's own service path. "If the
//! object is encrypted with SSE-KMS, you must also have the
//! kms:GenerateDataKey and kms:Decrypt permissions in IAM identity-based policies and KMS
//! key policies for the KMS key" (`HeadObject`'s own documentation of the permissions an
//! SSE-KMS object's read needs). A caller no authority in force reaches that action for is
//! refused — S3 surfaces KMS's denial rather than the body — and the refusal is S3's own
//! `AccessDenied` (HTTP 403) carrying the sentence KMS worded for it.
//!
//! WHO the caller is comes from the credential the request presents, which for a session
//! another service vended (a role session an identity pool hands out, one `sts:AssumeRole`
//! minted) is the principal of that session rather than the account: the decision is made
//! against the role the session is of, so a key policy that grants `kms:Decrypt` to one
//! role admits that role's sessions and leaves every other session to the policy's other
//! statements.
//!
//! One class of read presents no credential of a caller's at all: a read ANOTHER AWS SERVICE
//! performs as its own service principal for one of its resources — CloudFront fetching an
//! origin object for a distribution whose origin access control signed the fetch. AWS decides
//! such a read for that service principal, with `aws:SourceArn` naming the resource it acts
//! for, so a key whose policy grants `cloudfront.amazonaws.com` for one distribution opens an
//! object's bytes for that distribution's fetches and for nothing else. Which calls are in
//! that class is recorded by the calling service, under the access key id of the credential
//! it created for the one call and signed it with, for only as long as that call is in flight
//! ([`::service_common::service_principal_calls`]) — so the record is resolved here from the
//! access key id the request PRESENTS, which the emulator's verifier resolves and checks the
//! signature against when request authentication is on, and from the account this service
//! serves; a read presenting any other credential is decided as above.
//!
//! Both halves are the KMS service's OWN ([`kms_store::authorization`]), made against the
//! key policy, caller identity policies and grants that KMS's and IAM's handlers wrote, so
//! this service and KMS cannot disagree about a key and the refusal a caller sees is the
//! sentence KMS itself words for it. This module reaches no state of THIS service, so a
//! handler asks it while still holding its own store guard — the object whose bytes are
//! about to be served is the one the guard is held for, and the stores this reads (KMS's,
//! and through it STS's and IAM's, beside the shared directory of in-flight
//! service-principal calls) belong to services that do not read this one's, so no call here
//! can come back round to the guard a caller holds.
//!
//! The other half of this module is about a WRITE rather than a read: an object S3 is asked
//! to encrypt under a named customer key is encrypted under a data key KMS mints for that
//! key, so the write performs `kms:GenerateDataKey` on it and a caller the key's authorities
//! refuse that action to is refused the write, with nothing stored
//! ([`data_key_refusal`] — the same decision as the read's, asked of the write's action); and
//! the reference itself is resolved against KMS before anything is stored, so a reference
//! KMS holds no key for refuses the write for want of a key rather than of permission
//! ([`missing_key_refusal`]).
//!
//! What is NOT decided here: a read that serves no bytes. "A HEAD request has the same
//! options as a GET operation on an object" (`HeadObject`'s own documentation) except that
//! it answers no body at all, and the metadata it reports is S3's own record of the object
//! rather than anything the key has to open — which is also the recorded behaviour of the
//! real service, where a HEAD of an object keeps reporting its size and its key id while
//! the key itself can no longer open it.

use std::collections::HashMap;

use ::service_com_amazonaws_kms_trentservice::state as kms_store;

/// The KMS operation a read that serves an SSE-KMS object's bytes performs on its key.
const DECRYPT_OPERATION: &str = "Decrypt";

/// The KMS operation a WRITE that stores an SSE-KMS object performs on its key: the data
/// key the object's bytes are encrypted under is one KMS mints under that key.
///
/// "You must also have the kms:GenerateDataKey and kms:Decrypt permissions in IAM
/// identity-based policies and KMS key policies for the KMS key" (`HeadObject`'s own
/// documentation of what an SSE-KMS object needs), the first of the two being what the write
/// side of that pair performs.
const GENERATE_DATA_KEY_OPERATION: &str = "GenerateDataKey";

/// This service's endpoint prefix, the first label of its regional endpoint
/// (`smithy_query(query="service_info")` reports `endpointPrefix: "s3"`) — which is what
/// [`via_service`] names.
const ENDPOINT_PREFIX: &str = "s3";

/// The encryption-context entry S3 binds an SSE-KMS object's data key to: the Amazon
/// Resource Name of what is being encrypted, under this entry name.
///
/// Amazon S3 User Guide, *Using server-side encryption with KMS keys*, "Encryption
/// context": S3 "uses the object or bucket Amazon Resource Name (ARN) as the encryption
/// context pair", `aws:s3:arn` naming it — the bucket's ARN for an object using an S3
/// Bucket Key, and the object's own otherwise.
const ARN_CONTEXT_KEY: &str = "aws:s3:arn";

/// One stored object a read is about to serve the bytes of: where it lives, and the
/// encryption configuration the write that stored it settled on.
#[derive(Debug, Clone, Copy)]
pub struct EncryptedObject<'a> {
    /// The bucket holding the object — for a copy, the SOURCE bucket, since the key that
    /// has to open the bytes is the source object's.
    pub bucket: &'a str,
    /// The object's key.
    pub key: &'a str,
    /// What [`super::Object::encryption`] records for it.
    pub encryption: &'a super::ObjectEncryption,
}

/// Why KMS refuses to open `object` for the principal this request speaks for, or `None`
/// when the read may be served. Served: an object that is not SSE-KMS at all; one under a
/// key AWS MANAGES, whose policy AWS manages for the service that owns it, so a caller
/// needs no grant of its own on it — which is also what an SSE-KMS write naming no key id
/// is stored under, the Amazon Web Services managed key for Amazon S3; and one whose key
/// does permit the caller the decrypt.
///
/// Refused: a caller no authority on the key reaches `kms:Decrypt` for, and an identifier
/// whose account segment is not an account id, which KMS refuses for want of permission
/// BEFORE the resource it names is looked up — so no key is resolved for it and nothing can
/// open the object's bytes for this caller.
///
/// A well-formed identifier this endpoint holds no key for is left to be served: WHICH key
/// a write may name at all, and what a read answers when that key is gone, is the subject of
/// the object-write path rather than of this decision.
///
/// `region` is the Region the request was signed for, which is where the object's key is
/// resolved: an object and the key it is encrypted under live at the same endpoint.
///
/// An engine carrying no KMS service holds no key to ask and no policy to read, so nothing
/// is refused on evidence that is not there.
pub fn decrypt_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    object: EncryptedObject<'_>,
) -> Option<String> {
    key_use_refusal(engine, ctx, region, object, DECRYPT_OPERATION)
}

/// Why KMS refuses to mint the data key `object` would be encrypted under for the principal
/// this request speaks for, or `None` when the write may be stored.
///
/// Every write that stores an object under a customer-managed KMS key is encrypted under a
/// data key KMS mints for it, so the write performs `kms:GenerateDataKey` on that key — as
/// the caller and through this service's own path — before anything is recorded. A caller no
/// authority in force reaches that action for is refused, and nothing is written: the
/// refusal is the same `AccessDenied` (HTTP 403) a read's refusal is, carrying the sentence
/// KMS worded for it.
///
/// `object` is the object as the write WOULD store it: its bucket, its key and the
/// encryption configuration the write settled on — the write's own members, or the bucket's
/// default encryption configuration for a write that named none — so the encryption context
/// the data key is bound to is the one the stored object would be read back under
/// ([`EncryptedObject`] and [`encryption_context`] are shared with the read side for exactly
/// that reason, which is what makes an S3 Bucket Key observable: a key policy scoped to the
/// bucket's ARN admits a bucket-key write and withholds a write of the same bytes with the
/// Bucket Key off, whose data key is bound to the object's own ARN).
///
/// What is NOT refused here is what the read side does not refuse either: an object that is
/// not SSE-KMS, a key AWS manages (whose policy AWS manages for the service that owns it),
/// and a reference this endpoint resolves no key for — the absence of a key is
/// [`missing_key_refusal`]'s relay rather than a want of permission.
pub fn data_key_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    object: EncryptedObject<'_>,
) -> Option<String> {
    key_use_refusal(engine, ctx, region, object, GENERATE_DATA_KEY_OPERATION)
}

/// Why the authorities in force on `object`'s KMS key refuse `operation` to the principal
/// this request speaks for, or `None` when the call may be served.
///
/// ONE decision for both sides of the envelope — the `kms:GenerateDataKey` a write performs
/// and the `kms:Decrypt` a read performs — so a key's policy, its grants and the caller's
/// identity policies cannot be read one way for a write and another for a read, and the two
/// are judged under the same encryption context.
fn key_use_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    object: EncryptedObject<'_>,
    operation: &str,
) -> Option<String> {
    if !object.encryption.uses_kms() {
        return None;
    }
    let key_id = object.encryption.kms_key_id.as_deref()?;
    let kms = kms_store::service_state(engine)?;
    // The caller is resolved before the KMS store is locked, because resolving it reads
    // the STS and IAM stores.
    let caller = caller(engine, ctx);
    let store = kms.lock();
    let key = match store.resolve(region, key_id) {
        kms_store::KeyRef::Found(key) => key,
        // An identifier whose account segment is not an account id is refused for want of
        // permission before any key is looked up, with KMS's own sentence for it, so the
        // object's bytes cannot be opened under it at all.
        kms_store::KeyRef::UnauthorizedAccount => {
            return Some(kms_store::unauthorized_account_message(operation))
        }
        kms_store::KeyRef::NotFound | kms_store::KeyRef::MalformedArn => return None,
    };
    if key.key_manager != kms_store::KEY_MANAGER_CUSTOMER {
        return None;
    }
    kms_store::authorization::authorize_use(
        &caller,
        &store,
        region,
        key,
        kms_store::authorization::RequestedUse::through_service(operation, &via_service(region)),
        Some(&encryption_context(&object)),
        None,
    )
    .err()
}

/// The wire error code S3 answers a WRITE whose SSE-KMS key reference KMS holds no key for,
/// and the status it carries.
///
/// Such a write is encrypted under a data key S3 asks KMS for, so a reference that resolves
/// to no key fails inside KMS rather than in S3: S3 relays KMS's own failure, under the KMS
/// exception's name qualified by the service that raised it, and refuses the write. Recorded
/// behaviour — the s3 model binds no error shape for the relay, so neither the code nor the
/// status can come from it; the message is KMS's own ([`missing_key_refusal`]).
pub const KMS_NOT_FOUND_CODE: &str = "KMS.NotFoundException";

/// The HTTP status [`KMS_NOT_FOUND_CODE`] is answered with.
pub const KMS_NOT_FOUND_STATUS: u16 = 400;

/// KMS's own sentence for `key_id` naming no key at this endpoint, or `None` when the
/// reference is one a write may be encrypted under.
///
/// Every write that asks S3 to encrypt an object under a NAMED customer key resolves that
/// reference against KMS before anything is stored, because the object's data key comes of
/// asking KMS for one under it: a reference KMS holds no key for cannot produce a data key,
/// so there is nothing to encrypt the object with and the write is refused rather than
/// recorded under a key that does not exist. The resolution accepts every form a key
/// reference takes — a key id, a key ARN, `alias/<name>` and an alias ARN — because it is
/// KMS's own ([`kms_store::ComAmazonawsKmsTrentserviceState::resolve`]), and the sentence is
/// the one KMS words for the form the request supplied
/// ([`kms_store::key_not_found_message`]), so S3 and KMS cannot disagree about which key a
/// reference names or about how its absence reads.
///
/// `region` is the Region the request was signed for, which is where the key is resolved: a
/// KMS key is a regional resource, and the object and the key it is encrypted under live at
/// the same endpoint.
///
/// A PREDEFINED Amazon Web Services alias — `alias/aws/<service>`, the Amazon Web Services
/// managed key of a service — is resolved through KMS's own association instead of through
/// the plain lookup: such an alias "exists in every account and Region while carrying no
/// target key", and "if you specify a predefined Amazon Web Services alias … KMS associates
/// the alias with an Amazon Web Services managed key"
/// ([`kms_store::ComAmazonawsKmsTrentserviceState::associate_predefined_aws_alias`]), so
/// asking for it is what gives it a key and a write naming one is served — the same
/// resolution every other service of this tree makes of such a reference.
///
/// NOT refused here: a reference KMS declines BEFORE it resolves anything — an ARN naming
/// another account, or an identifier that is no key reference at all — which is a want of
/// permission rather than an absence, and so a different relay from this one.
///
/// An engine carrying no KMS service holds no key to ask, so nothing is refused on evidence
/// that is not there.
///
/// LOCKING: this takes the KMS store's lock and no other, and a caller holds NONE of this
/// service's own while it asks — the reference it asks about is read out of this service's
/// store and that guard released first, so no request can hold the two in opposite orders.
pub fn missing_key_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    key_id: &str,
) -> Option<String> {
    let kms = kms_store::service_state(engine)?;
    let mut store = kms.lock();
    if kms_store::is_predefined_aws_alias(key_id) {
        return store
            .associate_predefined_aws_alias(region, key_id)
            .is_none()
            .then(|| kms_store::key_not_found_message(region, key_id));
    }
    match store.resolve(region, key_id) {
        kms_store::KeyRef::NotFound => Some(kms_store::key_not_found_message(region, key_id)),
        kms_store::KeyRef::Found(_)
        | kms_store::KeyRef::MalformedArn
        | kms_store::KeyRef::UnauthorizedAccount => None,
    }
}

/// Whose read this is, as the key policy decides it.
///
/// A read ANOTHER AWS SERVICE is performing as its own service principal for one of its
/// resources is that principal, acting for that resource: the calling service signed the call
/// with a credential it created for it alone and recorded both under that credential's access
/// key id, so the record is resolved from the access key id the request PRESENTS — the
/// credential the emulator's verifier resolves, and checks the signature against when request
/// authentication is on — and only for a call of this account (see the module docs). Every
/// other read is the principal of the credential it presents, which is KMS's own resolution
/// ([`kms_store::authorization::caller`]).
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> kms_store::authorization::Caller {
    if let Some(directory) = service_principal_calls(engine) {
        let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
        let account = ::engine::emulator_account_id();
        let now = ::std::time::SystemTime::now();
        let held = directory.lock();
        if let Some(call) = held.call(&access_key_id, account, now) {
            ::tracing::debug!(
                principal = %call.principal,
                source_arn = %call.source_arn,
                "this read is one another service is making as its own service principal, so \
                 the object's key is asked about that principal"
            );
            return kms_store::authorization::Caller::service_principal(
                &call.principal,
                &call.source_arn,
            );
        }
    }
    kms_store::authorization::caller(engine, ctx)
}

/// The shared directory of the cross-service calls another service is making as its own
/// service principal, in the one shared engine: written by that service, read here.
fn service_principal_calls<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<::service_common::service_principal_calls::ServicePrincipalCalls>,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::service_principal_calls::ServicePrincipalCalls,
            >,
        )
}

/// The encryption context the data key of `object` is bound to, which is what a key policy
/// statement scoped to one bucket or one object is decided against.
///
/// [`ARN_CONTEXT_KEY`] names the resource being encrypted, and the pairs the write supplied
/// in `x-amz-server-side-encryption-context` come beside it: that header carries "the
/// Amazon Web Services KMS Encryption Context as an additional encryption context to use
/// for object encryption" (`PutObjectRequest$SSEKMSEncryptionContext`), so what a read
/// decrypts under is the one entry S3 always binds plus whatever the write added.
fn encryption_context(object: &EncryptedObject<'_>) -> HashMap<String, String> {
    let encrypted = match object.encryption.bucket_key_enabled {
        // "Setting this header to true causes Amazon S3 to use an S3 Bucket Key"
        // (`PutObjectRequest$BucketKeyEnabled`), and a bucket-level key is bound to the
        // bucket rather than to each object under it.
        Some(true) => super::bucket_arn(object.bucket),
        _ => super::object_arn(object.bucket, object.key),
    };
    let mut context = HashMap::from([(ARN_CONTEXT_KEY.to_string(), encrypted)]);
    context.extend(supplied_context(
        object.encryption.kms_encryption_context.as_deref(),
    ));
    context
}

/// The pairs an object's stored `SSEKMSEncryptionContext` names: "a Base64-encoded string
/// holding a UTF-8 encoded JSON of additional encryption context key-value pairs"
/// (`PutObjectRequest$SSEKMSEncryptionContext`), decoded.
///
/// A value that is not that — the member is stored verbatim as the write sent it — adds no
/// pair, so a read is decided under the entry S3 itself binds and nothing invented.
fn supplied_context(header: Option<&str>) -> Vec<(String, String)> {
    let Some(decoded) = header.and_then(|value| ::aws_smithy_types::base64::decode(value).ok())
    else {
        return Vec::new();
    };
    let Ok(::serde_json::Value::Object(pairs)) =
        ::serde_json::from_slice::<::serde_json::Value>(&decoded)
    else {
        return Vec::new();
    };
    pairs
        .into_iter()
        .filter_map(|(name, value)| Some((name, value.as_str()?.to_string())))
        .collect()
}

/// The value of the `kms:ViaService` condition key for a cryptographic operation this
/// service performs on a caller's behalf: this service's regional endpoint name, which is
/// what a key policy confining a key to S3's use of it names.
fn via_service(region: &str) -> String {
    format!("{ENDPOINT_PREFIX}.{region}.amazonaws.com")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of [`the_encryption_context_names_what_was_encrypted`]: the case, whether an
    /// S3 Bucket Key is in force, the context the write supplied, the ARN the entry must
    /// name, and how many entries the context must hold.
    type ContextRow = (&'static str, Option<bool>, Option<String>, &'static str, usize);

    /// The context an object's record is decrypted under: the rows differ along whether an
    /// S3 Bucket Key binds it to the bucket, and along what the write supplied beside it
    /// (nothing, one pair, several, and a value that is not an encoded JSON object).
    #[test]
    fn the_encryption_context_names_what_was_encrypted() {
        let pairs = ::aws_smithy_types::base64::encode(br#"{"tenant":"payments","zone":"a"}"#);
        let one = ::aws_smithy_types::base64::encode(br#"{"tenant":"payments"}"#);
        let rows: [ContextRow; 5] = [
            (
                "an object key, no supplied context",
                None,
                None,
                "arn:aws:s3:::ledger/reports/held.csv",
                1,
            ),
            (
                "an S3 Bucket Key binds the bucket",
                Some(true),
                None,
                "arn:aws:s3:::ledger",
                1,
            ),
            (
                "an S3 Bucket Key switched off binds the object",
                Some(false),
                None,
                "arn:aws:s3:::ledger/reports/held.csv",
                1,
            ),
            (
                "one supplied pair beside the object entry",
                None,
                Some(one),
                "arn:aws:s3:::ledger/reports/held.csv",
                2,
            ),
            (
                "several supplied pairs",
                None,
                Some(pairs),
                "arn:aws:s3:::ledger/reports/held.csv",
                3,
            ),
        ];
        for (case, bucket_key_enabled, supplied, arn, entries) in rows {
            let encryption = super::super::ObjectEncryption {
                sse_algorithm: Some(super::super::KMS_SSE_ALGORITHMS[0].to_string()),
                kms_key_id: Some("alias/ledger".to_string()),
                kms_encryption_context: supplied,
                bucket_key_enabled,
                ..super::super::ObjectEncryption::default()
            };
            let context = encryption_context(&EncryptedObject {
                bucket: "ledger",
                key: "reports/held.csv",
                encryption: &encryption,
            });
            assert_eq!(
                context.get(ARN_CONTEXT_KEY).map(String::as_str),
                Some(arn),
                "case {case:?}: expected {arn:?}, got {context:?}"
            );
            assert_eq!(
                context.len(),
                entries,
                "case {case:?}: expected {entries} entries, got {context:?}"
            );
        }
    }

    /// A stored context that is not an encoded JSON object of strings adds no pair rather
    /// than inventing one: the rows differ along how the value fails to be one.
    #[test]
    fn a_stored_context_that_is_not_encoded_pairs_adds_nothing() {
        let rows: [(&str, Option<String>); 5] = [
            ("no header at all", None),
            ("not base64", Some("not base64 at all".to_string())),
            (
                "base64 of something that is not JSON",
                Some(::aws_smithy_types::base64::encode(b"tenant=payments")),
            ),
            (
                "base64 of a JSON array",
                Some(::aws_smithy_types::base64::encode(br#"["tenant"]"#)),
            ),
            (
                "base64 of a JSON object whose value is not a string",
                Some(::aws_smithy_types::base64::encode(br#"{"tenant":7}"#)),
            ),
        ];
        for (case, header) in rows {
            let supplied = supplied_context(header.as_deref());
            assert!(
                supplied.is_empty(),
                "case {case:?}: expected no pair, got {supplied:?}"
            );
        }
    }
}
