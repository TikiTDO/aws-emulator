// SPDX-License-Identifier: CC0-1.0

//! WHICH KMS key an SSE-KMS write is performed under, and what happens when KMS will not
//! mint a data key for it.
//!
//! RULE: every write that stores an object under SSE-KMS (`aws:kms` or `aws:kms:dsse`,
//! whether the request named the algorithm or inherited it from the bucket's default
//! encryption configuration) is encrypted under a data key KMS mints, so the key is resolved
//! against KMS BEFORE anything is stored:
//!
//! * the effective key reference — the request's own `SSEKMSKeyId`, the bucket rule's
//!   `KMSMasterKeyID`, or, where neither names one, this account's Amazon Web Services
//!   managed key for Amazon S3 behind the predefined `alias/aws/s3` alias — resolves to ONE
//!   key, and what the object records and every read and write answer reports is that key's
//!   own ARN, `arn:<partition>:kms:<region>:<account>:key/<key-id>`. "You can specify the key
//!   ID, key alias, or the Amazon Resource Name (ARN) of the KMS key"
//!   (`PutObjectRequest$SSEKMSKeyId`, `ApplyServerSideEncryptionByDefault$KMSMasterKeyID`)
//!   are three spellings of one key, and `KMSMasterKeyID` is optional, so a rule naming no
//!   key is a legal configuration rather than an object encrypted under nothing;
//! * a reference KMS holds no key for, and a key whose key state refuses cryptographic
//!   operations, mint no data key: the write is REFUSED with the KMS failure S3 relays
//!   (HTTP 400, the KMS exception's name qualified by the service that raised it, and KMS's
//!   own sentence) and nothing is stored — which for a multipart upload means the key is
//!   resolved at the INITIATION, before any part can be written, and no upload is registered.
//!
//! The bucket's stored rule is NOT changed by any of this: S3 "doesn't validate the KMS key
//! ID provided in PutBucketEncryption requests" (`PutBucketEncryption`'s own documentation),
//! so the configuration keeps whatever spelling it was given and `GetBucketEncryption` echoes
//! it unchanged — the bucket-level echo and the object-level resolution are deliberately
//! different.
//!
//! RULE: an object's own `BucketKeyEnabled` is reported only where an S3 Bucket Key is in
//! force on it. "Setting this header to true causes Amazon S3 to use an S3 Bucket Key"
//! (`PutObjectRequest$BucketKeyEnabled`), and the member is bound to
//! `x-amz-server-side-encryption-bucket-key-enabled` with no `@default`, so the header states
//! that S3 used one: an object written with the Bucket Key off carries no such header at all.
//! The BUCKET-level rule is the opposite — `ServerSideEncryptionRule$BucketKeyEnabled` is a
//! setting with a documented off state ("General purpose buckets - By default, S3 Bucket Key
//! is not enabled"), which `GetBucketEncryption` reports as `false` explicitly.
//!
//! Every assertion drives real HTTP requests through a router carrying S3 and KMS on ONE
//! engine ([`super::kms_key_references::router_with_kms`]), so the key a write names is the
//! key KMS really holds: every key, alias and key-state change below is made with KMS's own
//! operations, and every expected ARN is the one KMS itself reports.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_encryption::{put_encryption, reported_rules, Rule};
use super::bucket_lifecycle::{send_bytes, Answer};
use super::kms_key_references::{
    create_multipart_upload, created_alias, created_key, described_key_arn, described_key_member,
    kms, router_with_kms, REGION,
};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_encryption::head_object;
use super::object_storage::list_objects_v2;

/// The two `ServerSideEncryption` values a KMS key belongs to
/// (`ApplyServerSideEncryptionByDefault$KMSMasterKeyID` is "allowed if and only if
/// SSEAlgorithm is set to aws:kms or aws:kms:dsse").
const KMS_ALGORITHMS: [&str; 2] = ["aws:kms", "aws:kms:dsse"];

