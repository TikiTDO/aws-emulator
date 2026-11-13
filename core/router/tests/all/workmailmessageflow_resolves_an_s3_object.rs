// SPDX-License-Identifier: CC0-1.0

//! What WorkMail Message Flow's `PutRawMessageContent` resolves, across the one
//! emulator: an object AMAZON S3 holds.
//!
//! RULE: the MIME content an update installs is the bytes S3 holds for the object the
//! request's `s3Reference` names ("Amazon S3 object representing the updated message
//! content, in MIME format" — `S3Reference`'s own documentation), so the way a client
//! makes a reference resolvable is to call S3 `PutObject` and to attach the bucket
//! policy that grants WorkMail the read ("for WorkMail to process an S3 object, it must
//! have permission to access that object"), and the three documented
//! `InvalidContentLocation` reasons are properties of the real bucket: a bucket in
//! another Region than the request, a bucket another account owns, and a bucket whose
//! policy does not grant that read.
//!
//! No single crate's test target can ask this: the object has to be written through
//! Amazon S3's own API and read back through WorkMail Message Flow's, so both
//! services are mounted here on the one engine the router mounts them on. Every step
//! goes over the wire — the upload is an S3 `CreateBucket`/`PutObject`, the update and
//! the read are the two modeled WorkMail Message Flow operations — and the only state
//! reached directly is the in-transit message itself, which WorkMail delivery puts
//! there and this API does not model.
//!
//! The rows differ along what the rule generalises over: the bucket, the key, the
//! bytes, whether the reference names a version S3 minted, whether the bucket is
//! in the Region the update was made in, and whether its policy grants the read.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call is signed for unless a row is exercising the Region rule.
const REGION: &str = "us-east-1";

/// The status Amazon S3 answers a stored `PutBucketPolicy` with: the write has no body to
/// return, so S3 reports `204 No Content`.
const BUCKET_POLICY_STORED: u16 = 204;

/// One wire answer, reduced to what the next step branches on.
struct Answer {
    status: u16,
    error_type: String,
    version_id: String,
    body: Bytes,
}

impl Answer {
    /// The body rendered for an assertion message, without dumping a whole message
    /// into the log.
    fn summary(&self) -> String {
        let shown = String::from_utf8_lossy(&self.body[..self.body.len().min(120)]);
        format!(
            "{} byte(s) {shown:?}{}",
            self.body.len(),
            if self.body.len() > 120 { "…" } else { "" }
        )
    }
}

