// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WorkMail Message Flow in-transit message surface.
//!
//! Every assertion goes over the wire: a request is built, dispatched through the
//! same [`MultiServiceRouter`] the production emulator mounts, and only the HTTP
//! status, the `x-amzn-errortype` header and the response payload are inspected.
//! State is reached only to SEED it — this service's own store to stand in for
//! WorkMail delivery, which puts a message in transit (a producer this API does not
//! model), and AMAZON S3's store for the object a `PutRawMessageContent` reference
//! names, which is S3's resource and not this service's: an object is staged with
//! S3's own `CreateBucket`/`PutObject` seams, exactly as an S3 client's calls leave
//! it, so what the handler resolves is what S3 holds. The cross-service path over
//! the wire, with both services mounted, is in the router's own suite
//! (`core/router/tests/all/workmailmessageflow_resolves_an_s3_object.rs`).
//!
//! The tests are tables: each row is one input along the axis the rule under test
//! generalises over (id form and length, content form and size, reference
//! versioning, bucket region, bucket owner, MIME defect), and the
//! `00000000-0000-0000-0000-000000000001` case from the bootstrap issue is one row.
//! The two round-trip invariants — what is stored is what is reported — are also
//! checked over a generated spread of ids, byte patterns and MIME messages,
//! computed arithmetically so every run drives exactly the same inputs.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_s3_amazons3::state::{
    service_state as s3_service_state, BucketCreation, Object as S3StoredObject, ObjectDeletion,
    ObjectWrite,
};
use service_com_amazonaws_workmailmessageflow_giraffemessageintransitservice::state::{
    service_state, S3Ref,
    ComAmazonawsWorkmailmessageflowGiraffemessageintransitserviceState as FlowState,
};
use service_com_amazonaws_workmailmessageflow_giraffemessageintransitservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every test signs with unless it is exercising the region rule itself.
pub(crate) const REGION: &str = "us-east-1";

/// `messageId` is `len 1..120, pattern ^[a-z0-9\-]*$` on both operations (model:
/// `smithy_query(query="operation", operation="GetRawMessageContent")`).
const MESSAGE_ID_MIN_LEN: usize = 1;
const MESSAGE_ID_MAX_LEN: usize = 120;

/// The RFC 5321 line limit the `RawMessageContent` documentation cites, including
/// the CRLF.
const MAX_LINE_OCTETS: usize = 1000;

/// How many cases each generated round-trip test runs. Bounded: these are
/// integration tests, not a fuzzing session.
const GENERATED_CASES: usize = 32;

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) error_type: String,
    pub(crate) payload: Bytes,
}

impl Reply {
    /// The payload as JSON, or `Value::Null` when it is not JSON (a
    /// `GetRawMessageContent` success payload is raw MIME, not JSON).
    pub(crate) fn json(&self) -> Value {
        if self.payload.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.payload).unwrap_or(Value::Null)
    }

    /// The `message` an error payload carries, or the empty string when the
    /// payload is not an error document with one. The member is modeled as
    /// `message` with no `jsonName` (`smithy_query(query="shape",
    /// shape="ResourceNotFoundException")`), so that is the wire key.
    pub(crate) fn error_message(&self) -> String {
        self.json()["message"].as_str().unwrap_or_default().to_string()
    }

    /// The payload rendered for an assertion message, without dumping a megabyte
    /// of MIME into the test log.
    pub(crate) fn summary(&self) -> String {
        let shown = String::from_utf8_lossy(&self.payload[..self.payload.len().min(120)]);
        format!(
            "{} byte(s) {shown:?}{}",
            self.payload.len(),
            if self.payload.len() > 120 { "…" } else { "" }
        )
    }
}