/// The predefined Amazon Web Services alias of this account's Amazon Web Services managed key
/// for Amazon S3: such an alias has "the format aws/<service-name>"
/// (`smithy_query(query="doc",shape="ListAliases")`), with this service's own name.
const AWS_MANAGED_KEY_ALIAS: &str = "alias/aws/s3";

/// The `KeyManager` KMS reports for a key Amazon Web Services manages, as against a customer
/// managed one (`smithy_query(query="shape",shape="KeyManagerType")`).
const KEY_MANAGER_AWS: &str = "AWS";

/// The codes S3 answers a write whose SSE-KMS key KMS will not mint a data key under, and the
/// status they carry. Recorded behaviour: the s3 model binds no error shape for the relay.
const KMS_NOT_FOUND_CODE: &str = "KMS.NotFoundException";
const KMS_INVALID_STATE_CODE: &str = "KMS.KMSInvalidStateException";
const KMS_RELAY_STATUS: u16 = 400;

/// The bytes every write in this suite carries.
const BODY: &[u8] = b"sse-kms-inventory-payload";

/// The bound header an object's KMS key is reported in, and the one its Bucket Key is.
const KEY_ID_HEADER: &str = "x-amz-server-side-encryption-aws-kms-key-id";
const BUCKET_KEY_HEADER: &str = "x-amz-server-side-encryption-bucket-key-enabled";

/// `GET /<bucket>?uploads` — `ListMultipartUploads`.
async fn list_multipart_uploads(router: &MultiServiceRouter, bucket_name: &str) -> Answer {
    send_bytes(router, "GET", &format!("/{bucket_name}?uploads"), &[], b"").await
}

/// The ARN of a KMS resource of this account at the endpoint these requests are signed for:
/// what KMS names a reference by when it reports one it could not resolve.
fn kms_arn(resource: &str) -> String {
    format!(
        "arn:aws:kms:{REGION}:{}:{resource}",
        engine::emulator_account_id()
    )
}

/// One row of the managed-key table: the case, the bucket's default rules (`None` for a
/// bucket left as `CreateBucket` made it), and the encryption headers the write carries.
type ManagedRow = (&'static str, Option<Vec<Rule>>, Vec<(&'static str, &'static str)>);

#[tokio::test]
async fn an_sse_kms_write_naming_no_key_is_stored_under_the_managed_key_and_reports_its_arn() {
    let router = router_with_kms();

    // The rows differ along what the rule generalises over: HOW the SSE-KMS encryption with
    // no key reaches the write (a bucket default rule, or the request's own header), WHICH of
    // the two KMS algorithms it names, and whether a Bucket Key accompanies it. None of them
    // names a key, so every one of them is encrypted under the same managed key.
    let rows: Vec<ManagedRow> = vec![
        (
            "a bucket default of aws:kms naming no key",
            Some(vec![Rule::by_default(KMS_ALGORITHMS[0])]),
            Vec::new(),
        ),
        (
            "a bucket default of aws:kms:dsse naming no key",
            Some(vec![Rule::by_default(KMS_ALGORITHMS[1])]),
            Vec::new(),
        ),
        (
            "a bucket default of aws:kms naming no key, with a Bucket Key",
            Some(vec![Rule::by_default(KMS_ALGORITHMS[0]).with_bucket_key(true)]),
            Vec::new(),
        ),
        (
            "a request header of aws:kms with no key id, on an SSE-S3 bucket",
            Some(vec![Rule::by_default("AES256")]),
            vec![("x-amz-server-side-encryption", KMS_ALGORITHMS[0])],
        ),
        (
            "a request header of aws:kms:dsse with no key id, on an unconfigured bucket",
            None,
            vec![("x-amz-server-side-encryption", KMS_ALGORITHMS[1])],
        ),
    ];

    for (index, (case, rules, headers)) in rows.iter().enumerate() {
        let name = format!("managed-key-bucket-{index}");
        bucket(&router, &name).await;
        if let Some(rules) = rules {
            let configured = put_encryption(&router, &name, rules).await;
            assert_eq!(
                configured.status, 200,
                "case {case:?}: the bucket's default must be configured first, got {} with \
                 body {:?}",
                configured.status,
                text(&configured)
            );
        }

        let key = format!("inventory/{index}.txt");
        let written = put_object(&router, &name, &key, BODY, headers).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the write must be served, got {} with body {:?}",
            written.status,
            text(&written)
        );

        // The key the write was performed under, as KMS itself reports it — and it is a key
        // Amazon Web Services manages, which is the only thing distinguishing SSE-KMS under
        // the managed key from SSE-KMS under a customer key.
        let expected = described_key_arn(&router, AWS_MANAGED_KEY_ALIAS).await;
        assert_eq!(
            expected,
            kms_arn(&format!(
                "key/{}",
                described_key_member(&router, AWS_MANAGED_KEY_ALIAS, "KeyId").await
            )),
            "case {case:?}: the managed key's ARN must have the published key-ARN shape"
        );
        assert_eq!(
            described_key_member(&router, &expected, "KeyManager").await,
            KEY_MANAGER_AWS,
            "case {case:?}: the key a keyless SSE-KMS write uses must be managed by Amazon \
             Web Services"
        );

        for (reader, answer) in [
            ("PutObject", written),
            (
                "GetObject",
                get_object(&router, &name, &key, "", &[]).await,
            ),
            ("HeadObject", head_object(&router, &name, &key).await),
        ] {
            assert_eq!(
                answer.header(KEY_ID_HEADER).as_deref(),
                Some(expected.as_str()),
                "case {case:?}/{reader}: expected the managed key's ARN {expected:?}, got {:?}",
                answer.header(KEY_ID_HEADER)
            );
        }
    }
}

