// SPDX-License-Identifier: CC0-1.0

//! Which reads of an SSE-KMS object the object's own KMS key lets through.
//!
//! The rule under test: an object stored with server-side encryption under a
//! customer-managed KMS key is readable only THROUGH that key, so every read that serves
//! such an object's bytes performs `kms:Decrypt` on it as the request's own principal and
//! through S3's service path — "if the object is encrypted with SSE-KMS, you must also have
//! the kms:GenerateDataKey and kms:Decrypt permissions in IAM identity-based policies and
//! KMS key policies for the KMS key" (`HeadObject`'s own documentation of what such a read
//! needs). A caller no authority in force reaches that action for is refused HTTP 403
//! `AccessDenied` carrying KMS's own denial sentence, instead of the body; a caller one of
//! them does reach it for is served the exact stored bytes.
//!
//! WHO the caller is comes from the credential the request presents: a session another
//! service vended speaks for the role it is a session of, not for the account, so a key
//! policy that grants the action to one role admits that role's sessions and leaves every
//! other caller to the policy's other statements. The sessions below are made with IAM's
//! and STS's OWN operations over the same engine (`CreateRole` + `PutRolePolicy` +
//! `AssumeRole`), and every key, key policy and object is made with KMS's and S3's own
//! operations: nothing here reaches into a store to plant a key, a policy or a decision.
//!
//! The tables span what the rule generalises over rather than one recorded read:
//!
//! * which AUTHORITY is asked — the key's own policy naming the caller's role outright, a
//!   policy that delegates to the account (where the principal's identity policies decide),
//!   an explicit deny beside an allow, and a policy that names neither the caller nor its
//!   account;
//! * the kind of CALLER — two role sessions whose roles carry different identity policies,
//!   and the account's own published credential;
//! * the CONDITION a statement is scoped by — `kms:ViaService` naming this service's path
//!   and another service's, and `kms:EncryptionContext:aws:s3:arn` naming the object the
//!   read is of and another object under the same key;
//! * the object's own ENCRYPTION — SSE-KMS, and SSE-S3, which names no key and so asks KMS
//!   nothing;
//! * the OPERATION that reads the bytes — `GetObject`, and the two copies that read a
//!   source object (`CopyObject`, `UploadPartCopy`);
//! * and that the refusal LIFTS the moment the key policy grants the role, with no
//!   intervening call.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::Answer;

/// The Region every request in this suite is signed for, and the endpoint it is addressed
/// to: an object and the key it is encrypted under live at the same endpoint.
const REGION: &str = "us-east-1";

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record of it, so it resolves to the account's ambient identity — the
/// position AWS account root credentials are in.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The bytes every object in this suite holds. A key policy is about who may have an
/// object's plaintext, not about what is in it, so one payload serves every row.
const BODY: &[u8] = b"sse-kms protected payload";

/// The bucket every row stores its object in.
const BUCKET: &str = "sse-kms-read-authz";

/// The `ServerSideEncryption` value that is SSE-KMS, the one a KMS key id belongs to
/// (`smithy_query(query="shape",shape="ServerSideEncryption")` lists `aws:kms` among its
/// values).
const SSE_KMS: &str = "aws:kms";

/// The `ServerSideEncryption` value that is SSE-S3 — "server-side encryption with Amazon S3
/// managed keys", which names no KMS key of this account (same enum).
const SSE_S3: &str = "AES256";

/// The IAM action a read that serves an SSE-KMS object's bytes is authorized under.
const DECRYPT_ACTION: &str = "kms:Decrypt";

/// The encryption-context ENTRY NAME S3 binds an object's data key to (Amazon S3 User
/// Guide, *Using server-side encryption with KMS keys*, "Encryption context"), which is
/// what `kms:EncryptionContextKeys` carries for such a read.
const ARN_CONTEXT_ENTRY: &str = "aws:s3:arn";

/// The code S3 answers an authorization refusal with, and the status it carries (Amazon S3
/// "Error responses": `AccessDenied`, HTTP 403).
const ACCESS_DENIED: &str = "AccessDenied";

/// An emulator holding S3 beside the services a decision about an SSE-KMS read reads from:
/// KMS (the key, its policy and the decision), IAM (the role and its identity policies) and
/// STS (the session a role is assumed into).
struct Emulator {
    router: MultiServiceRouter,
}

