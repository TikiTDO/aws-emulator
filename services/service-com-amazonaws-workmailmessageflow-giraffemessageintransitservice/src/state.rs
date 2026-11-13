// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-workmailmessageflow-giraffemessageintransitservice service.
//!
//! WorkMail Message Flow has no create or delete operation of its own: an
//! in-transit message exists because WorkMail delivery put it in transit, and the
//! MIME content `PutRawMessageContent` installs comes out of an S3 object that S3
//! holds. Both of those producers live outside this API, and they are owned in two
//! different places: the in-transit messages are this module's store, with the seed
//! helper that stands in for WorkMail delivery
//! ([`ComAmazonawsWorkmailmessageflowGiraffemessageintransitserviceState::seed_in_transit_message`]),
//! while an S3 object belongs to AMAZON S3 and is read out of S3's own store in the
//! shared engine ([`referenced_object`]) — never out of a copy kept here, which no
//! S3 client could write. Nothing here fabricates content: every byte a handler
//! returns was written into the store by a seed, or installed by a successful
//! `PutRawMessageContent` out of the object S3 holds.
//!
//! Both sides are regional. A message is in transit in the message flow of one
//! region and an S3 bucket lives in one region, so the message is keyed with its
//! region and the bucket's own region is read off the bucket, each resolved against
//! the region the request was made in ([`request_region`]).

use std::collections::HashMap;
use std::time::SystemTime;

use crate::ops::types::S3Reference;

pub mod bucket_policy;

/// The region a request is treated as having been made in when its SigV4
/// credential scope does not name one (an unsigned or anonymous request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `message` of the `ResourceNotFoundException` every operation of this
/// service raises for a `messageId` that is not in transit.
///
/// The model documents the error only as "The requested email message is not
/// found." (`smithy_query(query="shape", shape="ResourceNotFoundException")`) and
/// states no text, but the real service builds the text from the request: it is
/// the fixed prefix `Unknown messageId: ` followed verbatim by the `messageId`
/// the request asked for, so the caller is told which id was rejected. Both
/// modeled operations raise this error on the same condition, so both render it
/// through this one function rather than each inventing wording.
pub fn unknown_message_id(message_id: &str) -> String {
    format!("Unknown messageId: {message_id}")
}

/// A reference to an S3 object, as `PutRawMessageContent` supplies it.
///
/// `object_version` is part of the identity of what is referenced — the model
/// documents it as "If you enable versioning for the bucket, you can specify the
/// object version" (`smithy_query(query="shape", shape="S3Reference")`) — so it is
/// retained on the key and on the record of the last accepted reference rather
/// than dropped.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct S3Ref {
    pub bucket: String,
    pub key: String,
    pub object_version: Option<String>,
}

impl S3Ref {
    /// A reference to `bucket`/`key`, at `object_version` when one is given.
    pub fn new(
        bucket: impl Into<String>,
        key: impl Into<String>,
        object_version: Option<String>,
    ) -> Self {
        Self {
            bucket: bucket.into(),
            key: key.into(),
            object_version,
        }
    }

    /// The reference a request carried, as stored.
    pub fn from_modeled(reference: &S3Reference) -> Self {
        Self {
            bucket: reference.bucket.clone(),
            key: reference.key.clone(),
            object_version: reference.object_version.clone(),
        }
    }

    /// `s3://bucket/key` (plus `?versionId=` when the reference is versioned), for
    /// an error message that tells the caller which reference failed.
    pub fn display(&self) -> String {
        match &self.object_version {
            Some(version) => format!("s3://{}/{}?versionId={version}", self.bucket, self.key),
            None => format!("s3://{}/{}", self.bucket, self.key),
        }
    }
}

/// What the S3 object a reference names turns out to be, read from Amazon S3's own
/// store ([`referenced_object`]).
///
/// The variants other than [`Self::Content`] are the conditions that stop WorkMail
/// from reading the object, and each is a property of the REAL bucket or object:
/// the three the model's `InvalidContentLocation` documentation names
/// (`smithy_query(query="doc", shape="InvalidContentLocation")`) — "you made the
/// request in a region other than your S3 bucket region", "the S3 bucket owner
/// is not the same as the calling AWS account" and "you have an incomplete or
/// missing S3 bucket policy" — plus the reference naming no object at all, and the
/// one ceiling this emulator reads content up to. Which condition fired is reported
/// here rather than decided here, so the wording of the refusal stays with the
/// operation that answers it.
#[derive(Debug)]
pub enum ReferencedObject {
    /// The bytes the referenced object holds.
    Content(Vec<u8>),
    /// S3 holds no bucket of that name, no object at that key, or no such version
    /// of it — including a key whose current version is a delete marker, which is
    /// a key S3 behaves as though it held nothing at.
    NoObject,
    /// The bucket is in `bucket_region`, which is not the region the request was
    /// made in.
    OtherRegion { bucket_region: String },
    /// The bucket is not one of the buckets the calling account holds in S3.
    OtherAccount,
    /// The bucket's own resource policy does not grant WorkMail the read of that
    /// object — the bucket carries no policy at all, or the document it carries
    /// says nothing about this read or denies it ([`bucket_policy`]).
    NotPermitted,
    /// The object carries `octets` octets, beyond the ceiling the caller asked to
    /// read up to.
    Oversize { octets: usize },
}