/// One row of the Bucket Key table: the case, the bucket's default rules, the headers the
/// write carries, and the value the object's `BucketKeyEnabled` header must have (`None` for
/// a header that must be ABSENT).
type BucketKeyRow = (
    &'static str,
    Option<Vec<Rule>>,
    Vec<(&'static str, &'static str)>,
    Option<&'static str>,
);

#[tokio::test]
async fn an_object_reports_a_bucket_key_only_where_one_is_in_force() {
    let router = router_with_kms();
    let (_, key_arn) = created_key(&router, "the key these buckets name").await;
    let key_arn: &'static str = Box::leak(key_arn.into_boxed_str());

    // The rows differ along HOW the setting reaches the object (a bucket default rule or the
    // request's own header), along its VALUE (on, off, unnamed) and along the algorithm —
    // including a non-KMS one, which has no Bucket Key at all.
    let rows: Vec<BucketKeyRow> = vec![
        (
            "a bucket default with the Bucket Key off",
            Some(vec![Rule::by_default(KMS_ALGORITHMS[0])
                .with_key(key_arn)
                .with_bucket_key(false)]),
            Vec::new(),
            None,
        ),
        (
            "a bucket default naming no Bucket Key at all",
            Some(vec![Rule::by_default(KMS_ALGORITHMS[0]).with_key(key_arn)]),
            Vec::new(),
            None,
        ),
        (
            "a bucket default with the Bucket Key on",
            Some(vec![Rule::by_default(KMS_ALGORITHMS[0])
                .with_key(key_arn)
                .with_bucket_key(true)]),
            Vec::new(),
            Some("true"),
        ),
        (
            "a request header switching the Bucket Key off",
            Some(vec![Rule::by_default("AES256")]),
            vec![
                ("x-amz-server-side-encryption", KMS_ALGORITHMS[1]),
                (KEY_ID_HEADER, key_arn),
                (BUCKET_KEY_HEADER, "false"),
            ],
            None,
        ),
        (
            "a request header switching the Bucket Key on",
            Some(vec![Rule::by_default("AES256")]),
            vec![
                ("x-amz-server-side-encryption", KMS_ALGORITHMS[1]),
                (KEY_ID_HEADER, key_arn),
                (BUCKET_KEY_HEADER, "true"),
            ],
            Some("true"),
        ),
        (
            "an SSE-S3 default, which has no Bucket Key",
            Some(vec![Rule::by_default("AES256").with_bucket_key(true)]),
            Vec::new(),
            None,
        ),
    ];

    for (index, (case, rules, headers, expected)) in rows.iter().enumerate() {
        let name = format!("bucket-key-projection-{index}");
        bucket(&router, &name).await;
        if let Some(rules) = rules {
            let configured = put_encryption(&router, &name, rules).await;
            assert_eq!(
                configured.status, 200,
                "case {case:?}: the bucket's default must be configured first, got {} with \
                 body {:?}",
                configured.status,
                text(&configured)
            );
            // The BUCKET-level projection is the opposite of the object's: a rule stored with
            // the setting off, or without it, is reported with it explicitly `false`.
            let read = super::bucket_encryption::get_encryption(&router, &name).await;
            let reported = reported_rules(&read);
            assert_eq!(
                reported.len(),
                rules.len(),
                "case {case:?}: the configuration must read back, got {reported:?}"
            );
            assert_eq!(
                reported[0].bucket_key_enabled,
                Some(rules[0].bucket_key_enabled.unwrap_or(false)),
                "case {case:?}: the RULE must report its Bucket Key setting explicitly, got \
                 {reported:?}"
            );
        }

        let key = format!("projection/{index}.txt");
        let written = put_object(&router, &name, &key, BODY, headers).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the write must be served, got {} with body {:?}",
            written.status,
            text(&written)
        );
        // Every answer about the object agrees, the initiation of a multipart upload under
        // the same configuration included: the projection is the object's, not one reader's.
        let initiated = create_multipart_upload(&router, &name, &key, headers).await;
        assert_eq!(
            initiated.status, 200,
            "case {case:?}: the initiation must be served, got {} with body {:?}",
            initiated.status,
            text(&initiated)
        );
        for (reader, answer) in [
            ("PutObject", written),
            (
                "GetObject",
                get_object(&router, &name, &key, "", &[]).await,
            ),
            ("HeadObject", head_object(&router, &name, &key).await),
            ("CreateMultipartUpload", initiated),
        ] {
            assert_eq!(
                answer.header(BUCKET_KEY_HEADER).as_deref(),
                *expected,
                "case {case:?}/{reader}: expected {BUCKET_KEY_HEADER} as {expected:?}, got {:?}",
                answer.header(BUCKET_KEY_HEADER)
            );
        }
    }
}