/// An emulator with just this service mounted on a fresh engine, driven in-process
/// through the production router.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by
        // the SigV4 credential scope below, which is also where the request's region
        // comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Reach the store, to stand in for one of the two producers this API does not
    /// model. Never used to read a response value: those come off the wire.
    pub(crate) fn with_state<R>(&self, apply: impl FnOnce(&mut FlowState) -> R) -> R {
        let state = service_state(&*self.engine).expect("this service's store is reachable");
        let mut guard = state.lock();
        apply(&mut guard)
    }

    /// Put `content` in transit under `message_id`, as WorkMail delivery does.
    pub(crate) fn seed_message(&self, message_id: &str, content: &[u8]) {
        self.with_state(|state| {
            state.seed_in_transit_message(message_id, content.to_vec());
        });
    }

    /// Stage `object` in AMAZON S3's own store at the bucket and key `reference`
    /// names — as an S3 client's `CreateBucket` and `PutObject` leave it — and
    /// answer the reference that names what was written.
    ///
    /// A `reference` carrying an `objectVersion` asks for a VERSIONING-ENABLED
    /// bucket ([`versioned_reference`]); the id it carries is a placeholder, since a
    /// version id is S3's to mint, and the reference that comes back carries the id
    /// S3 minted for this write. An unversioned one comes back unversioned.
    ///
    /// Nothing of this service's own state is touched: the object belongs to S3,
    /// which is why a reference resolves for any client that can call `PutObject`.
    pub(crate) fn seed_object(&self, reference: &S3Ref, object: StagedObject) -> S3Ref {
        let s3 = s3_service_state(&*self.engine).expect("Amazon S3's store is reachable");
        let mut store = s3.lock();
        let creation = match &object.provisioned_by {
            Some(service) => {
                store.provision_bucket(&reference.bucket, &object.region, service)
            }
            None => store.create_bucket(&reference.bucket, &object.region),
        };
        assert!(
            matches!(creation, BucketCreation::Created | BucketCreation::NameTaken),
            "staging {:?}: expected the bucket to be created or to already exist, got \
             {creation:?}",
            reference.bucket
        );
        let held_region = store
            .bucket(&reference.bucket)
            .map(|bucket| bucket.region.clone());
        assert_eq!(
            held_region.as_deref(),
            Some(object.region.as_str()),
            "staging {:?}: a bucket is in exactly one Region, so stage each Region's objects \
             in their own bucket; got {held_region:?}",
            reference.bucket
        );
        if reference.object_version.is_some() {
            store
                .bucket_mut(&reference.bucket)
                .expect("the bucket just staged is held")
                .set_versioning("Enabled", None);
        }
        let write = store.put_object(
            &reference.bucket,
            &reference.key,
            S3StoredObject::stored(object.content, None),
        );
        assert!(
            matches!(
                write,
                Some(ObjectWrite::Created | ObjectWrite::Replaced)
            ),
            "staging {:?}: expected the object write to be recorded, got {write:?}",
            reference.key
        );
        let minted = store
            .bucket(&reference.bucket)
            .and_then(|bucket| bucket.object(&reference.key))
            .and_then(|stored| stored.version_id.clone());
        assert_eq!(
            minted.is_some(),
            reference.object_version.is_some(),
            "staging {:?}: expected a minted version id exactly for a versioned stage, got \
             {minted:?}",
            reference.key
        );
        S3Ref::new(reference.bucket.clone(), reference.key.clone(), minted)
    }

    /// Lay a DELETE MARKER over `reference`'s key in Amazon S3, as an S3
    /// `DeleteObject` naming no version does, so the key's current version is a
    /// marker and every version behind it is still stored.
    ///
    /// The marker is what the row this feeds turns on, so the outcome is asserted
    /// rather than assumed: a hard removal would make the same row pass for another
    /// reason.
    pub(crate) fn delete_object(&self, reference: &S3Ref) {
        let s3 = s3_service_state(&*self.engine).expect("Amazon S3's store is reachable");
        let mut store = s3.lock();
        let removal = store.remove_object(&reference.bucket, &reference.key);
        assert!(
            matches!(
                removal,
                Some(ObjectDeletion::MarkerLaid {
                    displaced_object: true,
                    ..
                })
            ),
            "deleting {:?}: expected a delete marker over a current object, got {removal:?}",
            reference.key
        );
    }

    async fn send(&self, method: &str, uri: &str, region: &str, body: Option<Vec<u8>>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/workmailmessageflow/aws4_request, \
             SignedHeaders=host, Signature=deadbeef"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("workmailmessageflow.{region}.amazonaws.com"))
            .header("authorization", authorization);
        let request = match body {
            Some(bytes) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(bytes))
                .expect("build request"),
            None => builder.body(Bytes::new()).expect("build request"),
        };

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        Reply {
            status,
            error_type,
            payload: response.into_body(),
        }
    }

    pub(crate) async fn get_raw_message_content(&self, region: &str, message_id: &str) -> Reply {
        self.send("GET", &format!("/messages/{message_id}"), region, None)
            .await
    }

    pub(crate) async fn put_raw_message_content(
        &self,
        region: &str,
        message_id: &str,
        reference: &S3Ref,
    ) -> Reply {
        let mut s3_reference = json!({ "bucket": reference.bucket, "key": reference.key });
        if let Some(version) = &reference.object_version {
            s3_reference["objectVersion"] = json!(version);
        }
        let body = json!({ "content": { "s3Reference": s3_reference } });
        self.send(
            "POST",
            &format!("/messages/{message_id}"),
            region,
            Some(serde_json::to_vec(&body).expect("serialize body")),
        )
        .await
    }
}

/// A well-formed MIME message whose `Subject` is `subject` and whose body is
/// `body`.
///
/// It carries the header fields RFC 5322 §3.6 requires every message to have —
/// `Date` and `From` — because the `RawMessageContent` documentation requires that
/// "the message must contain all the required header fields"
/// (`smithy_query(query="doc", shape="RawMessageContent")`), so content without
/// them is not content an update may install. It carries no `Message-ID` and no
/// `Received` trace, so an update from one of these messages to another changes no
/// immutable header; `crate::mime_header_criteria` is where those are asserted.
pub(crate) fn mime(subject: &str, body: &str) -> Vec<u8> {
    format!(
        "Date: {DATE}\r\nFrom: sender@example.com\r\nSubject: {subject}\r\n\r\n{body}\r\n"
    )
    .into_bytes()
}

/// An RFC 5322 date-time, for the `Date` field every message must carry. Its value
/// is never asserted on: only the field's presence is a documented criterion.
pub(crate) const DATE: &str = "Tue, 24 Oct 2026 17:03:49 +0000";

/// The bucket every test that is not exercising the location rules resolves
/// against. 3..63 chars, `^[a-z0-9][a-z0-9\-]*$` (model: `S3Reference$bucket`).
pub(crate) const BUCKET: &str = "mailbucket";

/// A reference to `key` in [`BUCKET`], unversioned.
///
/// Note the keys these tests use carry no `.`: the model constrains
/// `S3Reference$key` to `^[a-zA-Z0-9\-/]*$` (`smithy_query(query="shape",
/// shape="S3Reference")`), which excludes it, so a key like `updated/message.mime`
/// is refused at the boundary before any handler runs.
pub(crate) fn reference(key: &str) -> S3Ref {
    S3Ref::new(BUCKET, key, None)
}

/// A VERSIONED stage of `key`: a reference into a VERSIONING-ENABLED bucket of its
/// own, named after the key, whose `objectVersion` is a placeholder.
///
/// A bucket of its own because versioning is a property of the BUCKET: every write
/// into a versioning-enabled bucket gets a minted id, so the unversioned stages of
/// a test must not share the bucket with a versioned one.
/// [`Emulator::seed_object`] reads the placeholder as "enable versioning on this
/// bucket" and answers with the version id S3 minted.
pub(crate) fn versioned_reference(key: &str) -> S3Ref {
    S3Ref::new(
        format!("{BUCKET}-v-{}", key.replace('/', "-")),
        key,
        Some(STAGE_A_VERSION.to_string()),
    )
}