/// One role session a row reads as: the access key id its requests are signed with, and the
/// ARNs a key policy statement can name it by.
struct Session {
    /// The session's access key id, which is where this emulator reads WHO is calling.
    access_key_id: String,
    /// The assumed-role session ARN `sts:GetCallerIdentity` reports, which a refusal names.
    session_arn: String,
    /// The ARN of the role the session is of, which is how a key policy names it.
    role_arn: String,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the Region and the caller's access key id.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_s3_amazons3::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// One S3 request signed with `access_key_id` in its SigV4 credential scope — which is
    /// where this emulator reads which principal a request speaks for.
    async fn s3(
        &self,
        access_key_id: &str,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Answer {
        let host = format!("s3.{REGION}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host.clone())
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/s3/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            );
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the test request must build");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        Answer {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body,
        }
    }

    /// One KMS call, signed with `access_key_id`. KMS is an awsJson1_1 service reached by
    /// `X-Amz-Target: TrentService.<Operation>`.
    async fn kms(&self, access_key_id: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://kms.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("TrentService.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/kms/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the test request must build");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, payload)
    }

    /// A KMS call that must succeed.
    async fn kms_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, payload) = self.kms(ACCOUNT_KEY, operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: KMS {operation} {body} must succeed, got {status} with {payload}"
        );
        payload
    }

    /// One awsQuery call to IAM or STS, as the account's own credential.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// Create an IAM role carrying `policy` as its inline policy (when one is given), assume
    /// it through STS, and answer the session that assume minted.
    async fn assumed_role(&self, role_name: &str, policy: Option<&str>) -> Session {
        let account = engine::emulator_account_id();
        let role_arn = format!("arn:aws:iam::{account}:role/{role_name}");
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateRole",
                &[
                    ("RoleName", role_name.to_string()),
                    ("AssumeRolePolicyDocument", trust),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "creating the role {role_name:?} must succeed, got {status} {body}"
        );
        if let Some(policy) = policy {
            let (status, body) = self
                .query(
                    "iam",
                    "2010-05-08",
                    "PutRolePolicy",
                    &[
                        ("RoleName", role_name.to_string()),
                        ("PolicyName", "inline".to_string()),
                        ("PolicyDocument", policy.to_string()),
                    ],
                )
                .await;
            assert_eq!(
                status, 200,
                "putting the inline policy of {role_name:?} must succeed, got {status} {body}"
            );
        }
        let (status, body) = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    ("RoleArn", role_arn.clone()),
                    ("RoleSessionName", "probe".to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "assuming {role_name:?} must succeed, got {status} {body}"
        );
        Session {
            access_key_id: xml_element(&body, "AccessKeyId")
                .unwrap_or_else(|| panic!("AssumeRole must answer an AccessKeyId, got {body}")),
            session_arn: xml_element(&body, "Arn")
                .unwrap_or_else(|| panic!("AssumeRole must answer the session Arn, got {body}")),
            role_arn,
        }
    }

    /// A symmetric encryption key carrying `policy` as its key policy (its default one when
    /// `None`), as `(key id, key ARN)`.
    ///
    /// A `policy` here is the document UNDER TEST: it names whichever principal the row is
    /// about, so it need not leave the creating caller a path to a later `kms:PutKeyPolicy`
    /// and KMS's policy-lockout safety check would refuse it. That check has its own tests
    /// in the KMS crate, so the key is created the way the KMS model provides for.
    async fn key(&self, case: &str, policy: Option<&str>) -> (String, String) {
        let mut request = json!({"Description": case});
        if let Some(policy) = policy {
            request["Policy"] = json!(policy);
            request["BypassPolicyLockoutSafetyCheck"] = json!(true);
        }
        let metadata = self.kms_ok(case, "CreateKey", &request).await["KeyMetadata"].clone();
        (
            text(case, &metadata, "KeyId"),
            text(case, &metadata, "Arn"),
        )
    }

    /// Attach `document` to `key_id` — the call that makes a key readable by a principal its
    /// policy did not name before.
    async fn put_key_policy(&self, case: &str, key_id: &str, document: &str) {
        self.kms_ok(
            case,
            "PutKeyPolicy",
            &json!({
                "KeyId": key_id,
                "Policy": document,
                "BypassPolicyLockoutSafetyCheck": true,
            }),
        )
        .await;
    }

    /// `PUT /<bucket>` — the bucket every object in this suite is stored in.
    async fn create_bucket(&self) {
        let created = self
            .s3(ACCOUNT_KEY, "PUT", &format!("/{BUCKET}"), &[], b"")
            .await;
        assert_eq!(
            created.status, 200,
            "the bucket must be created first, got {} with body {:?}",
            created.status,
            body_text(&created)
        );
    }

    /// Store `key` under `algorithm`, naming `kms_key_id` when the algorithm is one that
    /// takes a key, and asking for an S3 Bucket Key when `bucket_key` — the write every
    /// row's read is of.
    async fn put_object(
        &self,
        case: &str,
        key: &str,
        algorithm: &str,
        kms_key_id: Option<&str>,
        bucket_key: bool,
    ) {
        let mut headers: Vec<(&str, &str)> = vec![("x-amz-server-side-encryption", algorithm)];
        if let Some(key_id) = kms_key_id {
            headers.push(("x-amz-server-side-encryption-aws-kms-key-id", key_id));
        }
        if bucket_key {
            headers.push(("x-amz-server-side-encryption-bucket-key-enabled", "true"));
        }
        let written = self
            .s3(
                ACCOUNT_KEY,
                "PUT",
                &format!("/{BUCKET}/{key}"),
                &headers,
                BODY,
            )
            .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the object must be stored first, got {} with body {:?}",
            written.status,
            body_text(&written)
        );
    }

    /// `GET /<bucket>/<key>` as `access_key_id` — the read under test.
    async fn get_object(&self, access_key_id: &str, key: &str) -> Answer {
        self.s3(
            access_key_id,
            "GET",
            &format!("/{BUCKET}/{key}"),
            &[],
            b"",
        )
        .await
    }
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC
/// 3986's unreserved characters literal — so a policy document's braces and quotes survive
/// the round trip.
///
/// A copy: an integration-test module cannot be imported across crates, so every suite that
/// drives an awsQuery service holds its own.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an awsQuery response. A copy, for the
/// same reason as [`form_encode`].
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// One string member of a JSON answer.
fn text(case: &str, payload: &Value, member: &str) -> String {
    payload[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member}, got {payload}"))
        .to_string()
}