/// Amazon S3 and WorkMail Message Flow mounted on ONE engine, exactly as the router
/// mounts them, with request authentication off so each call carries a credential
/// scope (which is what names the Region and the service) without a computed
/// signature.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_workmailmessageflow_giraffemessageintransitservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// Put `content` in transit under `message_id` in `region`'s message flow, as
    /// WorkMail delivery does. The only producer of an in-transit message, and not
    /// an operation this API models.
    fn seed_message(&self, region: &str, message_id: &str, content: &[u8]) {
        let state = service_com_amazonaws_workmailmessageflow_giraffemessageintransitservice::state::service_state(
            &*self.engine,
        )
        .expect("the WorkMail Message Flow store is reachable");
        state
            .lock()
            .seed_in_transit_message_in_region(region, message_id, content.to_vec());
    }

    /// One call of `method` to `path` on `signing_name`'s endpoint in `region`.
    async fn call(
        &self,
        method: &str,
        signing_name: &str,
        region: &str,
        path: &str,
        body: Vec<u8>,
    ) -> Answer {
        let host = format!("{signing_name}.{region}.amazonaws.com");
        let request = ::http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", host)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/aws4_request, \
                     SignedHeaders=host, Signature=not-verified"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        let error_type = header("x-amzn-errortype");
        let version_id = header("x-amz-version-id");
        Answer {
            status: response.status().as_u16(),
            error_type,
            version_id,
            body: response.into_body(),
        }
    }

    /// S3 `CreateBucket`: `region` is the bucket's Region, named both by the endpoint
    /// the create is sent to and by the location constraint it carries, which is what
    /// a regional endpoint serves.
    async fn create_bucket(&self, bucket: &str, region: &str) -> Answer {
        let body = if region == "us-east-1" {
            String::new()
        } else {
            format!(
                "<CreateBucketConfiguration><LocationConstraint>{region}\
                 </LocationConstraint></CreateBucketConfiguration>"
            )
        };
        self.call("PUT", "s3", region, &format!("/{bucket}"), body.into_bytes())
            .await
    }

    /// S3 `PutBucketVersioning` with status `Enabled`, so every later write into the
    /// bucket gets a version id of its own.
    async fn enable_versioning(&self, bucket: &str, region: &str) -> Answer {
        self.call(
            "PUT",
            "s3",
            region,
            &format!("/{bucket}?versioning"),
            b"<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>".to_vec(),
        )
        .await
    }

    /// S3 `PutBucketPolicy`: the grant a client has to attach before WorkMail can read
    /// the object, since WorkMail fetches the content as its own service principal and no
    /// identity-based policy of the calling account speaks for it. The document is the one
    /// WorkMail's own instructions name — an `Allow` of `s3:GetObject` on the bucket's
    /// objects to WorkMail's service principal.
    async fn put_bucket_policy(&self, bucket: &str, region: &str) -> Answer {
        let policy = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Sid": "AllowWorkMailToReadMessageContent",
                "Effect": "Allow",
                "Principal": { "Service": "workmail.amazonaws.com" },
                "Action": "s3:GetObject",
                "Resource": format!("arn:aws:s3:::{bucket}/*"),
            }],
        });
        self.call(
            "PUT",
            "s3",
            region,
            &format!("/{bucket}?policy"),
            serde_json::to_vec(&policy).expect("serialize the bucket policy"),
        )
        .await
    }

    /// S3 `PutObject`: the upload a client makes before it calls
    /// `PutRawMessageContent`.
    async fn put_object(&self, bucket: &str, key: &str, region: &str, content: &[u8]) -> Answer {
        self.call(
            "PUT",
            "s3",
            region,
            &format!("/{bucket}/{key}"),
            content.to_vec(),
        )
        .await
    }

    /// WorkMail Message Flow `PutRawMessageContent`.
    async fn put_raw_message_content(
        &self,
        region: &str,
        message_id: &str,
        bucket: &str,
        key: &str,
        object_version: Option<&str>,
    ) -> Answer {
        let mut s3_reference = json!({ "bucket": bucket, "key": key });
        if let Some(version) = object_version {
            s3_reference["objectVersion"] = json!(version);
        }
        let body = json!({ "content": { "s3Reference": s3_reference } });
        self.call(
            "POST",
            "workmailmessageflow",
            region,
            &format!("/messages/{message_id}"),
            serde_json::to_vec(&body).expect("serialize the update body"),
        )
        .await
    }

    /// WorkMail Message Flow `GetRawMessageContent`, whose success payload is the raw
    /// MIME content with no envelope.
    async fn get_raw_message_content(&self, region: &str, message_id: &str) -> Answer {
        self.call(
            "GET",
            "workmailmessageflow",
            region,
            &format!("/messages/{message_id}"),
            Vec::new(),
        )
        .await
    }
}

/// A well-formed MIME message: the `Date` and `From` fields RFC 5322 §3.6 requires
/// every message to carry, which is what makes the content installable.
fn mime(subject: &str, body: &str) -> Vec<u8> {
    format!(
        "Date: Tue, 24 Oct 2026 17:03:49 +0000\r\nFrom: sender@example.com\r\n\
         Subject: {subject}\r\n\r\n{body}\r\n"
    )
    .into_bytes()
}