/// The placeholder [`versioned_reference`] carries. It is never the id a stage comes
/// back with: S3 mints that, and nothing here can name it in advance.
pub(crate) const STAGE_A_VERSION: &str = "stage-a-version";

/// The bucket a test stages an object in FOR `region`, as a reference to `key` in
/// it.
///
/// One bucket name per region, because a real S3 bucket lives in exactly one
/// Region: a row that needs the same key in two regions needs two buckets, which is
/// the thing a per-object region tag got wrong. The name stays inside
/// `S3Reference$bucket`'s `^[a-z0-9][a-z0-9\-]*$`, since a Region code is lowercase
/// letters, digits and dashes.
pub(crate) fn regional_reference(region: &str, key: &str) -> S3Ref {
    S3Ref::new(format!("{BUCKET}-{region}"), key, None)
}

/// How an object is staged in AMAZON S3's own store: the bytes, the Region of the
/// bucket that holds it, whether that bucket is one another emulated service
/// provisioned (so it is held in a service-managed account rather than the calling
/// one), and whether it is versioning-enabled (so S3 mints a version id for the
/// write).
///
/// Every one of those is a property of the BUCKET or the OBJECT in S3, which is
/// what the handler reads them off.
pub(crate) struct StagedObject {
    content: Vec<u8>,
    region: String,
    provisioned_by: Option<String>,
}

impl StagedObject {
    /// `content` in a bucket of the caller's own account in [`REGION`].
    pub(crate) fn new(content: impl Into<Vec<u8>>) -> Self {
        Self {
            content: content.into(),
            region: REGION.to_string(),
            provisioned_by: None,
        }
    }

    /// The same object in a bucket created in `region`.
    #[must_use]
    pub(crate) fn in_region(mut self, region: &str) -> Self {
        self.region = region.to_string();
        self
    }

    /// The same object in a bucket `service` — another emulated service —
    /// provisioned as the object store of one of ITS resources. S3 leaves such a
    /// bucket out of "the buckets in your account", so for the calling account its
    /// owner is another account.
    #[must_use]
    pub(crate) fn provisioned_by(mut self, service: &str) -> Self {
        self.provisioned_by = Some(service.to_string());
        self
    }
}

// ---------------------------------------------------------------------------
// GetRawMessageContent reports the bytes the store holds.
// ---------------------------------------------------------------------------

/// Every content form, read back byte for byte as the raw HTTP payload with no
/// JSON envelope. The axis is the shape and size of the stored MIME content.
#[tokio::test]
async fn get_reports_the_stored_bytes_verbatim_as_the_raw_payload() {
    let cases: Vec<(&str, String, Vec<u8>)> = vec![
        (
            "the issue's recorded case",
            "00000000-0000-0000-0000-000000000001".to_string(),
            b"From: a@example.com\r\nSubject: original\r\n\r\nbody".to_vec(),
        ),
        (
            "headers only, no body",
            "headers-only".to_string(),
            b"Subject: headers only\r\n".to_vec(),
        ),
        (
            "multipart",
            "multipart-0".to_string(),
            concat!(
                "From: sender@example.com\r\n",
                "Subject: multipart\r\n",
                "MIME-Version: 1.0\r\n",
                "Content-Type: multipart/mixed; boundary=\"b0\"\r\n",
                "\r\n",
                "--b0\r\n",
                "Content-Type: text/plain\r\n\r\n",
                "first part\r\n",
                "--b0\r\n",
                "Content-Type: text/plain\r\n\r\n",
                "second part\r\n",
                "--b0--\r\n",
            )
            .as_bytes()
            .to_vec(),
        ),
        (
            "8-bit body bytes survive unchanged",
            "eight-bit".to_string(),
            {
                let mut content =
                    b"From: a@example.com\r\nSubject: binary\r\n\r\n".to_vec();
                content.extend((0u8..=255).rev());
                content
            },
        ),
        (
            "minimum-length id",
            "a".repeat(MESSAGE_ID_MIN_LEN),
            mime("min id", "body"),
        ),
        (
            "maximum-length id",
            "m".repeat(MESSAGE_ID_MAX_LEN),
            mime("max id", "body"),
        ),
        ("large content", "large-0".to_string(), {
            let mut content = b"From: a@example.com\r\nSubject: large\r\n\r\n".to_vec();
            for index in 0..1024 {
                content.extend(format!("line {index} {}\r\n", "x".repeat(60)).as_bytes());
            }
            content
        }),
    ];

    for (case, message_id, content) in cases {
        let message_id = message_id.as_str();
        let emulator = Emulator::new();
        emulator.seed_message(message_id, &content);

        let reply = emulator.get_raw_message_content(REGION, message_id).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} ({})",
            reply.status,
            reply.summary()
        );
        assert_eq!(
            reply.payload.as_ref(),
            content.as_slice(),
            "case {case:?}: expected the stored bytes back verbatim, got {}",
            reply.summary()
        );

        // A read does not change what the next caller sees.
        let again = emulator.get_raw_message_content(REGION, message_id).await;
        assert_eq!(
            again.payload.as_ref(),
            content.as_slice(),
            "case {case:?}: a second read reported something else: {}",
            again.summary()
        );
    }
}

/// A message that is no longer eligible for update is still readable: freezing
/// stops `PutRawMessageContent`, not `GetRawMessageContent`.
#[tokio::test]
async fn get_reports_a_frozen_message_too() {
    let emulator = Emulator::new();
    let content = mime("frozen", "body");
    emulator.with_state(|state| {
        state.seed_in_transit_message("frozen-0", content.clone()).frozen = true;
    });

    let reply = emulator.get_raw_message_content(REGION, "frozen-0").await;
    assert_eq!(
        reply.status, 200,
        "case \"frozen\": expected HTTP 200, got {} ({})",
        reply.status,
        reply.summary()
    );
    assert_eq!(
        reply.payload.as_ref(),
        content.as_slice(),
        "case \"frozen\": expected the stored bytes, got {}",
        reply.summary()
    );
}

