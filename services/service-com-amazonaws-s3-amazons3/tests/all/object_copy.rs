// SPDX-License-Identifier: CC0-1.0

//! `CopyObject`, end to end: what the copy carries over from the source object, what it
//! takes from the request, and what the DESTINATION decides.
//!
//! RULE: `CopyObject` records the bytes of the object `x-amz-copy-source` names at
//! `<Bucket>/<Key>` as a write of its own — so the destination object has its own
//! last-modified time, its own version id where the destination bucket is versioned, the
//! `STANDARD` storage class unless the request names one ("if the
//! `x-amz-storage-class` header is not used, the copied object will be stored in the
//! `STANDARD` Storage Class by default"), and the encryption of the DESTINATION: "when
//! copying an object, if you don't specify encryption information in your copy request,
//! the encryption setting of the target object is set to the default encryption
//! configuration of the destination bucket … If the destination bucket has a different
//! default encryption configuration, Amazon S3 uses the corresponding encryption key to
//! encrypt the target object copy"
//! (`smithy_query(query="doc",shape="CopyObjectRequest",member="ServerSideEncryption")`).
//! What IS carried over is what the two directives say: the user and system metadata
//! unless `x-amz-metadata-directive` is `REPLACE`, and the tag set unless
//! `x-amz-tagging-directive` is `REPLACE`, both defaulting to `COPY`
//! (`…member="MetadataDirective"` and `…member="TaggingDirective"`), except
//! `x-amz-website-redirect-location`, which "is unique to each object and is not copied".
//!
//! Two refusal families belong to the same rule, because each is a documented outcome of
//! the copy rather than a member being dropped:
//!
//! * the four `x-amz-copy-source-if-*` preconditions are judged against the stored source
//!   version, and one that does not hold answers HTTP 412 `PreconditionFailed`, names the
//!   member that did not hold in its `<Condition>` and writes
//!   nothing — including the two combinations the members document, where the entity-tag
//!   member decides and the date member beside it is ignored;
//! * a source whose storage class is `GLACIER` or `DEEP_ARCHIVE` has to be restored first
//!   ("before using an object as a source object for the copy operation, you must restore
//!   a copy of it if …"), which is refused with the storage-class refusal worded for the
//!   source object — `InvalidObjectState` (403);
//!
//! Every assertion drives a real HTTP request through the router (the harness and the
//! object, encryption, tagging and versioning helpers are the ones the suites beside this
//! one own), so what is pinned is the wire behaviour a client sees.

use service_com_amazonaws_s3_amazons3::state::{DEFAULT_SSE_ALGORITHM, DEFAULT_STORAGE_CLASS};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_encryption::{put_encryption, Rule};
use super::bucket_lifecycle::{router, send_bytes, Answer};
use super::bucket_tagging::{tag, tag_set_of};
use super::object_and_tagging_roundtrip::{bucket, expected_etag, get_object, put_object, text};
use super::object_encryption::head_object;
use super::object_tagging::get_object_tagging;
use super::object_versioning::set_versioning;

/// A KMS key in two of the three forms `KMSMasterKeyID` documents ("you can specify the
/// key ID, key alias, or the Amazon Resource Name (ARN) of the KMS key").
///
/// Only the rows that name a reference NOTHING resolves use these: a key an object write is
/// stored under is resolved against KMS before anything is stored, so the rows whose write
/// has to be served name a key made with KMS's own operations
/// ([`super::kms_key_references`]) instead.
const DESTINATION_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:111122223333:key/2222bbbb-22bb-22bb-22bb-222222222222";
const DESTINATION_KEY_ALIAS: &str = "alias/destination-cmk";

/// A `SSEKMSEncryptionContext`: "a Base64-encoded string holding a UTF-8 encoded JSON of
/// additional encryption context key-value pairs" — here `{"stage":"copy"}`.
const ENCRYPTION_CONTEXT: &str = "eyJzdGFnZSI6ImNvcHkifQ==";

/// A time before every object this suite writes, and one after all of them, for the
/// `x-amz-copy-source-if-*-since` members.
///
/// Spelled as the date-time the framework reads a header-bound timestamp as when the
/// member carries no `timestampFormat` of its own — which these four do not
/// (`smithy_query(query="operation",operation="CopyObject")`) — rather than as an
/// HTTP-date. What this suite pins is which side of the write each value falls on, which
/// is the same whichever encoding carries it.
const BEFORE_EVERY_WRITE: &str = "2015-01-01T00:00:00Z";
const AFTER_EVERY_WRITE: &str = "2030-01-01T00:00:00Z";

/// `PUT /<bucket>/<key>` carrying `x-amz-copy-source` — `CopyObject`. The header is what
/// separates this route from `PutObject` on the same method and path, so it is always
/// sent.
async fn copy_object(
    router: &MultiServiceRouter,
    destination_bucket: &str,
    destination_key: &str,
    copy_source: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let mut all: Vec<(&str, &str)> = vec![("x-amz-copy-source", copy_source)];
    all.extend_from_slice(headers);
    send_bytes(
        router,
        "PUT",
        &format!("/{destination_bucket}/{destination_key}"),
        &all,
        b"",
    )
    .await
}

/// `length` bytes spanning the whole 0..=255 range, from a fixed seed, so a payload is
/// large and byte-varied without anything random: a 32-bit LCG (the "ANSI C" multiplier
/// and increment), taking the high byte of each step. The same generator the
/// object-round-trip suite uses for its megabyte payload.
fn generated_bytes(length: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    (0..length)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (state >> 16) as u8
        })
        .collect()
}

/// Write `payload` at `bucket`/`key` with `headers`, failing the test if the write did
/// not take — a copy needs something to copy.
async fn source_object(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    payload: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    let written = put_object(router, bucket_name, key, payload, headers).await;
    assert_eq!(
        written.status, 200,
        "the source object {bucket_name}/{key} must be written first, got {} with body {:?}",
        written.status,
        text(&written)
    );
    written
}

/// One row of the copy table: the case, the source key, the payload, the destination
/// bucket's name suffix and key, and the `x-amz-copy-source` value as the client spells it
/// (URL-encoded, which is what the member requires).
struct CopyRow {
    case: &'static str,
    source_key: &'static str,
    payload: Vec<u8>,
    destination_key: &'static str,
    /// `true` when the destination is the SOURCE bucket, under another key.
    into_source_bucket: bool,
    /// The `x-amz-copy-source` value, with `{bucket}` standing for the source bucket's
    /// name, so a row can spell a percent-encoded key.
    copy_source: &'static str,
}

