// SPDX-License-Identifier: CC0-1.0

//! An object's SERVER-SIDE ENCRYPTION, end to end: what a write may ask for, what it is
//! stored under, and what every later read of it reports.
//!
//! RULE: `PutObject`'s modelled encryption members — `ServerSideEncryption`
//! (`x-amz-server-side-encryption`), `SSEKMSKeyId`, `SSEKMSEncryptionContext` and
//! `BucketKeyEnabled` — are part of the write: the object is recorded under the
//! configuration they name and `PutObject`, `GetObject` and `HeadObject` all report it
//! back in those same bound headers. The member list and the header each binds to is
//! `smithy_query(query="operation",operation="PutObject")`; the algorithm values are the
//! `ServerSideEncryption` enum
//! (`smithy_query(query="shape",shape="ServerSideEncryption")`).
//!
//! A write that names NONE of those members is stored under the bucket's default
//! encryption configuration instead — "Amazon S3 encrypts data with server-side encryption
//! by using Amazon S3 managed keys (SSE-S3) by default"
//! (`PutObjectRequest$ServerSideEncryption`) — so on a bucket no `PutBucketEncryption` has
//! configured the answers report [`DEFAULT_SSE_ALGORITHM`] and no KMS member at all. The
//! inheritance over a CONFIGURED bucket is the table in
//! [`super::object_default_encryption`]; what this suite pins is that an explicit member
//! stands on its own.
//!
//! Two refusals belong to the same rule, because they are what the emulator cannot carry
//! out rather than a member it drops:
//!
//! * a KMS-only member (a key id, an encryption context, an enabled Bucket Key) beside an
//!   algorithm that is not SSE-KMS is `InvalidArgument` (400) — "This parameter is allowed
//!   if and only if `SSEAlgorithm` is set to `aws:kms` or `aws:kms:dsse`" — and stores
//!   nothing;
//! * an SSE-C write keeps the scaffold's 501: S3 "doesn't store the encryption key you
//!   provide", so an object written with one could not be read back here.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`] and [`super::object_and_tagging_roundtrip`] own), so what
//! is pinned is the wire behaviour a client sees.

use service_com_amazonaws_s3_amazons3::state::DEFAULT_SSE_ALGORITHM;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};

/// `HEAD /<bucket>/<key>` — `HeadObject`, which reports the stored object's headers and
/// no body.
///
/// `pub(crate)`: the default-encryption suite beside this one reads its objects back
/// through the same two readers rather than standing up a second helper.
pub(crate) async fn head_object(router: &MultiServiceRouter, bucket: &str, key: &str) -> Answer {
    send_bytes(router, "HEAD", &format!("/{bucket}/{key}"), &[], b"").await
}

/// One row of the encryption table: the case, the request headers the write carries, and
/// the `(header, value)` pairs every answer about that object must then report. A header
/// named with `None` must be ABSENT from the answer.
type EncryptionRow<'a> = (
    &'a str,
    &'a [(&'a str, &'a str)],
    &'a [(&'a str, Option<&'a str>)],
);

/// The bound response headers an object's encryption is reported in, for the rows that
/// expect none of them at all.
const ENCRYPTION_HEADERS: &[&str] = &[
    "x-amz-server-side-encryption",
    "x-amz-server-side-encryption-aws-kms-key-id",
    "x-amz-server-side-encryption-context",
    "x-amz-server-side-encryption-bucket-key-enabled",
];

/// A KMS key id in the ARN form `SSEKMSKeyId` documents ("you can specify the key ID, key
/// alias, or the Amazon Resource Name (ARN) of the KMS key").
///
/// `pub(crate)`: the part-copy suite ([`super::multipart_part_copy_reported_members`]) names
/// the same key through this one constant rather than spelling an ARN of its own.
pub(crate) const KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// The ALIAS form of a reference, which is the second documented form: the Amazon Web
/// Services managed key for Amazon S3, a predefined Amazon Web Services alias that "exists
/// in every account and Region" and that KMS associates with its key the first time
/// something asks for it — so a write naming it needs no `CreateAlias` of its own and is
/// served ([`super::kms_key_references`] spans the other forms).
const KEY_ALIAS: &str = "alias/aws/s3";

/// A `SSEKMSEncryptionContext`: "a Base64-encoded string holding a UTF-8 encoded JSON of
/// additional encryption context key-value pairs" — here `{"project":"dags"}`.
const ENCRYPTION_CONTEXT: &str = "eyJwcm9qZWN0IjoiZGFncyJ9";