/// An answer's body as text, for error documents.
fn body_text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// A key policy document carrying exactly `statements`.
fn policy(statements: Vec<Value>) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// A statement over every resource with the effect, principal, actions and condition given
/// (no `Condition` when it is [`Value::Null`]).
fn statement(effect: &str, principal: &str, actions: Value, condition: Value) -> Value {
    let mut statement = json!({
        "Effect": effect,
        "Principal": {"AWS": principal},
        "Action": actions,
        "Resource": "*",
    });
    if !condition.is_null() {
        statement["Condition"] = condition;
    }
    statement
}

/// The statement the DEFAULT key policy carries: the account's root user, which delegates
/// the decision to that account's own identity-based policies.
fn allow_account() -> Value {
    statement(
        "Allow",
        &format!("arn:aws:iam::{}:root", engine::emulator_account_id()),
        json!("kms:*"),
        Value::Null,
    )
}

/// An identity policy allowing `actions` on every resource — what a role carries when its
/// own policies are what reach an action.
fn identity_policy(actions: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": actions, "Resource": "*"}],
    })
    .to_string()
}

/// The ARN of the object `key` in [`BUCKET`], which is the value of the encryption-context
/// entry S3 binds an object's data key to — the bucket's own ARN with the key as the rest of
/// the resource part.
fn object_arn(key: &str) -> String {
    format!("{}/{key}", bucket_arn())
}

/// The ARN of [`BUCKET`] itself, which is that entry's value for an object stored under an
/// S3 Bucket Key — asked of the one helper this test binary already names a bucket ARN with.
fn bucket_arn() -> String {
    super::bucket_analytics_configuration::bucket_arn(BUCKET)
}

/// The regional endpoint name of an AWS service, which is the value `kms:ViaService`
/// carries.
fn endpoint(service: &str) -> String {
    format!("{service}.{REGION}.amazonaws.com")
}