#[tokio::test]
async fn a_copy_records_the_source_bytes_at_the_destination_and_leaves_the_source_alone() {
    let router = router();
    bucket(&router, "copy-origin").await;
    bucket(&router, "copy-target").await;

    // The rows differ along what the rule generalises over: the payload's size (empty,
    // one byte, a kilobyte of arbitrary bytes) and type (ASCII, UTF-8, bytes including
    // NUL and 0xFF), the key's shape (flat, a `dags/` prefix, a deep path, a key whose
    // characters the copy source must percent-encode), whether the destination is another
    // bucket or the same one under a different key, and whether the copy source carries
    // the leading slash its pattern allows.
    let rows: Vec<CopyRow> = vec![
        CopyRow {
            case: "the empty object, across buckets",
            source_key: "empty",
            payload: Vec::new(),
            destination_key: "copies/empty",
            into_source_bucket: false,
            copy_source: "{bucket}/empty",
        },
        CopyRow {
            case: "a single byte, with the leading slash the pattern allows",
            source_key: "a.txt",
            payload: b"a".to_vec(),
            destination_key: "copies/a.txt",
            into_source_bucket: false,
            copy_source: "/{bucket}/a.txt",
        },
        CopyRow {
            case: "a nested key under the dags/ prefix",
            source_key: "dags/pipeline.py",
            payload: b"print('hello')\n".to_vec(),
            destination_key: "dags/backup/pipeline.py",
            into_source_bucket: false,
            copy_source: "{bucket}/dags/pipeline.py",
        },
        CopyRow {
            case: "a deep path copied into the SOURCE bucket under another key",
            source_key: "a/b/c/d/report.csv",
            payload: b"sepal,petal\n1,2\n".to_vec(),
            destination_key: "a/b/c/d/report.csv.bak",
            into_source_bucket: true,
            copy_source: "{bucket}/a/b/c/d/report.csv",
        },
        CopyRow {
            case: "UTF-8 text",
            source_key: "unicode.txt",
            payload: "DAG — données ✓".as_bytes().to_vec(),
            destination_key: "copies/unicode.txt",
            into_source_bucket: false,
            copy_source: "{bucket}/unicode.txt",
        },
        CopyRow {
            case: "bytes that are not text, including NUL and 0xFF",
            source_key: "binary.bin",
            payload: vec![0u8, 1, 2, 0xFE, 0xFF, 0x80, 0x00, 0x7F],
            destination_key: "copies/binary.bin",
            into_source_bucket: false,
            copy_source: "{bucket}/binary.bin",
        },
        CopyRow {
            case: "a kilobyte of arbitrary bytes",
            source_key: "bulk/payload.dat",
            payload: generated_bytes(1_024, 0x0C0F_FEE1),
            destination_key: "copies/bulk/payload.dat",
            into_source_bucket: false,
            copy_source: "{bucket}/bulk/payload.dat",
        },
        // "The value must be URL-encoded": the documented example is `test_file(3).png`
        // appearing as `test_file%283%29.png`, so the parens travel percent-encoded and
        // the key the copy reads is the decoded one.
        CopyRow {
            case: "a key whose characters the copy source percent-encodes",
            source_key: "test_file(3).png",
            payload: b"\x89PNG\r\n\x1a\n".to_vec(),
            destination_key: "copies/test_file(3).png",
            into_source_bucket: false,
            copy_source: "{bucket}/test_file%283%29.png",
        },
    ];

    for row in &rows {
        let CopyRow {
            case,
            source_key,
            payload,
            destination_key,
            into_source_bucket,
            copy_source,
        } = row;
        let destination_bucket = if *into_source_bucket {
            "copy-origin"
        } else {
            "copy-target"
        };
        source_object(&router, "copy-origin", source_key, payload, &[]).await;

        let copied = copy_object(
            &router,
            destination_bucket,
            destination_key,
            &copy_source.replace("{bucket}", "copy-origin"),
            &[],
        )
        .await;
        assert_eq!(
            copied.status, 200,
            "case {case:?}: a copy of a stored object must answer 200, got {} with body {:?}",
            copied.status,
            text(&copied)
        );
        // "Container for all response elements": the entity tag of the object the copy
        // created, which for the same bytes is the same digest the write of the source
        // reported.
        assert_eq!(
            copied.element("ETag"),
            Some(expected_etag(payload)),
            "case {case:?}: CopyObjectResult must report the copy's entity tag, got body {:?}",
            text(&copied)
        );
        assert!(
            copied
                .element("LastModified")
                .is_some_and(|at| !at.is_empty()),
            "case {case:?}: CopyObjectResult must report when the copy was written, got \
             body {:?}",
            text(&copied)
        );

        // The destination holds exactly the source's bytes...
        let read = get_object(&router, destination_bucket, destination_key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the copy must read back, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.body.as_ref(),
            payload.as_slice(),
            "case {case:?}: the copy must hold the source's bytes, got {} bytes",
            read.body.len()
        );
        assert_eq!(
            read.header("etag"),
            Some(expected_etag(payload)),
            "case {case:?}: a read of the copy must report the copy's entity tag, got {:?}",
            read.header("etag")
        );

        // ...and the SOURCE is untouched: a copy is not a move.
        let source_read = get_object(&router, "copy-origin", source_key, "", &[]).await;
        assert_eq!(
            source_read.status, 200,
            "case {case:?}: the source must still read back after the copy, got {}",
            source_read.status
        );
        assert_eq!(
            source_read.body.as_ref(),
            payload.as_slice(),
            "case {case:?}: the source's bytes must be unchanged by the copy"
        );
    }
}

/// What an answer about a copied object must report about its encryption, each `None`
/// where the bound header must be ABSENT.
#[derive(Debug)]
struct Reported {
    algorithm: Option<&'static str>,
    key_id: Option<&'static str>,
    bucket_key: Option<&'static str>,
    /// The `SSEKMSEncryptionContext` the COPY's own answer must report. `GetObject` and
    /// `HeadObject` are not asserted against it: their output shapes declare no such
    /// member (`smithy_query(query="operation",operation="GetObject")`), so a read of any
    /// object reports none.
    context: Option<&'static str>,
}

/// One row of the re-encryption table: the case, the source bucket's default
/// configuration, the destination bucket's, the encryption headers the COPY request
/// carries, and what every answer about the copy must then report.
struct EncryptionRow {
    case: &'static str,
    source_rules: Option<Vec<Rule>>,
    destination_rules: Option<Vec<Rule>>,
    request_headers: Vec<(&'static str, &'static str)>,
    expected: Reported,
}

