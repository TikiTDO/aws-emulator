// SPDX-License-Identifier: CC0-1.0

//! What an object write that names a KMS key for its server-side encryption does with that
//! reference, and what a listing does with an annotation selector no name can begin with.
//!
//! The rules under test:
//!
//! * A write asking S3 to encrypt an object under a NAMED customer key is encrypted under a
//!   data key KMS mints for that key — "you can optionally tell Amazon S3 to encrypt data at
//!   rest by using server-side encryption with other key options"
//!   (`PutObjectRequest$ServerSideEncryption`), the SSE-KMS option being the one whose key
//!   belongs to KMS — so the reference is resolved against KMS BEFORE anything is stored. A
//!   reference KMS holds no key for yields no data key, so S3 relays KMS's own failure
//!   (HTTP 400, the KMS exception's name qualified by the service that raised it, and the
//!   sentence KMS words for the form the request supplied) and the object is not created. A
//!   reference KMS DOES resolve names ONE key whatever spelling it took, so the object is
//!   stored under — and reports — that key's own ARN.
//! * A listing's `AnnotationPrefix` selects annotation NAMES — "filter results to
//!   annotations whose name begins with the specified prefix"
//!   (`ListObjectAnnotationsRequest$AnnotationPrefix`) — so a selector no name of the
//!   namespace could begin with is not a prefix of it at all and is refused with the
//!   modelled `InvalidPrefix` (HTTP 400) rather than answered with a page. An annotation name
//!   begins with a letter or a digit, so a selector opening with anything else selects
//!   nothing that could exist, and the same rule refuses such a name to the WRITER.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and object suites in this crate own), so what is pinned is the wire
//! answer. Every key and alias a row names is made with KMS's OWN operations over the same
//! engine: nothing here plants a key in a store.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_encryption::{put_encryption, Rule};
use super::bucket_lifecycle::Answer;
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};

/// The Region every request in this crate's suites is signed for, and so the Region whose
/// endpoint holds the keys those requests name: a KMS key is a regional resource, and an
/// object and the key it is encrypted under live at the same endpoint.
pub(crate) const REGION: &str = "us-east-1";

/// The `ServerSideEncryption` values a KMS key id belongs to
/// (`smithy_query(query="shape",shape="ServerSideEncryption")` lists both among its values,
/// and `ApplyServerSideEncryptionByDefault$KMSMasterKeyID` is "allowed if and only if
/// SSEAlgorithm is set to aws:kms or aws:kms:dsse").
const KMS_ALGORITHMS: [&str; 2] = ["aws:kms", "aws:kms:dsse"];

/// The code S3 answers a write whose KMS key reference resolves to no key with, and the
/// status it carries. Recorded behaviour: the s3 model binds no error shape for the relay.
const KMS_NOT_FOUND_CODE: &str = "KMS.NotFoundException";

/// The bytes every write in this suite carries.
const BODY: &[u8] = b"kms-protected-payload";

/// S3 and KMS mounted on ONE engine, which is what makes the key a write names the key KMS
/// really holds — reached exactly the way the production router reaches both.
///
/// `pub(crate)`: the encryption, default-encryption and write-authorization suites in this
/// crate name keys KMS has to resolve, so they stand their rows up through this one harness
/// rather than a second one.
pub(crate) fn router_with_kms() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(engine),
    );
    router
}

