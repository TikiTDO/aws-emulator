// SPDX-License-Identifier: CC0-1.0

//! A write that names NO server-side encryption inherits the BUCKET's default encryption
//! configuration, end to end.
//!
//! RULE: an object written to a general purpose bucket without any of `PutObject`'s
//! encryption members (`x-amz-server-side-encryption` and the three that qualify it) is
//! stored under the bucket's default encryption configuration — the first
//! `ServerSideEncryptionRule` naming an `ApplyServerSideEncryptionByDefault`, or the SSE-S3
//! base default for a bucket no `PutBucketEncryption` has configured — and `PutObject`,
//! `GetObject` and `HeadObject` all report that configuration's algorithm and KMS key in
//! their bound headers, with the S3 Bucket Key setting only where the algorithm is SSE-KMS.
//!
//! The sources:
//!
//! * "Amazon S3 encrypts data with server-side encryption by using Amazon S3 managed keys
//!   (SSE-S3) by default. You can optionally tell Amazon S3 to encrypt data at rest by
//!   using server-side encryption with other key options"
//!   (`smithy_query(query="doc",shape="PutObjectRequest",member="ServerSideEncryption")`) —
//!   the other key options being the ones `PutBucketEncryption` configures for the bucket.
//! * "By default, all buckets have a default encryption configuration that uses server-side
//!   encryption with Amazon S3 managed keys (SSE-S3)" (`GetBucketEncryption`'s own
//!   documentation): the default a never-configured bucket carries, which this crate
//!   publishes as [`DEFAULT_SSE_ALGORITHM`].
//! * "You can specify the key ID, key alias, or the Amazon Resource Name (ARN) of the KMS
//!   key" (`ApplyServerSideEncryptionByDefault$KMSMasterKeyID`): the three forms a
//!   configured key may take, each a row below. All three name ONE key, and an OBJECT names
//!   the key — so whichever form the rule carries, the object reports that key's own ARN; a
//!   rule naming no key at all leaves the write under this account's Amazon Web Services
//!   managed key for Amazon S3, whose ARN the object then reports.
//! * "Specifies whether Amazon S3 should use an S3 Bucket Key for object encryption with
//!   server-side encryption using Key Management Service (KMS) keys (SSE-KMS)"
//!   (`PutObjectRequest$BucketKeyEnabled`): the Bucket Key belongs to SSE-KMS alone, so a
//!   non-KMS default reports no Bucket Key header at all — and neither does an object written
//!   with the Bucket Key OFF, since the header states that S3 used one.
//!
//! The configuration is applied through `PutBucketEncryption` and the objects are written
//! and read through real HTTP requests to the router, so what is pinned is the wire
//! behaviour a client sees. The helpers are the ones the suites beside this one own
//! ([`super::bucket_encryption`] for the configuration, [`super::object_encryption`] for
//! `HeadObject`).