#[tokio::test]
async fn a_copy_is_encrypted_under_the_destination_rule_not_the_source_one() {
    // Each side's default names a key KMS really holds: a write stored under a KMS key
    // resolves that reference against KMS before storing anything, and the SOURCE object of
    // every copy below is such a write ([`super::kms_key_references`]). The ids are minted
    // by KMS rather than spelled here, and the rows carry `&'static str`, so they are kept
    // for the process.
    let router = super::kms_key_references::router_with_kms();
    let (_, source_key_arn) =
        super::kms_key_references::created_key(&router, "the source bucket's key").await;
    let (destination_key_id, destination_key_arn) =
        super::kms_key_references::created_key(&router, "the destination bucket's key").await;
    super::kms_key_references::created_alias(
        &router,
        DESTINATION_KEY_ALIAS,
        &destination_key_id,
    )
    .await;
    let source_key_arn: &'static str = Box::leak(source_key_arn.into_boxed_str());
    let destination_key_arn: &'static str = Box::leak(destination_key_arn.into_boxed_str());

    // The rows differ along what the rule generalises over: WHICH algorithm each side's
    // default names (SSE-KMS, dual-layer SSE-KMS, SSE-S3, and `aws:fsx`, a published value
    // no copy fixture cites), WHICH form the destination's key takes (ARN, alias, none),
    // whether the destination is configured at all, and whether the REQUEST names an
    // encryption of its own — in which case "the encryption setting in your request takes
    // precedence".
    let rows: Vec<EncryptionRow> = vec![
        EncryptionRow {
            case: "out of an SSE-KMS bucket into one no PutBucketEncryption configured",
            source_rules: Some(vec![Rule::by_default("aws:kms").with_key(source_key_arn)]),
            destination_rules: None,
            request_headers: Vec::new(),
            expected: Reported {
                algorithm: Some(DEFAULT_SSE_ALGORITHM),
                key_id: None,
                bucket_key: None,
                context: None,
            },
        },
        EncryptionRow {
            case: "out of an SSE-KMS bucket into one whose default names another CMK",
            source_rules: Some(vec![Rule::by_default("aws:kms").with_key(source_key_arn)]),
            destination_rules: Some(vec![
                Rule::by_default("aws:kms").with_key(destination_key_arn)
            ]),
            request_headers: Vec::new(),
            expected: Reported {
                algorithm: Some("aws:kms"),
                key_id: Some(destination_key_arn),
                bucket_key: None,
                context: None,
            },
        },
        EncryptionRow {
            case: "out of an SSE-S3 bucket into an SSE-KMS one with the Bucket Key on",
            source_rules: Some(vec![Rule::by_default("AES256")]),
            destination_rules: Some(vec![Rule::by_default("aws:kms")
                .with_key(DESTINATION_KEY_ALIAS)
                .with_bucket_key(true)]),
            request_headers: Vec::new(),
            expected: Reported {
                algorithm: Some("aws:kms"),
                // The rule names the key by ALIAS and the object names the key: a copy is a
                // write, so the reference resolves through KMS to the one key's ARN.
                key_id: Some(destination_key_arn),
                bucket_key: Some("true"),
                context: None,
            },
        },
        EncryptionRow {
            case: "out of an SSE-KMS bucket into a dual-layer SSE-KMS one",
            source_rules: Some(vec![Rule::by_default("aws:kms").with_key(source_key_arn)]),
            destination_rules: Some(vec![Rule::by_default("aws:kms:dsse")
                .with_key(destination_key_arn)
                .with_bucket_key(false)]),
            request_headers: Vec::new(),
            expected: Reported {
                algorithm: Some("aws:kms:dsse"),
                key_id: Some(destination_key_arn),
                // The destination's rule switches the Bucket Key OFF, and an object written
                // without one carries no such header at all.
                bucket_key: None,
                context: None,
            },
        },
        EncryptionRow {
            case: "out of an unconfigured bucket into one whose default is aws:fsx",
            source_rules: None,
            destination_rules: Some(vec![Rule::by_default("aws:fsx")]),
            request_headers: Vec::new(),
            expected: Reported {
                algorithm: Some("aws:fsx"),
                key_id: None,
                bucket_key: None,
                context: None,
            },
        },
        EncryptionRow {
            case: "out of an SSE-KMS bucket into an SSE-KMS one, the request naming SSE-S3",
            source_rules: Some(vec![Rule::by_default("aws:kms").with_key(source_key_arn)]),
            destination_rules: Some(vec![
                Rule::by_default("aws:kms").with_key(destination_key_arn)
            ]),
            request_headers: vec![("x-amz-server-side-encryption", "AES256")],
            expected: Reported {
                algorithm: Some("AES256"),
                key_id: None,
                bucket_key: None,
                context: None,
            },
        },
        EncryptionRow {
            case: "the request naming SSE-KMS with a key and an encryption context of its own",
            source_rules: Some(vec![Rule::by_default("AES256")]),
            destination_rules: Some(vec![Rule::by_default("AES256")]),
            request_headers: vec![
                ("x-amz-server-side-encryption", "aws:kms"),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id",
                    destination_key_arn,
                ),
                ("x-amz-server-side-encryption-context", ENCRYPTION_CONTEXT),
                ("x-amz-server-side-encryption-bucket-key-enabled", "true"),
            ],
            expected: Reported {
                algorithm: Some("aws:kms"),
                key_id: Some(destination_key_arn),
                bucket_key: Some("true"),
                context: Some(ENCRYPTION_CONTEXT),
            },
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let EncryptionRow {
            case,
            source_rules,
            destination_rules,
            request_headers,
            expected,
        } = row;
        let origin = format!("copy-sse-origin-{index}");
        let target = format!("copy-sse-target-{index}");
        bucket(&router, &origin).await;
        bucket(&router, &target).await;
        for (name, rules) in [(&origin, source_rules), (&target, destination_rules)] {
            if let Some(rules) = rules {
                let configured = put_encryption(&router, name, rules).await;
                assert_eq!(
                    configured.status, 200,
                    "case {case:?}: the bucket {name:?} must take its default encryption \
                     first, got {} with body {:?}",
                    configured.status,
                    text(&configured)
                );
            }
        }

        // The payload varies with the row, so the rule is exercised over different copies
        // rather than one recorded body.
        let key = format!("inventory/{index}.csv");
        let payload = format!("row {index}: {case}\n").into_bytes();
        let written = source_object(&router, &origin, &key, &payload, &[]).await;
        let source_algorithm = written.header("x-amz-server-side-encryption");

        let copied = copy_object(
            &router,
            &target,
            &key,
            &format!("{origin}/{key}"),
            request_headers,
        )
        .await;
        assert_eq!(
            copied.status, 200,
            "case {case:?}: the copy must answer 200, got {} with body {:?}",
            copied.status,
            text(&copied)
        );
        assert_reported(case, "CopyObject", &copied, expected);
        assert_eq!(
            copied
                .header("x-amz-server-side-encryption-context")
                .as_deref(),
            expected.context,
            "case {case:?}: CopyObject must report the encryption context as {:?}, got {:?}",
            expected.context,
            copied.header("x-amz-server-side-encryption-context")
        );

        // Every later reader of the copy reports the configuration it was STORED under.
        let read = get_object(&router, &target, &key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the copy must read back, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.body.as_ref(),
            payload.as_slice(),
            "case {case:?}: the copy must hold the source's bytes"
        );
        assert_reported(case, "GetObject", &read, expected);
        let headed = head_object(&router, &target, &key).await;
        assert_reported(case, "HeadObject", &headed, expected);

        // The SOURCE object is still under its own bucket's rule: a copy re-encrypts the
        // TARGET, and changes nothing about what it read.
        let source_read = get_object(&router, &origin, &key, "", &[]).await;
        assert_eq!(
            source_read.header("x-amz-server-side-encryption"),
            source_algorithm,
            "case {case:?}: the source must still report the encryption it was written \
             under, got {:?}",
            source_read.header("x-amz-server-side-encryption")
        );
    }
}