/// The rule: an object uploaded through Amazon S3's own API is what the update
/// installs, and the next read reports those bytes.
///
/// The rows differ along the bucket, the key and the bytes uploaded.
#[tokio::test]
async fn an_object_uploaded_through_s3_is_what_the_update_installs() {
    // (case, bucket, key, uploaded bytes)
    let rows: Vec<(&str, &str, &str, Vec<u8>)> = vec![
        (
            "the recorded case",
            "wmf-mail-objects",
            "updated/message-mime",
            mime("rewritten through S3", "the new body"),
        ),
        (
            "a nested key and a longer message",
            "wmf-mail-objects",
            "mail/2026/10/updated-mime",
            {
                let mut content = mime("nested key", "the new body");
                for index in 0..256 {
                    content.extend(format!("paragraph {index}\r\n").as_bytes());
                }
                content
            },
        ),
        (
            "a bucket name at the modeled minimum length",
            "abc",
            "k",
            mime("tiny bucket", "body"),
        ),
    ];

    for (case, bucket, key, uploaded) in rows {
        let emulator = Emulator::new();
        let message_id = "00000000-0000-0000-0000-000000000001";
        let original = mime("original", "the body the message arrived with");
        emulator.seed_message(REGION, message_id, &original);

        let created = emulator.create_bucket(bucket, REGION).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the S3 CreateBucket to succeed, got {} ({})",
            created.status,
            created.summary()
        );
        let granted = emulator.put_bucket_policy(bucket, REGION).await;
        assert_eq!(
            granted.status, BUCKET_POLICY_STORED,
            "case {case:?}: expected the S3 PutBucketPolicy to be stored, got {} ({})",
            granted.status,
            granted.summary()
        );
        let written = emulator.put_object(bucket, key, REGION, &uploaded).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected the S3 PutObject to succeed, got {} ({})",
            written.status,
            written.summary()
        );

        let update = emulator
            .put_raw_message_content(REGION, message_id, bucket, key, None)
            .await;
        assert_eq!(
            update.status, 200,
            "case {case:?}: expected HTTP 200 from PutRawMessageContent, got {} ({})",
            update.status,
            update.summary()
        );

        let after = emulator.get_raw_message_content(REGION, message_id).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: expected HTTP 200 from GetRawMessageContent, got {} ({})",
            after.status,
            after.summary()
        );
        assert_eq!(
            after.body.as_ref(),
            uploaded.as_slice(),
            "case {case:?}: expected the {} octet(s) uploaded to S3, got {}",
            uploaded.len(),
            after.summary()
        );
    }
}

/// `objectVersion` names one version of the object: the reference resolves to the
/// bytes of the version S3 minted for that upload, not to the key's newest.
#[tokio::test]
async fn a_reference_naming_a_version_s3_minted_installs_that_version() {
    let emulator = Emulator::new();
    let bucket = "wmf-versioned-objects";
    let key = "updated/message-mime";
    let message_id = "versioned-0";

    assert_eq!(
        emulator.create_bucket(bucket, REGION).await.status,
        200,
        "the S3 CreateBucket must succeed"
    );
    assert_eq!(
        emulator.put_bucket_policy(bucket, REGION).await.status,
        BUCKET_POLICY_STORED,
        "the S3 PutBucketPolicy granting WorkMail the read must be stored"
    );
    assert_eq!(
        emulator.enable_versioning(bucket, REGION).await.status,
        200,
        "the S3 PutBucketVersioning must succeed"
    );

    // (case, bytes) — one upload per row, each a version of the same key.
    let uploads: [(&str, &str); 3] = [
        ("the first upload", "first"),
        ("the second upload", "second"),
        ("the third upload", "third"),
    ];
    let mut versions: Vec<(&str, Vec<u8>, String)> = Vec::new();
    for (case, label) in uploads {
        let content = mime(label, "body");
        let written = emulator.put_object(bucket, key, REGION, &content).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected the S3 PutObject to succeed, got {} ({})",
            written.status,
            written.summary()
        );
        assert!(
            !written.version_id.is_empty(),
            "case {case:?}: expected S3 to report a minted version id for a write into a \
             versioning-enabled bucket, got none"
        );
        versions.push((case, content, written.version_id.clone()));
    }

    for (case, content, version) in &versions {
        emulator.seed_message(REGION, message_id, &mime("original", "body"));
        let update = emulator
            .put_raw_message_content(REGION, message_id, bucket, key, Some(version))
            .await;
        assert_eq!(
            update.status, 200,
            "case {case:?}: expected HTTP 200 for version {version:?}, got {} ({})",
            update.status,
            update.summary()
        );
        let after = emulator.get_raw_message_content(REGION, message_id).await;
        assert_eq!(
            after.body.as_ref(),
            content.as_slice(),
            "case {case:?}: expected version {version:?}'s own bytes, got {}",
            after.summary()
        );
    }

    // A reference naming no version addresses the key's CURRENT version, which is
    // the newest upload.
    emulator.seed_message(REGION, message_id, &mime("original", "body"));
    let update = emulator
        .put_raw_message_content(REGION, message_id, bucket, key, None)
        .await;
    assert_eq!(
        update.status, 200,
        "case \"no version named\": expected HTTP 200, got {} ({})",
        update.status,
        update.summary()
    );
    let after = emulator.get_raw_message_content(REGION, message_id).await;
    let (_, newest, _) = versions.last().expect("three uploads were made");
    assert_eq!(
        after.body.as_ref(),
        newest.as_slice(),
        "case \"no version named\": expected the newest upload's bytes, got {}",
        after.summary()
    );
}

