// SPDX-License-Identifier: CC0-1.0

//! Which SSE-KMS WRITES the object's own KMS key lets through.
//!
//! The rule under test: an object stored with server-side encryption under a customer-managed
//! KMS key is encrypted under a data key KMS mints for that key, so every write that stores
//! such an object performs `kms:GenerateDataKey` on it — as the request's own principal and
//! through S3's service path — before anything is recorded. "You must also have the
//! kms:GenerateDataKey and kms:Decrypt permissions in IAM identity-based policies and KMS key
//! policies for the KMS key" (`HeadObject`'s own documentation of what an SSE-KMS object
//! needs), the first of that pair being the write's. A caller no authority in force reaches
//! that action for is refused HTTP 403 `AccessDenied` carrying KMS's own denial sentence, and
//! the refused write stores NOTHING; a caller one of them does reach it for is stored and
//! reads back the exact bytes.
//!
//! What the data key is bound to is the `aws:s3:arn` encryption context: the BUCKET's ARN for
//! a write using an S3 Bucket Key — "setting this header to true causes Amazon S3 to use an S3
//! Bucket Key for object encryption with SSE-KMS" (`PutObjectRequest$BucketKeyEnabled`), one
//! data key per bucket rather than per object — and the OBJECT's own ARN otherwise, so a key
//! policy conditioned on that entry admits one of the two and withholds the other.
//!
//! The tables span what the rule generalises over rather than one recorded write:
//!
//! * which AUTHORITY is asked — the key's default policy (which delegates to the account),
//!   an explicit deny of the data-key action beside an allow, and a policy that allows the
//!   account another action only;
//! * the CONDITION a statement is scoped by — `kms:ViaService` naming this service's path and
//!   another service's, `kms:EncryptionContext:aws:s3:arn` naming the bucket and the object,
//!   and `kms:EncryptionContextKeys` naming the entry names the write carries;
//! * whether an S3 BUCKET KEY is in force, which is what switches that entry's value;
//! * the ALGORITHM — both `ServerSideEncryption` values a KMS key belongs to, and SSE-S3,
//!   which names no key and so asks KMS nothing;
//! * HOW the encryption reaches the write — the request's own headers, and the bucket's
//!   DEFAULT encryption configuration for a write that names none;
//! * the OPERATION that stores the object — `PutObject`, `CopyObject`, the
//!   `CreateMultipartUpload` that settles an upload's encryption, and the
//!   `UpdateObjectEncryption` that re-encrypts a stored object under another key;
//! * and that the refusal LIFTS the moment the key policy grants the action again, with no
//!   intervening call, while objects written before the deny stay readable.
//!
//! Every key, key policy, bucket, default encryption rule and object here is made with KMS's
//! and S3's own operations over one shared engine: nothing reaches into a store to plant a
//! key, a policy or a decision.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_encryption::{put_encryption, Rule};
use super::bucket_lifecycle::{send, send_bytes};
use super::kms_key_references::{kms, router_with_kms, REGION};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};

/// The `ServerSideEncryption` values a KMS key id belongs to
/// (`smithy_query(query="shape",shape="ServerSideEncryption")` lists both, and
/// `ApplyServerSideEncryptionByDefault$KMSMasterKeyID` is "allowed if and only if SSEAlgorithm
/// is set to aws:kms or aws:kms:dsse").
const KMS_ALGORITHMS: [&str; 2] = ["aws:kms", "aws:kms:dsse"];

/// The `ServerSideEncryption` value that is SSE-S3 — "server-side encryption with Amazon S3
/// managed keys", which names no KMS key of this account (same enum).
const SSE_S3: &str = "AES256";

/// The bytes every write in this suite carries. A key policy is about who may store an object
/// under the key, not about what is in it, so one payload serves every row.
const BODY: &[u8] = b"sse-kms data-key payload";

/// The code S3 answers an authorization refusal with, and the status it carries (Amazon S3
/// "Error responses": `AccessDenied`, HTTP 403).
const ACCESS_DENIED: &str = "AccessDenied";