/// One row of the directive table: the case, the headers the copy carries, and what the
/// destination object must then report — its content type, its `x-amz-meta-stage`, its tag
/// set and its storage class, each `None` where the header must be ABSENT.
struct DirectiveRow {
    case: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    content_type: Option<&'static str>,
    stage: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
    storage_class: Option<&'static str>,
    redirect: Option<&'static str>,
}

#[tokio::test]
async fn the_directives_decide_what_a_copy_carries_over_from_the_source() {
    let router = router();
    bucket(&router, "copy-directive-origin").await;
    bucket(&router, "copy-directive-target").await;

    // The source object carries every member a directive governs: a content type, a
    // cache-control, user metadata, a tag set, a non-default storage class and a website
    // redirect location.
    source_object(
        &router,
        "copy-directive-origin",
        "source.csv",
        b"sepal,petal\n1,2\n",
        &[
            ("content-type", "text/csv"),
            ("cache-control", "max-age=60"),
            ("x-amz-meta-stage", "raw"),
            ("x-amz-tagging", "team=data&stage=raw"),
            ("x-amz-storage-class", "STANDARD_IA"),
            ("x-amz-website-redirect-location", "/origin.html"),
        ],
    )
    .await;

    // The rows differ along WHICH directive is named and WHICH value it takes, and along
    // what the request carries beside it — the dimensions the two directives generalise
    // over. The storage class and the redirect location are in every row, because neither
    // follows a directive: the class defaults to `STANDARD` and the redirect is "unique to
    // each object and is not copied".
    let rows: Vec<DirectiveRow> = vec![
        DirectiveRow {
            case: "no directive at all, so COPY is the default for both",
            headers: Vec::new(),
            content_type: Some("text/csv"),
            stage: Some("raw"),
            tags: vec![("team", "data"), ("stage", "raw")],
            storage_class: None,
            redirect: None,
        },
        DirectiveRow {
            case: "both directives named COPY explicitly",
            headers: vec![
                ("x-amz-metadata-directive", "COPY"),
                ("x-amz-tagging-directive", "COPY"),
            ],
            content_type: Some("text/csv"),
            stage: Some("raw"),
            tags: vec![("team", "data"), ("stage", "raw")],
            storage_class: None,
            redirect: None,
        },
        DirectiveRow {
            case: "metadata REPLACE with new metadata, the tag set still copied",
            headers: vec![
                ("x-amz-metadata-directive", "REPLACE"),
                ("content-type", "application/json"),
                ("x-amz-meta-stage", "curated"),
            ],
            content_type: Some("application/json"),
            stage: Some("curated"),
            tags: vec![("team", "data"), ("stage", "raw")],
            storage_class: None,
            redirect: None,
        },
        // A `REPLACE` that names no metadata replaces the source's with nothing, so the
        // copy carries S3's default `Content-Type` for a write that names none rather
        // than the source's `text/csv`.
        DirectiveRow {
            case: "metadata REPLACE naming none, which replaces it with nothing",
            headers: vec![("x-amz-metadata-directive", "REPLACE")],
            content_type: Some("binary/octet-stream"),
            stage: None,
            tags: vec![("team", "data"), ("stage", "raw")],
            storage_class: None,
            redirect: None,
        },
        DirectiveRow {
            case: "tagging REPLACE with a new tag set, the metadata still copied",
            headers: vec![
                ("x-amz-tagging-directive", "REPLACE"),
                ("x-amz-tagging", "team=platform&tier=gold"),
            ],
            content_type: Some("text/csv"),
            stage: Some("raw"),
            tags: vec![("team", "platform"), ("tier", "gold")],
            storage_class: None,
            redirect: None,
        },
        DirectiveRow {
            case: "tagging REPLACE naming no header, which leaves the copy untagged",
            headers: vec![("x-amz-tagging-directive", "REPLACE")],
            content_type: Some("text/csv"),
            stage: Some("raw"),
            tags: Vec::new(),
            storage_class: None,
            redirect: None,
        },
        DirectiveRow {
            case: "a storage class and a redirect location named by the request",
            headers: vec![
                ("x-amz-storage-class", "ONEZONE_IA"),
                ("x-amz-website-redirect-location", "/copy.html"),
            ],
            content_type: Some("text/csv"),
            stage: Some("raw"),
            tags: vec![("team", "data"), ("stage", "raw")],
            storage_class: Some("ONEZONE_IA"),
            redirect: Some("/copy.html"),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let DirectiveRow {
            case,
            headers,
            content_type,
            stage,
            tags,
            storage_class,
            redirect,
        } = row;
        let key = format!("copies/{index}.csv");
        let copied = copy_object(
            &router,
            "copy-directive-target",
            &key,
            "copy-directive-origin/source.csv",
            headers,
        )
        .await;
        assert_eq!(
            copied.status, 200,
            "case {case:?}: the copy must answer 200, got {} with body {:?}",
            copied.status,
            text(&copied)
        );

        let read = get_object(&router, "copy-directive-target", &key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the copy must read back, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.header("content-type").as_deref(),
            *content_type,
            "case {case:?}: the copy's Content-Type must be {content_type:?}, got {:?}",
            read.header("content-type")
        );
        assert_eq!(
            read.header("x-amz-meta-stage").as_deref(),
            *stage,
            "case {case:?}: the copy's x-amz-meta-stage must be {stage:?}, got {:?}",
            read.header("x-amz-meta-stage")
        );
        // "Amazon S3 returns this header for all objects except for S3 Standard storage
        // class objects", so a copy that named no class reports none at all — the source's
        // `STANDARD_IA` is NOT carried over.
        assert_eq!(
            read.header("x-amz-storage-class").as_deref(),
            *storage_class,
            "case {case:?}: the copy's storage class must be reported as \
             {storage_class:?} (the default being {DEFAULT_STORAGE_CLASS}), got {:?}",
            read.header("x-amz-storage-class")
        );
        assert_eq!(
            read.header("x-amz-website-redirect-location").as_deref(),
            *redirect,
            "case {case:?}: the redirect location is not copied, so it must be \
             {redirect:?}, got {:?}",
            read.header("x-amz-website-redirect-location")
        );

        let reported = get_object_tagging(&router, "copy-directive-target", &key, "").await;
        assert_eq!(
            reported.status, 200,
            "case {case:?}: the copy's tag set must be readable, got {} with body {:?}",
            reported.status,
            text(&reported)
        );
        let expected_tags: Vec<(String, String)> =
            tags.iter().map(|(key, value)| tag(key, value)).collect();
        assert_eq!(
            tag_set_of(&reported),
            expected_tags,
            "case {case:?}: the copy's tag set must be {expected_tags:?}, got {:?}",
            tag_set_of(&reported)
        );
    }
}

/// One row of the precondition table: the case, the `x-amz-copy-source-if-*` headers the
/// copy carries, and whether the copy must be served.
struct PreconditionRow {
    case: &'static str,
    headers: Vec<(String, String)>,
    served: bool,
}

#[tokio::test]
async fn a_copy_source_precondition_that_does_not_hold_answers_412_and_writes_nothing() {
    let router = router();
    bucket(&router, "copy-precondition-origin").await;
    bucket(&router, "copy-precondition-target").await;
    let payload = b"sepal,petal\n3,4\n";
    let written = source_object(
        &router,
        "copy-precondition-origin",
        "source.csv",
        payload,
        &[],
    )
    .await;
    let etag = written
        .header("etag")
        .expect("the source write must report an entity tag");
    let other_etag = expected_etag(b"not the source's bytes");

    // The rows differ along WHICH member is named, whether its condition holds, and the
    // FORM its value takes — a quoted entity tag, the same tag unquoted, a weak tag, a
    // comma-separated list, the `*` wildcard, and an HTTP-date on either side of the
    // write. The last two rows are the combinations the members document, where the
    // entity-tag member decides and the date member beside it is ignored.
    let rows: Vec<PreconditionRow> = vec![
        PreconditionRow {
            case: "if-match with the source's entity tag, quoted as a read reports it",
            headers: vec![("x-amz-copy-source-if-match".into(), etag.clone())],
            served: true,
        },
        PreconditionRow {
            case: "if-match with the same tag unquoted",
            headers: vec![(
                "x-amz-copy-source-if-match".into(),
                etag.trim_matches('"').to_string(),
            )],
            served: true,
        },
        PreconditionRow {
            case: "if-match with the tag in a list beside another",
            headers: vec![(
                "x-amz-copy-source-if-match".into(),
                format!("{other_etag}, {etag}"),
            )],
            served: true,
        },
        PreconditionRow {
            case: "if-match with the wildcard, which names any existing object",
            headers: vec![("x-amz-copy-source-if-match".into(), "*".into())],
            served: true,
        },
        PreconditionRow {
            case: "if-match with a tag that is not the source's",
            headers: vec![("x-amz-copy-source-if-match".into(), other_etag.clone())],
            served: false,
        },
        PreconditionRow {
            case: "if-none-match with a tag that is not the source's",
            headers: vec![("x-amz-copy-source-if-none-match".into(), other_etag.clone())],
            served: true,
        },
        PreconditionRow {
            case: "if-none-match with the source's own tag",
            headers: vec![("x-amz-copy-source-if-none-match".into(), etag.clone())],
            served: false,
        },
        PreconditionRow {
            case: "if-none-match with the source's tag marked weak",
            headers: vec![(
                "x-amz-copy-source-if-none-match".into(),
                format!("W/{etag}"),
            )],
            served: false,
        },
        PreconditionRow {
            case: "if-none-match with the wildcard, and the object exists",
            headers: vec![("x-amz-copy-source-if-none-match".into(), "*".into())],
            served: false,
        },
        PreconditionRow {
            case: "if-unmodified-since a time after the write",
            headers: vec![(
                "x-amz-copy-source-if-unmodified-since".into(),
                AFTER_EVERY_WRITE.into(),
            )],
            served: true,
        },
        PreconditionRow {
            case: "if-unmodified-since a time before the write",
            headers: vec![(
                "x-amz-copy-source-if-unmodified-since".into(),
                BEFORE_EVERY_WRITE.into(),
            )],
            served: false,
        },
        PreconditionRow {
            case: "if-modified-since a time before the write",
            headers: vec![(
                "x-amz-copy-source-if-modified-since".into(),
                BEFORE_EVERY_WRITE.into(),
            )],
            served: true,
        },
        PreconditionRow {
            case: "if-modified-since a time after the write",
            headers: vec![(
                "x-amz-copy-source-if-modified-since".into(),
                AFTER_EVERY_WRITE.into(),
            )],
            served: false,
        },
        PreconditionRow {
            case: "if-match true beside if-unmodified-since false, which is served",
            headers: vec![
                ("x-amz-copy-source-if-match".into(), etag.clone()),
                (
                    "x-amz-copy-source-if-unmodified-since".into(),
                    BEFORE_EVERY_WRITE.into(),
                ),
            ],
            served: true,
        },
        PreconditionRow {
            case: "if-none-match false beside if-modified-since true, which is refused",
            headers: vec![
                ("x-amz-copy-source-if-none-match".into(), etag.clone()),
                (
                    "x-amz-copy-source-if-modified-since".into(),
                    BEFORE_EVERY_WRITE.into(),
                ),
            ],
            served: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let PreconditionRow {
            case,
            headers,
            served,
        } = row;
        let key = format!("conditional/{index}.csv");
        let borrowed: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let copied = copy_object(
            &router,
            "copy-precondition-target",
            &key,
            "copy-precondition-origin/source.csv",
            &borrowed,
        )
        .await;

        if *served {
            assert_eq!(
                copied.status, 200,
                "case {case:?}: the precondition holds, so the copy must answer 200, got \
                 {} with body {:?}",
                copied.status,
                text(&copied)
            );
            let read = get_object(&router, "copy-precondition-target", &key, "", &[]).await;
            assert_eq!(
                read.body.as_ref(),
                payload.as_slice(),
                "case {case:?}: the served copy must hold the source's bytes"
            );
        } else {
            assert_eq!(
                copied.status, 412,
                "case {case:?}: a precondition that does not hold answers HTTP 412, got \
                 {} with body {:?}",
                copied.status,
                text(&copied)
            );
            assert_eq!(
                copied.error_code(),
                "PreconditionFailed",
                "case {case:?}: expected the PreconditionFailed code, got {:?}",
                copied.error_code()
            );
            assert_eq!(
                copied.error_message(),
                "At least one of the pre-conditions you specified did not hold",
                "case {case:?}: expected the fixed sentence the code publishes, got {:?}",
                copied.error_message()
            );
            // WHICH member did not hold is the envelope's `<Condition>`, where S3 states it
            // — the sentence above is the same for every condition that fails.
            assert!(
                copied
                    .element("Condition")
                    .is_some_and(|condition| condition.starts_with("x-amz-copy-source-If")),
                "case {case:?}: the refusal must name the member it refused in its \
                 <Condition>, got {:?} in {:?}",
                copied.element("Condition"),
                text(&copied)
            );
            let absent = get_object(&router, "copy-precondition-target", &key, "", &[]).await;
            assert_eq!(
                absent.status, 404,
                "case {case:?}: a refused copy must write nothing, got GetObject {}",
                absent.status
            );
        }
    }
}

/// One row of the refusal table: the case, the destination bucket and key, the copy source
/// the request carries, its extra headers, and the status and wire error code it must be
/// answered with.
struct RefusalRow {
    case: &'static str,
    destination_bucket: &'static str,
    destination_key: &'static str,
    copy_source: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn a_copy_the_emulator_cannot_serve_is_refused_with_the_code_s3_answers() {
    let router = router();
    bucket(&router, "copy-refusal-origin").await;
    bucket(&router, "copy-refusal-target").await;
    source_object(
        &router,
        "copy-refusal-origin",
        "source.csv",
        b"sepal,petal\n5,6\n",
        &[],
    )
    .await;
    // Two sources that have to be restored before they can be copied, one per archived
    // storage class.
    for (key, class) in [("frozen.csv", "GLACIER"), ("deep.csv", "DEEP_ARCHIVE")] {
        source_object(
            &router,
            "copy-refusal-origin",
            key,
            b"archived\n",
            &[("x-amz-storage-class", class)],
        )
        .await;
    }

    // The rows differ along WHICH part of the request cannot be served: the destination
    // bucket, the source bucket, the source key, the source version, the copy source's
    // form, the source's storage class, a member whose semantics this emulator cannot
    // carry out, and the expected owner of either bucket.
    let rows: Vec<RefusalRow> = vec![
        RefusalRow {
            case: "a destination bucket this emulator does not hold",
            destination_bucket: "copy-refusal-absent",
            destination_key: "copies/source.csv",
            copy_source: "copy-refusal-origin/source.csv",
            headers: Vec::new(),
            status: 404,
            code: "NoSuchBucket",
        },
        RefusalRow {
            case: "a source bucket this emulator does not hold",
            destination_bucket: "copy-refusal-target",
            destination_key: "copies/absent-bucket.csv",
            copy_source: "copy-refusal-absent/source.csv",
            headers: Vec::new(),
            status: 404,
            code: "NoSuchBucket",
        },
        RefusalRow {
            case: "a source key the source bucket does not hold",
            destination_bucket: "copy-refusal-target",
            destination_key: "copies/absent-key.csv",
            copy_source: "copy-refusal-origin/absent.csv",
            headers: Vec::new(),
            status: 404,
            code: "NoSuchKey",
        },
        RefusalRow {
            case: "a WELL-FORMED source version the source bucket holds no version for",
            destination_bucket: "copy-refusal-target",
            destination_key: "copies/absent-version.csv",
            copy_source:
                "copy-refusal-origin/source.csv?versionId=0123456789abcdef0123456789abcdef",
            headers: Vec::new(),
            status: 404,
            code: "NoSuchVersion",
        },
        RefusalRow {
            case: "a source version id that is not shaped like a version id",
            destination_bucket: "copy-refusal-target",
            destination_key: "copies/malformed-version.csv",
            copy_source: "copy-refusal-origin/source.csv?versionId=no-such-version",
            headers: Vec::new(),
            // The version QUALIFIES the source reference rather than addressing the
            // resource the request is about, so what S3 reports is that the request itself
            // is not one it can serve — see
            // `state::COPY_SOURCE_INVALID_VERSION_MESSAGE`, and
            // `super::copy_source_header_envelopes` for the sentence.
            status: 400,
            code: "InvalidRequest",
        },
        RefusalRow {
            case: "a copy source naming no bucket at all",
            destination_bucket: "copy-refusal-target",
            destination_key: "copies/malformed.csv",
            copy_source: "//source.csv",
            headers: Vec::new(),
            status: 400,
            code: "InvalidArgument",
        },
        RefusalRow {
            case: "a copy source carrying a subresource other than versionId",
            destination_bucket: "copy-refusal-target",
            destination_key: "copies/subresource.csv",
            copy_source: "copy-refusal-origin/source.csv?tagging",
            headers: Vec::new(),
            status: 400,
            code: "InvalidArgument",
        },
        RefusalRow {
            case: "a source whose storage class is GLACIER",
            destination_bucket: "copy-refusal-target",
            destination_key: "copies/frozen.csv",
            copy_source: "copy-refusal-origin/frozen.csv",
            headers: Vec::new(),
            status: 403,
            code: "InvalidObjectState",
        },
        RefusalRow {
            case: "a source whose storage class is DEEP_ARCHIVE",
            destination_bucket: "copy-refusal-target",
            destination_key: "copies/deep.csv",
            copy_source: "copy-refusal-origin/deep.csv",
            headers: Vec::new(),
            status: 403,
            code: "InvalidObjectState",
        },
        RefusalRow {
            case: "an expected destination bucket owner that is not this account",
            destination_bucket: "copy-refusal-target",
            destination_key: "copies/owner.csv",
            copy_source: "copy-refusal-origin/source.csv",
            headers: vec![("x-amz-expected-bucket-owner", "210987654321")],
            status: 403,
            code: "AccessDenied",
        },
        RefusalRow {
            case: "an expected source bucket owner that is not this account",
            destination_bucket: "copy-refusal-target",
            destination_key: "copies/source-owner.csv",
            copy_source: "copy-refusal-origin/source.csv",
            headers: vec![("x-amz-source-expected-bucket-owner", "210987654321")],
            status: 403,
            code: "AccessDenied",
        },
        RefusalRow {
            case: "a KMS key id beside an algorithm that is not SSE-KMS",
            destination_bucket: "copy-refusal-target",
            destination_key: "copies/kms-only.csv",
            copy_source: "copy-refusal-origin/source.csv",
            headers: vec![
                ("x-amz-server-side-encryption", "AES256"),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id",
                    DESTINATION_KEY_ARN,
                ),
            ],
            status: 400,
            code: "InvalidArgument",
        },
        // The 501s: a member whose semantics this emulator cannot carry out keeps the
        // scaffold's "nobody has written this yet" rather than being stripped and answered
        // as a plain success.
        RefusalRow {
            case: "a customer-provided key for the destination object",
            destination_bucket: "copy-refusal-target",
            destination_key: "copies/sse-c.csv",
            copy_source: "copy-refusal-origin/source.csv",
            headers: vec![
                (
                    "x-amz-server-side-encryption-customer-algorithm",
                    "AES256",
                ),
                (
                    "x-amz-server-side-encryption-customer-key",
                    "MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE=",
                ),
            ],
            status: 501,
            code: "NotImplemented",
        },
        RefusalRow {
            case: "a customer-provided key for the source object",
            destination_bucket: "copy-refusal-target",
            destination_key: "copies/source-sse-c.csv",
            copy_source: "copy-refusal-origin/source.csv",
            headers: vec![(
                "x-amz-copy-source-server-side-encryption-customer-algorithm",
                "AES256",
            )],
            status: 501,
            code: "NotImplemented",
        },
        RefusalRow {
            case: "an Object Lock event hold for the copy",
            destination_bucket: "copy-refusal-target",
            destination_key: "copies/event-held.csv",
            copy_source: "copy-refusal-origin/source.csv",
            headers: vec![("x-amz-object-lock-event-hold", "ON")],
            status: 501,
            code: "NotImplemented",
        },
        RefusalRow {
            case: "an Object Lock retention mode without the date it runs to",
            destination_bucket: "copy-refusal-target",
            destination_key: "copies/locked.csv",
            copy_source: "copy-refusal-origin/source.csv",
            headers: vec![("x-amz-object-lock-mode", "GOVERNANCE")],
            status: 400,
            code: "InvalidRequest",
        },
        RefusalRow {
            case: "a copy source naming an object through an access point ARN",
            destination_bucket: "copy-refusal-target",
            destination_key: "copies/access-point.csv",
            copy_source: "arn:aws:s3:us-east-1:000000000000:accesspoint/ap/object/source.csv",
            headers: Vec::new(),
            status: 501,
            code: "NotImplemented",
        },
    ];

    for row in &rows {
        let RefusalRow {
            case,
            destination_bucket,
            destination_key,
            copy_source,
            headers,
            status,
            code,
        } = row;
        let refused = copy_object(
            &router,
            destination_bucket,
            destination_key,
            copy_source,
            headers,
        )
        .await;
        assert_eq!(
            refused.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case:?}: expected the {code} code, got {:?}",
            refused.error_code()
        );
        assert!(
            !refused.error_message().is_empty(),
            "case {case:?}: every S3 error document carries a message, got {:?}",
            refused.error_message()
        );
        // Whatever the refusal, nothing was written at the destination key.
        let absent = get_object(&router, "copy-refusal-target", destination_key, "", &[]).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: a refused copy must write nothing, got GetObject {}",
            absent.status
        );
    }
}

