// SPDX-License-Identifier: CC0-1.0

//! A Cognito-vended session reading an SSE-KMS object: the whole path a client takes to
//! verify an object's encryption on read, across three services on one engine.
//!
//! RULE: a read of a stored object reports the server-side-encryption configuration the
//! object was STORED under — `GetObject`'s `ServerSideEncryption`
//! (`x-amz-server-side-encryption`), `SSEKMSKeyId`
//! (`x-amz-server-side-encryption-aws-kms-key-id`) and `BucketKeyEnabled`
//! (`x-amz-server-side-encryption-bucket-key-enabled`) are the same members `PutObject`
//! echoes on write (`smithy_query(query="operation",operation="GetObject,HeadObject")`),
//! so a reader can tell an SSE-KMS object from a plain one. And the identity-pool calls a
//! client reaches that read with — `GetId` and `GetCredentialsForIdentity`, whose model
//! lists NO auth scheme (`auth: []`, `@optionalAuth`) — are served without a SigV4
//! credential scope, because a client exchanging a login for credentials has nothing to
//! sign with.
//!
//! Only a cross-service suite can pin the two halves together: each crate's own tests can
//! assert its own half (`service-…-s3`'s `object_encryption`, `service-…-cognitoidentity`'s
//! `anonymous_credential_exchange`), but not that the session a vend answers reaches the
//! read and sees the object's encryption.
//!
//! What each leg pins. The VEND legs carry no credential scope at all, so they are served
//! only because this router claims a request that names Cognito Identity in its
//! `X-Amz-Target` alone. The S3 legs carry the vended access key id in their credential
//! scope on a router with request authentication OFF, as every suite in this test crate
//! runs it (a signed row would otherwise need a real signature), so what they pin is the
//! READ's answer for the session's key id — the encryption each object was stored under —
//! and not signature verification, which is the emulator's own and is covered where it
//! lives.
//!
//! The rows vary what the rule generalises over: the key prefix the object is written
//! under, the algorithm (`aws:kms`, `aws:kms:dsse`, `AES256`, and a write naming no
//! encryption member at all, which inherits the bucket's default), the form of the KMS key
//! id the write names (the ARN KMS minted, its bare key id), and whether a Bucket Key is
//! enabled. The key id a READ reports is held to what the WRITE reported rather than to a
//! literal, because the rule here is that the two agree; whether S3 normalises a bare key
//! id to its full ARN is a question of the WRITE path, which this rule does not decide.
//!
//! The refusal suite below is about the other half of such a read: WHOSE read it is.
//! A read that serves an SSE-KMS object's bytes performs `kms:Decrypt` on the object's key as
//! the principal the request's credential speaks for, and a vended session speaks for the
//! role the POOL RESOLVED for the exchange that minted it — the pool's `unauthenticated`
//! role for a vend presenting no login, its `authenticated` role for one that does
//! (`SetIdentityPoolRoles`' own `Roles` map, and "Logins should not be specified when trying
//! to get credentials for an unauthenticated identity",
//! `GetCredentialsForIdentityInput$Logins`) — so a key policy naming one of those roles opens
//! the object for the sessions of THAT role and leaves every other session to the policy's
//! other statements.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The emulator's own address, which is what a client is pointed at: an address, so the
/// `Host` header names no service and the unsigned calls carry their service name in
/// `X-Amz-Target` alone.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The `SSEAlgorithm` a bucket no `PutBucketEncryption` has configured encrypts an object
/// with ("all buckets have a default encryption configuration that uses server-side
/// encryption with Amazon S3 managed keys", `GetBucketEncryption`'s documentation).
const DEFAULT_SSE_ALGORITHM: &str = "AES256";

/// What one call answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// A bound response header's value.
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// The body as JSON (`Null` when it is empty or not JSON), for the awsJson services.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&self.body).unwrap_or(Value::Null)
        }
    }

    /// The body as text, for S3's object bytes and its restXml error documents.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// How a request is signed: with a SigV4 credential scope naming a service and the access
/// key id it is signed with, or with no credential scope at all — which is how an SDK
/// sends an operation whose model lists no auth scheme.
#[derive(Clone, Copy, Debug)]
enum Signing<'a> {
    As {
        signing_name: &'a str,
        access_key_id: &'a str,
    },
    Anonymous,
}