/// One KMS call over the shared engine. KMS is an awsJson1_1 service reached by
/// `X-Amz-Target: TrentService.<Operation>`.
///
/// `pub(crate)`: the SSE-KMS data-key and write-key suites beside this one drive their keys,
/// key policies and key STATES through this one dispatcher, and ask KMS what it holds through
/// it, rather than building a second one.
pub(crate) async fn kms(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://kms.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TrentService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/kms/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// A customer-managed KMS key, created with KMS's own `CreateKey`, as `(key id, key ARN)`.
///
/// `pub(crate)`, like [`created_alias`] below: a suite whose write names a key KMS has to
/// resolve makes that key here.
pub(crate) async fn created_key(router: &MultiServiceRouter, description: &str) -> (String, String) {
    let (status, payload) = kms(router, "CreateKey", &json!({"Description": description})).await;
    assert_eq!(
        status, 200,
        "creating the key {description:?} must succeed, got {status} with {payload}"
    );
    let metadata = &payload["KeyMetadata"];
    let text = |member: &str| {
        metadata[member]
            .as_str()
            .unwrap_or_else(|| panic!("CreateKey must answer {member}, got {payload}"))
            .to_string()
    };
    (text("KeyId"), text("Arn"))
}

/// An alias of `key_id`, created with KMS's own `CreateAlias` — the second form a
/// `SSEKMSKeyId` takes ("you can specify the key ID, key alias, or the Amazon Resource Name
/// (ARN) of the KMS key").
pub(crate) async fn created_alias(router: &MultiServiceRouter, alias: &str, key_id: &str) {
    let (status, payload) = kms(
        router,
        "CreateAlias",
        &json!({"AliasName": alias, "TargetKeyId": key_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "creating the alias {alias:?} must succeed, got {status} with {payload}"
    );
}

/// The ARN of a KMS resource of this account at this endpoint: what KMS names a reference by
/// when it reports one it could not resolve.
fn kms_arn(resource: &str) -> String {
    format!(
        "arn:aws:kms:{REGION}:{}:{resource}",
        engine::emulator_account_id()
    )
}

/// One row of the dangling-reference table: the case, the `SSEKMSKeyId` the write sends, and
/// the sentence KMS words for that reference.
type DanglingRow = (&'static str, String, String);

#[tokio::test]
async fn a_write_naming_a_kms_key_reference_that_resolves_to_no_key_is_refused_and_stores_nothing()
{
    let router = router_with_kms();
    bucket(&router, "kms-reference-bucket").await;

    // A key id KMS could have issued (the hyphenated 8-4-4-4-12 text of a UUID) that no
    // `CreateKey` of this endpoint ever minted, and the same id as a key ARN of this account
    // and Region.
    let unminted = "4f5fb9d9-3e64-4ee1-a07a-0cf0bcfee9f9";
    // The rows differ along the FORM the reference takes — every form `SSEKMSKeyId`
    // documents: a bare key id, a key ARN, `alias/<name>` and an alias ARN — and, for each,
    // along the sentence KMS words for a reference of that kind: a key is reported as not
    // existing by its key ARN, an alias as not being found by its alias ARN.
    let rows: [DanglingRow; 5] = [
        (
            "a key ARN of this account and Region that no key has",
            kms_arn(&format!("key/{unminted}")),
            format!("Key '{}' does not exist", kms_arn(&format!("key/{unminted}"))),
        ),
        (
            "a bare key id that no key has",
            unminted.to_string(),
            format!("Key '{}' does not exist", kms_arn(&format!("key/{unminted}"))),
        ),
        (
            "another unminted key id, so nothing is keyed to one value",
            "0d8a1e3f-1111-4a1b-9c2d-5e6f70819203".to_string(),
            format!(
                "Key '{}' does not exist",
                kms_arn("key/0d8a1e3f-1111-4a1b-9c2d-5e6f70819203")
            ),
        ),
        (
            "an alias no CreateAlias ever made",
            "alias/ghost".to_string(),
            format!("Alias {} is not found.", kms_arn("alias/ghost")),
        ),
        (
            "that same alias given as an alias ARN",
            kms_arn("alias/ghost"),
            format!("Alias {} is not found.", kms_arn("alias/ghost")),
        ),
    ];

    for (index, (case, reference, message)) in rows.iter().enumerate() {
        // Every KMS algorithm, so the relay is not keyed to one of the two values a key id
        // may accompany.
        for algorithm in KMS_ALGORITHMS {
            let key = format!("dangling/{index}-{}.bin", algorithm.replace(':', "-"));
            let written = put_object(
                &router,
                "kms-reference-bucket",
                &key,
                BODY,
                &[
                    ("x-amz-server-side-encryption", algorithm),
                    (
                        "x-amz-server-side-encryption-aws-kms-key-id",
                        reference.as_str(),
                    ),
                ],
            )
            .await;
            assert_eq!(
                written.status, 400,
                "case {case:?}/{algorithm}: expected HTTP 400, got {} with body {:?}",
                written.status,
                text(&written)
            );
            assert_eq!(
                written.error_code(),
                KMS_NOT_FOUND_CODE,
                "case {case:?}/{algorithm}: expected the code {KMS_NOT_FOUND_CODE:?}, got {:?} \
                 with body {:?}",
                written.error_code(),
                text(&written)
            );
            assert_eq!(
                written.error_message(),
                *message,
                "case {case:?}/{algorithm}: expected the message {message:?}, got {:?}",
                written.error_message()
            );

            // Nothing was stored: a refused write leaves the key empty.
            let read = get_object(&router, "kms-reference-bucket", &key, "", &[]).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}/{algorithm}: a refused write stores nothing, so the read must \
                 answer 404, got {} with body {:?}",
                read.status,
                text(&read)
            );
            assert_eq!(
                read.error_code(),
                "NoSuchKey",
                "case {case:?}/{algorithm}: expected the code \"NoSuchKey\", got {:?}",
                read.error_code()
            );
        }
    }
}

/// One row of the resolvable-reference table: the case, the reference the write sends, and
/// whether a Bucket Key is asked for beside it.
type ResolvableRow = (&'static str, String, bool);

/// What KMS itself reports for the key a reference names: the member of its `KeyMetadata`
/// named — the `Arn` an object written under that reference carries, whichever spelling the
/// reference took, or the `KeyManager` that says who manages it.
///
/// `pub(crate)`: the encryption suites in this crate assert the same facts against their own
/// objects, so each comes from KMS's own `DescribeKey` rather than from a string one of them
/// builds.
pub(crate) async fn described_key_member(
    router: &MultiServiceRouter,
    reference: &str,
    member: &str,
) -> String {
    let (status, payload) = kms(router, "DescribeKey", &json!({"KeyId": reference})).await;
    assert_eq!(
        status, 200,
        "describing {reference:?} must succeed, got {status} with {payload}"
    );
    payload["KeyMetadata"][member]
        .as_str()
        .unwrap_or_else(|| {
            panic!("DescribeKey must answer {member} for {reference:?}, got {payload}")
        })
        .to_string()
}

/// [`described_key_member`] for the one member every caller of it needs: the key's ARN.
pub(crate) async fn described_key_arn(router: &MultiServiceRouter, reference: &str) -> String {
    described_key_member(router, reference, "Arn").await
}

#[tokio::test]
async fn a_write_naming_a_key_kms_holds_is_stored_under_that_key_s_arn() {
    let router = router_with_kms();
    bucket(&router, "kms-resolvable-bucket").await;
    let (key_id, key_arn) = created_key(&router, "the key these writes name").await;
    created_alias(&router, "alias/ledger", &key_id).await;

    // The rows differ along the FORM of a reference KMS DOES resolve — the same four forms
    // the dangling table walks, plus the PREDEFINED Amazon Web Services alias of this
    // service's managed key, which exists in every Region and carries no key until something
    // asks for it (so nothing creates it here) — and along whether an S3 Bucket Key
    // accompanies it. Every form names ONE key, so every object carries that key's ARN.
    let rows: [ResolvableRow; 6] = [
        ("the key ARN", key_arn.clone(), false),
        ("the bare key id", key_id.clone(), false),
        ("an alias of that key", "alias/ledger".to_string(), false),
        (
            "an alias ARN of that key",
            kms_arn("alias/ledger"),
            false,
        ),
        (
            "the predefined Amazon Web Services alias for this service",
            "alias/aws/s3".to_string(),
            false,
        ),
        ("the key ARN with a Bucket Key", key_arn.clone(), true),
    ];

    for (index, (case, reference, bucket_key)) in rows.iter().enumerate() {
        let key = format!("resolvable/{index}.bin");
        let mut headers: Vec<(&str, &str)> = vec![
            ("x-amz-server-side-encryption", KMS_ALGORITHMS[0]),
            (
                "x-amz-server-side-encryption-aws-kms-key-id",
                reference.as_str(),
            ),
        ];
        if *bucket_key {
            headers.push(("x-amz-server-side-encryption-bucket-key-enabled", "true"));
        }
        let written = put_object(&router, "kms-resolvable-bucket", &key, BODY, &headers).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: a reference KMS resolves must be served, got {} with body {:?}",
            written.status,
            text(&written)
        );
        // The key the reference names, as KMS reports it: the object carries the KEY, not the
        // spelling the request used, so the expected value is KMS's own `Arn` for it.
        let expected = described_key_arn(&router, reference).await;
        assert!(
            expected.starts_with("arn:aws:kms:") && expected.contains(":key/"),
            "case {case:?}: KMS must name the key by a key ARN, got {expected:?}"
        );
        assert_eq!(
            written
                .header("x-amz-server-side-encryption-aws-kms-key-id")
                .as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected the key ARN {expected:?} reported back, got {:?}",
            written.header("x-amz-server-side-encryption-aws-kms-key-id")
        );

        // Read back through an implemented reader: the object is really there, under the key
        // the write's reference resolved to.
        let read = get_object(&router, "kms-resolvable-bucket", &key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the stored object must read back, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.body.as_ref(),
            BODY,
            "case {case:?}: the stored bytes must come back unchanged"
        );
        assert_eq!(
            read.header("x-amz-server-side-encryption-aws-kms-key-id")
                .as_deref(),
            Some(expected.as_str()),
            "case {case:?}: a read must report the same key ARN, got {:?}",
            read.header("x-amz-server-side-encryption-aws-kms-key-id")
        );
    }
}

/// One row of the other-endpoints table: the case, the reference the write names, the status
/// and error code S3 answers that write with (`None` for a write it serves), and the status a
/// read of that object then answers.
type OtherEndpointRow = (&'static str, String, Option<(u16, &'static str)>, u16);

#[tokio::test]
async fn a_reference_of_another_endpoint_is_refused_and_one_kms_declines_unresolved_is_not() {
    let router = router_with_kms();
    bucket(&router, "kms-other-endpoint-bucket").await;
    let unminted = "3f6a0c18-4b2e-4d71-9a53-8e1c7b204d9f";

    // A key ARN "addresses one key in one Region of one account", so a reference naming
    // ANOTHER endpoint's key names no key here and is refused exactly as an unminted one is.
    // A reference KMS declines BEFORE it resolves anything is refused for want of PERMISSION
    // rather than for absence — the data key the write needs cannot be minted under it, so
    // the write is refused with KMS's own denial sentence on S3's `AccessDenied` channel
    // instead of the not-found relay — and an identifier that is no key reference at all
    // resolves to no key of this account and is served. The rows pin that division so the
    // scope of each refusal is deliberate.
    let rows: [OtherEndpointRow; 5] = [
        (
            "a key ARN of another account at this endpoint",
            format!("arn:aws:kms:{REGION}:111122223333:key/{unminted}"),
            Some((400, KMS_NOT_FOUND_CODE)),
            404,
        ),
        (
            "a key ARN of another Region",
            format!("arn:aws:kms:eu-west-1:{}:key/{unminted}", engine::emulator_account_id()),
            Some((400, KMS_NOT_FOUND_CODE)),
            404,
        ),
        (
            "an alias ARN of another account",
            format!("arn:aws:kms:{REGION}:111122223333:alias/ledger"),
            Some((400, KMS_NOT_FOUND_CODE)),
            404,
        ),
        (
            // Refused for want of permission: KMS declines such an identifier before it
            // resolves anything, so it can mint no data key for the write
            // (`state::sse_kms::data_key_refusal`), and nothing is stored.
            "an ARN whose account segment is not an account id",
            format!("arn:aws:kms:{REGION}:not-an-account:key/{unminted}"),
            Some((403, "AccessDenied")),
            404,
        ),
        (
            "an ARN that is no KMS key or alias ARN at all",
            "arn:aws:s3:::ledger".to_string(),
            None,
            200,
        ),
    ];

    for (index, (case, reference, refusal, read_expected)) in rows.iter().enumerate() {
        let key = format!("other-endpoint/{index}.bin");
        let written = put_object(
            &router,
            "kms-other-endpoint-bucket",
            &key,
            BODY,
            &[
                ("x-amz-server-side-encryption", KMS_ALGORITHMS[0]),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id",
                    reference.as_str(),
                ),
            ],
        )
        .await;
        let expected = refusal.map_or(200, |(status, _)| status);
        assert_eq!(
            written.status, expected,
            "case {case:?}: expected HTTP {expected} for {reference:?}, got {} with body {:?}",
            written.status,
            text(&written)
        );
        if let Some((_, code)) = refusal {
            assert_eq!(
                written.error_code(),
                *code,
                "case {case:?}: expected the code {code:?}, got {:?}",
                written.error_code()
            );
            assert!(
                !written.error_message().is_empty(),
                "case {case:?}: the relay carries KMS's own sentence, got none"
            );
        }
        let read = get_object(&router, "kms-other-endpoint-bucket", &key, "", &[]).await;
        assert_eq!(
            read.status, *read_expected,
            "case {case:?}: expected the read to answer {read_expected}, got {} with body {:?}",
            read.status,
            text(&read)
        );
    }
}