/// An id no message is stored under is a 404 `ResourceNotFoundException`, whatever
/// form the id takes.
#[tokio::test]
async fn get_of_an_unknown_message_is_not_found() {
    let emulator = Emulator::new();
    emulator.seed_message("present-0", &mime("present", "body"));

    let cases = [
        ("the issue's unknown id", "does-not-exist".to_string()),
        ("one character", "z".to_string()),
        ("digits", "123456789".to_string()),
        ("dashes only", "---".to_string()),
        ("a near miss on a stored id", "present-1".to_string()),
        ("maximum-length unknown id", "q".repeat(MESSAGE_ID_MAX_LEN)),
    ];

    for (case, message_id) in cases {
        let reply = emulator.get_raw_message_content(REGION, &message_id).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} ({})",
            reply.status,
            reply.summary()
        );
        assert_eq!(
            reply.error_type, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.error_message(),
            format!("Unknown messageId: {message_id}"),
            "case {case:?}: expected the unknown-messageId message for {message_id:?}, got {:?}",
            reply.error_message()
        );
    }
}

// ---------------------------------------------------------------------------
// PutRawMessageContent resolves the reference and replaces the stored content.
// ---------------------------------------------------------------------------

/// The acceptance demo, over a table: seed, read the original, put a reference to
/// different content, read the new content. The axis is the content being
/// installed and the form of the reference that names it.
#[tokio::test]
async fn put_replaces_the_stored_content_and_the_next_get_reports_it() {
    let cases: Vec<(&str, S3Ref, Vec<u8>)> = vec![
        (
            "the issue's recorded case",
            reference("updated/message-mime"),
            format!("Date: {DATE}\r\nFrom: a@example.com\r\nSubject: rewritten").into_bytes(),
        ),
        (
            "a versioned reference",
            versioned_reference("updated/message-mime"),
            mime("rewritten at the version S3 minted", "body"),
        ),
        (
            "content shorter than the original",
            reference("short"),
            format!("Date: {DATE}\r\nFrom: a@b\r\n").into_bytes(),
        ),
        ("content much longer than the original", reference("long"), {
            let mut content =
                format!("Date: {DATE}\r\nFrom: a@example.com\r\nSubject: long\r\n\r\n")
                    .into_bytes();
            for index in 0..512 {
                content.extend(format!("paragraph {index}\r\n").as_bytes());
            }
            content
        }),
        ("multipart content", reference("multipart/object"), {
            let mut content = format!("Date: {DATE}\r\n").into_bytes();
            content.extend_from_slice(
                concat!(
                    "From: sender@example.com\r\n",
                    "Subject: rewritten multipart\r\n",
                    "MIME-Version: 1.0\r\n",
                    "Content-Type: multipart/alternative; boundary=\"zz\"\r\n",
                    "\r\n",
                    "--zz\r\n",
                    "Content-Type: text/plain\r\n\r\n",
                    "text\r\n",
                    "--zz\r\n",
                    "Content-Type: text/html\r\n\r\n",
                    "<p>html</p>\r\n",
                    "--zz--\r\n",
                )
                .as_bytes(),
            );
            content
        }),
        (
            "a line exactly at the RFC 5321 limit",
            reference("at-the-limit"),
            padded_line_message(MAX_LINE_OCTETS),
        ),
        (
            "a bucket name at the modeled minimum length",
            S3Ref::new("abc", "k", None),
            mime("tiny bucket", "body"),
        ),
    ];

    for (case, s3_reference, replacement) in cases {
        let emulator = Emulator::new();
        let message_id = "00000000-0000-0000-0000-000000000001";
        let original = b"From: a@example.com\r\nSubject: original\r\n\r\nbody".to_vec();
        emulator.seed_message(message_id, &original);
        let s3_reference = emulator.seed_object(&s3_reference, StagedObject::new(replacement.clone()));

        let before = emulator.get_raw_message_content(REGION, message_id).await;
        assert_eq!(
            before.payload.as_ref(),
            original.as_slice(),
            "case {case:?}: expected the seeded content before the update, got {}",
            before.summary()
        );

        let put = emulator
            .put_raw_message_content(REGION, message_id, &s3_reference)
            .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected HTTP 200 from the update, got {} ({})",
            put.status,
            put.summary()
        );
        // `PutRawMessageContentResponse` has no members, so the framework renders
        // it as the empty restJson1 document.
        assert!(
            put.payload.is_empty() || put.json() == json!({}),
            "case {case:?}: expected a response carrying no members, got {}",
            put.summary()
        );

        let after = emulator.get_raw_message_content(REGION, message_id).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: expected HTTP 200 after the update, got {} ({})",
            after.status,
            after.summary()
        );
        assert_eq!(
            after.payload.as_ref(),
            replacement.as_slice(),
            "case {case:?}: expected the resolved object's bytes, got {}",
            after.summary()
        );
        assert_ne!(
            after.payload.as_ref(),
            original.as_slice(),
            "case {case:?}: the update did not change the stored content"
        );
    }
}

/// The accepted reference is retained whole, object version included, so the
/// update that produced the current content stays observable. Only the two modeled
/// operations are on the wire, and neither reports it, so this one assertion reads
/// the store.
#[tokio::test]
async fn an_accepted_reference_is_retained_with_its_object_version() {
    let cases = [
        ("unversioned", reference("updated/object")),
        ("versioned", versioned_reference("updated/object")),
    ];

    for (case, staged) in cases {
        let emulator = Emulator::new();
        let message_id = "retained-0";
        emulator.seed_message(message_id, &mime("original", "body"));
        let s3_reference = emulator.seed_object(&staged, StagedObject::new(mime("rewritten", "body")));
        assert_eq!(
            s3_reference.object_version.is_some(),
            staged.object_version.is_some(),
            "case {case:?}: expected the staged reference to be versioned exactly when it was \
             asked to be, got {:?}",
            s3_reference.object_version
        );

        let put = emulator
            .put_raw_message_content(REGION, message_id, &s3_reference)
            .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected HTTP 200, got {} ({})",
            put.status,
            put.summary()
        );

        let retained = emulator.with_state(|state| {
            state
                .message(REGION, message_id)
                .and_then(|message| message.last_s3_reference.clone())
        });
        assert_eq!(
            retained,
            Some(s3_reference.clone()),
            "case {case:?}: expected the accepted reference to be retained whole, got {retained:?}"
        );
    }
}

