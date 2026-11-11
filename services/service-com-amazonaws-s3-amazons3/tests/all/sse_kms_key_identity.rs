// SPDX-License-Identifier: CC0-1.0

//! WHICH key an SSE-KMS write says protected the object: the reference the request supplied,
//! resolved against KMS, reported as the ARN of the key it resolved to.
//!
//! RULE: an object write that asks for server-side encryption with a KMS key is encrypted
//! under a data key KMS mints for the key the request's reference names, so S3 resolves that
//! reference against KMS before anything is stored and records the ARN of the key it
//! resolved to. The request may address the key in any of the forms the member accepts —
//! "the KMS key ID (Key ID, Key ARN, or Key Alias)"
//! (`smithy_query(query="doc",shape="PutObjectRequest",member="SSEKMSKeyId")`), and an alias
//! ARN beside them — while what every answer reports is one thing: "the ID of the KMS key
//! that was used for object encryption"
//! (`smithy_query(query="doc",shape="PutObjectOutput",member="SSEKMSKeyId")`), which is the
//! key's own ARN. A key id is relative to an account and a Region and an alias is a mutable
//! pointer, so neither identifies the key that holds the object's bytes shut; the ARN does.
//!
//! The rule holds for every write that SETTLES such an encryption, each carrying the same
//! `SSEKMSKeyId` member — `PutObject`, `CopyObject` and `CreateMultipartUpload`, whose
//! initiation settles what every part write and the finished object report, and
//! `CreateSession`, whose settings are what "Amazon S3 will use … to protect new objects in
//! the directory bucket" — and for both ways a reference reaches a write: the request's own
//! member, and the destination bucket's default `KMSMasterKeyID` when the request names no
//! encryption member at all ("you can specify the key ID, key alias, or the Amazon Resource
//! Name (ARN) of the KMS key", `ApplyServerSideEncryptionByDefault$KMSMasterKeyID`).
//!
//! The other side of the same resolution: a reference KMS holds no key for yields no data
//! key, so the call that would have been served under it is refused with KMS's own sentence
//! and nothing is stored or established — the table at the end of this file drives that over
//! every write, and [`super::kms_key_references`] spans the forms a dangling reference takes.
//!
//! What the expected ARN is never spelled here: it is KMS's own answer about the reference
//! ([`super::kms_key_references::described_key_arn`], `DescribeKey`), so the two services
//! cannot disagree about which key a reference names and no row is keyed to a recorded value.
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire answer a client sees.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_encryption::{put_encryption, Rule};
use super::bucket_lifecycle::{send, send_bytes, Answer};
use super::directory_bucket_sessions_and_appends::directory_bucket;
use super::kms_key_references::{
    copy_object, create_multipart_upload, created_alias, created_key, described_key_arn, kms,
    kms_arn,
};
use super::multipart_upload_completion::completion_payload;
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_encryption::head_object;

/// The response header `SSEKMSKeyId` binds to
/// (`smithy_query(query="operation",operation="PutObject")`).
const KEY_ID_HEADER: &str = "x-amz-server-side-encryption-aws-kms-key-id";

/// The header `ServerSideEncryption` binds to.
const ALGORITHM_HEADER: &str = "x-amz-server-side-encryption";

/// The two `ServerSideEncryption` values a KMS key id belongs to
/// (`smithy_query(query="shape",shape="ServerSideEncryption")` lists both, and
/// `ApplyServerSideEncryptionByDefault$KMSMasterKeyID` is "allowed if and only if
/// SSEAlgorithm is set to aws:kms or aws:kms:dsse").
const KMS_ALGORITHMS: [&str; 2] = ["aws:kms", "aws:kms:dsse"];

/// The code S3 answers a write whose KMS key reference resolves to no key with. Recorded
/// behaviour: the s3 model binds no error shape for the relay.
const KMS_NOT_FOUND_CODE: &str = "KMS.NotFoundException";

/// The bytes every write in this suite carries.
const BODY: &[u8] = b"sse-kms-protected-payload";

/// How the KMS key reference reaches the write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delivery {
    /// The request names it in `SSEKMSKeyId`, beside the algorithm.
    Request,
    /// The request names no encryption member, so the write inherits the bucket's default
    /// `KMSMasterKeyID`.
    BucketDefault,
}

impl Delivery {
    /// The two ways, for a table that walks both.
    const BOTH: [Self; 2] = [Self::Request, Self::BucketDefault];

    /// How a case names it.
    fn label(self) -> &'static str {
        match self {
            Self::Request => "named by the request",
            Self::BucketDefault => "inherited from the bucket default",
        }
    }

    /// A name fragment, so the two deliveries of one row address different buckets.
    fn slug(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::BucketDefault => "default",
        }
    }
}