/// One row of the inherited-default table: the case, the `KMSMasterKeyID` the bucket's
/// default names, and the sentence KMS words for it.
type InheritedRow = (&'static str, String, String);

#[tokio::test]
async fn a_write_inheriting_a_bucket_default_whose_key_is_gone_is_refused_and_stores_nothing() {
    let router = router_with_kms();

    // The rows differ along the FORM the bucket's default names its key in — "you can
    // specify the key ID, key alias, or the Amazon Resource Name (ARN) of the KMS key"
    // (`ApplyServerSideEncryptionByDefault$KMSMasterKeyID`) — and along the algorithm the
    // default applies.
    let unminted = "9c1e7b40-2c3d-4e55-8a71-6f0b5d2a19ee";
    let rows: [InheritedRow; 3] = [
        (
            "a default naming a key ARN no key has",
            kms_arn(&format!("key/{unminted}")),
            format!("Key '{}' does not exist", kms_arn(&format!("key/{unminted}"))),
        ),
        (
            "a default naming a bare key id no key has",
            unminted.to_string(),
            format!("Key '{}' does not exist", kms_arn(&format!("key/{unminted}"))),
        ),
        (
            "a default naming an alias nothing targets",
            "alias/retired".to_string(),
            format!("Alias {} is not found.", kms_arn("alias/retired")),
        ),
    ];

    for (index, (case, reference, message)) in rows.iter().enumerate() {
        for (algorithm_index, algorithm) in KMS_ALGORITHMS.into_iter().enumerate() {
            let name = format!("kms-default-{index}-{algorithm_index}");
            bucket(&router, &name).await;
            // S3 "doesn't validate the KMS key ID provided in PutBucketEncryption requests"
            // (`PutBucketEncryption`'s own documentation), so the configuration itself is
            // stored: it is the WRITE that would have used the key which fails.
            let configured = put_encryption(
                &router,
                &name,
                &[Rule::by_default(algorithm).with_key(reference)],
            )
            .await;
            assert_eq!(
                configured.status, 200,
                "case {case:?}/{algorithm}: the default must be configured first, got {} with \
                 body {:?}",
                configured.status,
                text(&configured)
            );

            // The write names NO encryption member of its own, so it is stored under that
            // default — and refused for the key the default names.
            let written = put_object(&router, &name, "inherited.bin", BODY, &[]).await;
            assert_eq!(
                written.status, 400,
                "case {case:?}/{algorithm}: expected HTTP 400, got {} with body {:?}",
                written.status,
                text(&written)
            );
            assert_eq!(
                written.error_code(),
                KMS_NOT_FOUND_CODE,
                "case {case:?}/{algorithm}: expected the code {KMS_NOT_FOUND_CODE:?}, got {:?} \
                 with body {:?}",
                written.error_code(),
                text(&written)
            );
            assert_eq!(
                written.error_message(),
                *message,
                "case {case:?}/{algorithm}: expected the message {message:?}, got {:?}",
                written.error_message()
            );
            let read = get_object(&router, &name, "inherited.bin", "", &[]).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}/{algorithm}: a refused write stores nothing, so the read must \
                 answer 404, got {} with body {:?}",
                read.status,
                text(&read)
            );
        }
    }
}