#[tokio::test]
async fn a_default_rule_echoes_its_key_reference_while_its_objects_name_the_key() {
    let router = router_with_kms();
    let (key_id, key_arn) = created_key(&router, "the key every reference form names").await;
    created_alias(&router, "alias/sse-inventory", &key_id).await;

    // The rows differ along the FORM the rule's `KMSMasterKeyID` takes — every form the
    // member documents, plus the alias ARN — and along the algorithm the rule applies. All
    // five name ONE key.
    let rows: [(&str, String, &str); 5] = [
        ("the key ARN", key_arn.clone(), KMS_ALGORITHMS[0]),
        ("a bare key id", key_id.clone(), KMS_ALGORITHMS[0]),
        (
            "an alias of the key",
            "alias/sse-inventory".to_string(),
            KMS_ALGORITHMS[0],
        ),
        (
            "an alias ARN of the key",
            kms_arn("alias/sse-inventory"),
            KMS_ALGORITHMS[1],
        ),
        ("the key ARN under DSSE", key_arn.clone(), KMS_ALGORITHMS[1]),
    ];

    for (index, (case, reference, algorithm)) in rows.iter().enumerate() {
        let name = format!("reference-forms-{index}");
        bucket(&router, &name).await;
        let rules = vec![Rule::by_default(algorithm).with_key(reference)];
        let configured = put_encryption(&router, &name, &rules).await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: the rule must be accepted, got {} with body {:?}",
            configured.status,
            text(&configured)
        );

        // The bucket's rule comes back as it was GIVEN: nothing resolves the configuration.
        let read = super::bucket_encryption::get_encryption(&router, &name).await;
        let reported = reported_rules(&read);
        assert_eq!(
            reported.first().and_then(|rule| rule.kms_master_key_id.clone()),
            Some(reference.clone()),
            "case {case:?}: the rule must echo {reference:?} verbatim, got {reported:?}"
        );

        // The OBJECT names the key: the ARN of the one key all five forms resolve to.
        let key = format!("forms/{index}.txt");
        let written = put_object(&router, &name, &key, BODY, &[]).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the write must be served, got {} with body {:?}",
            written.status,
            text(&written)
        );
        for (reader, answer) in [
            ("PutObject", written),
            (
                "GetObject",
                get_object(&router, &name, &key, "", &[]).await,
            ),
            ("HeadObject", head_object(&router, &name, &key).await),
        ] {
            assert_eq!(
                answer.header(KEY_ID_HEADER).as_deref(),
                Some(key_arn.as_str()),
                "case {case:?}/{reader}: expected the key ARN {key_arn:?}, got {:?}",
                answer.header(KEY_ID_HEADER)
            );
        }
    }
}