use service_com_amazonaws_s3_amazons3::state::{
    DEFAULT_BLOCKED_ENCRYPTION_TYPES, DEFAULT_BUCKET_KEY_ENABLED, DEFAULT_SSE_ALGORITHM,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_encryption::{as_reported, get_encryption, put_encryption, reported_rules, Rule};
use super::bucket_lifecycle::Answer;
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_encryption::head_object;

/// The alias every row that names a key BY ALIAS uses.
const KEY_ALIAS: &str = "alias/compliance-cmk";

/// ONE KMS key in each of the three forms `KMSMasterKeyID` documents.
///
/// The key is made with KMS's own `CreateKey` and `CreateAlias` over the same engine
/// ([`super::kms_key_references`]) rather than spelled here, because a write stored under a
/// bucket default naming a KMS key resolves that reference against KMS before it stores
/// anything: the three forms have to name a key KMS really holds. The id and the ARN are
/// minted by KMS, and the rows carry `&'static str`, so they are kept for the process.
struct KeyForms {
    id: &'static str,
    arn: &'static str,
    alias: &'static str,
    /// The ARN of this account's Amazon Web Services managed key for Amazon S3, as KMS
    /// reports it: what an object written under a rule naming NO key is encrypted under.
    managed: &'static str,
}

/// [`KeyForms`] over a router that holds KMS beside S3.
async fn key_forms(router: &MultiServiceRouter) -> KeyForms {
    let (id, arn) =
        super::kms_key_references::created_key(router, "the key these defaults name").await;
    super::kms_key_references::created_alias(router, KEY_ALIAS, &id).await;
    KeyForms {
        id: Box::leak(id.into_boxed_str()),
        arn: Box::leak(arn.into_boxed_str()),
        alias: KEY_ALIAS,
        managed: Box::leak(
            super::kms_key_references::described_key_arn(router, AWS_MANAGED_KEY_ALIAS)
                .await
                .into_boxed_str(),
        ),
    }
}

/// The predefined Amazon Web Services alias of this account's Amazon Web Services managed key
/// for Amazon S3: such an alias has "the format aws/<service-name>"
/// (`smithy_query(query="doc",shape="ListAliases")`), with this service's own name.
const AWS_MANAGED_KEY_ALIAS: &str = "alias/aws/s3";

/// What an answer about an inherited-encryption object must report: the algorithm, the KMS
/// key id and the Bucket Key setting, each `None` where the header must be ABSENT.
#[derive(Debug)]
struct Reported {
    algorithm: Option<&'static str>,
    key_id: Option<&'static str>,
    bucket_key: Option<&'static str>,
}

/// One row of the inheritance table: the case, the configuration the bucket carries (`None`
/// for a bucket left exactly as `CreateBucket` made it), and what every answer about an
/// object written with no encryption member must then report.
struct Row {
    case: &'static str,
    configuration: Option<Vec<Rule>>,
    expected: Reported,
}

/// The table: every row differs along what the rule generalises over — WHICH algorithm the
/// bucket's default names (every value of the `ServerSideEncryption` enum, including the
/// three the issue never cited), WHICH form its KMS key takes (key id, ARN, alias), whether
/// the Bucket Key is on, off or unnamed, whether the configuration names an algorithm at
/// all, and HOW MANY rules it carries and in which order.
fn rows(key: &KeyForms) -> Vec<Row> {
    vec![
        Row {
            case: "a bucket no PutBucketEncryption has configured",
            configuration: None,
            expected: Reported {
                algorithm: Some(DEFAULT_SSE_ALGORITHM),
                key_id: None,
                bucket_key: None,
            },
        },
        Row {
            case: "a default of SSE-S3, named explicitly",
            configuration: Some(vec![Rule::by_default("AES256")]),
            expected: Reported {
                algorithm: Some("AES256"),
                key_id: None,
                bucket_key: None,
            },
        },
        Row {
            case: "a default of SSE-S3 with the Bucket Key switched on, which SSE-S3 has none of",
            configuration: Some(vec![Rule::by_default("AES256").with_bucket_key(true)]),
            expected: Reported {
                algorithm: Some("AES256"),
                key_id: None,
                bucket_key: None,
            },
        },
        Row {
            case: "a default of SSE-KMS with a customer managed key by ARN and the Bucket Key off",
            configuration: Some(vec![Rule::by_default("aws:kms")
                .with_key(key.arn)
                .with_bucket_key(false)]),
            expected: Reported {
                algorithm: Some("aws:kms"),
                key_id: Some(key.arn),
                bucket_key: None,
            },
        },
        Row {
            case: "a default of SSE-KMS with the key by alias and the Bucket Key on",
            configuration: Some(vec![Rule::by_default("aws:kms")
                .with_key(key.alias)
                .with_bucket_key(true)]),
            expected: Reported {
                algorithm: Some("aws:kms"),
                key_id: Some(key.arn),
                bucket_key: Some("true"),
            },
        },
        Row {
            case: "a default of SSE-KMS with a bare key id and no Bucket Key named",
            configuration: Some(vec![Rule::by_default("aws:kms").with_key(key.id)]),
            expected: Reported {
                algorithm: Some("aws:kms"),
                key_id: Some(key.arn),
                bucket_key: None,
            },
        },
        Row {
            case: "a default of SSE-KMS naming no key, so the write uses the managed key",
            configuration: Some(vec![Rule::by_default("aws:kms")]),
            expected: Reported {
                algorithm: Some("aws:kms"),
                key_id: Some(key.managed),
                bucket_key: None,
            },
        },
        Row {
            case: "a default of dual-layer SSE-KMS naming no key, which uses the same key",
            configuration: Some(vec![Rule::by_default("aws:kms:dsse")]),
            expected: Reported {
                algorithm: Some("aws:kms:dsse"),
                key_id: Some(key.managed),
                bucket_key: None,
            },
        },
        Row {
            case: "a default of dual-layer SSE-KMS with a key ARN",
            configuration: Some(vec![Rule::by_default("aws:kms:dsse").with_key(key.arn)]),
            expected: Reported {
                algorithm: Some("aws:kms:dsse"),
                key_id: Some(key.arn),
                bucket_key: None,
            },
        },
        Row {
            case: "a default of aws:fsx, a published algorithm the issue never cited",
            configuration: Some(vec![Rule::by_default("aws:fsx")]),
            expected: Reported {
                algorithm: Some("aws:fsx"),
                key_id: None,
                bucket_key: None,
            },
        },
        Row {
            case: "a default of aws:backup, the other algorithm the issue never cited",
            configuration: Some(vec![Rule::by_default("aws:backup")]),
            expected: Reported {
                algorithm: Some("aws:backup"),
                key_id: None,
                bucket_key: None,
            },
        },
        Row {
            case: "a configuration naming only BucketKeyEnabled, so the base SSE-S3 default stands",
            configuration: Some(vec![Rule::bucket_key_only(true)]),
            expected: Reported {
                algorithm: Some(DEFAULT_SSE_ALGORITHM),
                key_id: None,
                bucket_key: None,
            },
        },
        Row {
            case: "two rules, the first of which names the algorithm",
            configuration: Some(vec![
                Rule::by_default("aws:kms:dsse").with_key(key.id),
                Rule::by_default("AES256"),
            ]),
            expected: Reported {
                algorithm: Some("aws:kms:dsse"),
                key_id: Some(key.arn),
                bucket_key: None,
            },
        },
        Row {
            case: "two rules, only the second of which names an algorithm",
            configuration: Some(vec![
                Rule::bucket_key_only(false),
                Rule::by_default("aws:kms")
                    .with_key(key.arn)
                    .with_bucket_key(true),
            ]),
            expected: Reported {
                algorithm: Some("aws:kms"),
                key_id: Some(key.arn),
                bucket_key: Some("true"),
            },
        },
    ]
}

#[tokio::test]
async fn a_write_naming_no_encryption_inherits_the_bucket_default_and_every_read_reports_it() {
    let router = super::kms_key_references::router_with_kms();
    let key = key_forms(&router).await;

    for (index, row) in rows(&key).into_iter().enumerate() {
        let Row {
            case,
            configuration,
            expected,
        } = row;
        let name = format!("default-encryption-{index}");
        bucket(&router, &name).await;

        if let Some(rules) = &configuration {
            let configured = put_encryption(&router, &name, rules).await;
            assert_eq!(
                configured.status, 200,
                "case {case:?}: the bucket's default encryption must be configured first, \
                 got {} with body {:?}",
                configured.status,
                text(&configured)
            );
        }

        // The payload varies with the row, so the rule is exercised over different writes
        // rather than one recorded body.
        let key = format!("inherited/{index}/asset.txt");
        let payload = format!("row {index}: {case}\n").into_bytes();
        let written = put_object(&router, &name, &key, &payload, &[]).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: a write naming no encryption member must answer 200, got {} \
             with body {:?}",
            written.status,
            text(&written)
        );
        assert_reported(case, "PutObject", &written, &expected);

        // Both implemented readers report the configuration the object was STORED under.
        let read = get_object(&router, &name, &key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the stored object must read back, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.body.as_ref(),
            payload.as_slice(),
            "case {case:?}: the stored bytes must come back unchanged, got {:?}",
            text(&read)
        );
        assert_reported(case, "GetObject", &read, &expected);

        let headed = head_object(&router, &name, &key).await;
        assert_eq!(
            headed.status, 200,
            "case {case:?}: HeadObject of the stored object must answer 200, got {}",
            headed.status
        );
        assert_reported(case, "HeadObject", &headed, &expected);

        // What the object inherited is what the bucket REPORTS as its default: the write
        // resolved it from the configuration `GetBucketEncryption` answers with, and the
        // write did not materialise a configuration onto a bucket that had none.
        let reported = get_encryption(&router, &name).await;
        assert_eq!(
            reported.status, 200,
            "case {case:?}: GetBucketEncryption must still answer 200 after the write, got \
             {} with body {:?}",
            reported.status,
            text(&reported)
        );
        let rules = reported_rules(&reported);
        assert_eq!(
            rules,
            as_reported(&configuration.unwrap_or_else(|| {
                vec![Rule::by_default(DEFAULT_SSE_ALGORITHM)
                    .with_bucket_key(DEFAULT_BUCKET_KEY_ENABLED)
                    .blocking(&DEFAULT_BLOCKED_ENCRYPTION_TYPES)]
            })),
            "case {case:?}: the write must leave the bucket's reported configuration \
             exactly as it was, got {rules:?}"
        );
    }
}