/// The object `reference` names in AMAZON S3, resolved against S3's own store in
/// the shared engine, for a request made in `request_region` and reading content up
/// to `max_octets`.
///
/// `None` when that store is unreachable (the engine is not the shared in-memory
/// one) — the same condition [`service_state`] reports for this service's own
/// store, and never an empty resolution, so an unreachable store is not reported as
/// a reference that does not resolve.
///
/// The model documents `S3Reference` as "Amazon S3 object representing the updated
/// message content, in MIME format", and notes that "the region for the S3 bucket
/// containing the S3 object must match the region used for WorkMail operations" and
/// that "for WorkMail to process an S3 object, it must have permission to access
/// that object" (`smithy_query(query="doc", shape="S3Reference")`), so the object is
/// S3's and so is every condition below: a client makes it resolvable by calling
/// `PutObject` and `PutBucketPolicy`, and this reads what those writes left. `objectVersion` selects one
/// version of it, and a reference that names none addresses the key's current
/// version, which is what an S3 read naming no version answers with.
///
/// Ownership is decided by the two properties S3's own "buckets in your account"
/// seam filters on (`account_buckets` is `buckets()` less `provisioned_by` and
/// `aws_published`): a bucket another emulated service provisioned as the object
/// store of one of ITS resources, and one AWS publishes, are held in a
/// service-managed account, so for the calling account they are exactly the
/// documented "bucket owner is not the same as the calling AWS account". Asked of
/// the bucket the address already resolved, so it costs no second lookup. Those two
/// are the ONLY ownership signal the object store carries — this emulator serves one
/// account, so a bucket created over S3's own API belongs to the caller and can
/// never earn this refusal.
///
/// Being the calling account's bucket is not yet permission to READ the object: the
/// fetch is WorkMail's own, so it is the bucket's resource policy that has to grant
/// it ([`bucket_policy::read_is_granted`]), which is the third condition the model's
/// `InvalidContentLocation` documentation names. It is asked of the bucket the
/// address already resolved, like the two above it, so it costs no second lookup.
///
/// LOCK ORDER: this takes Amazon S3's store lock and holds no other while it does,
/// and it hands back owned values rather than a guard — so it is called with NO
/// guard of this service's own store held. Keeping the cross-service read
/// one-directional and guard-free is what makes a lock cycle between the two stores
/// unreachable.
pub fn referenced_object<Eng: Send + Sync + 'static>(
    engine: &Eng,
    reference: &S3Ref,
    request_region: &str,
    max_octets: usize,
) -> Option<ReferencedObject> {
    let s3 = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let store = s3.lock();
    let Some(bucket) = store.bucket(&reference.bucket) else {
        return Some(ReferencedObject::NoObject);
    };
    if bucket.region != request_region {
        return Some(ReferencedObject::OtherRegion {
            bucket_region: bucket.region.clone(),
        });
    }
    if bucket.provisioned_by().is_some() || bucket.is_aws_published() {
        return Some(ReferencedObject::OtherAccount);
    }
    if !bucket_policy::read_is_granted(
        bucket.policy.as_deref(),
        &reference.bucket,
        &reference.key,
        request_region,
    ) {
        return Some(ReferencedObject::NotPermitted);
    }
    let Some(object) =
        bucket.addressed_object(&reference.key, reference.object_version.as_deref())
    else {
        return Some(ReferencedObject::NoObject);
    };
    // The ceiling is decided while the object is still BORROWED from S3's store, so
    // content beyond it is refused without being copied: the copy below is the one
    // place this read duplicates an object's bytes.
    if object.body.len() > max_octets {
        return Some(ReferencedObject::Oversize {
            octets: object.body.len(),
        });
    }
    Some(ReferencedObject::Content(object.body.clone()))
}