#[tokio::test]
async fn a_write_is_stored_under_the_encryption_it_names_and_every_read_reports_it() {
    // The KMS rows name a key KMS really holds: a write asking for SSE-KMS under a named key
    // resolves that reference against KMS before storing anything, so the key is made with
    // KMS's own operations over the same engine ([`super::kms_key_references`]). The
    // predefined `alias/aws/s3` row needs none: KMS associates such an alias with its Amazon
    // Web Services managed key the first time a caller names it.
    let router = super::kms_key_references::router_with_kms();
    let (_, key_arn) =
        super::kms_key_references::created_key(&router, "the key these writes name").await;
    let key_arn = key_arn.as_str();
    // The key the predefined `alias/aws/s3` names, as KMS reports it: an object carries the
    // KEY its reference resolved to rather than the spelling the request used.
    let managed_key_arn = super::kms_key_references::described_key_arn(&router, KEY_ALIAS).await;
    let managed_key_arn = managed_key_arn.as_str();
    bucket(&router, "encryption-bucket").await;

    // The rows differ along what the rule generalises over: WHICH algorithm (every value
    // of the `ServerSideEncryption` enum, including the two the issue never cited —
    // `aws:fsx` and `aws:backup`), and WHICH qualifying members accompany it (none, a key
    // id by ARN, a key id by alias with an encryption context, a Bucket Key switched on
    // and one switched off).
    let rows: &[EncryptionRow<'_>] = &[
        (
            "no encryption member at all, on a bucket with no configuration of its own",
            &[],
            &[
                ("x-amz-server-side-encryption", Some(DEFAULT_SSE_ALGORITHM)),
                ("x-amz-server-side-encryption-aws-kms-key-id", None),
                ("x-amz-server-side-encryption-bucket-key-enabled", None),
            ],
        ),
        (
            "SSE-S3, the AES256 value alone",
            &[("x-amz-server-side-encryption", "AES256")],
            &[
                ("x-amz-server-side-encryption", Some("AES256")),
                ("x-amz-server-side-encryption-aws-kms-key-id", None),
            ],
        ),
        (
            "SSE-KMS with the key named by ARN",
            &[
                ("x-amz-server-side-encryption", "aws:kms"),
                ("x-amz-server-side-encryption-aws-kms-key-id", key_arn),
            ],
            &[
                ("x-amz-server-side-encryption", Some("aws:kms")),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id",
                    Some(key_arn),
                ),
                ("x-amz-server-side-encryption-bucket-key-enabled", None),
            ],
        ),
        (
            "SSE-KMS with an alias, an encryption context and a Bucket Key",
            &[
                ("x-amz-server-side-encryption", "aws:kms"),
                ("x-amz-server-side-encryption-aws-kms-key-id", KEY_ALIAS),
                ("x-amz-server-side-encryption-context", ENCRYPTION_CONTEXT),
                (
                    "x-amz-server-side-encryption-bucket-key-enabled",
                    "true",
                ),
            ],
            &[
                ("x-amz-server-side-encryption", Some("aws:kms")),
                (
                    // Three spellings, one key: the object names the key the predefined
                    // Amazon Web Services alias resolved to.
                    "x-amz-server-side-encryption-aws-kms-key-id",
                    Some(managed_key_arn),
                ),
                (
                    "x-amz-server-side-encryption-bucket-key-enabled",
                    Some("true"),
                ),
            ],
        ),
        (
            "dual-layer SSE-KMS, with the Bucket Key switched off",
            &[
                ("x-amz-server-side-encryption", "aws:kms:dsse"),
                ("x-amz-server-side-encryption-aws-kms-key-id", key_arn),
                (
                    "x-amz-server-side-encryption-bucket-key-enabled",
                    "false",
                ),
            ],
            &[
                ("x-amz-server-side-encryption", Some("aws:kms:dsse")),
                (
                    // The header states that S3 USED an S3 Bucket Key, so an object written
                    // with the setting off carries none at all.
                    "x-amz-server-side-encryption-bucket-key-enabled",
                    None,
                ),
            ],
        ),
        (
            "the aws:fsx value",
            &[("x-amz-server-side-encryption", "aws:fsx")],
            &[("x-amz-server-side-encryption", Some("aws:fsx"))],
        ),
        (
            "the aws:backup value",
            &[("x-amz-server-side-encryption", "aws:backup")],
            &[("x-amz-server-side-encryption", Some("aws:backup"))],
        ),
    ];

    for (index, (case, request_headers, expected)) in rows.iter().enumerate() {
        let key = format!("encrypted/{index}.csv");
        let payload = format!("sepal,petal\n{index},{index}\n").into_bytes();

        let written = put_object(
            &router,
            "encryption-bucket",
            &key,
            &payload,
            request_headers,
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: a modelled encryption member is part of the write, so it must \
             answer 200, got {} with body {:?}",
            written.status,
            text(&written)
        );
        assert_header_set(case, "PutObject", &written, expected);

        // Read back through the two implemented readers: both report the configuration the
        // object was STORED under, not the one the read happened to ask about.
        let read = get_object(&router, "encryption-bucket", &key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the stored object must read back, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.body.as_ref(),
            payload.as_slice(),
            "case {case:?}: the stored bytes must come back unchanged"
        );
        assert_header_set(case, "GetObject", &read, expected);

        let headed = head_object(&router, "encryption-bucket", &key).await;
        assert_eq!(
            headed.status, 200,
            "case {case:?}: HeadObject of the stored object must answer 200, got {}",
            headed.status
        );
        assert_header_set(case, "HeadObject", &headed, expected);
    }

    // A write with no encryption member reports the bucket's default algorithm and no
    // KMS member: the bucket here was never configured, so its default is SSE-S3, which
    // has no key id, no encryption context and no S3 Bucket Key.
    let plain = put_object(&router, "encryption-bucket", "plain.csv", b"a,b\n", &[]).await;
    for header in ENCRYPTION_HEADERS {
        let expected = (*header == "x-amz-server-side-encryption")
            .then_some(DEFAULT_SSE_ALGORITHM)
            .map(str::to_string);
        assert_eq!(
            plain.header(header),
            expected,
            "case \"no encryption member at all\": {header} must be reported as \
             {expected:?}, got {:?}",
            plain.header(header)
        );
    }
}