/// WHY a refused read was refused, which decides the sentence KMS words for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Denial {
    /// The key's own policy allows the action to neither the caller nor its account, so
    /// that document is the one that has to change.
    NoResourcePolicy,
    /// The key policy delegates the action to the caller's account, and the principal's own
    /// identity policies do not allow it.
    NoIdentityPolicy,
    /// A statement of the key policy explicitly denies the action.
    ExplicitDeny,
}

impl Denial {
    /// The sentence KMS words this refusal in, written out here rather than built from the
    /// service's own helper so the wording has something to be wrong about: who was refused,
    /// which action, on which key, and the clause naming the document that decided it.
    fn message(self, principal: &str, key_arn: &str) -> String {
        let head = format!(
            "User: {principal} is not authorized to perform: {DECRYPT_ACTION} on resource: \
             {key_arn}"
        );
        match self {
            Self::NoResourcePolicy => format!(
                "{head} because no resource-based policy allows the {DECRYPT_ACTION} action"
            ),
            Self::NoIdentityPolicy => format!(
                "{head} because no identity-based policy allows the {DECRYPT_ACTION} action"
            ),
            Self::ExplicitDeny => format!("{head} with an explicit deny in a resource-based policy"),
        }
    }
}

/// One row: an object stored under one encryption configuration and one key policy, read by
/// one caller, and what that read must answer.
struct Row {
    case: &'static str,
    /// The object key this row writes and reads — its own, so one row's policy cannot
    /// decide another's read.
    object_key: &'static str,
    /// The `ServerSideEncryption` the object is stored under.
    algorithm: &'static str,
    /// Whether the write asks for an S3 Bucket Key, which binds the data key to the BUCKET
    /// rather than to the object.
    bucket_key: bool,
    /// The key policy the object's key carries, built once the roles exist; `None` for a key
    /// carrying its default policy.
    policy: Option<String>,
    /// Which caller reads, as its access key id, and the principal a refusal names.
    reader: (String, String),
    /// `None` for a read that must be served the stored bytes.
    denied: Option<Denial>,
}