/// `PUT /<bucket>/<key>` carrying `x-amz-copy-source` — `CopyObject`, whose own write of the
/// destination object is what the reference is resolved for.
async fn copy_object(
    router: &MultiServiceRouter,
    destination_bucket: &str,
    destination_key: &str,
    copy_source: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let mut all: Vec<(&str, &str)> = vec![("x-amz-copy-source", copy_source)];
    all.extend_from_slice(headers);
    super::bucket_lifecycle::send_bytes(
        router,
        "PUT",
        &format!("/{destination_bucket}/{destination_key}"),
        &all,
        b"",
    )
    .await
}

/// `POST /<bucket>/<key>?uploads` — `CreateMultipartUpload`, which settles the encryption
/// every write of the upload is stored under.
///
/// `pub(crate)`: the write-key suite beside this one initiates its uploads through this one
/// rather than keeping a second copy of the same request.
pub(crate) async fn create_multipart_upload(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    headers: &[(&str, &str)],
) -> Answer {
    super::bucket_lifecycle::send_bytes(
        router,
        "POST",
        &format!("/{bucket_name}/{key}?uploads"),
        headers,
        b"",
    )
    .await
}

/// One row of the other-writes table: the case, whether the reference reaches the write
/// through the REQUEST or through the bucket's default, and the reference itself.
type OtherWriteRow = (&'static str, bool, String);

#[tokio::test]
async fn every_write_stored_under_a_kms_key_resolves_the_reference_before_recording_anything() {
    let router = router_with_kms();
    bucket(&router, "kms-write-origin").await;
    let source = put_object(&router, "kms-write-origin", "source.bin", BODY, &[]).await;
    assert_eq!(
        source.status, 200,
        "the copy source must be written first, got {} with body {:?}",
        source.status,
        text(&source)
    );

    let unminted = "7b3c5d21-9f44-4ac8-8e10-2c6d3f4a5b60";
    let message = format!("Key '{}' does not exist", kms_arn(&format!("key/{unminted}")));
    // The rows differ along HOW the reference reaches the write — named by the request, or
    // inherited from the bucket's own default encryption, which is the other way an object
    // comes to be stored under a key.
    let rows: [OtherWriteRow; 2] = [
        (
            "the reference the request names",
            true,
            kms_arn(&format!("key/{unminted}")),
        ),
        (
            "the reference the bucket default names",
            false,
            kms_arn(&format!("key/{unminted}")),
        ),
    ];

    for (index, (case, in_request, reference)) in rows.iter().enumerate() {
        let target = format!("kms-write-target-{index}");
        bucket(&router, &target).await;
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if *in_request {
            headers.push(("x-amz-server-side-encryption", KMS_ALGORITHMS[0]));
            headers.push((
                "x-amz-server-side-encryption-aws-kms-key-id",
                reference.as_str(),
            ));
        } else {
            let configured = put_encryption(
                &router,
                &target,
                &[Rule::by_default(KMS_ALGORITHMS[0]).with_key(reference)],
            )
            .await;
            assert_eq!(
                configured.status, 200,
                "case {case:?}: the default must be configured first, got {} with body {:?}",
                configured.status,
                text(&configured)
            );
        }

        // The COPY writes the destination object, so the reference it would store it under is
        // resolved before anything is written.
        let copied = copy_object(
            &router,
            &target,
            "copied.bin",
            "kms-write-origin/source.bin",
            &headers,
        )
        .await;
        assert_eq!(
            copied.status, 400,
            "case {case:?}: CopyObject must be refused, got {} with body {:?}",
            copied.status,
            text(&copied)
        );
        assert_eq!(
            copied.error_code(),
            KMS_NOT_FOUND_CODE,
            "case {case:?}: CopyObject must answer {KMS_NOT_FOUND_CODE:?}, got {:?}",
            copied.error_code()
        );
        assert_eq!(
            copied.error_message(),
            message,
            "case {case:?}: CopyObject must carry KMS's own sentence {message:?}, got {:?}",
            copied.error_message()
        );
        let destination = get_object(&router, &target, "copied.bin", "", &[]).await;
        assert_eq!(
            destination.status, 404,
            "case {case:?}: a refused copy writes nothing, so the read must answer 404, got \
             {} with body {:?}",
            destination.status,
            text(&destination)
        );

        // And the INITIATION settles the encryption of every write of the upload, so it
        // resolves the same reference before the upload is recorded.
        let initiated = create_multipart_upload(&router, &target, "multipart.bin", &headers).await;
        assert_eq!(
            initiated.status, 400,
            "case {case:?}: CreateMultipartUpload must be refused, got {} with body {:?}",
            initiated.status,
            text(&initiated)
        );
        assert_eq!(
            initiated.error_code(),
            KMS_NOT_FOUND_CODE,
            "case {case:?}: CreateMultipartUpload must answer {KMS_NOT_FOUND_CODE:?}, got {:?}",
            initiated.error_code()
        );
        assert_eq!(
            initiated.element("UploadId"),
            None,
            "case {case:?}: a refused initiation records no upload, got {:?}",
            initiated.element("UploadId")
        );
    }
}

/// One row of the encryption-without-a-key table: the case and the headers the write carries.
type NoKeyRow = (&'static str, Vec<(&'static str, &'static str)>);

#[tokio::test]
async fn a_write_naming_no_kms_key_asks_kms_nothing() {
    let router = router_with_kms();
    bucket(&router, "kms-unasked-bucket").await;

    // A write that names no key id has no reference for KMS to resolve, whatever else it
    // names: the rows differ along the algorithm (none at all, SSE-S3, and each KMS
    // algorithm with no key id beside it) — none of them may be refused for want of a key.
    let rows: [NoKeyRow; 4] = [
        ("no encryption member at all", Vec::new()),
        (
            "SSE-S3",
            vec![("x-amz-server-side-encryption", "AES256")],
        ),
        (
            "SSE-KMS with no key id",
            vec![("x-amz-server-side-encryption", KMS_ALGORITHMS[0])],
        ),
        (
            "dual-layer SSE-KMS with no key id",
            vec![("x-amz-server-side-encryption", KMS_ALGORITHMS[1])],
        ),
    ];

    for (index, (case, headers)) in rows.iter().enumerate() {
        let key = format!("unasked/{index}.bin");
        let written = put_object(&router, "kms-unasked-bucket", &key, BODY, headers).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: a write naming no key reference must be served, got {} with body \
             {:?}",
            written.status,
            text(&written)
        );
        let read = get_object(&router, "kms-unasked-bucket", &key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the stored object must read back, got {} with body {:?}",
            read.status,
            text(&read)
        );
    }
}

/// `PUT /<bucket>/<key>?annotation&annotationName=<name>` — `PutObjectAnnotation`.
async fn put_annotation(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    name: &str,
    payload: &[u8],
) -> Answer {
    super::bucket_lifecycle::send_bytes(
        router,
        "PUT",
        &format!(
            "/{bucket_name}/{key}?annotation&annotationName={}",
            urlencoded(name)
        ),
        &[],
        payload,
    )
    .await
}

/// `GET /<bucket>/<key>?annotation&annotation-prefix=<prefix>` — `ListObjectAnnotations`
/// filtered by a prefix.
async fn list_annotations(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    prefix: &str,
) -> Answer {
    super::bucket_lifecycle::send_bytes(
        router,
        "GET",
        &format!(
            "/{bucket_name}/{key}?annotation&annotation-prefix={}",
            urlencoded(prefix)
        ),
        &[],
        b"",
    )
    .await
}

/// Percent-encode one query-component value, through the crate this service already
/// percent-encodes with rather than by hand — leaving only RFC 3986's unreserved characters
/// literal, so a selector spelled with a separator or a space reaches the handler as sent.
fn urlencoded(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC)
        .to_string()
}