/// The IAM action a write that stores an SSE-KMS object is authorized under.
const GENERATE_DATA_KEY_ACTION: &str = "kms:GenerateDataKey";

/// The action a READ of such an object is authorized under, which these rows leave alone: an
/// object written before a deny of the data-key action stays readable.
const DECRYPT_ACTION: &str = "kms:Decrypt";

/// The encryption-context ENTRY NAME S3 binds an object's data key to (Amazon S3 User Guide,
/// *Using server-side encryption with KMS keys*, "Encryption context").
const ARN_CONTEXT_ENTRY: &str = "aws:s3:arn";

/// The bucket every object of the first table is stored in.
const BUCKET: &str = "sse-kms-write-authz";

/// WHY a refused write was refused, which decides the sentence KMS words for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Denial {
    /// The key's own policy allows the action to neither the caller nor its account, so that
    /// document is the one that has to change.
    NoResourcePolicy,
    /// A statement of the key policy explicitly denies the action.
    ExplicitDeny,
}

impl Denial {
    /// The sentence KMS words this refusal in, written out here rather than built from the
    /// service's own helper so the wording has something to be wrong about: who was refused,
    /// which action, on which key, and the clause naming the document that decided it.
    fn message(self, action: &str, key_arn: &str) -> String {
        let head =
            format!("User: {} is not authorized to perform: {action} on resource: {key_arn}", principal());
        match self {
            Self::NoResourcePolicy => {
                format!("{head} because no resource-based policy allows the {action} action")
            }
            Self::ExplicitDeny => format!("{head} with an explicit deny in a resource-based policy"),
        }
    }
}

/// The principal every request of this suite speaks for: its access key id is one neither STS
/// nor IAM holds a record of, so it resolves to the account's own published credential — the
/// position AWS account root credentials are in, which no identity policy governs.
fn principal() -> String {
    format!(
        "arn:aws:iam::{}:user/emulator",
        engine::emulator_account_id()
    )
}

/// A customer-managed key carrying `document` as its key policy (its DEFAULT one when
/// `None`), as `(key id, key ARN)`.
///
/// A document here is the one UNDER TEST: it names whichever authority the row is about, so it
/// need not leave the creating caller a path to a later `kms:PutKeyPolicy` and KMS's policy
/// lockout safety check would refuse it. That check has its own tests in the KMS crate, so the
/// key is created the way the KMS model provides for.
async fn key_with(
    router: &MultiServiceRouter,
    case: &str,
    document: Option<&str>,
) -> (String, String) {
    let mut request = json!({"Description": case});
    if let Some(document) = document {
        request["Policy"] = json!(document);
        request["BypassPolicyLockoutSafetyCheck"] = json!(true);
    }
    let (status, payload) = kms(router, "CreateKey", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating the key must succeed, got {status} with {payload}"
    );
    let metadata = &payload["KeyMetadata"];
    let member = |name: &str| {
        metadata[name]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateKey must answer {name}, got {payload}"))
            .to_string()
    };
    (member("KeyId"), member("Arn"))
}