#[tokio::test]
async fn an_sse_kms_read_is_served_only_to_a_caller_the_objects_key_permits_the_decrypt() {
    let emulator = Emulator::new();
    emulator.create_bucket().await;

    // Two role sessions whose roles differ in ONE thing: whether the role's own identity
    // policy reaches `kms:Decrypt`. Both may read objects out of S3.
    let reader = emulator
        .assumed_role("sse-kms-reader", Some(&identity_policy(json!("s3:*"))))
        .await;
    let decryptor = emulator
        .assumed_role(
            "sse-kms-decryptor",
            Some(&identity_policy(json!(["s3:*", DECRYPT_ACTION]))),
        )
        .await;
    // The account's own published credential, which no identity policy governs.
    let account = (
        ACCOUNT_KEY.to_string(),
        format!("arn:aws:iam::{}:user/emulator", engine::emulator_account_id()),
    );
    let as_reader = (reader.access_key_id.clone(), reader.session_arn.clone());
    let as_decryptor = (decryptor.access_key_id.clone(), decryptor.session_arn.clone());

    let rows = vec![
        Row {
            case: "the default key policy and a role whose identity policy allows the decrypt",
            object_key: "delegated-granted",
            algorithm: SSE_KMS,
            bucket_key: false,
            policy: None,
            reader: as_decryptor.clone(),
            denied: None,
        },
        Row {
            case: "the default key policy and a role whose identity policy does not",
            object_key: "delegated-withheld",
            algorithm: SSE_KMS,
            bucket_key: false,
            policy: None,
            reader: as_reader.clone(),
            denied: Some(Denial::NoIdentityPolicy),
        },
        Row {
            case: "a key policy naming the reading session's own role",
            object_key: "named-role",
            algorithm: SSE_KMS,
            bucket_key: false,
            policy: Some(policy(vec![statement(
                "Allow",
                &reader.role_arn,
                json!(DECRYPT_ACTION),
                Value::Null,
            )])),
            reader: as_reader.clone(),
            denied: None,
        },
        Row {
            case: "a key policy naming only ANOTHER role",
            object_key: "another-role",
            algorithm: SSE_KMS,
            bucket_key: false,
            policy: Some(policy(vec![statement(
                "Allow",
                &decryptor.role_arn,
                json!(DECRYPT_ACTION),
                Value::Null,
            )])),
            reader: as_reader.clone(),
            denied: Some(Denial::NoResourcePolicy),
        },
        Row {
            case: "a key policy naming only one role, read by the account's own credential",
            object_key: "role-only-account-reads",
            algorithm: SSE_KMS,
            bucket_key: false,
            policy: Some(policy(vec![statement(
                "Allow",
                &reader.role_arn,
                json!(DECRYPT_ACTION),
                Value::Null,
            )])),
            reader: account.clone(),
            denied: Some(Denial::NoResourcePolicy),
        },
        Row {
            case: "the default key policy, read by the account's own credential",
            object_key: "delegated-account-reads",
            algorithm: SSE_KMS,
            bucket_key: false,
            policy: None,
            reader: account.clone(),
            denied: None,
        },
        Row {
            case: "an explicit deny of the role beside an allow of it",
            object_key: "explicit-deny",
            algorithm: SSE_KMS,
            bucket_key: false,
            policy: Some(policy(vec![
                statement("Allow", &reader.role_arn, json!("kms:*"), Value::Null),
                statement("Deny", &reader.role_arn, json!(DECRYPT_ACTION), Value::Null),
            ])),
            reader: as_reader.clone(),
            denied: Some(Denial::ExplicitDeny),
        },
        Row {
            case: "a grant confined to THIS service's path to the key",
            object_key: "via-service-s3",
            algorithm: SSE_KMS,
            bucket_key: false,
            policy: Some(policy(vec![statement(
                "Allow",
                &reader.role_arn,
                json!(DECRYPT_ACTION),
                json!({"StringEquals": {"kms:ViaService": endpoint("s3")}}),
            )])),
            reader: as_reader.clone(),
            denied: None,
        },
        Row {
            case: "a grant confined to ANOTHER service's path to the key",
            object_key: "via-service-dynamodb",
            algorithm: SSE_KMS,
            bucket_key: false,
            policy: Some(policy(vec![statement(
                "Allow",
                &reader.role_arn,
                json!(DECRYPT_ACTION),
                json!({"StringEquals": {"kms:ViaService": endpoint("dynamodb")}}),
            )])),
            reader: as_reader.clone(),
            denied: Some(Denial::NoResourcePolicy),
        },
        Row {
            case: "a grant scoped to the encryption-context entry of the object being read",
            object_key: "context-in-scope",
            algorithm: SSE_KMS,
            bucket_key: false,
            policy: Some(policy(vec![statement(
                "Allow",
                &reader.role_arn,
                json!(DECRYPT_ACTION),
                json!({"StringEquals": {
                    "kms:EncryptionContext:aws:s3:arn": object_arn("context-in-scope"),
                }}),
            )])),
            reader: as_reader.clone(),
            denied: None,
        },
        Row {
            case: "a grant scoped to the entry of ANOTHER object under the same key",
            object_key: "context-out-of-scope",
            algorithm: SSE_KMS,
            bucket_key: false,
            policy: Some(policy(vec![statement(
                "Allow",
                &reader.role_arn,
                json!(DECRYPT_ACTION),
                json!({"StringEquals": {
                    "kms:EncryptionContext:aws:s3:arn": object_arn("context-in-scope"),
                }}),
            )])),
            reader: as_reader.clone(),
            denied: Some(Denial::NoResourcePolicy),
        },
        Row {
            // `kms:EncryptionContextKeys` names the ENTRY NAMES of the request's encryption
            // context, one value per entry (KMS's own condition keys), so a statement scoped
            // by it governs the requests whose context carries exactly the entries it names.
            case: "a grant scoped to the entry NAMES the read's context carries",
            object_key: "context-keys-in-scope",
            algorithm: SSE_KMS,
            bucket_key: false,
            policy: Some(policy(vec![statement(
                "Allow",
                &reader.role_arn,
                json!(DECRYPT_ACTION),
                json!({"ForAllValues:StringEquals": {
                    "kms:EncryptionContextKeys": ARN_CONTEXT_ENTRY,
                }}),
            )])),
            reader: as_reader.clone(),
            denied: None,
        },
        Row {
            case: "a grant scoped to an entry NAME the read's context does not carry",
            object_key: "context-keys-out-of-scope",
            algorithm: SSE_KMS,
            bucket_key: false,
            policy: Some(policy(vec![statement(
                "Allow",
                &reader.role_arn,
                json!(DECRYPT_ACTION),
                json!({"ForAllValues:StringEquals": {"kms:EncryptionContextKeys": "tenant"}}),
            )])),
            reader: as_reader.clone(),
            denied: Some(Denial::NoResourcePolicy),
        },
        Row {
            case: "an SSE-S3 object, which names no key, under a policy that grants nobody",
            object_key: "sse-s3",
            algorithm: SSE_S3,
            bucket_key: false,
            policy: Some(policy(vec![allow_account()])),
            reader: as_reader.clone(),
            denied: None,
        },
        Row {
            // An S3 Bucket Key binds the data key to the BUCKET, so the entry a statement is
            // scoped by names the bucket's ARN rather than the object's
            // (`PutObjectRequest$BucketKeyEnabled`).
            case: "a Bucket Key object under a grant scoped to the BUCKET's entry",
            object_key: "bucket-key-in-scope",
            algorithm: SSE_KMS,
            bucket_key: true,
            policy: Some(policy(vec![statement(
                "Allow",
                &reader.role_arn,
                json!(DECRYPT_ACTION),
                json!({"StringEquals": {"kms:EncryptionContext:aws:s3:arn": bucket_arn()}}),
            )])),
            reader: as_reader.clone(),
            denied: None,
        },
        Row {
            case: "an object with NO Bucket Key under that same bucket-scoped grant",
            object_key: "bucket-key-absent",
            algorithm: SSE_KMS,
            bucket_key: false,
            policy: Some(policy(vec![statement(
                "Allow",
                &reader.role_arn,
                json!(DECRYPT_ACTION),
                json!({"StringEquals": {"kms:EncryptionContext:aws:s3:arn": bucket_arn()}}),
            )])),
            reader: as_reader.clone(),
            denied: Some(Denial::NoResourcePolicy),
        },
    ];

    for row in rows {
        let case = row.case;
        // The key is created carrying its DEFAULT policy and the object written under that,
        // and the row's document attached AFTERWARDS: a write of an SSE-KMS object mints its
        // data key under the key too, so a policy written for the READ under test says
        // nothing about who may write and the staging write is made while the account's own
        // credential still reaches the key — the order the real service is exercised in,
        // where a key policy is narrowed around objects that already exist.
        let (key_id, key_arn) = emulator.key(case, None).await;
        let names_key = row.algorithm == SSE_KMS;
        emulator
            .put_object(
                case,
                row.object_key,
                row.algorithm,
                names_key.then_some(key_id.as_str()),
                row.bucket_key,
            )
            .await;
        if let Some(document) = &row.policy {
            emulator.put_key_policy(case, &key_id, document).await;
        }

        let (access_key_id, principal) = &row.reader;
        let read = emulator.get_object(access_key_id, row.object_key).await;
        match row.denied {
            None => {
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: the read must be served, got {} with body {:?}",
                    read.status,
                    body_text(&read)
                );
                assert_eq!(
                    read.body.as_ref(),
                    BODY,
                    "case {case:?}: expected the stored bytes, got {:?}",
                    body_text(&read)
                );
            }
            Some(denial) => {
                assert_eq!(
                    read.status, 403,
                    "case {case:?}: the read must be refused with HTTP 403, got {} with body \
                     {:?}",
                    read.status,
                    body_text(&read)
                );
                assert_eq!(
                    read.error_code(),
                    ACCESS_DENIED,
                    "case {case:?}: expected the {ACCESS_DENIED:?} code, got {:?}",
                    body_text(&read)
                );
                assert_eq!(
                    read.error_message(),
                    denial.message(principal, &key_arn),
                    "case {case:?}: expected KMS's denial sentence, got {:?}",
                    read.error_message()
                );
                assert!(
                    !read.body.as_ref().windows(BODY.len()).any(|at| at == BODY),
                    "case {case:?}: a refused read must not carry the object's bytes, got {:?}",
                    body_text(&read)
                );
            }
        }
    }
}