#[tokio::test]
async fn an_annotation_selector_no_name_can_begin_with_is_refused() {
    let router = router_with_kms();
    bucket(&router, "annotation-selector-bucket").await;
    put_object(&router, "annotation-selector-bucket", "report.txt", BODY, &[]).await;
    // One annotation to list, so a served selector has something to select and a refused one
    // is refused about the selector rather than about an empty object.
    let attached = put_annotation(
        &router,
        "annotation-selector-bucket",
        "report.txt",
        "report/2026",
        b"first",
    )
    .await;
    assert_eq!(
        attached.status, 200,
        "the annotation must be attached first, got {} with body {:?}",
        attached.status,
        text(&attached)
    );

    // The rows differ along the CLASS of character the selector opens with: a path
    // separator (the recorded refusal), an exclamation mark, a space, a colon, a symbol
    // outside any alphabet, and a separator with a name after it. The sentence is
    // `InvalidPrefix`'s own, which S3 sends as fixed text.
    let refused: [(&str, &str); 6] = [
        ("a path separator", "/"),
        ("a separator with a name after it", "/report"),
        ("an exclamation mark", "!report"),
        ("a space", " report"),
        ("a colon", ":report"),
        ("a symbol outside any alphabet", "✓report"),
    ];
    for (case, prefix) in refused {
        let listed = list_annotations(
            &router,
            "annotation-selector-bucket",
            "report.txt",
            prefix,
        )
        .await;
        assert_eq!(
            listed.status, 400,
            "case {case:?}: expected HTTP 400 for the selector {prefix:?}, got {} with body \
             {:?}",
            listed.status,
            text(&listed)
        );
        assert_eq!(
            listed.error_code(),
            "InvalidPrefix",
            "case {case:?}: expected the code \"InvalidPrefix\", got {:?}",
            listed.error_code()
        );
        assert_eq!(
            listed.error_message(),
            "The annotation prefix you have provided is invalid.",
            "case {case:?}: expected the published sentence, got {:?}",
            listed.error_message()
        );
        assert!(
            listed.elements("AnnotationName").is_empty(),
            "case {case:?}: a refused listing reports no annotation, got {:?}",
            listed.elements("AnnotationName")
        );

        // The WRITER refuses the same name, so the namespace a listing filters is the
        // namespace a write can fill: a name no listing could select is a name no write
        // records.
        let written = put_annotation(
            &router,
            "annotation-selector-bucket",
            "report.txt",
            prefix,
            b"payload",
        )
        .await;
        assert_eq!(
            written.status, 400,
            "case {case:?}: a name opening with the same character must be refused the write, \
             got {} with body {:?}",
            written.status,
            text(&written)
        );
        assert_eq!(
            written.error_code(),
            "InvalidAnnotationName",
            "case {case:?}: expected the code \"InvalidAnnotationName\", got {:?}",
            written.error_code()
        );
    }

    // And a selector that DOES open with a letter, a digit or one of the separators a
    // namespaced name is spelled with is a prefix of the namespace, whatever it carries after
    // that: the rows span a letter, a digit, a name with a separator inside it, a letter
    // outside ASCII, each of the three leading separators, and the whole stored name.
    let served: [(&str, &str, usize); 9] = [
        ("a letter", "r", 1),
        ("the whole stored name", "report/2026", 1),
        ("a name with a separator inside it", "report/", 1),
        ("a digit", "2026", 0),
        ("a letter outside ASCII", "émile", 0),
        ("a leading hyphen", "-report", 0),
        ("a leading full stop", ".report", 0),
        ("a leading underscore", "_report", 0),
        ("the empty selector, which filters nothing", "", 1),
    ];
    for (case, prefix, expected) in served {
        let listed = list_annotations(
            &router,
            "annotation-selector-bucket",
            "report.txt",
            prefix,
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200 for the selector {prefix:?}, got {} with body \
             {:?}",
            listed.status,
            text(&listed)
        );
        assert_eq!(
            listed.elements("AnnotationName").len(),
            expected,
            "case {case:?}: expected {expected} annotation(s) under {prefix:?}, got {:?}",
            listed.elements("AnnotationName")
        );
    }
}