/// Dispatch one request, addressed and signed as `signing` says.
fn dispatch(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing: Signing<'_>,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: bytes::Bytes,
) -> Answer {
    let host = match signing {
        Signing::As { signing_name, .. } => format!("{signing_name}.{REGION}.amazonaws.com"),
        Signing::Anonymous => EMULATOR_HOST.to_string(),
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host.clone());
    if let Signing::As {
        signing_name,
        access_key_id,
    } = signing
    {
        // The credential scope is how a signed request names the access key id it is
        // signed with, the service it is for, and the region a handler builds its ARNs
        // in. The signature is never checked: this router runs with authentication off.
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        );
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(body).expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The access key id every setup call is signed with: the emulator's own fixture key,
/// which is NOT the vended session.
const SETUP_KEY_ID: &str = "AKIDTEST";

/// The identity-provider name a login carrying a token THIS service minted is handed over
/// under — the Amazon Cognito Identity principal: "If the token is for
/// cognito-identity.amazonaws.com, it will be passed through to Security Token Service with
/// the appropriate role for the token" (`GetCredentialsForIdentity`'s own documentation).
const COGNITO_IDENTITY_PROVIDER: &str = "cognito-identity.amazonaws.com";

/// Which exchange a vend is, which is what the pool resolves its role from: "Logins should
/// not be specified when trying to get credentials for an unauthenticated identity"
/// (`GetCredentialsForIdentityInput$Logins`), so a vend presenting a login is the
/// AUTHENTICATED exchange and one presenting none is the guest exchange — and the pool's
/// `authenticated` and `unauthenticated` roles are the roles each resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Guest,
    Authenticated,
}

/// A setup call's signing, for the service whose signing name is `signing_name`.
fn as_setup(signing_name: &str) -> Signing<'_> {
    Signing::As {
        signing_name,
        access_key_id: SETUP_KEY_ID,
    }
}

/// One awsJson1_1 call, signed as `signing` says.
fn rpc(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing: Signing<'_>,
    target: &str,
    body: &Value,
) -> Answer {
    dispatch(
        router,
        runtime,
        signing,
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", target),
        ],
        bytes::Bytes::from(body.to_string()),
    )
}

/// `kms:CreateKey`, answering the new key's `(key id, key ARN)`.
fn create_key(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime) -> (String, String) {
    let created = rpc(
        router,
        runtime,
        as_setup("kms"),
        "TrentService.CreateKey",
        &json!({ "Description": "the CMK the objects below are written under" }),
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateKey must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let metadata = created.json();
    let key_id = metadata["KeyMetadata"]["KeyId"]
        .as_str()
        .expect("CreateKey reports a KeyId")
        .to_string();
    let key_arn = metadata["KeyMetadata"]["Arn"]
        .as_str()
        .expect("CreateKey reports the key ARN")
        .to_string();
    (key_id, key_arn)
}

/// `s3:CreateBucket`.
fn create_bucket(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime, bucket: &str) {
    let created = dispatch(
        router,
        runtime,
        as_setup("s3"),
        "PUT",
        &format!("/{bucket}"),
        &[("content-type", "application/xml")],
        bytes::Bytes::new(),
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateBucket({bucket:?}) must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
}

/// The trust policy of the role an identity pool vends sessions of, written the way the
/// Amazon Cognito Developer Guide writes one: the `Federated` Cognito Identity principal,
/// the web-identity assume, and the `:aud` condition that restricts the assume to the users
/// of that one pool.
///
/// Compact and carrying none of the characters that are significant in an
/// `application/x-www-form-urlencoded` body (`&`, `=`, `+`, `%`, space), so it needs no
/// encoding.
fn pool_trust_policy(pool_id: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Federated\":\"cognito-identity.amazonaws.com\"}},\
         \"Action\":\"sts:AssumeRoleWithWebIdentity\",\"Condition\":{{\"StringEquals\":\
         {{\"cognito-identity.amazonaws.com:aud\":\"{pool_id}\"}}}}}}]}}"
    )
}

/// The identity policy the first suite's pool role carries, so a session of it may READ those
/// rows' objects and have their plaintext: S3 authorizes the signed read itself against the
/// documents in force on the session's principal (`s3:GetObject`), and the objects' key
/// carries its DEFAULT key policy, which delegates the decision to this account's own
/// identity-based policies, so a read that serves an SSE-KMS object's bytes also performs
/// `kms:Decrypt` on that key as that principal. A pool role carrying NO such policy is
/// refused, which is what
/// [`a_vended_session_the_key_policy_does_not_reach_cannot_read_the_object`] drives below;
/// what the rows are about is what a read the policies DO permit reports about the object's
/// encryption.
///
/// Compact and carrying none of the characters that are significant in an
/// `application/x-www-form-urlencoded` body, exactly like [`pool_trust_policy`].
const POOL_ROLE_POLICY: &str = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
     \"Action\":[\"kms:Decrypt\",\"s3:GetObject\"],\"Resource\":\"*\"}]}";

/// The identity policy the roles of the SECOND suite's pool carry: the S3 read alone, and no
/// `kms:Decrypt`.
///
/// S3 authorizes the signed read itself against the session principal's own documents, so a
/// role with no policy at all is refused before the object's key is ever consulted — which
/// would not be the refusal that suite is about. With the read granted and the decrypt left to
/// the KEY POLICY, the refusal it drives is exactly the one it names.
const POOL_ROLE_READ_POLICY: &str = "{\"Version\":\"2012-10-17\",\"Statement\":\
     [{\"Effect\":\"Allow\",\"Action\":\"s3:GetObject\",\"Resource\":\"*\"}]}";

/// `iam:CreateRole` for a role an identity pool vends sessions of, carrying `policy` as its
/// inline policy when one is given, and answering its ARN.
fn create_pool_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    role_name: &str,
    pool_id: &str,
    policy: Option<&str>,
) -> String {
    let created = dispatch(
        router,
        runtime,
        as_setup("iam"),
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={role_name}\
             &AssumeRolePolicyDocument={}",
            pool_trust_policy(pool_id)
        )),
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateRole({role_name:?}) must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    if let Some(policy) = policy {
        let attached = dispatch(
            router,
            runtime,
            as_setup("iam"),
            "POST",
            "/",
            &[("content-type", "application/x-www-form-urlencoded")],
            bytes::Bytes::from(format!(
                "Action=PutRolePolicy&Version=2010-05-08&RoleName={role_name}\
                 &PolicyName=read-sse-kms-objects&PolicyDocument={policy}"
            )),
        );
        assert_eq!(
            attached.status,
            200,
            "setup: PutRolePolicy({role_name:?}) must succeed, got {} with {:?}",
            attached.status,
            attached.text()
        );
    }
    let body = created.text();
    let open = "<Arn>";
    let close = "</Arn>";
    let start = body
        .find(open)
        .unwrap_or_else(|| panic!("setup: no <Arn> in {body}"))
        + open.len();
    let end = body[start..]
        .find(close)
        .unwrap_or_else(|| panic!("setup: no </Arn> in {body}"))
        + start;
    body[start..end].to_string()
}