#[tokio::test]
async fn a_copy_of_an_object_onto_itself_is_refused_unless_it_changes_something() {
    // S3 refuses a copy that would replace an object with itself and change nothing about
    // "the object's metadata, storage class, website redirect location or encryption
    // attributes", and serves the same copy when it changes one of them — which is the
    // documented way to change that attribute in place.
    let router = router();
    bucket(&router, "copy-self").await;
    source_object(
        &router,
        "copy-self",
        "report.csv",
        b"sepal,petal\n7,8\n",
        &[("content-type", "text/csv"), ("x-amz-meta-stage", "raw")],
    )
    .await;

    let unchanged = copy_object(&router, "copy-self", "report.csv", "copy-self/report.csv", &[]).await;
    assert_eq!(
        unchanged.status, 400,
        "a copy onto itself that changes nothing is refused with HTTP 400, got {} with \
         body {:?}",
        unchanged.status,
        text(&unchanged)
    );
    assert_eq!(
        unchanged.error_code(),
        "InvalidRequest",
        "expected the InvalidRequest code, got {:?}",
        unchanged.error_code()
    );
    // Refused, so the object is exactly as it was.
    let intact = get_object(&router, "copy-self", "report.csv", "", &[]).await;
    assert_eq!(
        intact.header("x-amz-meta-stage").as_deref(),
        Some("raw"),
        "the refused self-copy must leave the object's metadata alone, got {:?}",
        intact.header("x-amz-meta-stage")
    );

    // Each row changes exactly one of the four attributes the refusal names, so each is
    // served.
    let rows: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "metadata REPLACE, the documented way to change metadata in place",
            vec![
                ("x-amz-metadata-directive", "REPLACE"),
                ("x-amz-meta-stage", "curated"),
            ],
        ),
        (
            "a new storage class",
            vec![("x-amz-storage-class", "STANDARD_IA")],
        ),
        (
            "a website redirect location",
            vec![("x-amz-website-redirect-location", "/self.html")],
        ),
        (
            "an encryption attribute of its own",
            vec![("x-amz-server-side-encryption", "aws:kms")],
        ),
        (
            "tagging REPLACE",
            vec![
                ("x-amz-tagging-directive", "REPLACE"),
                ("x-amz-tagging", "stage=curated"),
            ],
        ),
    ];
    for (case, headers) in &rows {
        let served = copy_object(
            &router,
            "copy-self",
            "report.csv",
            "copy-self/report.csv",
            headers,
        )
        .await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: a self-copy that changes an attribute is served, got {} with \
             body {:?}",
            served.status,
            text(&served)
        );
        let read = get_object(&router, "copy-self", "report.csv", "", &[]).await;
        assert_eq!(
            read.body.as_ref(),
            b"sepal,petal\n7,8\n".as_slice(),
            "case {case:?}: the object's bytes must survive a self-copy"
        );
    }
}