/// The documented location reasons are decided from the REAL bucket: an object in a
/// bucket S3 holds in another Region, a key S3 holds nothing at, and an object in a
/// bucket whose policy does not grant WorkMail the read are each
/// `InvalidContentLocation`, and the message keeps its content.
///
/// The last row is the one a client meets most often: it uploaded the object and
/// forgot the bucket policy, so nothing authorizes WorkMail's own fetch of it. The
/// control beside it is the row above that uploads the object AND attaches the grant,
/// which the first test of this suite asserts is accepted.
#[tokio::test]
async fn a_region_key_or_missing_policy_is_an_invalid_content_location() {
    // (case, the Region the bucket is created in, whether the object is uploaded,
    //  whether the bucket policy granting WorkMail the read is attached)
    let rows: [(&str, &str, bool, bool); 4] = [
        ("a bucket in another Region", "eu-west-1", true, true),
        ("a bucket in a third Region", "ap-southeast-2", true, true),
        (
            "a bucket in the request's Region with nothing at the key",
            REGION,
            false,
            true,
        ),
        (
            "an uploaded object in a bucket carrying no policy at all",
            REGION,
            true,
            false,
        ),
    ];

    for (case, bucket_region, uploaded, granted) in rows {
        let emulator = Emulator::new();
        let bucket = format!("wmf-objects-{bucket_region}");
        let key = "updated/message-mime";
        let message_id = "location-0";
        let original = mime("original", "the body the message arrived with");
        emulator.seed_message(REGION, message_id, &original);

        let created = emulator.create_bucket(&bucket, bucket_region).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the S3 CreateBucket in {bucket_region} to succeed, got {} \
             ({})",
            created.status,
            created.summary()
        );
        if granted {
            let policy = emulator.put_bucket_policy(&bucket, bucket_region).await;
            assert_eq!(
                policy.status, BUCKET_POLICY_STORED,
                "case {case:?}: expected the S3 PutBucketPolicy to be stored, got {} ({})",
                policy.status,
                policy.summary()
            );
        }
        if uploaded {
            let written = emulator
                .put_object(&bucket, key, bucket_region, &mime("elsewhere", "body"))
                .await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected the S3 PutObject to succeed, got {} ({})",
                written.status,
                written.summary()
            );
        }

        let update = emulator
            .put_raw_message_content(REGION, message_id, &bucket, key, None)
            .await;
        assert_eq!(
            update.status, 400,
            "case {case:?}: expected HTTP 400, got {} ({})",
            update.status,
            update.summary()
        );
        assert_eq!(
            update.error_type, "InvalidContentLocation",
            "case {case:?}: expected InvalidContentLocation, got {:?} ({})",
            update.error_type,
            update.summary()
        );

        let after = emulator.get_raw_message_content(REGION, message_id).await;
        assert_eq!(
            after.body.as_ref(),
            original.as_slice(),
            "case {case:?}: the refused update changed the stored content: {}",
            after.summary()
        );
    }
}