/// One row of the override table: the case, the encryption headers the write carries, and
/// what every answer about the object must then report.
type OverrideRow = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Reported,
);

#[tokio::test]
async fn a_write_naming_its_own_encryption_is_stored_under_that_and_not_the_bucket_default() {
    // The bucket default applies "if and only if" the request names no encryption of its
    // own: "You can optionally tell Amazon S3 to encrypt data at rest by using server-side
    // encryption with other key options" — so what the request names stands on its own,
    // with nothing of the bucket's configuration merged into it.
    let router = super::kms_key_references::router_with_kms();
    // The key each write names, and the one the bucket's default names, are keys KMS really
    // holds: a write stored under a KMS key resolves that reference against KMS before
    // storing anything ([`super::kms_key_references`]).
    let (_, other_key) =
        super::kms_key_references::created_key(&router, "the key each write names").await;
    let key = key_forms(&router).await;
    bucket(&router, "default-encryption-override").await;
    let configured = put_encryption(
        &router,
        "default-encryption-override",
        &[Rule::by_default("aws:kms")
            .with_key(key.arn)
            .with_bucket_key(true)],
    )
    .await;
    assert_eq!(
        configured.status, 200,
        "the bucket's SSE-KMS default must be configured first, got {}",
        configured.status
    );

    // The rows differ along WHICH member the write names: the algorithm alone, the
    // algorithm with a key of its own, and a Bucket Key setting beside it.
    let other_key: &'static str = Box::leak(other_key.into_boxed_str());
    let rows: Vec<OverrideRow> = vec![
        (
            "a write naming SSE-S3 on a bucket whose default is SSE-KMS",
            vec![("x-amz-server-side-encryption", "AES256")],
            Reported {
                algorithm: Some("AES256"),
                key_id: None,
                bucket_key: None,
            },
        ),
        (
            "a write naming SSE-KMS with a key of its own",
            vec![
                ("x-amz-server-side-encryption", "aws:kms"),
                ("x-amz-server-side-encryption-aws-kms-key-id", other_key),
            ],
            Reported {
                algorithm: Some("aws:kms"),
                key_id: Some(other_key),
                bucket_key: None,
            },
        ),
        (
            "a write naming dual-layer SSE-KMS with the Bucket Key off",
            vec![
                ("x-amz-server-side-encryption", "aws:kms:dsse"),
                ("x-amz-server-side-encryption-aws-kms-key-id", other_key),
                ("x-amz-server-side-encryption-bucket-key-enabled", "false"),
            ],
            Reported {
                algorithm: Some("aws:kms:dsse"),
                key_id: Some(other_key),
                // The write switches the Bucket Key OFF, so the object was written without
                // one and carries no `BucketKeyEnabled` at all.
                bucket_key: None,
            },
        ),
    ];

    for (index, (case, headers, expected)) in rows.iter().enumerate() {
        let key = format!("explicit/{index}.txt");
        let written = put_object(
            &router,
            "default-encryption-override",
            &key,
            b"a,b\n1,2\n",
            headers,
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the write must answer 200, got {} with body {:?}",
            written.status,
            text(&written)
        );
        assert_reported(case, "PutObject", &written, expected);

        let read = get_object(&router, "default-encryption-override", &key, "", &[]).await;
        assert_reported(case, "GetObject", &read, expected);
        let headed = head_object(&router, "default-encryption-override", &key).await;
        assert_reported(case, "HeadObject", &headed, expected);
    }
}