/// `cognito-identity:CreateIdentityPool`, answering the pool's id.
fn create_identity_pool(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    pool_name: &str,
) -> String {
    let pool = rpc(
        router,
        runtime,
        as_setup("cognito-identity"),
        "AWSCognitoIdentityService.CreateIdentityPool",
        &json!({ "IdentityPoolName": pool_name, "AllowUnauthenticatedIdentities": true }),
    );
    assert_eq!(
        pool.status,
        200,
        "setup: CreateIdentityPool must succeed, got {} with {:?}",
        pool.status,
        pool.text()
    );
    pool.json()["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool reports an IdentityPoolId")
        .to_string()
}

/// `cognito-identity:SetIdentityPoolRoles` — which role the pool vends sessions of for each
/// kind of identity.
fn set_pool_roles(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    pool_id: &str,
    roles: Value,
) {
    let configured = rpc(
        router,
        runtime,
        as_setup("cognito-identity"),
        "AWSCognitoIdentityService.SetIdentityPoolRoles",
        &json!({ "IdentityPoolId": pool_id, "Roles": roles }),
    );
    assert_eq!(
        configured.status,
        200,
        "setup: SetIdentityPoolRoles must succeed, got {} with {:?}",
        configured.status,
        configured.text()
    );
}

/// The access key id of a session vended to a fresh identity of `pool_id`, reached the way a
/// client reaches it: `GetId` and `GetCredentialsForIdentity` sent with NO credential scope.
///
/// A vend with no `Logins` is an UNAUTHENTICATED exchange, so the session is one of the
/// pool's unauthenticated (guest) role.
fn vend_guest_session(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    pool_id: &str,
) -> String {
    vend_session(router, runtime, pool_id, Flow::Guest)
}