#[tokio::test]
async fn an_encryption_member_s3_could_not_honour_is_refused_and_stores_nothing() {
    let router = router();
    bucket(&router, "encryption-refusals").await;

    // The rows differ along WHICH KMS-only member is named and WHICH algorithm (or none)
    // it is named beside. Each is refused with S3's `InvalidArgument`; the row for an
    // algorithm the model's enum does not declare is asserted separately below, because
    // the boundary validator refuses THAT through the framework's own channel before the
    // handler runs.
    let rows: &[(&str, &[(&str, &str)])] = &[
        (
            "a key id beside AES256",
            &[
                ("x-amz-server-side-encryption", "AES256"),
                ("x-amz-server-side-encryption-aws-kms-key-id", KEY_ARN),
            ],
        ),
        (
            "a key id with no algorithm at all",
            &[("x-amz-server-side-encryption-aws-kms-key-id", KEY_ARN)],
        ),
        (
            "an encryption context beside AES256",
            &[
                ("x-amz-server-side-encryption", "AES256"),
                ("x-amz-server-side-encryption-context", ENCRYPTION_CONTEXT),
            ],
        ),
        (
            "an encryption context with no algorithm at all",
            &[("x-amz-server-side-encryption-context", ENCRYPTION_CONTEXT)],
        ),
        (
            "a Bucket Key beside aws:fsx",
            &[
                ("x-amz-server-side-encryption", "aws:fsx"),
                (
                    "x-amz-server-side-encryption-bucket-key-enabled",
                    "true",
                ),
            ],
        ),
    ];

    for (case, headers) in rows {
        let refused = put_object(
            &router,
            "encryption-refusals",
            "refused.csv",
            b"a,b\n",
            headers,
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: an encryption member S3 would not honour is refused with HTTP \
             400, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            "InvalidArgument",
            "case {case:?}: expected the InvalidArgument code, got {:?}",
            refused.error_code()
        );
        assert!(
            !refused.error_message().is_empty(),
            "case {case:?}: the refusal must say which member it refused, got {:?}",
            refused.error_message()
        );
        let absent = get_object(&router, "encryption-refusals", "refused.csv", "", &[]).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: a refused write must store nothing, got GetObject {}",
            absent.status
        );
    }

    // An algorithm outside the model's `ServerSideEncryption` enum is refused too — by
    // the boundary validator, before the handler runs, with the code S3 answers a bad
    // header value: `InvalidArgument`, HTTP 400. What this pins is that it is REFUSED
    // rather than stored and reported back as an encryption no S3 knows; the handler
    // carries the same guard behind it, the way `PutBucketEncryption` guards its own
    // `SSEAlgorithm`.
    let unmodelled_algorithm = put_object(
        &router,
        "encryption-refusals",
        "unmodelled.csv",
        b"a,b\n",
        &[("x-amz-server-side-encryption", "AES128")],
    )
    .await;
    assert_eq!(
        unmodelled_algorithm.status, 400,
        "case \"an algorithm the model does not declare\": expected HTTP 400, got {} with \
         body {:?}",
        unmodelled_algorithm.status,
        text(&unmodelled_algorithm)
    );
    assert_eq!(
        unmodelled_algorithm.error_code(),
        "InvalidArgument",
        "case \"an algorithm the model does not declare\": expected the boundary's refusal \
         code, got {:?}",
        unmodelled_algorithm.error_code()
    );
    let absent = get_object(&router, "encryption-refusals", "unmodelled.csv", "", &[]).await;
    assert_eq!(
        absent.status, 404,
        "case \"an algorithm the model does not declare\": a refused write must store \
         nothing, got GetObject {}",
        absent.status
    );

    // An SSE-C write whose three members do not name one usable key is refused for the
    // member that does not hold, and stores nothing: this one supplies the algorithm and the
    // key with no digest of it, so the integrity check the third member exists for cannot be
    // made at all.
    let customer_key = put_object(
        &router,
        "encryption-refusals",
        "sse-c.csv",
        b"a,b\n",
        &[
            (
                "x-amz-server-side-encryption-customer-algorithm",
                "AES256",
            ),
            (
                "x-amz-server-side-encryption-customer-key",
                "MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE=",
            ),
        ],
    )
    .await;
    assert_eq!(
        customer_key.status, 400,
        "case \"a customer-provided key with no digest\": expected HTTP 400, got {} with body \
         {:?}",
        customer_key.status,
        text(&customer_key)
    );
    assert_eq!(
        customer_key.error_code(),
        "InvalidArgument",
        "case \"a customer-provided key with no digest\": expected InvalidArgument, got {:?}",
        customer_key.error_code()
    );
    let absent_sse_c = get_object(&router, "encryption-refusals", "sse-c.csv", "", &[]).await;
    assert_eq!(
        absent_sse_c.status, 404,
        "case \"a customer-provided key with no digest\": a refused write must store nothing, \
         got GetObject {}",
        absent_sse_c.status
    );
}