#[tokio::test]
async fn the_encryption_an_object_inherited_is_the_one_in_force_when_it_was_written() {
    // The encryption is part of the stored object — "You cannot use PutObject to only
    // update a single piece of metadata for an existing object" — so a later
    // `PutBucketEncryption` changes what the NEXT write inherits and nothing about the
    // object already stored.
    let router = super::kms_key_references::router_with_kms();
    let key = key_forms(&router).await;
    bucket(&router, "default-encryption-in-force").await;

    let before = put_object(
        &router,
        "default-encryption-in-force",
        "before.txt",
        b"written under the base default\n",
        &[],
    )
    .await;
    assert_eq!(
        before.status, 200,
        "the first write must answer 200, got {} with body {:?}",
        before.status,
        text(&before)
    );
    assert_reported(
        "written before any configuration",
        "PutObject",
        &before,
        &Reported {
            algorithm: Some(DEFAULT_SSE_ALGORITHM),
            key_id: None,
            bucket_key: None,
        },
    );

    let configured = put_encryption(
        &router,
        "default-encryption-in-force",
        &[Rule::by_default("aws:kms").with_key(key.arn)],
    )
    .await;
    assert_eq!(
        configured.status, 200,
        "the configuration must be applied, got {} with body {:?}",
        configured.status,
        text(&configured)
    );

    // The object written earlier still reports what it was stored under.
    let unchanged = get_object(&router, "default-encryption-in-force", "before.txt", "", &[]).await;
    assert_reported(
        "read after the bucket default changed",
        "GetObject",
        &unchanged,
        &Reported {
            algorithm: Some(DEFAULT_SSE_ALGORITHM),
            key_id: None,
            bucket_key: None,
        },
    );

    // The next write inherits the configuration now in force.
    let after = put_object(
        &router,
        "default-encryption-in-force",
        "after.txt",
        b"written under the CMK default\n",
        &[],
    )
    .await;
    let expected = Reported {
        algorithm: Some("aws:kms"),
        key_id: Some(key.arn),
        bucket_key: None,
    };
    assert_reported("written after the configuration", "PutObject", &after, &expected);
    let reread = get_object(&router, "default-encryption-in-force", "after.txt", "", &[]).await;
    assert_reported("written after the configuration", "GetObject", &reread, &expected);
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
    // An encryption context is a per-request value no bucket configuration carries, so an
    // inherited encryption never reports one.
    assert_eq!(
        answer.header("x-amz-server-side-encryption-context"),
        None,
        "case {case:?}: {operation} must report no encryption context, got {:?}",
        answer.header("x-amz-server-side-encryption-context")
    );
}