/// An update to a message that is not in transit is a 404, and nothing is written.
#[tokio::test]
async fn put_to_an_unknown_message_is_not_found() {
    let emulator = Emulator::new();
    let s3_reference =
        emulator.seed_object(&reference("updated/object"), StagedObject::new(mime("rewritten", "body")));

    let cases = [
        ("the issue's unknown id", "does-not-exist".to_string()),
        ("one character", "q".to_string()),
        ("maximum-length unknown id", "w".repeat(MESSAGE_ID_MAX_LEN)),
    ];

    for (case, message_id) in cases {
        let reply = emulator
            .put_raw_message_content(REGION, &message_id, &s3_reference)
            .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} ({})",
            reply.status,
            reply.summary()
        );
        assert_eq!(
            reply.error_type, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.error_message(),
            format!("Unknown messageId: {message_id}"),
            "case {case:?}: expected the unknown-messageId message for {message_id:?}, got {:?}",
            reply.error_message()
        );
        let readback = emulator.get_raw_message_content(REGION, &message_id).await;
        assert_eq!(
            readback.status, 404,
            "case {case:?}: the refused update created a message: {}",
            readback.summary()
        );
    }
}

// ---------------------------------------------------------------------------
// The message a not-in-transit id is refused with, on every path that refuses it.
// ---------------------------------------------------------------------------

/// `ResourceNotFoundException` names the id that was rejected: its `message` is
/// the fixed prefix `Unknown messageId: ` followed verbatim by the request's
/// `messageId`, on every operation and every path that raises the error.
///
/// The axis is the form and length of the rejected id (the whole permitted range
/// of `len 1..120, ^[a-z0-9\-]*$`), crossed with the four paths that can raise
/// it: the read, an update whose reference resolves, an update whose reference
/// does not (so the id check precedes the content-location check), an update
/// whose reference is versioned, and the independent follow-up read after a
/// refused update.
#[tokio::test]
async fn an_unknown_message_id_is_refused_with_the_id_it_was_asked_about() {
    let staged_resolvable = reference("updated/object");
    let staged_versioned = versioned_reference("updated/versioned");
    // A reference nothing is stored at: if the content-location check ran first
    // this row would answer InvalidContentLocation instead.
    let unresolvable = reference("never/stored");

    let ids = [
        ("a uuid-shaped id", "8e220c71-0e24-437a-98b8-f83d6c64d1c8".to_string()),
        (
            "a long synthetic id",
            "sim2real-wmf-x-5cabbe533e2d-0b21d79406ff4151aa42c6fa8d23e4a6".to_string(),
        ),
        ("the shortest permitted id", "a".to_string()),
        ("a single dash", "-".to_string()),
        ("digits only", "0123456789".to_string()),
        ("dashes only", "----------".to_string()),
        ("the longest permitted id", "z".repeat(MESSAGE_ID_MAX_LEN)),
        ("a near miss on a stored id", "present-1".to_string()),
    ];

    for (case, message_id) in ids {
        let emulator = Emulator::new();
        // A message IS in transit under another id, so the rows below are refused
        // for the id they name and not because the store is empty.
        emulator.seed_message("present-0", &mime("present", "body"));
        let resolvable =
            emulator.seed_object(&staged_resolvable, StagedObject::new(mime("rewritten", "body")));
        let versioned = emulator.seed_object(
            &staged_versioned,
            StagedObject::new(mime("rewritten at a version", "body")),
        );

        let expected = format!("Unknown messageId: {message_id}");
        let paths: Vec<(&str, Reply)> = vec![
            (
                "GetRawMessageContent",
                emulator.get_raw_message_content(REGION, &message_id).await,
            ),
            (
                "PutRawMessageContent with a resolvable reference",
                emulator
                    .put_raw_message_content(REGION, &message_id, &resolvable)
                    .await,
            ),
            (
                "PutRawMessageContent with an unresolvable reference",
                emulator
                    .put_raw_message_content(REGION, &message_id, &unresolvable)
                    .await,
            ),
            (
                "PutRawMessageContent with a versioned reference",
                emulator
                    .put_raw_message_content(REGION, &message_id, &versioned)
                    .await,
            ),
            (
                "the follow-up GetRawMessageContent",
                emulator.get_raw_message_content(REGION, &message_id).await,
            ),
        ];

        for (path, reply) in paths {
            assert_eq!(
                reply.status, 404,
                "case {case:?} on {path}: expected HTTP 404, got {} ({})",
                reply.status,
                reply.summary()
            );
            assert_eq!(
                reply.error_type, "ResourceNotFoundException",
                "case {case:?} on {path}: expected ResourceNotFoundException, got {:?} ({})",
                reply.error_type,
                reply.summary()
            );
            assert_eq!(
                reply.error_message(),
                expected,
                "case {case:?} on {path}: expected {expected:?}, got {:?}",
                reply.error_message()
            );
        }
    }

    // The control: the id that IS in transit is not refused at all, so the rows
    // above answer the not-found rule rather than a blanket 404.
    let emulator = Emulator::new();
    emulator.seed_message("present-0", &mime("present", "body"));
    let reply = emulator.get_raw_message_content(REGION, "present-0").await;
    assert_eq!(
        reply.status, 200,
        "case \"control\": expected HTTP 200 for an id in transit, got {} ({})",
        reply.status,
        reply.summary()
    );
}