/// The access key id of a session vended to a fresh identity of `pool_id` for the exchange
/// `flow` is: `GetId`, the `GetOpenIdToken` an authenticated exchange presents as its login,
/// and `GetCredentialsForIdentity` — each sent with NO credential scope, the way a client
/// holding no Amazon Web Services credential sends them.
fn vend_session(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    pool_id: &str,
    flow: Flow,
) -> String {
    let identity = rpc(
        router,
        runtime,
        Signing::Anonymous,
        "AWSCognitoIdentityService.GetId",
        &json!({ "IdentityPoolId": pool_id }),
    );
    assert_eq!(
        identity.status,
        200,
        "an unsigned GetId must be served, got {} with {:?}",
        identity.status,
        identity.text()
    );
    let identity_id = identity.json()["IdentityId"]
        .as_str()
        .expect("GetId reports an IdentityId")
        .to_string();

    // The login an AUTHENTICATED exchange presents: a token this service itself minted FOR
    // THIS identity, handed over under this service's own provider name. The guest exchange
    // presents none at all, which is what makes it the unauthenticated one.
    let mut request = json!({ "IdentityId": &identity_id });
    if flow == Flow::Authenticated {
        let mut logins = serde_json::Map::new();
        logins.insert(
            COGNITO_IDENTITY_PROVIDER.to_string(),
            Value::String(open_id_token(router, runtime, &identity_id)),
        );
        request["Logins"] = Value::Object(logins);
    }
    let vended = rpc(
        router,
        runtime,
        Signing::Anonymous,
        "AWSCognitoIdentityService.GetCredentialsForIdentity",
        &request,
    );
    assert_eq!(
        vended.status,
        200,
        "case {flow:?}: an unsigned GetCredentialsForIdentity must be served, got {} with \
         {:?}",
        vended.status,
        vended.text()
    );
    let credentials = vended.json();
    let access_key_id = credentials["Credentials"]["AccessKeyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !access_key_id.is_empty(),
        "the vend must carry an AccessKeyId, but the member was absent or empty"
    );
    // A client cannot sign a request without the session's secret and token either, so
    // each must be there — asserted by presence alone, so no secret reaches the
    // assertion text.
    for member in ["SecretKey", "SessionToken"] {
        assert!(
            !credentials["Credentials"][member]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {member:?}: the vended session must carry it, but the member was absent \
             or empty"
        );
    }
    access_key_id
}

/// An unsigned `cognito-identity:GetOpenIdToken`, answering the OpenID Connect token this
/// service issued for `identity_id` — the value an authenticated exchange presents as its
/// login under this service's own provider name.
fn open_id_token(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    identity_id: &str,
) -> String {
    let issued = rpc(
        router,
        runtime,
        Signing::Anonymous,
        "AWSCognitoIdentityService.GetOpenIdToken",
        &json!({ "IdentityId": identity_id }),
    );
    assert_eq!(
        issued.status,
        200,
        "setup: an unsigned GetOpenIdToken must be served, got {} with {:?}",
        issued.status,
        issued.text()
    );
    issued.json()["Token"]
        .as_str()
        .expect("GetOpenIdToken reports a Token")
        .to_string()
}

/// The access key id of a guest session of a fresh pool whose unauthenticated role carries
/// `unauthenticated_policy`, with the ARN of that role.
///
/// A vend is a session of the pool's role, so the pool is first given one: the role is
/// created by `iam:CreateRole` and attached by `cognito-identity:SetIdentityPoolRoles`,
/// both over the wire, so the trust policy the vend's assume is decided against is the
/// document IAM's own handler stored.
fn vended_access_key_id(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    pool_name: &str,
    unauthenticated_policy: Option<&str>,
) -> (String, String) {
    let pool_id = create_identity_pool(router, runtime, pool_name);
    let role_arn = create_pool_role(
        router,
        runtime,
        &format!("{pool_name}-unauth"),
        &pool_id,
        unauthenticated_policy,
    );
    set_pool_roles(
        router,
        runtime,
        &pool_id,
        json!({ "unauthenticated": &role_arn }),
    );
    (vend_guest_session(router, runtime, &pool_id), role_arn)
}

/// `s3:GetObject` signed with the session `access_key_id` names — the read the vended
/// session performs.
fn vended_get_object(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
    bucket: &str,
    key: &str,
) -> Answer {
    dispatch(
        router,
        runtime,
        Signing::As {
            signing_name: "s3",
            access_key_id,
        },
        "GET",
        &format!("/{bucket}/{key}"),
        &[],
        bytes::Bytes::new(),
    )
}

/// One row: the key the object is written under, the encryption headers the write carries,
/// and the encryption the read must then report. The KMS key id is not written out as an
/// expectation of its own: the rule is that a READ agrees with the WRITE about it, so each
/// row's reference is what the write itself reported — which leaves open whether S3
/// normalises a bare key id to its ARN, a question of the WRITE path and not of this rule.
struct Row {
    case: &'static str,
    key: &'static str,
    write_headers: Vec<(String, String)>,
    expected_algorithm: Option<String>,
    /// Whether the object is stored under SSE-KMS, which is the only encryption a key id
    /// and a Bucket Key setting belong to.
    kms: bool,
    expected_bucket_key: Option<&'static str>,
}