/// One row of the refused-write table: the case, the `KMSMasterKeyID` the bucket's rule
/// names, the code S3 must relay and the sentence KMS words for it.
type RefusedRow = (&'static str, String, &'static str, String);

/// The rows, and the key each of them had to be put into a state for: a reference KMS holds
/// no key for, and a key KMS holds that will not perform a cryptographic operation.
async fn refused_rows(router: &MultiServiceRouter) -> Vec<RefusedRow> {
    let ghost = "alias/sse-inventory-ghost";
    let (pending_id, pending_arn) = created_key(router, "the key scheduled for deletion").await;
    let (scheduled, payload) = kms(
        router,
        "ScheduleKeyDeletion",
        &json!({"KeyId": pending_id, "PendingWindowInDays": 7}),
    )
    .await;
    assert_eq!(
        scheduled, 200,
        "scheduling the key's deletion must succeed, got {scheduled} with {payload}"
    );
    let (disabled_id, disabled_arn) = created_key(router, "the key that is disabled").await;
    let (turned_off, payload) = kms(router, "DisableKey", &json!({"KeyId": disabled_id})).await;
    assert_eq!(
        turned_off, 200,
        "disabling the key must succeed, got {turned_off} with {payload}"
    );
    vec![
        (
            "an alias no CreateAlias ever created",
            ghost.to_string(),
            KMS_NOT_FOUND_CODE,
            format!("Alias {} is not found.", kms_arn("alias/sse-inventory-ghost")),
        ),
        (
            "a key id no CreateKey ever minted",
            "6c2a8f0e-5d41-4b2f-9a77-1e4b8c0d3a51".to_string(),
            KMS_NOT_FOUND_CODE,
            format!(
                "Key '{}' does not exist",
                kms_arn("key/6c2a8f0e-5d41-4b2f-9a77-1e4b8c0d3a51")
            ),
        ),
        (
            "a key whose deletion is scheduled",
            pending_arn.clone(),
            KMS_INVALID_STATE_CODE,
            format!("{pending_arn} is pending deletion."),
        ),
        (
            "a key that is disabled",
            disabled_arn.clone(),
            KMS_INVALID_STATE_CODE,
            format!("{disabled_arn} is disabled."),
        ),
    ]
}