/// A frozen message refuses the update with `MessageFrozen` and keeps its content.
#[tokio::test]
async fn put_to_a_frozen_message_is_refused_and_changes_nothing() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("plain", mime("frozen plain", "body")),
        ("headers only", b"Subject: frozen\r\n".to_vec()),
    ];

    for (case, original) in cases {
        let emulator = Emulator::new();
        let message_id = "frozen-1";
        emulator.with_state(|state| {
            state
                .seed_in_transit_message(message_id, original.clone())
                .frozen = true;
        });
        let s3_reference = emulator.seed_object(
            &reference("updated/object"),
            StagedObject::new(mime("rewritten", "body")),
        );

        let reply = emulator
            .put_raw_message_content(REGION, message_id, &s3_reference)
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} ({})",
            reply.status,
            reply.summary()
        );
        assert_eq!(
            reply.error_type, "MessageFrozen",
            "case {case:?}: expected MessageFrozen, got {:?}",
            reply.error_type
        );

        let after = emulator.get_raw_message_content(REGION, message_id).await;
        assert_eq!(
            after.payload.as_ref(),
            original.as_slice(),
            "case {case:?}: the refused update changed the stored content: {}",
            after.summary()
        );
    }
}

/// What one emulator holds in AMAZON S3 for the unreadable-location rows below,
/// each in a bucket of its own: a Region and an owning account are properties of a
/// BUCKET, so no two of these can be the same bucket.
pub(crate) struct StagedObjects {
    /// A readable object in a bucket of the caller's account in [`REGION`].
    pub(crate) readable: S3Ref,
    /// A readable object in a versioning-enabled bucket, at the version S3 minted.
    pub(crate) versioned: S3Ref,
    /// A readable object in a bucket in another Region.
    pub(crate) elsewhere: S3Ref,
    /// A readable object in a bucket another emulated service provisioned, so S3
    /// holds it outside the calling account's own buckets.
    pub(crate) service_managed: S3Ref,
    /// A versioned key whose CURRENT version is a delete marker: every version
    /// behind it is still stored, and S3 behaves as though the key held nothing.
    pub(crate) deleted: S3Ref,
}

impl StagedObjects {
    /// Stage all five in `emulator`'s Amazon S3 store.
    pub(crate) fn stage(emulator: &Emulator) -> Self {
        let deleted = emulator.seed_object(
            &versioned_reference("deleted/object"),
            StagedObject::new(mime("deleted", "body")),
        );
        emulator.delete_object(&deleted);
        Self {
            readable: emulator.seed_object(
                &reference("updated/message-mime"),
                StagedObject::new(mime("rewritten", "body")),
            ),
            versioned: emulator.seed_object(
                &versioned_reference("versioned/object"),
                StagedObject::new(mime("rewritten at a version", "body")),
            ),
            elsewhere: emulator.seed_object(
                &regional_reference("eu-west-1", "elsewhere/object"),
                StagedObject::new(mime("elsewhere", "body")).in_region("eu-west-1"),
            ),
            service_managed: emulator.seed_object(
                &S3Ref::new("mailbucket-service-managed", "managed/object", None),
                StagedObject::new(mime("someone else's", "body"))
                    .provisioned_by("com.amazonaws.lightsail"),
            ),
            deleted,
        }
    }
}

/// One unreadable-location row: its name and the reference the request carries,
/// computed from what Amazon S3 holds — a version id is S3's to mint, so a row that
/// names one cannot be a constant.
type UnreadableRow = (&'static str, fn(&StagedObjects) -> S3Ref);

/// Every way a reference can fail to resolve to a readable object in Amazon S3
/// answers `InvalidContentLocation` and leaves the message alone. The rows are the
/// documented reasons: nothing stored there, the bucket's region is not the
/// request's region, and the bucket owner is not the calling account.
#[tokio::test]
async fn put_with_an_unreadable_object_is_an_invalid_content_location() {
    let rows: Vec<UnreadableRow> = vec![
        ("the issue's missing key", |_| reference("missing/object")),
        ("a bucket S3 does not hold", |staged| {
            S3Ref::new("otherbucket", staged.readable.key.clone(), None)
        }),
        ("a version no write minted", |staged| {
            S3Ref::new(
                staged.versioned.bucket.clone(),
                staged.versioned.key.clone(),
                Some("not-a-minted-version".to_string()),
            )
        }),
        ("a key whose current version is a delete marker", |staged| {
            S3Ref::new(staged.deleted.bucket.clone(), staged.deleted.key.clone(), None)
        }),
        ("a bucket in another region", |staged| staged.elsewhere.clone()),
        ("a bucket held in another account", |staged| {
            staged.service_managed.clone()
        }),
    ];

    for (case, row) in rows {
        let emulator = Emulator::new();
        let message_id = "00000000-0000-0000-0000-000000000001";
        let original = mime("original", "body");
        emulator.seed_message(message_id, &original);
        let staged = StagedObjects::stage(&emulator);
        let s3_reference = row(&staged);

        let reply = emulator
            .put_raw_message_content(REGION, message_id, &s3_reference)
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} ({})",
            reply.status,
            reply.summary()
        );
        assert_eq!(
            reply.error_type, "InvalidContentLocation",
            "case {case:?}: expected InvalidContentLocation, got {:?} ({})",
            reply.error_type,
            reply.summary()
        );

        let after = emulator.get_raw_message_content(REGION, message_id).await;
        assert_eq!(
            after.payload.as_ref(),
            original.as_slice(),
            "case {case:?}: the refused update changed the stored content: {}",
            after.summary()
        );
    }

    // The controls: the two objects the rows above name indirectly are accepted
    // when referenced as S3 holds them — so each row fails for the reason under
    // test and not because the fixture is broken.
    for (case, pick) in [
        ("the readable object", 0usize),
        ("the versioned object", 1usize),
    ] {
        let emulator = Emulator::new();
        emulator.seed_message("control-0", &mime("original", "body"));
        let staged = StagedObjects::stage(&emulator);
        let s3_reference = if pick == 0 {
            staged.readable.clone()
        } else {
            staged.versioned.clone()
        };
        let reply = emulator
            .put_raw_message_content(REGION, "control-0", &s3_reference)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} ({})",
            reply.status,
            reply.summary()
        );
    }
}