/// The key an in-transit message is stored under.
///
/// This service is a regional data plane: its endpoint carries a region
/// (`endpointPrefix` `workmailmessageflow` with `sigv4` and an endpoint rule set —
/// `smithy_query(query="service_info")`), and an in-transit message belongs to the
/// message flow of exactly one region. The id alone is therefore not an address:
/// the region a request was made in is part of it, exactly as it is part of the
/// identity of an S3 bucket.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MessageKey {
    pub region: String,
    pub message_id: String,
}

impl MessageKey {
    /// The address of `message_id` in `region`'s message flow.
    pub fn new(region: impl Into<String>, message_id: impl Into<String>) -> Self {
        Self {
            region: region.into(),
            message_id: message_id.into(),
        }
    }
}

/// One email message currently in transit through WorkMail's message flow.
#[derive(Debug, Clone)]
pub struct InTransitMessage {
    /// The id the URI label names this message by.
    pub message_id: String,
    /// The region whose message flow this message is in transit in. Only a request
    /// made in this region addresses it; every other region's endpoint answers as
    /// though the message did not exist.
    pub region: String,
    /// The raw MIME bytes `GetRawMessageContent` returns verbatim as
    /// `messageContent`.
    pub content: Vec<u8>,
    /// Whether the message is still eligible for update. A frozen message answers
    /// `PutRawMessageContent` with `MessageFrozen` and still serves
    /// `GetRawMessageContent`.
    pub frozen: bool,
    /// The reference the most recent successful `PutRawMessageContent` resolved,
    /// so the update that produced the current content is observable.
    pub last_s3_reference: Option<S3Ref>,
    /// When the content last changed.
    pub updated_at: SystemTime,
}

impl InTransitMessage {
    fn new(region: String, message_id: String, content: Vec<u8>) -> Self {
        Self {
            message_id,
            region,
            content,
            frozen: false,
            last_s3_reference: None,
            updated_at: SystemTime::now(),
        }
    }
}

/// Per-service state for com-amazonaws-workmailmessageflow-giraffemessageintransitservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsWorkmailmessageflowGiraffemessageintransitserviceState {
    /// Every in-transit message, keyed by the region its message flow is in and
    /// the `messageId` that region's endpoint addresses it by, so one region's
    /// endpoint never resolves another region's message.
    pub messages: HashMap<MessageKey, InTransitMessage>,
}

impl ComAmazonawsWorkmailmessageflowGiraffemessageintransitserviceState {
    /// Put a message into transit in [`DEFAULT_REGION`] with `content` as its MIME
    /// body, as that region's WorkMail delivery does, and hand back the stored
    /// record so a caller can mark it frozen.
    ///
    /// This is the producer this API does not model; it is NOT an AWS operation.
    pub fn seed_in_transit_message(
        &mut self,
        message_id: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> &mut InTransitMessage {
        self.seed_in_transit_message_in_region(DEFAULT_REGION, message_id, content)
    }

    /// Put a message into transit in `region`, as that region's WorkMail delivery
    /// does. The same id may be in transit in another region: the two are distinct
    /// messages, each addressable only through its own region's endpoint.
    ///
    /// This is the producer this API does not model; it is NOT an AWS operation.
    pub fn seed_in_transit_message_in_region(
        &mut self,
        region: impl Into<String>,
        message_id: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> &mut InTransitMessage {
        let key = MessageKey::new(region, message_id);
        let message = InTransitMessage::new(
            key.region.clone(),
            key.message_id.clone(),
            content.into(),
        );
        self.messages.entry(key).insert_entry(message).into_mut()
    }

    /// The message `message_id` names in `region`'s message flow, or `None` when no
    /// message is in transit under that id in that region — including when one is
    /// in transit under the same id in a different region.
    pub fn message(&self, region: &str, message_id: &str) -> Option<&InTransitMessage> {
        self.messages
            .get(&MessageKey::new(region, message_id))
    }

    /// Replace the MIME content of `message_id` in `region`'s message flow with
    /// `content`, recording the reference it came from, and answer whether such a
    /// message existed. A message in transit in another region is left untouched.
    pub fn replace_message_content(
        &mut self,
        region: &str,
        message_id: &str,
        content: Vec<u8>,
        reference: S3Ref,
    ) -> bool {
        match self.messages.get_mut(&MessageKey::new(region, message_id)) {
            Some(message) => {
                message.content = content;
                message.last_s3_reference = Some(reference);
                message.updated_at = SystemTime::now();
                true
            }
            None => false,
        }
    }
}

/// The region of the request being served: the third field of its SigV4
/// credential scope, falling back to [`DEFAULT_REGION`] for a request that is not
/// signed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWorkmailmessageflowGiraffemessageintransitserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWorkmailmessageflowGiraffemessageintransitserviceState>)
}