/// Attach `document` to `key_id` — the one call that changes who may store an object under the
/// key.
async fn put_key_policy(router: &MultiServiceRouter, case: &str, key_id: &str, document: &str) {
    let (status, payload) = kms(
        router,
        "PutKeyPolicy",
        &json!({
            "KeyId": key_id,
            "Policy": document,
            "BypassPolicyLockoutSafetyCheck": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: attaching the key policy must succeed, got {status} with {payload}"
    );
}

/// A key policy document carrying exactly `statements`.
fn policy(statements: Vec<Value>) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// A statement over every resource with the effect, actions and condition given, naming the
/// account's root user — the principal the DEFAULT key policy names, which delegates the
/// decision to that account's own identity-based policies (no `Condition` when it is
/// [`Value::Null`]).
fn account_statement(effect: &str, actions: Value, condition: Value) -> Value {
    let mut statement = json!({
        "Effect": effect,
        "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id())},
        "Action": actions,
        "Resource": "*",
    });
    if !condition.is_null() {
        statement["Condition"] = condition;
    }
    statement
}

/// The statement the default key policy carries: the account's root user over every KMS
/// action.
fn allow_account() -> Value {
    account_statement("Allow", json!("kms:*"), Value::Null)
}

/// The ARN of one bucket, asked of the one helper this test binary already names a bucket ARN
/// with — which is the value of the encryption-context entry a Bucket Key write binds its data
/// key to.
fn bucket_arn(name: &str) -> String {
    super::bucket_analytics_configuration::bucket_arn(name)
}

/// The ARN of one object, which is that entry's value for a write with no S3 Bucket Key: the
/// bucket's own ARN with the key as the rest of the resource part.
fn object_arn(name: &str, key: &str) -> String {
    format!("{}/{key}", bucket_arn(name))
}

/// The regional endpoint name of an AWS service, which is the value `kms:ViaService` carries.
fn endpoint(service: &str) -> String {
    format!("{service}.{REGION}.amazonaws.com")
}

/// One additional encryption-context pair as `SSEKMSEncryptionContext` carries it: "a
/// Base64-encoded string holding a UTF-8 encoded JSON of additional encryption context
/// key-value pairs" (`PutObjectRequest$SSEKMSEncryptionContext`), encoded with the same codec
/// the service decodes it with.
fn supplied_context(pairs: &Value) -> String {
    aws_smithy_types::base64::encode(
        serde_json::to_vec(pairs).expect("the context pairs serialize"),
    )
}

/// The headers one write sends for its encryption: the algorithm, the key it names, whether an
/// S3 Bucket Key is asked for, and any additional encryption context.
struct Headers {
    algorithm: &'static str,
    key_id: Option<String>,
    bucket_key: bool,
    context: Option<String>,
}

impl Headers {
    /// The header list, borrowed for one request.
    fn as_pairs(&self) -> Vec<(&str, &str)> {
        let mut headers: Vec<(&str, &str)> =
            vec![("x-amz-server-side-encryption", self.algorithm)];
        if let Some(key_id) = &self.key_id {
            headers.push((
                "x-amz-server-side-encryption-aws-kms-key-id",
                key_id.as_str(),
            ));
        }
        if self.bucket_key {
            headers.push(("x-amz-server-side-encryption-bucket-key-enabled", "true"));
        }
        if let Some(context) = &self.context {
            headers.push(("x-amz-server-side-encryption-context", context.as_str()));
        }
        headers
    }
}

/// One row of the authority table: an object written under one encryption configuration and
/// one key policy, and what that write must answer.
struct Row {
    case: &'static str,
    /// The object key this row writes — its own, so one row's policy cannot decide another's
    /// write.
    object_key: &'static str,
    /// The `ServerSideEncryption` the write names.
    algorithm: &'static str,
    /// Whether the write asks for an S3 Bucket Key, which binds the data key to the BUCKET
    /// rather than to the object.
    bucket_key: bool,
    /// The additional encryption context the write supplies beside the entry S3 always binds.
    context: Option<Value>,
    /// The key policy the object's key carries; `None` for a key carrying its default policy.
    policy: Option<String>,
    /// `None` for a write that must be stored.
    denied: Option<Denial>,
}

#[tokio::test]
async fn an_sse_kms_write_is_stored_only_when_the_keys_policy_permits_the_data_key() {
    let router = router_with_kms();
    bucket(&router, BUCKET).await;

    let rows = vec![
        Row {
            case: "the default key policy, which delegates to the account",
            object_key: "delegated",
            algorithm: KMS_ALGORITHMS[0],
            bucket_key: false,
            context: None,
            policy: None,
            denied: None,
        },
        Row {
            case: "the default key policy, dual-layer SSE-KMS",
            object_key: "delegated-dsse",
            algorithm: KMS_ALGORITHMS[1],
            bucket_key: false,
            context: None,
            policy: None,
            denied: None,
        },
        Row {
            case: "an explicit deny of the data-key action beside an allow of everything",
            object_key: "explicit-deny",
            algorithm: KMS_ALGORITHMS[0],
            bucket_key: false,
            context: None,
            policy: Some(policy(vec![
                allow_account(),
                account_statement("Deny", json!(GENERATE_DATA_KEY_ACTION), Value::Null),
            ])),
            denied: Some(Denial::ExplicitDeny),
        },
        Row {
            case: "that same deny, dual-layer SSE-KMS",
            object_key: "explicit-deny-dsse",
            algorithm: KMS_ALGORITHMS[1],
            bucket_key: false,
            context: None,
            policy: Some(policy(vec![
                allow_account(),
                account_statement("Deny", json!(GENERATE_DATA_KEY_ACTION), Value::Null),
            ])),
            denied: Some(Denial::ExplicitDeny),
        },
        Row {
            case: "a policy allowing the account the READ action only",
            object_key: "decrypt-only",
            algorithm: KMS_ALGORITHMS[0],
            bucket_key: false,
            context: None,
            policy: Some(policy(vec![account_statement(
                "Allow",
                json!(DECRYPT_ACTION),
                Value::Null,
            )])),
            denied: Some(Denial::NoResourcePolicy),
        },
        Row {
            case: "a policy confined to THIS service's path to the key",
            object_key: "via-service-s3",
            algorithm: KMS_ALGORITHMS[0],
            bucket_key: false,
            context: None,
            policy: Some(policy(vec![account_statement(
                "Allow",
                json!("kms:*"),
                json!({"StringEquals": {"kms:ViaService": endpoint("s3")}}),
            )])),
            denied: None,
        },
        Row {
            case: "a policy confined to ANOTHER service's path to the key",
            object_key: "via-service-dynamodb",
            algorithm: KMS_ALGORITHMS[0],
            bucket_key: false,
            context: None,
            policy: Some(policy(vec![account_statement(
                "Allow",
                json!("kms:*"),
                json!({"StringEquals": {"kms:ViaService": endpoint("dynamodb")}}),
            )])),
            denied: Some(Denial::NoResourcePolicy),
        },
        Row {
            case: "a policy scoped to the OBJECT's context entry, written with no Bucket Key",
            object_key: "object-context",
            algorithm: KMS_ALGORITHMS[0],
            bucket_key: false,
            context: None,
            policy: Some(policy(vec![account_statement(
                "Allow",
                json!("kms:*"),
                json!({"StringEquals": {
                    "kms:EncryptionContext:aws:s3:arn": object_arn(BUCKET, "object-context"),
                }}),
            )])),
            denied: None,
        },
        Row {
            case: "that same object-scoped policy, written WITH a Bucket Key",
            object_key: "object-context-bucket-key",
            algorithm: KMS_ALGORITHMS[0],
            bucket_key: true,
            context: None,
            policy: Some(policy(vec![account_statement(
                "Allow",
                json!("kms:*"),
                json!({"StringEquals": {
                    "kms:EncryptionContext:aws:s3:arn":
                        object_arn(BUCKET, "object-context-bucket-key"),
                }}),
            )])),
            denied: Some(Denial::NoResourcePolicy),
        },
        Row {
            case: "a policy scoped to the BUCKET's context entry, written WITH a Bucket Key",
            object_key: "bucket-context-bucket-key",
            algorithm: KMS_ALGORITHMS[0],
            bucket_key: true,
            context: None,
            policy: Some(policy(vec![account_statement(
                "Allow",
                json!("kms:*"),
                json!({"StringEquals": {"kms:EncryptionContext:aws:s3:arn": bucket_arn(BUCKET)}}),
            )])),
            denied: None,
        },
        Row {
            case: "that same bucket-scoped policy, written with no Bucket Key",
            object_key: "bucket-context",
            algorithm: KMS_ALGORITHMS[0],
            bucket_key: false,
            context: None,
            policy: Some(policy(vec![account_statement(
                "Allow",
                json!("kms:*"),
                json!({"StringEquals": {"kms:EncryptionContext:aws:s3:arn": bucket_arn(BUCKET)}}),
            )])),
            denied: Some(Denial::NoResourcePolicy),
        },
        Row {
            // `kms:EncryptionContextKeys` names the ENTRY NAMES of the request's encryption
            // context, one value per entry (KMS's own condition keys), so a statement scoped
            // by it governs the writes whose context carries exactly the entries it names.
            case: "a policy scoped to the entry NAMES the write's context carries",
            object_key: "context-keys",
            algorithm: KMS_ALGORITHMS[0],
            bucket_key: false,
            context: None,
            policy: Some(policy(vec![account_statement(
                "Allow",
                json!("kms:*"),
                json!({"ForAllValues:StringEquals": {
                    "kms:EncryptionContextKeys": ARN_CONTEXT_ENTRY,
                }}),
            )])),
            denied: None,
        },
        Row {
            case: "that same policy, with a context pair the write supplied beside the entry",
            object_key: "context-keys-supplied",
            algorithm: KMS_ALGORITHMS[0],
            bucket_key: false,
            context: Some(json!({"tenant": "payments"})),
            policy: Some(policy(vec![account_statement(
                "Allow",
                json!("kms:*"),
                json!({"ForAllValues:StringEquals": {
                    "kms:EncryptionContextKeys": ARN_CONTEXT_ENTRY,
                }}),
            )])),
            denied: Some(Denial::NoResourcePolicy),
        },
        Row {
            case: "an SSE-S3 write, which names no key, under a policy that grants nobody",
            object_key: "sse-s3",
            algorithm: SSE_S3,
            bucket_key: false,
            context: None,
            policy: Some(policy(vec![account_statement(
                "Deny",
                json!("kms:*"),
                Value::Null,
            )])),
            denied: None,
        },
    ];

    for row in rows {
        let case = row.case;
        let (key_id, key_arn) = key_with(&router, case, row.policy.as_deref()).await;
        let names_key = KMS_ALGORITHMS.contains(&row.algorithm);
        let headers = Headers {
            algorithm: row.algorithm,
            key_id: names_key.then(|| key_id.clone()),
            bucket_key: row.bucket_key,
            context: row.context.as_ref().map(supplied_context),
        };
        let written =
            put_object(&router, BUCKET, row.object_key, BODY, &headers.as_pairs()).await;

        match row.denied {
            None => {
                assert_eq!(
                    written.status, 200,
                    "case {case:?}: the write must be stored, got {} with body {:?}",
                    written.status,
                    text(&written)
                );
                // Stored, and really there: the object reads back as the bytes the write
                // carried, under the key the write named.
                let read = get_object(&router, BUCKET, row.object_key, "", &[]).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: the stored object must read back, got {} with body {:?}",
                    read.status,
                    text(&read)
                );
                assert_eq!(
                    read.body.as_ref(),
                    BODY,
                    "case {case:?}: expected the stored bytes, got {:?}",
                    text(&read)
                );
                assert_eq!(
                    read.header("x-amz-server-side-encryption").as_deref(),
                    Some(row.algorithm),
                    "case {case:?}: expected the algorithm {:?}, got {:?}",
                    row.algorithm,
                    read.header("x-amz-server-side-encryption")
                );
            }
            Some(denial) => {
                assert_eq!(
                    written.status, 403,
                    "case {case:?}: the write must be refused with HTTP 403, got {} with body \
                     {:?}",
                    written.status,
                    text(&written)
                );
                assert_eq!(
                    written.error_code(),
                    ACCESS_DENIED,
                    "case {case:?}: expected the {ACCESS_DENIED:?} code, got {:?}",
                    text(&written)
                );
                assert_eq!(
                    written.error_message(),
                    denial.message(GENERATE_DATA_KEY_ACTION, &key_arn),
                    "case {case:?}: expected KMS's denial sentence, got {:?}",
                    written.error_message()
                );
                // And nothing was stored: "Amazon S3 never adds partial objects"
                // (`PutObject`'s own documentation), so a refused write leaves the key empty.
                let read = get_object(&router, BUCKET, row.object_key, "", &[]).await;
                assert_eq!(
                    read.status, 404,
                    "case {case:?}: a refused write must store nothing, got {} with body {:?}",
                    read.status,
                    text(&read)
                );
            }
        }
    }
}

/// One row of the default-encryption table: the algorithm the bucket's default applies, and
/// whether that default asks for an S3 Bucket Key.
type DefaultRow = (&'static str, bool);

#[tokio::test]
async fn a_write_inheriting_a_bucket_default_mints_its_data_key_under_that_default_key() {
    let router = router_with_kms();

    // The rows differ along the algorithm the default applies and along whether it asks for an
    // S3 Bucket Key — which switches the entry the data key is bound to and leaves the
    // decision over an explicit deny of the action the same.
    let rows: [DefaultRow; 4] = [
        (KMS_ALGORITHMS[0], false),
        (KMS_ALGORITHMS[0], true),
        (KMS_ALGORITHMS[1], false),
        (KMS_ALGORITHMS[1], true),
    ];

    for (index, (algorithm, bucket_key)) in rows.iter().enumerate() {
        let case = format!("a bucket default of {algorithm} with a Bucket Key of {bucket_key}");
        let name = format!("sse-kms-default-{index}");
        bucket(&router, &name).await;
        let (key_id, key_arn) = key_with(&router, &case, None).await;
        let configured = put_encryption(
            &router,
            &name,
            &[Rule::by_default(algorithm)
                .with_key(&key_id)
                .with_bucket_key(*bucket_key)],
        )
        .await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: the bucket's default must be configured first, got {} with body {:?}",
            configured.status,
            text(&configured)
        );

        // A write carrying NO encryption header of its own is stored under that default, so
        // the data key is minted under the key the default names.
        let before = put_object(&router, &name, "before.bin", BODY, &[]).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the default-encrypted write must be stored, got {} with body {:?}",
            before.status,
            text(&before)
        );
        assert_eq!(
            before.header("x-amz-server-side-encryption").as_deref(),
            Some(*algorithm),
            "case {case:?}: expected the default's algorithm, got {:?}",
            before.header("x-amz-server-side-encryption")
        );

        // The one call that withdraws the data-key action, leaving the read action alone.
        put_key_policy(
            &router,
            &case,
            &key_id,
            &policy(vec![
                allow_account(),
                account_statement("Deny", json!(GENERATE_DATA_KEY_ACTION), Value::Null),
            ]),
        )
        .await;

        let refused = put_object(&router, &name, "after.bin", BODY, &[]).await;
        assert_eq!(
            refused.status, 403,
            "case {case:?}: the next default-encrypted write must be refused, got {} with body \
             {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            ACCESS_DENIED,
            "case {case:?}: expected the {ACCESS_DENIED:?} code, got {:?}",
            text(&refused)
        );
        assert_eq!(
            refused.error_message(),
            Denial::ExplicitDeny.message(GENERATE_DATA_KEY_ACTION, &key_arn),
            "case {case:?}: expected KMS's denial sentence, got {:?}",
            refused.error_message()
        );
        let missing = get_object(&router, &name, "after.bin", "", &[]).await;
        assert_eq!(
            missing.status, 404,
            "case {case:?}: the refused write must have created nothing, got {} with body {:?}",
            missing.status,
            text(&missing)
        );

        // The object written while the action was permitted stays readable: the deny is of the
        // data-key action alone, and a read opens the data key the write already minted.
        let still_readable = get_object(&router, &name, "before.bin", "", &[]).await;
        assert_eq!(
            still_readable.status, 200,
            "case {case:?}: an object written earlier must stay readable, got {} with body {:?}",
            still_readable.status,
            text(&still_readable)
        );
        assert_eq!(
            still_readable.body.as_ref(),
            BODY,
            "case {case:?}: expected the stored bytes, got {:?}",
            text(&still_readable)
        );

        // And the refusal lifts the moment the statement is withdrawn — the very next write is
        // stored, so no answer here is cached.
        put_key_policy(&router, &case, &key_id, &policy(vec![allow_account()])).await;
        let restored = put_object(&router, &name, "after.bin", BODY, &[]).await;
        assert_eq!(
            restored.status, 200,
            "case {case:?}: the write must be stored once the deny is gone, got {} with body \
             {:?}",
            restored.status,
            text(&restored)
        );
        let read = get_object(&router, &name, "after.bin", "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: that object must read back, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.body.as_ref(),
            BODY,
            "case {case:?}: expected the stored bytes, got {:?}",
            text(&read)
        );
    }
}