#[tokio::test]
async fn a_key_id_whose_account_segment_is_no_account_encrypts_nothing() {
    // `KeyId` takes a key ARN, `arn:<partition>:kms:<region>:<account>:key/<id>`
    // (`smithy_query(query="doc",shape="DecryptRequest",member="KeyId")`). An ARN whose
    // account segment is not an account id names no account, so no account's permission can
    // cover the key it addresses: KMS refuses such an identifier for want of permission
    // before it looks up anything. No data key can be minted under it, so the write that
    // would be encrypted under it is refused and leaves nothing at the key — there is no
    // stored object for a later read to be refused the plaintext of.
    let case = "an SSE-KMS write naming a key id that names no account";
    let emulator = Emulator::new();
    emulator.create_bucket().await;
    let object_key = "no-account-key-id";
    let written = emulator
        .s3(
            ACCOUNT_KEY,
            "PUT",
            &format!("/{BUCKET}/{object_key}"),
            &[
                ("x-amz-server-side-encryption", SSE_KMS),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id",
                    "arn:aws:kms:us-east-1:not-an-account:key/0d8a1e3f-1111-4a1b-9c2d-5e6f70819203",
                ),
            ],
            BODY,
        )
        .await;
    assert_eq!(
        written.status, 403,
        "case {case:?}: the write must be refused with HTTP 403, got {} with body {:?}",
        written.status,
        body_text(&written)
    );
    assert_eq!(
        written.error_code(),
        ACCESS_DENIED,
        "case {case:?}: expected the {ACCESS_DENIED:?} code, got {:?}",
        body_text(&written)
    );
    // KMS's sentence for a refusal that can report NOTHING about the resource — not even
    // whether it exists — written out here rather than built from the service's own helper.
    let expected = format!(
        "User: arn:aws:iam::{}:user/emulator is not authorized to perform: \
         kms:GenerateDataKey on this resource because the resource does not exist in this \
         Region, no resource-based policies allow access, or a resource-based policy \
         explicitly denies access",
        engine::emulator_account_id()
    );
    assert_eq!(
        written.error_message(),
        expected,
        "case {case:?}: expected KMS's sentence for an identifier it cannot authorize, got {:?}",
        written.error_message()
    );

    let read = emulator.get_object(ACCOUNT_KEY, object_key).await;
    assert_eq!(
        read.status, 404,
        "case {case:?}: a refused write must store nothing, got {} with body {:?}",
        read.status,
        body_text(&read)
    );
}