/// An object in the request's own region resolves; the same object in any other
/// region does not. The axis is the region pair.
#[tokio::test]
async fn an_object_resolves_only_in_the_region_the_request_was_made_in() {
    let cases = [
        ("us-east-1", "us-east-1", true),
        ("us-west-2", "us-west-2", true),
        ("eu-central-1", "eu-central-1", true),
        ("us-east-1", "us-west-2", false),
        ("eu-west-1", "us-east-1", false),
        ("ap-southeast-2", "eu-central-1", false),
    ];

    for (request_region, bucket_region, expected_ok) in cases {
        let case = format!("request in {request_region}, bucket in {bucket_region}");
        let emulator = Emulator::new();
        let message_id = "region-0";
        let original = mime("original", "body");
        let replacement = mime("rewritten", "body");
        emulator.with_state(|state| {
            // The message is in transit in the region the request is made in, so
            // this row turns only on the bucket's region.
            state.seed_in_transit_message_in_region(request_region, message_id, original.clone());
        });
        let s3_reference = emulator.seed_object(
            &regional_reference(bucket_region, "updated/object"),
            StagedObject::new(replacement.clone()).in_region(bucket_region),
        );

        let reply = emulator
            .put_raw_message_content(request_region, message_id, &s3_reference)
            .await;
        let after = emulator
            .get_raw_message_content(request_region, message_id)
            .await;
        if expected_ok {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200, got {} ({})",
                reply.status,
                reply.summary()
            );
            assert_eq!(
                after.payload.as_ref(),
                replacement.as_slice(),
                "case {case:?}: expected the resolved bytes, got {}",
                after.summary()
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected HTTP 400, got {} ({})",
                reply.status,
                reply.summary()
            );
            assert_eq!(
                reply.error_type, "InvalidContentLocation",
                "case {case:?}: expected InvalidContentLocation, got {:?}",
                reply.error_type
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

/// A message whose MIME content breaks one of the documented criteria is rejected
/// with `MessageRejected`, and the stored content is untouched. Each row is one
/// criterion from the `RawMessageContent` documentation; the accepted rows pin the
/// published bound from the inside.
#[tokio::test]
async fn put_with_unusable_mime_content_is_rejected() {
    let refused: Vec<(&str, Vec<u8>)> = vec![
        ("an empty object", Vec::new()),
        (
            "no header field at all",
            b"this is not a message at all\r\n".to_vec(),
        ),
        ("a header field with an empty name", b": no name\r\n\r\nbody".to_vec()),
        (
            "a header name containing a space",
            b"Bad Header Name\r\n\r\nbody".to_vec(),
        ),
        (
            "one octet past the RFC 5321 line limit",
            padded_line_message(MAX_LINE_OCTETS + 1),
        ),
        (
            "far past the RFC 5321 line limit",
            padded_line_message(MAX_LINE_OCTETS * 4),
        ),
        (
            "a duplicated Subject header",
            format!("Date: {DATE}\r\nFrom: a@example.com\r\nSubject: one\r\nSubject: two\r\n\r\nbody\r\n")
                .into_bytes(),
        ),
        (
            "a duplicated From header",
            format!(
                "Date: {DATE}\r\nFrom: a@example.com\r\nFrom: b@example.com\r\nSubject: s\r\n\r\nbody\r\n"
            )
            .into_bytes(),
        ),
        (
            "a duplicated Message-ID header, in different cases",
            format!(
                "Date: {DATE}\r\nFrom: a@example.com\r\nmessage-id: <1@example.com>\r\nMESSAGE-ID: <2@example.com>\r\n\r\nbody\r\n"
            )
            .into_bytes(),
        ),
        (
            "a duplicated Subject header, the second one folded over two lines",
            format!(
                "Date: {DATE}\r\nFrom: a@example.com\r\nSubject: one\r\nSubject: two\r\n and more\r\n\r\nbody\r\n"
            )
            .into_bytes(),
        ),
        (
            "the shortest possible header-only message, which carries no required field",
            b"a:\r\n".to_vec(),
        ),
    ];

    for (case, content) in refused {
        let emulator = Emulator::new();
        let message_id = "mime-0";
        let original = mime("original", "body");
        emulator.seed_message(message_id, &original);
        let s3_reference = emulator
            .seed_object(&reference("updated/object"), StagedObject::new(content));

        let reply = emulator
            .put_raw_message_content(REGION, message_id, &s3_reference)
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} ({})",
            reply.status,
            reply.summary()
        );
        assert_eq!(
            reply.error_type, "MessageRejected",
            "case {case:?}: expected MessageRejected, got {:?} ({})",
            reply.error_type,
            reply.summary()
        );
        assert!(
            !reply.json()["message"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {case:?}: expected a message saying what caused the rejection, got {}",
            reply.summary()
        );

        let after = emulator.get_raw_message_content(REGION, message_id).await;
        assert_eq!(
            after.payload.as_ref(),
            original.as_slice(),
            "case {case:?}: the rejected update changed the stored content: {}",
            after.summary()
        );
    }

    let accepted: Vec<(&str, Vec<u8>)> = vec![
        ("a line exactly at the limit", padded_line_message(MAX_LINE_OCTETS)),
        (
            "a line one octet inside the limit",
            padded_line_message(MAX_LINE_OCTETS - 1),
        ),
        (
            "the shortest message carrying both required header fields",
            format!("Date: {DATE}\r\nFrom: a@b\r\n").into_bytes(),
        ),
        (
            "a header that may repeat, repeated",
            format!(
                "Date: {DATE}\r\nFrom: a@example.com\r\nComments: one\r\nComments: two\r\n\r\nbody\r\n"
            )
            .into_bytes(),
        ),
        (
            "a folded continuation line is not a second header",
            format!(
                "Date: {DATE}\r\nFrom: a@example.com\r\nSubject: one\r\n continued here\r\n\r\nbody\r\n"
            )
            .into_bytes(),
        ),
    ];
    for (case, content) in accepted {
        let emulator = Emulator::new();
        let message_id = "mime-1";
        emulator.seed_message(message_id, &mime("original", "body"));
        let s3_reference = emulator
            .seed_object(&reference("updated/object"), StagedObject::new(content.clone()));

        let reply = emulator
            .put_raw_message_content(REGION, message_id, &s3_reference)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} ({})",
            reply.status,
            reply.summary()
        );
        let after = emulator.get_raw_message_content(REGION, message_id).await;
        assert_eq!(
            after.payload.as_ref(),
            content.as_slice(),
            "case {case:?}: expected the accepted content back, got {}",
            after.summary()
        );
    }
}