/// One row of the encryption-update table: the case, and whether the key the object is
/// re-encrypted TO denies the data-key action.
type UpdateRow = (&'static str, bool);

#[tokio::test]
async fn an_encryption_update_mints_a_data_key_under_the_key_it_re_encrypts_to() {
    let router = router_with_kms();
    let name = "sse-kms-encryption-update";
    bucket(&router, name).await;
    let object_key = "re-encrypted.bin";
    let (_, source_arn) = key_with(&router, "the key the object is stored under", None).await;
    let headers = Headers {
        algorithm: KMS_ALGORITHMS[0],
        key_id: Some(source_arn.clone()),
        bucket_key: false,
        context: None,
    };
    let stored = put_object(&router, name, object_key, BODY, &headers.as_pairs()).await;
    assert_eq!(
        stored.status, 200,
        "the object must be stored first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );

    // The deny comes first, so the row that is refused is judged against an object still
    // stored under the key it was written with.
    let rows: [UpdateRow; 2] = [
        ("a target key that denies the data key", true),
        ("a target key that permits it", false),
    ];

    for (case, denied) in rows {
        let document = denied.then(|| {
            policy(vec![
                allow_account(),
                account_statement("Deny", json!(GENERATE_DATA_KEY_ACTION), Value::Null),
            ])
        });
        let (_, target_arn) = key_with(&router, case, document.as_deref()).await;
        let answer = send(
            &router,
            "PUT",
            &format!("/{name}/{object_key}?encryption"),
            &[],
            &update_payload(&target_arn),
        )
        .await;
        if denied {
            assert_eq!(
                answer.status, 403,
                "case {case:?}: the update must be refused with HTTP 403, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.error_code(),
                ACCESS_DENIED,
                "case {case:?}: expected the {ACCESS_DENIED:?} code, got {:?}",
                text(&answer)
            );
            assert_eq!(
                answer.error_message(),
                Denial::ExplicitDeny.message(GENERATE_DATA_KEY_ACTION, &target_arn),
                "case {case:?}: expected KMS's denial sentence, got {:?}",
                answer.error_message()
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the update must be served, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
        }
        // What the object is stored under afterwards: the key it was written with for the
        // refused update, which changed nothing, and the target key for the served one.
        let expected = if denied { &source_arn } else { &target_arn };
        let read = get_object(&router, name, object_key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the object must still read back, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.body.as_ref(),
            BODY,
            "case {case:?}: expected the stored bytes, got {:?}",
            text(&read)
        );
        assert_eq!(
            read.header("x-amz-server-side-encryption-aws-kms-key-id")
                .as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected the object to be under {expected:?}, got {:?}",
            read.header("x-amz-server-side-encryption-aws-kms-key-id")
        );
    }
}

/// The `UpdateObjectEncryption` payload naming the key the object is re-encrypted to:
/// `ObjectEncryption` is a union whose only member is `SSEKMS`, published as `SSE-KMS`
/// (`smithy_query(query="shape",shape="ObjectEncryption")`), carrying that key's ARN.
fn update_payload(key_arn: &str) -> String {
    format!(
        "<ObjectEncryption xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><SSE-KMS>\
         <KMSKeyArn>{key_arn}</KMSKeyArn></SSE-KMS></ObjectEncryption>"
    )
}

/// One row of the write-operations table: the operation's name, and whether the key it stores
/// the object under denies the data-key action.
type OperationRow = (&'static str, bool);

#[tokio::test]
async fn every_write_that_stores_an_sse_kms_object_mints_a_data_key_under_its_key() {
    let router = router_with_kms();
    let name = "sse-kms-write-operations";
    bucket(&router, name).await;
    let source = put_object(&router, name, "source.bin", BODY, &[]).await;
    assert_eq!(
        source.status, 200,
        "the copy source must be written first, got {} with body {:?}",
        source.status,
        text(&source)
    );

    // Each operation stores an object of its own under a key the row decides the policy of:
    // every one of them settles the encryption an object is stored under, so every one of them
    // is the write that mints its data key.
    let rows: [OperationRow; 6] = [
        ("PutObject", false),
        ("PutObject", true),
        ("CopyObject", false),
        ("CopyObject", true),
        ("CreateMultipartUpload", false),
        ("CreateMultipartUpload", true),
    ];

    for (index, (operation, denied)) in rows.iter().enumerate() {
        let case = format!("{operation} under a key that {} the data key", if *denied { "denies" } else { "permits" });
        let document = denied.then(|| {
            policy(vec![
                allow_account(),
                account_statement("Deny", json!(GENERATE_DATA_KEY_ACTION), Value::Null),
            ])
        });
        let (key_id, key_arn) = key_with(&router, &case, document.as_deref()).await;
        let object_key = format!("written/{index}.bin");
        let headers = Headers {
            algorithm: KMS_ALGORITHMS[0],
            key_id: Some(key_id.clone()),
            bucket_key: false,
            context: None,
        };
        let mut pairs = headers.as_pairs();
        let copy_source = format!("/{name}/source.bin");
        let answer = match *operation {
            "PutObject" => put_object(&router, name, &object_key, BODY, &pairs).await,
            "CopyObject" => {
                pairs.push(("x-amz-copy-source", copy_source.as_str()));
                send_bytes(
                    &router,
                    "PUT",
                    &format!("/{name}/{object_key}"),
                    &pairs,
                    b"",
                )
                .await
            }
            _ => {
                send_bytes(
                    &router,
                    "POST",
                    &format!("/{name}/{object_key}?uploads"),
                    &pairs,
                    b"",
                )
                .await
            }
        };

        if *denied {
            assert_eq!(
                answer.status, 403,
                "case {case:?}: the write must be refused with HTTP 403, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.error_code(),
                ACCESS_DENIED,
                "case {case:?}: expected the {ACCESS_DENIED:?} code, got {:?}",
                text(&answer)
            );
            assert_eq!(
                answer.error_message(),
                Denial::ExplicitDeny.message(GENERATE_DATA_KEY_ACTION, &key_arn),
                "case {case:?}: expected KMS's denial sentence, got {:?}",
                answer.error_message()
            );
            // Nothing was recorded: no object at the key, and — for the initiation — no
            // upload for a later part write to join.
            let read = get_object(&router, name, &object_key, "", &[]).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}: a refused write must store nothing, got {} with body {:?}",
                read.status,
                text(&read)
            );
            let uploads = send_bytes(&router, "GET", &format!("/{name}?uploads"), &[], b"").await;
            assert!(
                !uploads.elements("Key").contains(&object_key),
                "case {case:?}: a refused initiation must record no upload, got {:?}",
                text(&uploads)
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the write must be served, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            if *operation == "CreateMultipartUpload" {
                assert!(
                    answer.element("UploadId").is_some(),
                    "case {case:?}: a served initiation reports an UploadId, got {:?}",
                    text(&answer)
                );
            } else {
                let read = get_object(&router, name, &object_key, "", &[]).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: the stored object must read back, got {} with body {:?}",
                    read.status,
                    text(&read)
                );
                assert_eq!(
                    read.body.as_ref(),
                    BODY,
                    "case {case:?}: expected the stored bytes, got {:?}",
                    text(&read)
                );
            }
        }
    }
}