/// One row of the replacement table: the case, the headers the write carries, and the
/// `ServerSideEncryption` every read must report afterwards (`None` for none at all).
type ReplacementRow<'a> = (&'a str, &'a [(&'a str, &'a str)], Option<&'a str>);

#[tokio::test]
async fn a_replacing_write_replaces_the_encryption_the_key_was_stored_under() {
    // "You cannot use PutObject to only update a single piece of metadata for an existing
    // object. You must put the entire object with updated metadata": the encryption is
    // part of the object, so a replacing write that names another one — or none — leaves
    // the key under THAT, and the readers follow.
    let router = super::kms_key_references::router_with_kms();
    let (_, key_arn) =
        super::kms_key_references::created_key(&router, "the key the first write names").await;
    let key_arn = key_arn.as_str();
    bucket(&router, "encryption-replacement").await;

    // (case, the headers the write carries, what the readers must report afterwards)
    let writes: &[ReplacementRow<'_>] = &[
        (
            "first written with SSE-KMS",
            &[
                ("x-amz-server-side-encryption", "aws:kms"),
                ("x-amz-server-side-encryption-aws-kms-key-id", key_arn),
            ],
            Some("aws:kms"),
        ),
        (
            "replaced with SSE-S3",
            &[("x-amz-server-side-encryption", "AES256")],
            Some("AES256"),
        ),
        // The key was under SSE-KMS with a key of its own; replacing it with a write that
        // names no encryption member puts it back under the bucket's default — SSE-S3 on
        // this never-configured bucket — rather than leaving the KMS configuration behind.
        (
            "replaced with no encryption member",
            &[],
            Some(DEFAULT_SSE_ALGORITHM),
        ),
    ];

    for (case, headers, expected) in writes {
        let written = put_object(
            &router,
            "encryption-replacement",
            "train/train.csv",
            b"sepal,petal\n1,2\n",
            headers,
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the write must answer 200, got {} with body {:?}",
            written.status,
            text(&written)
        );
        let read = get_object(
            &router,
            "encryption-replacement",
            "train/train.csv",
            "",
            &[],
        )
        .await;
        assert_eq!(
            read.header("x-amz-server-side-encryption").as_deref(),
            *expected,
            "case {case:?}: a read must report the encryption of the version now stored, \
             got {:?}",
            read.header("x-amz-server-side-encryption")
        );
        // The key id goes with the algorithm it belonged to: nothing of a replaced
        // version's encryption survives it.
        if expected != &Some("aws:kms") {
            assert_eq!(
                read.header("x-amz-server-side-encryption-aws-kms-key-id"),
                None,
                "case {case:?}: the replaced version's key id must not survive, got {:?}",
                read.header("x-amz-server-side-encryption-aws-kms-key-id")
            );
        }
    }
}

/// Assert that `answer` carries exactly the `(header, value)` expectations of a row:
/// `Some(value)` must be reported with that value, `None` must be absent.
fn assert_header_set(
    case: &str,
    operation: &str,
    answer: &Answer,
    expected: &[(&str, Option<&str>)],
) {
    for (header, value) in expected {
        assert_eq!(
            answer.header(header).as_deref(),
            *value,
            "case {case:?}: {operation} must report {header} as {value:?}, got {:?}",
            answer.header(header)
        );
    }
}