/// The sentence Amazon S3 publishes for the illegal self-copy, spelled out here rather than
/// read from the crate, so this suite pins the WORDING a client branches on and not merely
/// that the handler and the test agree: "This copy request is illegal because it is trying to
/// copy an object to itself without changing the object's metadata, storage class, website
/// redirect location or encryption attributes." It states the condition itself and
/// interpolates nothing of the request, so it is the same sentence for every such copy.
const SELF_COPY_REFUSAL_MESSAGE: &str =
    "This copy request is illegal because it is trying to copy an object to itself without \
     changing the object's metadata, storage class, website redirect location or encryption \
     attributes.";

/// The generic sentence the `InvalidRequest` SHAPE carries for any parameter or header that
/// is not valid (`smithy_query(query="shape",shape="com.amazonaws.s3#InvalidRequest")`) — the
/// refusal a client cannot act on, which this condition must NOT answer with.
const GENERIC_INVALID_REQUEST_MESSAGE: &str =
    "A parameter or header in your request isn't valid. For details, see the description of \
     this API operation.";

#[tokio::test]
async fn the_illegal_self_copy_states_the_rule_it_was_refused_by() {
    // The refusal of a copy onto itself that changes nothing is a condition S3 COMPUTES from
    // the request, so the `<Message>` beside the `InvalidRequest` code is the sentence S3
    // publishes for that condition — the one naming the attributes such a copy has to change
    // — and never the generic "a parameter isn't valid" of the shape: a client that retries
    // with `MetadataDirective=REPLACE` has to be able to tell the two apart.
    //
    // The rows differ along everything the rule generalises over and nothing it is keyed to:
    // which bucket and key the copy addresses, the FORM of the key (a bare name, a nested
    // path, one carrying a space in its percent-encoded wire spelling), whether the copy
    // source carries the leading slash the member allows, what the object itself carries (a
    // content type and user metadata, or neither, or a tag set), and whether the request
    // restates the directives as `COPY`, which is their default and so changes nothing.
    let router = router();
    bucket(&router, "copy-self-message").await;
    bucket(&router, "copy-self-message-two").await;

    struct Row {
        case: &'static str,
        bucket: &'static str,
        key: &'static str,
        /// The headers the SOURCE object was written with.
        written_with: &'static [(&'static str, &'static str)],
        /// Whether `x-amz-copy-source` carries the optional leading slash.
        leading_slash: bool,
        /// The headers the self-copy carries, none of which changes the object.
        headers: &'static [(&'static str, &'static str)],
    }

    let rows: &[Row] = &[
        Row {
            case: "a bare key, a content type and user metadata, no header on the copy",
            bucket: "copy-self-message",
            key: "report.csv",
            written_with: &[("content-type", "text/csv"), ("x-amz-meta-stage", "raw")],
            leading_slash: false,
            headers: &[],
        },
        Row {
            case: "the copy source spelled with the leading slash the member allows",
            bucket: "copy-self-message",
            key: "report.csv",
            written_with: &[("content-type", "text/csv")],
            leading_slash: true,
            headers: &[],
        },
        Row {
            case: "a nested path, with both directives restated as their COPY default",
            bucket: "copy-self-message",
            key: "reports/2026/q1.csv",
            written_with: &[],
            leading_slash: false,
            headers: &[
                ("x-amz-metadata-directive", "COPY"),
                ("x-amz-tagging-directive", "COPY"),
            ],
        },
        Row {
            case: "a key carrying a space, and an object written with a tag set",
            bucket: "copy-self-message",
            key: "quarterly%20report.csv",
            written_with: &[("x-amz-tagging", "stage=raw")],
            leading_slash: true,
            headers: &[],
        },
        Row {
            case: "another bucket entirely, so the rule is not about one bucket",
            bucket: "copy-self-message-two",
            key: "report.csv",
            written_with: &[("content-type", "application/xml")],
            leading_slash: false,
            headers: &[("x-amz-metadata-directive", "COPY")],
        },
    ];

    for row in rows {
        let case = row.case;
        source_object(
            &router,
            row.bucket,
            row.key,
            b"sepal,petal\n7,8\n",
            row.written_with,
        )
        .await;
        // The key travels in `x-amz-copy-source` as it is addressed: these rows spell it in
        // the percent-encoded form a client puts on the wire, which is the form the path
        // carries too, so both sides name the same stored key.
        let copy_source = if row.leading_slash {
            format!("/{}/{}", row.bucket, row.key)
        } else {
            format!("{}/{}", row.bucket, row.key)
        };
        let refused = copy_object(&router, row.bucket, row.key, &copy_source, row.headers).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: an illegal self-copy is refused with HTTP 400, got {} with body \
             {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            "InvalidRequest",
            "case {case:?}: expected the InvalidRequest code, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            SELF_COPY_REFUSAL_MESSAGE,
            "case {case:?}: the refusal must state the rule it was refused by, got {:?}",
            refused.error_message()
        );
        assert_ne!(
            refused.error_message(),
            GENERIC_INVALID_REQUEST_MESSAGE,
            "case {case:?}: the generic shape message tells a client nothing about the rule"
        );
        // Refused, so the object is exactly as it was: a refusal states a rule and writes
        // nothing.
        let intact = get_object(&router, row.bucket, row.key, "", &[]).await;
        assert_eq!(
            intact.status, 200,
            "case {case:?}: the refused self-copy must leave the object readable, got {}",
            intact.status
        );
        assert_eq!(
            intact.body.as_ref(),
            b"sepal,petal\n7,8\n".as_slice(),
            "case {case:?}: the refused self-copy must leave the object's bytes alone"
        );
    }
}