/// `PUT /<bucket>/<key>?partNumber=1&uploadId=<id>` — `UploadPart`.
async fn upload_part(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    upload_id: &str,
    body: &[u8],
) -> Answer {
    send_bytes(
        router,
        "PUT",
        &format!("/{bucket_name}/{key}?partNumber=1&uploadId={upload_id}&x-id=UploadPart"),
        &[],
        body,
    )
    .await
}

/// `POST /<bucket>/<key>?uploadId=<id>` — `CompleteMultipartUpload`.
async fn complete(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    upload_id: &str,
    parts: &[(i32, String)],
) -> Answer {
    send(
        router,
        "POST",
        &format!("/{bucket_name}/{key}?uploadId={upload_id}"),
        &[],
        &completion_payload(parts),
    )
    .await
}

/// Assert `answer` reports `expected` as the key that encrypted the object.
fn assert_key_id(case: &str, operation: &str, answer: &Answer, expected: &str) {
    assert_eq!(
        answer.status / 100,
        2,
        "case {case:?}: {operation} must be served, got {} with body {:?}",
        answer.status,
        text(answer)
    );
    assert_eq!(
        answer.header(KEY_ID_HEADER).as_deref(),
        Some(expected),
        "case {case:?}: {operation} must report the key ARN {expected:?}, got {:?}",
        answer.header(KEY_ID_HEADER)
    );
}

/// One row of the identity table: the case, the reference the write supplies, the algorithm
/// it names beside it, and the key that reference names, as KMS reports it.
struct IdentityRow {
    case: &'static str,
    reference: String,
    algorithm: &'static str,
    expected: String,
}