/// A message with one header line of exactly `octets` octets, counting its CRLF.
fn padded_line_message(octets: usize) -> Vec<u8> {
    let prefix = "X-Pad: ";
    let padding = octets
        .saturating_sub(prefix.len() + "\r\n".len())
        .max(1);
    format!(
        "Date: {DATE}\r\nFrom: a@example.com\r\n{prefix}{}\r\n\r\nbody\r\n",
        "a".repeat(padding)
    )
    .into_bytes()
}

// ---------------------------------------------------------------------------
// The round-trip invariants, over a generated spread of inputs.
// ---------------------------------------------------------------------------

/// What the store holds is what `GetRawMessageContent` reports, for any bytes: the
/// operation returns the payload verbatim and never interprets it.
///
/// The cases are generated rather than listed, so the spread covers lengths from
/// empty to a few kilobytes and every byte value, not a handful of hand-picked
/// strings. They are generated arithmetically (no generator crate is installed),
/// so every run drives exactly the same inputs and a failure names its case index.
#[tokio::test]
async fn get_round_trips_every_generated_byte_pattern() {
    for case in 0..GENERATED_CASES {
        let message_id = case_message_id(case);
        let content = case_bytes(case);
        let emulator = Emulator::new();
        emulator.seed_message(&message_id, &content);

        let reply = emulator.get_raw_message_content(REGION, &message_id).await;
        assert_eq!(
            reply.status, 200,
            "case {case} (id {message_id:?}, {} byte(s)): expected HTTP 200, got {} ({})",
            content.len(),
            reply.status,
            reply.summary()
        );
        assert_eq!(
            reply.payload.as_ref(),
            content.as_slice(),
            "case {case} (id {message_id:?}): expected the stored bytes back, got {}",
            reply.summary()
        );
    }
}

/// What `PutRawMessageContent` resolves is what the next `GetRawMessageContent`
/// reports, for any MIME content: the update installs the object's bytes unchanged.
#[tokio::test]
async fn put_then_get_round_trips_every_generated_mime_message() {
    for case in 0..GENERATED_CASES {
        let message_id = case_message_id(case);
        let original = case_mime(case);
        let replacement = case_mime(case + GENERATED_CASES);
        let staged = if case % 2 == 0 {
            // Half the cases resolve a version S3 minted, half the key's current
            // version.
            versioned_reference(&format!("objects/{case}"))
        } else {
            reference(&format!("objects/{case}"))
        };

        let emulator = Emulator::new();
        emulator.seed_message(&message_id, &original);
        let s3_reference = emulator.seed_object(&staged, StagedObject::new(replacement.clone()));

        let put = emulator
            .put_raw_message_content(REGION, &message_id, &s3_reference)
            .await;
        assert_eq!(
            put.status, 200,
            "case {case} (id {message_id:?}, {} byte(s)): expected HTTP 200, got {} ({})",
            replacement.len(),
            put.status,
            put.summary()
        );

        let after = emulator.get_raw_message_content(REGION, &message_id).await;
        assert_eq!(
            after.payload.as_ref(),
            replacement.as_slice(),
            "case {case} (id {message_id:?}): expected the resolved bytes, got {}",
            after.summary()
        );
    }
}

/// The id for case `index`, inside the modeled constraint: `len 1..120`,
/// `^[a-z0-9\-]*$`. The length walks the whole permitted range as `index` grows.
fn case_message_id(index: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789-";
    let span = MESSAGE_ID_MAX_LEN - MESSAGE_ID_MIN_LEN + 1;
    let length = MESSAGE_ID_MIN_LEN + (index * 37) % span;
    (0..length)
        .map(|position| ALPHABET[(position * 13 + index * 5) % ALPHABET.len()] as char)
        .collect()
}

/// The bytes for case `index`: a length between 0 and a few kilobytes, filled with
/// a byte sequence that walks all 256 values.
fn case_bytes(index: usize) -> Vec<u8> {
    let length = (index * 271) % 4096;
    (0..length)
        .map(|position| ((position * 31 + index * 17) % 256) as u8)
        .collect()
}

/// A well-formed MIME message whose body is [`case_bytes`] mapped into printable
/// US-ASCII and folded into lines well inside the RFC 5321 limit.
fn case_mime(index: usize) -> Vec<u8> {
    let mut content =
        format!("Date: {DATE}\r\nFrom: a@example.com\r\nSubject: case {index}\r\n\r\n")
            .into_bytes();
    for chunk in case_bytes(index).chunks(64) {
        content.extend(chunk.iter().map(|byte| 33 + (byte % 94)));
        content.extend(b"\r\n");
    }
    content
}