#[tokio::test]
async fn the_refusal_lifts_the_moment_the_key_policy_grants_the_role() {
    let case = "a key policy that grants the role only after the first read";
    let emulator = Emulator::new();
    emulator.create_bucket().await;
    let reader = emulator
        .assumed_role("late-grant-reader", Some(&identity_policy(json!("s3:*"))))
        .await;
    let (key_id, key_arn) = emulator
        .key(case, Some(&policy(vec![allow_account()])))
        .await;
    emulator
        .put_object(case, "late-grant", SSE_KMS, Some(&key_id), false)
        .await;

    // Nothing the account wrote reaches this session's role, so the key withholds the
    // plaintext from it.
    let refused = emulator
        .get_object(&reader.access_key_id, "late-grant")
        .await;
    assert_eq!(
        refused.status, 403,
        "case {case:?}: the first read must be refused, got {} with body {:?}",
        refused.status,
        body_text(&refused)
    );
    assert_eq!(
        refused.error_message(),
        Denial::NoIdentityPolicy.message(&reader.session_arn, &key_arn),
        "case {case:?}: expected KMS's denial sentence, got {:?}",
        refused.error_message()
    );

    // The one call that changes the decision — and the very next read is served, so no
    // answer here is cached.
    emulator
        .put_key_policy(
            case,
            &key_id,
            &policy(vec![
                allow_account(),
                statement(
                    "Allow",
                    &reader.role_arn,
                    json!(DECRYPT_ACTION),
                    Value::Null,
                ),
            ]),
        )
        .await;
    let served = emulator
        .get_object(&reader.access_key_id, "late-grant")
        .await;
    assert_eq!(
        served.status, 200,
        "case {case:?}: the read must be served once the policy grants the role, got {} with \
         body {:?}",
        served.status,
        body_text(&served)
    );
    assert_eq!(
        served.body.as_ref(),
        BODY,
        "case {case:?}: expected the stored bytes, got {:?}",
        body_text(&served)
    );
}