#[tokio::test]
async fn a_copy_names_the_source_version_it_read_and_the_version_it_created() {
    // "If your source bucket versioning is enabled, the x-amz-copy-source header by
    // default identifies the current version of an object to copy … To copy a different
    // version, use the versionId query parameter", and "if you enable versioning on the
    // destination bucket, Amazon S3 generates a unique version ID for the copied object.
    // This version ID is different from the version ID of the source object."
    let router = router();
    bucket(&router, "copy-version-origin").await;
    bucket(&router, "copy-version-target").await;
    set_versioning(&router, "copy-version-origin", "Enabled").await;
    set_versioning(&router, "copy-version-target", "Enabled").await;

    // Two versions of one key, so a copy can name either.
    let first = source_object(
        &router,
        "copy-version-origin",
        "dataset.csv",
        b"version one\n",
        &[],
    )
    .await;
    let second = source_object(
        &router,
        "copy-version-origin",
        "dataset.csv",
        b"version two\n",
        &[],
    )
    .await;
    let first_version = first
        .header("x-amz-version-id")
        .expect("a write into a versioning-enabled bucket must report a version id");
    let second_version = second
        .header("x-amz-version-id")
        .expect("a write into a versioning-enabled bucket must report a version id");
    assert_ne!(
        first_version, second_version,
        "two writes of one key must be two versions, got {first_version:?} twice"
    );

    // (case, the versionId the copy source names, the bytes it must copy, the source
    // version it must report)
    let rows: Vec<(&str, Option<&String>, &[u8], &String)> = vec![
        (
            "a copy source naming no version, which reads the current one",
            None,
            b"version two\n",
            &second_version,
        ),
        (
            "a copy source naming the noncurrent version",
            Some(&first_version),
            b"version one\n",
            &first_version,
        ),
        (
            "a copy source naming the current version explicitly",
            Some(&second_version),
            b"version two\n",
            &second_version,
        ),
    ];

    for (index, (case, version_id, payload, expected_source_version)) in rows.iter().enumerate() {
        let key = format!("copies/{index}.csv");
        let copy_source = match version_id {
            None => "copy-version-origin/dataset.csv".to_string(),
            Some(version_id) => {
                format!("copy-version-origin/dataset.csv?versionId={version_id}")
            }
        };
        let copied = copy_object(&router, "copy-version-target", &key, &copy_source, &[]).await;
        assert_eq!(
            copied.status, 200,
            "case {case:?}: the copy must answer 200, got {} with body {:?}",
            copied.status,
            text(&copied)
        );
        assert_eq!(
            copied.header("x-amz-copy-source-version-id").as_deref(),
            Some(expected_source_version.as_str()),
            "case {case:?}: the copy must name the source version it read, got {:?}",
            copied.header("x-amz-copy-source-version-id")
        );
        let created = copied
            .header("x-amz-version-id")
            .unwrap_or_else(|| panic!("case {case:?}: a copy into a versioned bucket must report the version it created"));
        assert_ne!(
            &created, *expected_source_version,
            "case {case:?}: the copy's version id must differ from the source's, got \
             {created:?} for both"
        );

        // The version the copy created holds the bytes of the version it read.
        let read = get_object(
            &router,
            "copy-version-target",
            &key,
            &format!("?versionId={created}"),
            &[],
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the version the copy created must read back, got {} with body \
             {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.body.as_ref(),
            *payload,
            "case {case:?}: the copy must hold the bytes of the version it read, got {:?}",
            text(&read)
        );
    }

    // A copy into a bucket with NO versioning state reports no version id at all: "if you
    // do not enable versioning or suspend it on the destination bucket, the version ID
    // that Amazon S3 generates in the x-amz-version-id response header is always null."
    bucket(&router, "copy-version-plain").await;
    let plain = copy_object(
        &router,
        "copy-version-plain",
        "dataset.csv",
        "copy-version-origin/dataset.csv",
        &[],
    )
    .await;
    assert_eq!(
        plain.status, 200,
        "the copy into an unversioned bucket must answer 200, got {} with body {:?}",
        plain.status,
        text(&plain)
    );
    assert_eq!(
        plain.header("x-amz-version-id"),
        None,
        "case \"an unversioned destination\": no version id is reported, got {:?}",
        plain.header("x-amz-version-id")
    );
    assert_eq!(
        plain.header("x-amz-copy-source-version-id").as_deref(),
        Some(second_version.as_str()),
        "case \"an unversioned destination\": the source version it read is still \
         reported, got {:?}",
        plain.header("x-amz-copy-source-version-id")
    );
}

/// Assert that `answer` reports exactly the encryption `expected` names: each header with
/// that value, and absent where it names `None`.
fn assert_reported(case: &str, operation: &str, answer: &Answer, expected: &Reported) {
    for (header, value) in [
        ("x-amz-server-side-encryption", expected.algorithm),
        (
            "x-amz-server-side-encryption-aws-kms-key-id",
            expected.key_id,
        ),
        (
            "x-amz-server-side-encryption-bucket-key-enabled",
            expected.bucket_key,
        ),
    ] {
        assert_eq!(
            answer.header(header).as_deref(),
            value,
            "case {case:?}: {operation} must report {header} as {value:?}, got {:?}",
            answer.header(header)
        );
    }
}