/// The table, built around the key KMS minted for this run.
fn rows(key_id: &str, key_arn: &str) -> Vec<Row> {
    vec![
        Row {
            case: "SSE-KMS under one prefix, the key named by ARN, no Bucket Key",
            key: "open/report.csv",
            write_headers: vec![
                ("x-amz-server-side-encryption".to_string(), "aws:kms".to_string()),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id".to_string(),
                    key_arn.to_string(),
                ),
                (
                    "x-amz-server-side-encryption-bucket-key-enabled".to_string(),
                    "false".to_string(),
                ),
            ],
            expected_algorithm: Some("aws:kms".to_string()),
            kms: true,
            // The header states that S3 USED an S3 Bucket Key on the object, so a write that
            // switched it off leaves no such header on the object at all.
            expected_bucket_key: None,
        },
        Row {
            case: "SSE-KMS under another prefix, the key named by its bare id",
            key: "closed/report.csv",
            write_headers: vec![
                ("x-amz-server-side-encryption".to_string(), "aws:kms".to_string()),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id".to_string(),
                    key_id.to_string(),
                ),
            ],
            expected_algorithm: Some("aws:kms".to_string()),
            kms: true,
            expected_bucket_key: None,
        },
        Row {
            case: "dual-layer SSE-KMS with a Bucket Key, deep under a prefix",
            key: "open/nested/deeper/report.csv",
            write_headers: vec![
                (
                    "x-amz-server-side-encryption".to_string(),
                    "aws:kms:dsse".to_string(),
                ),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id".to_string(),
                    key_arn.to_string(),
                ),
                (
                    "x-amz-server-side-encryption-bucket-key-enabled".to_string(),
                    "true".to_string(),
                ),
            ],
            expected_algorithm: Some("aws:kms:dsse".to_string()),
            kms: true,
            expected_bucket_key: Some("true"),
        },
        Row {
            // Not SSE-KMS: the read reports the algorithm this object was stored under
            // and no KMS member, so what a reader sees follows the write rather than a
            // constant.
            case: "SSE-S3, named explicitly",
            key: "open/plain.csv",
            write_headers: vec![(
                "x-amz-server-side-encryption".to_string(),
                "AES256".to_string(),
            )],
            expected_algorithm: Some("AES256".to_string()),
            kms: false,
            expected_bucket_key: None,
        },
        Row {
            // A write naming no encryption member at all is stored under the bucket's
            // default encryption configuration, which on a bucket nothing configured is
            // SSE-S3.
            case: "no encryption member at all: the bucket's default",
            key: "open/inherited.csv",
            write_headers: Vec::new(),
            expected_algorithm: Some(DEFAULT_SSE_ALGORITHM.to_string()),
            kms: false,
            expected_bucket_key: None,
        },
    ]
}

#[test]
fn a_vended_session_reads_the_encryption_each_object_was_stored_under() {
    std::thread::Builder::new()
        .name("cognito-vended-sse-kms-read".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            let bucket = "vended-session-encryption";
            let (key_id, key_arn) = create_key(&router, &runtime);
            create_bucket(&router, &runtime, bucket);
            let (access_key_id, _role_arn) = vended_access_key_id(
                &router,
                &runtime,
                "vended-session-pool",
                Some(POOL_ROLE_POLICY),
            );

            for row in rows(&key_id, &key_arn) {
                let payload = format!("bytes of {}", row.key).into_bytes();
                let write_headers: Vec<(&str, &str)> = row
                    .write_headers
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.as_str()))
                    .collect();
                let written = dispatch(
                    &router,
                    &runtime,
                    as_setup("s3"),
                    "PUT",
                    &format!("/{bucket}/{}", row.key),
                    &write_headers,
                    bytes::Bytes::from(payload.clone()),
                );
                assert_eq!(
                    written.status,
                    200,
                    "case {:?}: the write must succeed, got {} with {:?}",
                    row.case,
                    written.status,
                    written.text()
                );
                assert_eq!(
                    written.header("x-amz-server-side-encryption"),
                    row.expected_algorithm,
                    "case {:?}: the write must report the encryption it stored the object \
                     under, got {:?}",
                    row.case,
                    written.header("x-amz-server-side-encryption")
                );
                // The KMS key id the WRITE reported is the reference the read is held to:
                // a key id belongs to SSE-KMS alone, so a row stored under any other
                // algorithm must report none at all.
                let written_key_id = written.header("x-amz-server-side-encryption-aws-kms-key-id");
                assert_eq!(
                    written_key_id.is_some(),
                    row.kms,
                    "case {:?}: a KMS key id belongs to an SSE-KMS write and to no other, \
                     got {written_key_id:?}",
                    row.case
                );

                let read = vended_get_object(&router, &runtime, &access_key_id, bucket, row.key);
                assert_eq!(
                    read.status,
                    200,
                    "case {:?}: the vended session's read must succeed, got {} with {:?}",
                    row.case,
                    read.status,
                    read.text()
                );
                assert_eq!(
                    read.body.as_ref(),
                    payload.as_slice(),
                    "case {:?}: the read must answer the stored bytes, got {:?}",
                    row.case,
                    read.text()
                );
                assert_eq!(
                    read.header("x-amz-server-side-encryption"),
                    row.expected_algorithm,
                    "case {:?}: the read must report the encryption the object was stored \
                     under, got {:?}",
                    row.case,
                    read.header("x-amz-server-side-encryption")
                );
                assert_eq!(
                    read.header("x-amz-server-side-encryption-aws-kms-key-id"),
                    written_key_id,
                    "case {:?}: the read must report the same KMS key the write did, got \
                     {:?}",
                    row.case,
                    read.header("x-amz-server-side-encryption-aws-kms-key-id")
                );
                assert_eq!(
                    read.header("x-amz-server-side-encryption-bucket-key-enabled")
                        .as_deref(),
                    row.expected_bucket_key,
                    "case {:?}: the read must report the Bucket Key setting the object was \
                     stored under, got {:?}",
                    row.case,
                    read.header("x-amz-server-side-encryption-bucket-key-enabled")
                );

                // The same statement from the other reader: `HeadObject` binds the same
                // three members, so a client that branches on either sees one answer.
                let headed = dispatch(
                    &router,
                    &runtime,
                    as_setup("s3"),
                    "HEAD",
                    &format!("/{bucket}/{}", row.key),
                    &[],
                    bytes::Bytes::new(),
                );
                assert_eq!(
                    headed.status,
                    200,
                    "case {:?}: HeadObject must succeed, got {} with {:?}",
                    row.case,
                    headed.status,
                    headed.text()
                );
                assert_eq!(
                    headed.header("x-amz-server-side-encryption"),
                    row.expected_algorithm,
                    "case {:?}: HeadObject must report the same encryption GetObject does, \
                     got {:?}",
                    row.case,
                    headed.header("x-amz-server-side-encryption")
                );
            }
        })
        .expect("spawn the vended-session read thread")
        .join()
        .expect("a vended session must read every object's stored encryption");
}