#[tokio::test]
async fn every_sse_kms_write_reports_the_arn_of_the_key_its_reference_resolved_to() {
    let router = super::kms_key_references::router_with_kms();
    let (key_id, key_arn) = created_key(&router, "the key these writes name").await;
    created_alias(&router, "alias/ledger", &key_id).await;
    // A SECOND alias of the SAME key, so no answer can be keyed to one alias name.
    created_alias(&router, "alias/ledger-mirror", &key_id).await;
    // The predefined Amazon Web Services alias of this service carries no key until
    // something asks for it, and asking is what gives it one, so its key's ARN is KMS's own
    // answer about the alias.
    let managed_key_arn = described_key_arn(&router, "alias/aws/s3").await;

    // A plain source object for the copies: the COPY writes the destination object, so it is
    // the destination's encryption that resolves a reference.
    bucket(&router, "kms-identity-source").await;
    let source = put_object(&router, "kms-identity-source", "source.bin", BODY, &[]).await;
    assert_eq!(
        source.status, 200,
        "the copy source must be written first, got {} with body {:?}",
        source.status,
        text(&source)
    );

    // The rows differ along what the rule generalises over: WHICH form the reference takes
    // (key ARN, bare key id, `alias/<name>`, an alias ARN, a second alias of the same key,
    // and the predefined Amazon Web Services alias), and WHICH KMS algorithm it accompanies.
    let rows: [IdentityRow; 6] = [
        IdentityRow {
            case: "the key ARN",
            reference: key_arn.clone(),
            algorithm: KMS_ALGORITHMS[0],
            expected: key_arn.clone(),
        },
        IdentityRow {
            case: "a bare key id",
            reference: key_id.clone(),
            algorithm: KMS_ALGORITHMS[1],
            expected: key_arn.clone(),
        },
        IdentityRow {
            case: "an alias of the key",
            reference: "alias/ledger".to_string(),
            algorithm: KMS_ALGORITHMS[0],
            expected: key_arn.clone(),
        },
        IdentityRow {
            case: "an alias ARN of the key",
            reference: kms_arn("alias/ledger"),
            algorithm: KMS_ALGORITHMS[1],
            expected: key_arn.clone(),
        },
        IdentityRow {
            case: "a second alias of the same key",
            reference: "alias/ledger-mirror".to_string(),
            algorithm: KMS_ALGORITHMS[0],
            expected: key_arn.clone(),
        },
        IdentityRow {
            case: "the predefined Amazon Web Services alias for this service",
            reference: "alias/aws/s3".to_string(),
            algorithm: KMS_ALGORITHMS[0],
            expected: managed_key_arn,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        for delivery in Delivery::BOTH {
            let case = format!("{} {}", row.case, delivery.label());
            let case = case.as_str();
            let name = format!("kms-identity-{index}-{}", delivery.slug());
            bucket(&router, &name).await;

            // Whichever way the reference reaches the write, what KMS resolves it to is the
            // same key — so the two deliveries must answer the same ARN.
            let mut headers: Vec<(&str, &str)> = Vec::new();
            match delivery {
                Delivery::Request => {
                    headers.push((ALGORITHM_HEADER, row.algorithm));
                    headers.push((KEY_ID_HEADER, row.reference.as_str()));
                }
                Delivery::BucketDefault => {
                    // S3 "doesn't validate the KMS key ID provided in PutBucketEncryption
                    // requests", so the configuration is stored as sent and it is the WRITE
                    // that resolves the key it names.
                    let configured = put_encryption(
                        &router,
                        &name,
                        &[Rule::by_default(row.algorithm).with_key(&row.reference)],
                    )
                    .await;
                    assert_eq!(
                        configured.status, 200,
                        "case {case:?}: the bucket default must be configured first, got {} \
                         with body {:?}",
                        configured.status,
                        text(&configured)
                    );
                }
            }

            // PutObject, and both implemented readers of what it stored.
            let written = put_object(&router, &name, "put.bin", BODY, &headers).await;
            assert_key_id(case, "PutObject", &written, &row.expected);
            assert_eq!(
                written.header(ALGORITHM_HEADER).as_deref(),
                Some(row.algorithm),
                "case {case:?}: PutObject must report the algorithm {:?}, got {:?}",
                row.algorithm,
                written.header(ALGORITHM_HEADER)
            );
            let read = get_object(&router, &name, "put.bin", "", &[]).await;
            assert_key_id(case, "GetObject", &read, &row.expected);
            assert_eq!(
                read.body.as_ref(),
                BODY,
                "case {case:?}: the stored bytes must come back unchanged"
            );
            let headed = head_object(&router, &name, "put.bin").await;
            assert_key_id(case, "HeadObject", &headed, &row.expected);

            // CopyObject writes a new object, so it settles — and reports — its own key.
            let copied = copy_object(
                &router,
                &name,
                "copied.bin",
                "kms-identity-source/source.bin",
                &headers,
            )
            .await;
            assert_key_id(case, "CopyObject", &copied, &row.expected);
            let copy_read = get_object(&router, &name, "copied.bin", "", &[]).await;
            assert_key_id(case, "GetObject of the copy", &copy_read, &row.expected);

            // The INITIATION settles the encryption of the whole upload, so the part write,
            // the completion and the finished object all report the key it resolved.
            let initiated =
                create_multipart_upload(&router, &name, "multipart.bin", &headers).await;
            assert_key_id(case, "CreateMultipartUpload", &initiated, &row.expected);
            let upload_id = initiated
                .element("UploadId")
                .unwrap_or_else(|| panic!("case {case:?}: an initiation must report an UploadId"));
            let part = upload_part(&router, &name, "multipart.bin", &upload_id, BODY).await;
            assert_key_id(case, "UploadPart", &part, &row.expected);
            let etag = part
                .header("etag")
                .unwrap_or_else(|| panic!("case {case:?}: a part write must report an ETag"));
            let completed =
                complete(&router, &name, "multipart.bin", &upload_id, &[(1, etag)]).await;
            assert_key_id(case, "CompleteMultipartUpload", &completed, &row.expected);
            let assembled = get_object(&router, &name, "multipart.bin", "", &[]).await;
            assert_key_id(case, "GetObject of the upload", &assembled, &row.expected);
        }
    }
}

#[tokio::test]
async fn an_alias_is_resolved_at_the_write_so_retargeting_it_leaves_stored_objects_alone() {
    // An alias is a POINTER to a key, and the write resolves it: so an object records the key
    // the alias named WHEN it was written, and retargeting the alias afterwards changes what
    // the NEXT write records and nothing about the object already stored.
    let router = super::kms_key_references::router_with_kms();
    let (first_id, first_arn) = created_key(&router, "the key the alias names first").await;
    let (second_id, second_arn) = created_key(&router, "the key the alias is moved to").await;
    created_alias(&router, "alias/moving", &first_id).await;
    bucket(&router, "kms-identity-retarget").await;

    let headers: Vec<(&str, &str)> = vec![
        (ALGORITHM_HEADER, KMS_ALGORITHMS[0]),
        (KEY_ID_HEADER, "alias/moving"),
    ];
    let before = put_object(&router, "kms-identity-retarget", "before.bin", BODY, &headers).await;
    assert_key_id("written before the alias moved", "PutObject", &before, &first_arn);

    // KMS's own `UpdateAlias` moves the pointer: "associates an existing KMS alias with a
    // different KMS key".
    let (status, payload) = kms(
        &router,
        "UpdateAlias",
        &json!({"AliasName": "alias/moving", "TargetKeyId": second_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the alias must be retargeted, got {status} with {payload}"
    );

    let after = put_object(&router, "kms-identity-retarget", "after.bin", BODY, &headers).await;
    assert_key_id("written after the alias moved", "PutObject", &after, &second_arn);
    assert_ne!(
        first_arn, second_arn,
        "the two keys must be different keys, got {first_arn:?} twice"
    );

    // The object written first still names the key that encrypted it.
    for (case, key, expected) in [
        ("the object written before the move", "before.bin", &first_arn),
        ("the object written after the move", "after.bin", &second_arn),
    ] {
        let read = get_object(&router, "kms-identity-retarget", key, "", &[]).await;
        assert_key_id(case, "GetObject", &read, expected);
        let headed = head_object(&router, "kms-identity-retarget", key).await;
        assert_key_id(case, "HeadObject", &headed, expected);
    }
}

/// `GET /<bucket>?session` — `CreateSession`, whose encryption settings are what "Amazon S3
/// will use … to protect new objects in the directory bucket", so its `SSEKMSKeyId` reports
/// the key those objects are encrypted under.
async fn create_session(
    router: &MultiServiceRouter,
    bucket_name: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(router, "GET", &format!("/{bucket_name}?session"), headers, "").await
}

#[tokio::test]
async fn a_session_reports_the_arn_of_the_key_its_reference_resolved_to_and_refuses_the_rest() {
    let router = super::kms_key_references::router_with_kms();
    let (key_id, key_arn) = created_key(&router, "the key the session names").await;
    let name = "kms-identity--use1-az4--x-s3";
    directory_bucket(&router, name, "use1-az4").await;

    // The rows differ along the form the session's reference takes — a bare key id and a key
    // ARN, the two a directory bucket's SSE-KMS configuration is addressed by ("it must match
    // the bucket's default customer managed key (using key ID or ARN, not alias)",
    // `PutObjectRequest$SSEKMSKeyId` on directory buckets) — plus a control that names no KMS
    // member at all, which asks KMS nothing and reports no key.
    let rows: [(&str, Option<&str>, Option<&str>); 3] = [
        (
            "the bare key id",
            Some(key_id.as_str()),
            Some(key_arn.as_str()),
        ),
        (
            "the key ARN",
            Some(key_arn.as_str()),
            Some(key_arn.as_str()),
        ),
        ("no KMS member at all", None, None),
    ];

    for (case, reference, expected) in rows {
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(reference) = reference {
            headers.push((ALGORITHM_HEADER, KMS_ALGORITHMS[0]));
            headers.push((KEY_ID_HEADER, reference));
        }
        let session = create_session(&router, name, &headers).await;
        assert_eq!(
            session.status, 200,
            "case {case:?}: the session must be vended, got {} with body {:?}",
            session.status,
            text(&session)
        );
        assert_eq!(
            session.header(KEY_ID_HEADER).as_deref(),
            expected,
            "case {case:?}: the session must report {expected:?} as the key its objects are \
             encrypted under, got {:?}",
            session.header(KEY_ID_HEADER)
        );
        // It is a real session beside that: the call vends the credentials a Zonal-endpoint
        // request presents.
        assert!(
            session.element("AccessKeyId").is_some(),
            "case {case:?}: the session must carry the credentials it vends, got {:?}",
            text(&session)
        );
    }

    // A reference KMS holds no key for can protect no object of the session, so the session
    // is refused with KMS's own relay rather than established around a key that does not
    // exist — and the write that would have used it is refused the same way.
    let unminted = "2a7d4e91-5c63-4b0e-9d81-7f4c2b6a3e55";
    let headers = [
        (ALGORITHM_HEADER, KMS_ALGORITHMS[0]),
        (KEY_ID_HEADER, unminted),
    ];
    let refused = create_session(&router, name, &headers).await;
    assert_eq!(
        refused.status, 400,
        "case \"a key id no key of this endpoint has\": expected HTTP 400, got {} with body \
         {:?}",
        refused.status,
        text(&refused)
    );
    assert_eq!(
        refused.error_code(),
        KMS_NOT_FOUND_CODE,
        "case \"a key id no key of this endpoint has\": expected the code \
         {KMS_NOT_FOUND_CODE:?}, got {:?}",
        refused.error_code()
    );
    assert_eq!(
        refused.element("AccessKeyId"),
        None,
        "case \"a key id no key of this endpoint has\": a refused session vends no \
         credentials, got {:?}",
        text(&refused)
    );
    let written = put_object(&router, name, "unresolved.bin", BODY, &headers).await;
    assert_eq!(
        written.status, 400,
        "case \"a write under a key id no key of this endpoint has\": expected HTTP 400, got \
         {} with body {:?}",
        written.status,
        text(&written)
    );
    assert_eq!(
        written.error_code(),
        KMS_NOT_FOUND_CODE,
        "case \"a write under a key id no key of this endpoint has\": expected the code \
         {KMS_NOT_FOUND_CODE:?}, got {:?}",
        written.error_code()
    );
}

/// One row of the missing-alias table: the case, the reference the write supplies, and the
/// sentence KMS words for it.
type MissingRow = (&'static str, String, String);

#[tokio::test]
async fn an_sse_kms_write_naming_an_alias_kms_has_no_key_for_is_refused_and_stores_nothing() {
    let router = super::kms_key_references::router_with_kms();
    bucket(&router, "kms-identity-missing").await;
    let source = put_object(&router, "kms-identity-missing", "source.bin", BODY, &[]).await;
    assert_eq!(
        source.status, 200,
        "the copy source must be written first, got {} with body {:?}",
        source.status,
        text(&source)
    );
    // One alias that DOES exist, so the refusals below are about the reference each names
    // rather than about an endpoint holding no aliases at all.
    let (held_id, _) = created_key(&router, "a key one alias really names").await;
    created_alias(&router, "alias/held", &held_id).await;

    // The rows differ along HOW the alias fails to name a key: a name nothing created, a
    // name that extends one that exists, a name differing from one that exists only in case
    // (an alias name is case-sensitive), and the same missing name given as an alias ARN.
    let rows: [MissingRow; 4] = [
        (
            "an alias nothing created",
            "alias/ghost".to_string(),
            format!("Alias {} is not found.", kms_arn("alias/ghost")),
        ),
        (
            "an alias name extending one that exists",
            "alias/held-missing".to_string(),
            format!("Alias {} is not found.", kms_arn("alias/held-missing")),
        ),
        (
            "an alias name differing from one that exists only in case",
            "alias/HELD".to_string(),
            format!("Alias {} is not found.", kms_arn("alias/HELD")),
        ),
        (
            "a missing alias given as an alias ARN",
            kms_arn("alias/ghost"),
            format!("Alias {} is not found.", kms_arn("alias/ghost")),
        ),
    ];

    for (index, (case, reference, message)) in rows.iter().enumerate() {
        for algorithm in KMS_ALGORITHMS {
            let headers: Vec<(&str, &str)> = vec![
                (ALGORITHM_HEADER, algorithm),
                (KEY_ID_HEADER, reference.as_str()),
            ];
            let key = format!("missing/{index}-{}.bin", algorithm.replace(':', "-"));
            let put = put_object(&router, "kms-identity-missing", &key, BODY, &headers).await;
            let copied = copy_object(
                &router,
                "kms-identity-missing",
                &format!("copy-{key}"),
                "kms-identity-missing/source.bin",
                &headers,
            )
            .await;
            let initiated = create_multipart_upload(
                &router,
                "kms-identity-missing",
                &format!("mpu-{key}"),
                &headers,
            )
            .await;

            for (operation, answer) in [
                ("PutObject", &put),
                ("CopyObject", &copied),
                ("CreateMultipartUpload", &initiated),
            ] {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}/{algorithm}: {operation} must be refused with HTTP 400, got \
                     {} with body {:?}",
                    answer.status,
                    text(answer)
                );
                assert_eq!(
                    answer.error_code(),
                    KMS_NOT_FOUND_CODE,
                    "case {case:?}/{algorithm}: {operation} must answer {KMS_NOT_FOUND_CODE:?}, \
                     got {:?}",
                    answer.error_code()
                );
                assert_eq!(
                    answer.error_message(),
                    *message,
                    "case {case:?}/{algorithm}: {operation} must carry KMS's own sentence \
                     {message:?}, got {:?}",
                    answer.error_message()
                );
            }

            // And nothing was stored by any of the three.
            assert_eq!(
                initiated.element("UploadId"),
                None,
                "case {case:?}/{algorithm}: a refused initiation records no upload, got {:?}",
                initiated.element("UploadId")
            );
            for stored in [key.clone(), format!("copy-{key}"), format!("mpu-{key}")] {
                let read = get_object(&router, "kms-identity-missing", &stored, "", &[]).await;
                assert_eq!(
                    read.status, 404,
                    "case {case:?}/{algorithm}: a refused write stores nothing, so a read of \
                     {stored:?} must answer 404, got {} with body {:?}",
                    read.status,
                    text(&read)
                );
            }
        }
    }
}