#[tokio::test]
async fn a_copy_reads_its_source_object_through_that_objects_own_key() {
    let emulator = Emulator::new();
    emulator.create_bucket().await;
    let reader = emulator
        .assumed_role("copy-source-reader", Some(&identity_policy(json!("s3:*"))))
        .await;
    let decryptor = emulator
        .assumed_role(
            "copy-source-decryptor",
            Some(&identity_policy(json!(["s3:*", DECRYPT_ACTION]))),
        )
        .await;
    let case = "a copy whose source is an SSE-KMS object";
    let (key_id, key_arn) = emulator.key(case, None).await;
    let source = "copy-source";
    emulator
        .put_object(case, source, SSE_KMS, Some(&key_id), false)
        .await;

    // (the operation, how it is driven) — both read the SOURCE object's bytes, so both are
    // authorized by `kms:Decrypt` on the source's key.
    for operation in ["CopyObject", "UploadPartCopy"] {
        for (who, session, denied) in [
            ("a role the key permits the decrypt", &decryptor, false),
            ("a role it does not", &reader, true),
        ] {
            let case = format!("{operation} as {who}");
            let destination = format!("{operation}-{}", if denied { "refused" } else { "served" });
            let copy_source = format!("/{BUCKET}/{source}");
            let answer = match operation {
                "CopyObject" => {
                    emulator
                        .s3(
                            &session.access_key_id,
                            "PUT",
                            &format!("/{BUCKET}/{destination}"),
                            &[("x-amz-copy-source", copy_source.as_str())],
                            b"",
                        )
                        .await
                }
                _ => {
                    // A part copy needs an upload to copy into, initiated by the account.
                    let initiated = emulator
                        .s3(
                            ACCOUNT_KEY,
                            "POST",
                            &format!("/{BUCKET}/{destination}?uploads"),
                            &[],
                            b"",
                        )
                        .await;
                    assert_eq!(
                        initiated.status, 200,
                        "case {case:?}: the upload must be initiated first, got {} with body \
                         {:?}",
                        initiated.status,
                        body_text(&initiated)
                    );
                    let upload_id = initiated.element("UploadId").unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: an initiation reports an UploadId, got {:?}",
                            body_text(&initiated)
                        )
                    });
                    emulator
                        .s3(
                            &session.access_key_id,
                            "PUT",
                            &format!(
                                "/{BUCKET}/{destination}?partNumber=1&uploadId={upload_id}\
                                 &x-id=UploadPartCopy"
                            ),
                            &[("x-amz-copy-source", copy_source.as_str())],
                            b"",
                        )
                        .await
                }
            };
            if denied {
                assert_eq!(
                    answer.status, 403,
                    "case {case:?}: the copy must be refused with HTTP 403, got {} with body \
                     {:?}",
                    answer.status,
                    body_text(&answer)
                );
                assert_eq!(
                    answer.error_code(),
                    ACCESS_DENIED,
                    "case {case:?}: expected the {ACCESS_DENIED:?} code, got {:?}",
                    body_text(&answer)
                );
                assert_eq!(
                    answer.error_message(),
                    Denial::NoIdentityPolicy.message(&session.session_arn, &key_arn),
                    "case {case:?}: expected KMS's denial sentence, got {:?}",
                    answer.error_message()
                );
                // And nothing was written at the destination: a refused copy copies nothing.
                let read = emulator.get_object(ACCOUNT_KEY, &destination).await;
                assert_eq!(
                    read.status, 404,
                    "case {case:?}: a refused copy must store nothing, got {} with body {:?}",
                    read.status,
                    body_text(&read)
                );
            } else {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: the copy must be served, got {} with body {:?}",
                    answer.status,
                    body_text(&answer)
                );
                // The copy really happened: it reports the entity tag of what it stored,
                // and for the whole-object copy the destination reads back as the source's
                // bytes.
                assert!(
                    answer.element("ETag").is_some(),
                    "case {case:?}: a served copy must report the stored ETag, got {:?}",
                    body_text(&answer)
                );
                if operation == "CopyObject" {
                    let read = emulator.get_object(ACCOUNT_KEY, &destination).await;
                    assert_eq!(
                        read.status, 200,
                        "case {case:?}: the copy's destination must read back, got {} with \
                         body {:?}",
                        read.status,
                        body_text(&read)
                    );
                    assert_eq!(
                        read.body.as_ref(),
                        BODY,
                        "case {case:?}: expected the source's bytes at the destination, got \
                         {:?}",
                        body_text(&read)
                    );
                }
            }
        }
    }
}