#[tokio::test]
async fn a_write_under_a_default_whose_key_mints_no_data_key_is_refused_and_stores_nothing() {
    let router = router_with_kms();
    let rows = refused_rows(&router).await;

    for (index, (case, reference, code, message)) in rows.iter().enumerate() {
        let name = format!("refused-default-{index}");
        bucket(&router, &name).await;
        // The CONFIGURATION is accepted whatever it names: S3 validates no key id here.
        let rules = vec![Rule::by_default(KMS_ALGORITHMS[index % KMS_ALGORITHMS.len()])
            .with_key(reference)];
        let configured = put_encryption(&router, &name, &rules).await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: the rule must be accepted, got {} with body {:?}",
            configured.status,
            text(&configured)
        );

        let refused = put_object(&router, &name, "refused.txt", BODY, &[]).await;
        assert_eq!(
            refused.status, KMS_RELAY_STATUS,
            "case {case:?}: expected HTTP {KMS_RELAY_STATUS}, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case:?}: expected the code {code:?}, got {:?} ({:?})",
            refused.error_code(),
            text(&refused)
        );
        assert_eq!(
            refused.error_message(),
            *message,
            "case {case:?}: expected the message {message:?}, got {:?}",
            refused.error_message()
        );

        // Nothing was stored: the refusal happens before the object is recorded.
        let listed = list_objects_v2(&router, &name, "").await;
        assert!(
            !text(&listed).contains("refused.txt"),
            "case {case:?}: the refused write must leave no object, got {:?}",
            text(&listed)
        );
        let read = get_object(&router, &name, "refused.txt", "", &[]).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: the refused key must hold nothing, got {} with body {:?}",
            read.status,
            text(&read)
        );

        // And the bucket's own rule is untouched by the refusal.
        let still = super::bucket_encryption::get_encryption(&router, &name).await;
        assert_eq!(
            reported_rules(&still)
                .first()
                .and_then(|rule| rule.kms_master_key_id.clone()),
            Some(reference.clone()),
            "case {case:?}: the rule must still name {reference:?}, got {:?}",
            text(&still)
        );

        // The control: re-pointing the same bucket at the Amazon Web Services managed key
        // makes the identical write land.
        let repointed =
            put_encryption(&router, &name, &[Rule::by_default(KMS_ALGORITHMS[0])]).await;
        assert_eq!(
            repointed.status, 200,
            "case {case:?}: re-pointing the rule must be accepted, got {} with body {:?}",
            repointed.status,
            text(&repointed)
        );
        let served = put_object(&router, &name, "refused.txt", BODY, &[]).await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: the write under the managed key must be served, got {} with body \
             {:?}",
            served.status,
            text(&served)
        );
        assert_eq!(
            served.header(KEY_ID_HEADER),
            Some(described_key_arn(&router, AWS_MANAGED_KEY_ALIAS).await),
            "case {case:?}: the served write must name the managed key's ARN, got {:?}",
            served.header(KEY_ID_HEADER)
        );
    }
}