/// The text of the first `<name>…</name>` element of a restXml body — S3's error document
/// carries its `<Code>` and `<Message>` there.
fn element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// `kms:CreateKey` carrying `policy` as the key's own key policy, answering `(key id, key
/// ARN)`.
///
/// The document is the one UNDER TEST: it names the roles the case is about and need not
/// leave the creating caller a path to a later `kms:PutKeyPolicy`, which KMS's policy-lockout
/// safety check otherwise requires — so the key is created the way the KMS model provides
/// for.
fn create_key_with_policy(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    policy: &str,
) -> (String, String) {
    let created = rpc(
        router,
        runtime,
        as_setup("kms"),
        "TrentService.CreateKey",
        &json!({
            "Description": "the CMK whose policy decides the vended session's read",
            "Policy": policy,
            "BypassPolicyLockoutSafetyCheck": true,
        }),
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateKey must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let metadata = created.json();
    (
        metadata["KeyMetadata"]["KeyId"]
            .as_str()
            .expect("CreateKey reports a KeyId")
            .to_string(),
        metadata["KeyMetadata"]["Arn"]
            .as_str()
            .expect("CreateKey reports the key ARN")
            .to_string(),
    )
}

/// A key policy document granting `kms:Decrypt` to each principal in `roles`, beside the
/// statement every manageable key carries: the account's root user, which delegates every
/// other decision to this account's own identity-based policies.
fn key_policy(roles: &[&str]) -> String {
    let account = engine::emulator_account_id();
    let mut statements = vec![json!({
        "Sid": "EnableIAMUserPermissions",
        "Effect": "Allow",
        "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
        "Action": "kms:*",
        "Resource": "*",
    })];
    for role in roles {
        statements.push(json!({
            "Effect": "Allow",
            "Principal": {"AWS": role},
            "Action": "kms:Decrypt",
            "Resource": "*",
        }));
    }
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// A Cognito-vended GUEST session reading an object encrypted under a CMK whose key policy
/// grants `kms:Decrypt` to the pool's AUTHENTICATED role only.
///
/// RULE: a read that serves an SSE-KMS object's BYTES performs `kms:Decrypt` on the object's
/// key as the principal the request's credential speaks for — for a vended session, the role
/// it is a session of — so a key policy that reaches only another role refuses it with HTTP
/// 403 `AccessDenied` and serves no bytes, and the very next read is served once the policy
/// reaches the session's own role. Only a cross-service suite can pin this: the credential
/// is minted by `cognito-identity:GetCredentialsForIdentity`, the session is recorded by STS,
/// the roles and their policies by IAM, the key policy by KMS, and the read is S3's.
///
/// The S3 crate's `sse_kms_decrypt_authorization` suite tables the same rule over the
/// authorities, callers, conditions and operations it generalises over; this is the one leg
/// only the assembled router can drive.
///
/// It closes by tabling the dimension that leg is one row of, and that only an identity pool
/// has: WHICH of the pool's roles a vended session speaks for. The pool resolves its
/// `unauthenticated` role for an exchange presenting no login and its `authenticated` role
/// for one that does, so the key policy serves such a read exactly when it reaches the role
/// the pool resolved for that session's own exchange — every subset of the two roles against
/// a session of each exchange ([`RESOLVED_ROLE_ROWS`]), on the one router, pool, key and
/// object this suite already stands up.
#[test]
fn a_vended_session_the_key_policy_does_not_reach_cannot_read_the_object() {
    std::thread::Builder::new()
        .name("cognito-vended-sse-kms-denied".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            let bucket = "vended-session-key-policy";
            let key = "private/ledger.csv";
            let payload = b"the bytes the key policy protects";
            create_bucket(&router, &runtime, bucket);

            // The pool and its two roles. Each session below is a session of one of them,
            // and BOTH carry the S3 read and no `kms:Decrypt` of their own
            // ([`POOL_ROLE_READ_POLICY`]), so what decides either session's read is the KEY
            // policy.
            let pool_name = "vended-session-denied-pool";
            let pool_id = create_identity_pool(&router, &runtime, pool_name);
            let guest_role = create_pool_role(
                &router,
                &runtime,
                &format!("{pool_name}-unauth"),
                &pool_id,
                Some(POOL_ROLE_READ_POLICY),
            );
            let authenticated = create_pool_role(
                &router,
                &runtime,
                &format!("{pool_name}-auth"),
                &pool_id,
                Some(POOL_ROLE_READ_POLICY),
            );
            set_pool_roles(
                &router,
                &runtime,
                &pool_id,
                json!({
                    "unauthenticated": &guest_role,
                    "authenticated": &authenticated,
                }),
            );
            // The key grants the decrypt to the AUTHENTICATED role alone.
            let (key_id, _key_arn) =
                create_key_with_policy(&router, &runtime, &key_policy(&[&authenticated]));
            let guest_key_id = vend_guest_session(&router, &runtime, &pool_id);

            let written = dispatch(
                &router,
                &runtime,
                as_setup("s3"),
                "PUT",
                &format!("/{bucket}/{key}"),
                &[
                    ("x-amz-server-side-encryption", "aws:kms"),
                    ("x-amz-server-side-encryption-aws-kms-key-id", &key_id),
                ],
                bytes::Bytes::from(payload.to_vec()),
            );
            assert_eq!(
                written.status,
                200,
                "setup: the SSE-KMS write must succeed, got {} with {:?}",
                written.status,
                written.text()
            );

            let refused = vended_get_object(&router, &runtime, &guest_key_id, bucket, key);
            assert_eq!(
                refused.status,
                403,
                "the guest session's read must be refused with HTTP 403, got {} with {:?}",
                refused.status,
                refused.text()
            );
            assert_eq!(
                element(&refused.text(), "Code").as_deref(),
                Some("AccessDenied"),
                "the refusal must carry S3's AccessDenied code, got {:?}",
                refused.text()
            );
            assert!(
                !refused
                    .body
                    .as_ref()
                    .windows(payload.len())
                    .any(|at| at == payload),
                "a refused read must carry none of the object's bytes, got {:?}",
                refused.text()
            );

            // The one call that changes the decision: the key policy now reaches the role
            // the guest session is a session of, and the very next read is served.
            let granted = rpc(
                &router,
                &runtime,
                as_setup("kms"),
                "TrentService.PutKeyPolicy",
                &json!({
                    "KeyId": &key_id,
                    "Policy": key_policy(&[&authenticated, &guest_role]),
                    "BypassPolicyLockoutSafetyCheck": true,
                }),
            );
            assert_eq!(
                granted.status,
                200,
                "setup: PutKeyPolicy must succeed, got {} with {:?}",
                granted.status,
                granted.text()
            );

            let served = vended_get_object(&router, &runtime, &guest_key_id, bucket, key);
            assert_eq!(
                served.status,
                200,
                "the read must be served once the key policy reaches the session's role, got \
                 {} with {:?}",
                served.status,
                served.text()
            );
            assert_eq!(
                served.body.as_ref(),
                payload.as_slice(),
                "the served read must answer the stored bytes, got {:?}",
                served.text()
            );

            // And the same statement over the whole class the two legs above are rows of:
            // which of the pool's roles a session speaks for is what the key policy decides
            // the read by, so a session of the pool's OTHER exchange is admitted by the
            // grant of ITS role and by no other. The same router, pool, key and object
            // serve every row; what each row varies is the key policy and which session
            // reads ([`RESOLVED_ROLE_ROWS`]).
            let authenticated_key_id =
                vend_session(&router, &runtime, &pool_id, Flow::Authenticated);
            for row in RESOLVED_ROLE_ROWS {
                let granted: Vec<&str> = row
                    .grants_the_role_of
                    .iter()
                    .map(|flow| match flow {
                        Flow::Guest => guest_role.as_str(),
                        Flow::Authenticated => authenticated.as_str(),
                    })
                    .collect();
                let policy = rpc(
                    &router,
                    &runtime,
                    as_setup("kms"),
                    "TrentService.PutKeyPolicy",
                    &json!({
                        "KeyId": &key_id,
                        "Policy": key_policy(&granted),
                        "BypassPolicyLockoutSafetyCheck": true,
                    }),
                );
                assert_eq!(
                    policy.status,
                    200,
                    "case {:?}: setup PutKeyPolicy must succeed, got {} with {:?}",
                    row.case,
                    policy.status,
                    policy.text()
                );

                let access_key_id = match row.flow {
                    Flow::Guest => guest_key_id.as_str(),
                    Flow::Authenticated => authenticated_key_id.as_str(),
                };
                let read = vended_get_object(&router, &runtime, access_key_id, bucket, key);
                if row.served {
                    assert_eq!(
                        read.status,
                        200,
                        "case {:?}: the read must be served, got {} with {:?}",
                        row.case,
                        read.status,
                        read.text()
                    );
                    assert_eq!(
                        read.body.as_ref(),
                        payload.as_slice(),
                        "case {:?}: the served read must answer the stored bytes, got {:?}",
                        row.case,
                        read.text()
                    );
                } else {
                    assert_eq!(
                        read.status,
                        403,
                        "case {:?}: the read must be refused with HTTP 403, got {} with {:?}",
                        row.case,
                        read.status,
                        read.text()
                    );
                    assert_eq!(
                        element(&read.text(), "Code").as_deref(),
                        Some("AccessDenied"),
                        "case {:?}: the refusal must carry S3's AccessDenied code, got {:?}",
                        row.case,
                        read.text()
                    );
                    assert!(
                        !read
                            .body
                            .as_ref()
                            .windows(payload.len())
                            .any(|at| at == payload),
                        "case {:?}: a refused read must carry none of the object's bytes, got \
                         {:?}",
                        row.case,
                        read.text()
                    );
                }
            }
        })
        .expect("spawn the vended-session refusal thread")
        .join()
        .expect("a vended session the key policy does not reach must be refused the plaintext");
}

/// One row of the table
/// [`a_vended_session_the_key_policy_does_not_reach_cannot_read_the_object`] closes with:
/// which exchange the reading session came of, which of the pool's two roles the key policy
/// grants `kms:Decrypt` to, and whether the read is then served.
struct ResolvedRoleRow {
    case: &'static str,
    /// The exchange the reading session was vended by, which is what the pool resolves its
    /// role from.
    flow: Flow,
    /// The roles the key policy names, each given as the flow whose role it is.
    grants_the_role_of: &'static [Flow],
    /// Whether the read must then be served the object's bytes.
    served: bool,
}

/// The table: both exchanges a pool vends for, against every subset of its two roles the key
/// policy can name. A read is served exactly when the policy reaches the role the POOL
/// resolved for that session's own exchange — neither more (the other role's grant opens
/// nothing for it) nor less (a grant of its own role opens it whichever other roles are
/// named beside it).
const RESOLVED_ROLE_ROWS: &[ResolvedRoleRow] = &[
    ResolvedRoleRow {
        case: "neither role granted, the guest session reads",
        flow: Flow::Guest,
        grants_the_role_of: &[],
        served: false,
    },
    ResolvedRoleRow {
        case: "neither role granted, the authenticated session reads",
        flow: Flow::Authenticated,
        grants_the_role_of: &[],
        served: false,
    },
    ResolvedRoleRow {
        case: "the authenticated role alone granted, the guest session reads",
        flow: Flow::Guest,
        grants_the_role_of: &[Flow::Authenticated],
        served: false,
    },
    ResolvedRoleRow {
        case: "the authenticated role alone granted, the authenticated session reads",
        flow: Flow::Authenticated,
        grants_the_role_of: &[Flow::Authenticated],
        served: true,
    },
    ResolvedRoleRow {
        case: "the guest role alone granted, the guest session reads",
        flow: Flow::Guest,
        grants_the_role_of: &[Flow::Guest],
        served: true,
    },
    ResolvedRoleRow {
        case: "the guest role alone granted, the authenticated session reads",
        flow: Flow::Authenticated,
        grants_the_role_of: &[Flow::Guest],
        served: false,
    },
    ResolvedRoleRow {
        case: "both roles granted, the guest session reads",
        flow: Flow::Guest,
        grants_the_role_of: &[Flow::Guest, Flow::Authenticated],
        served: true,
    },
    ResolvedRoleRow {
        case: "both roles granted, the authenticated session reads",
        flow: Flow::Authenticated,
        grants_the_role_of: &[Flow::Guest, Flow::Authenticated],
        served: true,
    },
];

// The table above is driven by
// [`a_vended_session_the_key_policy_does_not_reach_cannot_read_the_object`], on the router,
// pool, key and object that suite already stands up.