#[tokio::test]
async fn a_multipart_initiation_resolves_its_key_before_registering_the_upload() {
    let router = router_with_kms();
    let rows = refused_rows(&router).await;
    bucket(&router, "multipart-key-resolution").await;

    for (index, (case, reference, code, message)) in rows.iter().enumerate() {
        let key = format!("refused-upload-{index}.bin");
        let refused = create_multipart_upload(
            &router,
            "multipart-key-resolution",
            &key,
            &[
                (
                    "x-amz-server-side-encryption",
                    KMS_ALGORITHMS[index % KMS_ALGORITHMS.len()],
                ),
                (KEY_ID_HEADER, reference.as_str()),
            ],
        )
        .await;
        assert_eq!(
            refused.status, KMS_RELAY_STATUS,
            "case {case:?}: expected HTTP {KMS_RELAY_STATUS}, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case:?}: expected the code {code:?}, got {:?} ({:?})",
            refused.error_code(),
            text(&refused)
        );
        assert_eq!(
            refused.error_message(),
            *message,
            "case {case:?}: expected the message {message:?}, got {:?}",
            refused.error_message()
        );
        let listed = list_multipart_uploads(&router, "multipart-key-resolution").await;
        assert!(
            !text(&listed).contains(key.as_str()),
            "case {case:?}: the refused initiation must register no upload, got {:?}",
            text(&listed)
        );
    }

    // The identical call under a key KMS WILL mint a data key for is served end to end, and
    // the upload it registers names that key by its ARN.
    let (key_id, key_arn) = created_key(&router, "the key the served upload names").await;
    created_alias(&router, "alias/sse-inventory-live", &key_id).await;
    let initiated = create_multipart_upload(
        &router,
        "multipart-key-resolution",
        "served-upload.bin",
        &[
            ("x-amz-server-side-encryption", KMS_ALGORITHMS[0]),
            (KEY_ID_HEADER, "alias/sse-inventory-live"),
        ],
    )
    .await;
    assert_eq!(
        initiated.status, 200,
        "the initiation under a real alias must be served, got {} with body {:?}",
        initiated.status,
        text(&initiated)
    );
    assert_eq!(
        initiated.header(KEY_ID_HEADER).as_deref(),
        Some(key_arn.as_str()),
        "the initiation must name the key ARN the alias resolved to, got {:?}",
        initiated.header(KEY_ID_HEADER)
    );
    let listed = list_multipart_uploads(&router, "multipart-key-resolution").await;
    assert!(
        text(&listed).contains("served-upload.bin"),
        "the served initiation must register its upload, got {:?}",
        text(&listed)
    );
}

#[tokio::test]
async fn the_managed_key_is_materialised_once_and_reused_by_every_keyless_write() {
    // "If you specify a predefined Amazon Web Services alias … KMS associates the alias with
    // an Amazon Web Services managed key": the association happens once, so every later write
    // under it is encrypted under the SAME key rather than under a new one each time.
    let router = router_with_kms();
    bucket(&router, "managed-key-reuse").await;
    let configured =
        put_encryption(&router, "managed-key-reuse", &[Rule::by_default(KMS_ALGORITHMS[0])]).await;
    assert_eq!(
        configured.status, 200,
        "the bucket's keyless SSE-KMS default must be configured first, got {} with body {:?}",
        configured.status,
        text(&configured)
    );

    let mut reported: Vec<String> = Vec::new();
    for index in 0..3 {
        let key = format!("reused/{index}.txt");
        let written = put_object(&router, "managed-key-reuse", &key, BODY, &[]).await;
        assert_eq!(
            written.status, 200,
            "write {index} must be served, got {} with body {:?}",
            written.status,
            text(&written)
        );
        reported.push(written.header(KEY_ID_HEADER).unwrap_or_default());
    }
    let first = reported.first().cloned().unwrap_or_default();
    assert!(
        reported.iter().all(|arn| *arn == first) && !first.is_empty(),
        "every keyless SSE-KMS write must name the one managed key, got {reported:?}"
    );
    let (status, listed) = kms(&router, "ListAliases", &json!({})).await;
    assert_eq!(
        status, 200,
        "ListAliases must answer 200, got {status} with {listed}"
    );
    let associations = managed_alias_associations(&listed);
    assert_eq!(
        associations, 1,
        "the predefined alias must carry exactly one target key, got {associations} in {listed}"
    );
}

/// How many entries of a `ListAliases` body name [`AWS_MANAGED_KEY_ALIAS`] WITH a target key:
/// the association the first keyless write made, which no later write may duplicate.
fn managed_alias_associations(listed: &Value) -> usize {
    listed["Aliases"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|alias| {
            alias["AliasName"].as_str() == Some(AWS_MANAGED_KEY_ALIAS)
                && alias["TargetKeyId"].as_str().is_some()
        })
        .count()
}
