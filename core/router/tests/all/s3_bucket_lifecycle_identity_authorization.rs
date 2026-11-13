// SPDX-License-Identifier: CC0-1.0

//! A bucket's own LIFECYCLE is authorized against the calling principal's identity-based
//! policies: a role session may create and delete a bucket exactly when the documents in force
//! on it allow the actions Amazon S3 documents for those calls.
//!
//! The rule under test: **S3 judges a signed bucket-lifecycle request against the
//! identity-based policy documents in force on the principal it speaks for, under the
//! permissions that operation documents — `s3:CreateBucket` for a create, plus one further
//! permission for each header the create carries that configures the bucket
//! (`s3:PutBucketAcl` for an ACL beyond the owner's own, `s3:PutBucketVersioning` and
//! `s3:PutBucketObjectLockConfiguration` for Object Lock, `s3:PutBucketOwnershipControls` for
//! an Object Ownership header), and `s3:DeleteBucket` for a delete — and a request no document
//! allows, or one an explicit `Deny` refuses, is answered HTTP 403 `AccessDenied` with the
//! bucket's existence left exactly as it was.**
//!
//! Three crates have to agree for that to hold — IAM holds the documents, STS mints the
//! session and S3 decides the call — so this is the suite that can pin it: a crate-local suite
//! can only ever see one side.
//!
//! The rows differ along what the rule generalises over: WHICH actions the session's policy
//! grants (the create permission alone, a wildcard, an unrelated action, an explicit deny
//! beside a grant), WHICH resource the grant names (everything, this bucket, another bucket),
//! whether the grant is scoped by a `Condition` over the ACL the create names, and WHICH
//! headers the create carries, since each one adds a permission of its own.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder every awsQuery suite of this crate shares, and the reader that pulls one
// element's text out of an XML answer: written ONCE in `query_wire`.
use crate::query_wire::{element, form_encode};

/// The region every request here is signed for, and the one a create naming no location
/// constraint places its bucket in.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries, which owns every bucket it holds.
const ACCOUNT_ID: &str = "000000000000";

/// The long-term key every emulator publishes, which is the account's own credential: what the
/// roles are created with and what the sessions are assumed from.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The three services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and whose
/// credential is `access_key_id` — the key a service reads the caller's identity from. The
/// signature is never checked: request authentication is off.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Post one awsQuery form body to `signing_name`, presenting `access_key_id`.
async fn query(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    body: String,
) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(access_key_id, signing_name))
        .body(Bytes::from(body))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One IAM call over its awsQuery wire, as the account's own credential.
async fn iam(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    query(router, AMBIENT_KEY, "iam", body).await
}

/// One S3 request, presented as signed by `access_key_id`, carrying `headers` beside the ones
/// every request here carries.
async fn s3(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
) -> (u16, String) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization(access_key_id, "s3"));
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::new())
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The trust policy a role this suite creates carries: the account may assume it, named by the
/// account's id, which is how a deploy role's trust is written.
fn trust_policy() -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"{ACCOUNT_ID}"}},"Action":"sts:AssumeRole"}}]}}"#
    )
}

/// An identity-based policy allowing `allowed` on `resource`, under `condition` where a row
/// names one, and — where `denied` names any — explicitly denying those on the same resource.
fn identity_policy(
    allowed: &[&str],
    resource: &str,
    denied: &[&str],
    condition: Option<&str>,
) -> String {
    let list = |actions: &[&str]| {
        let quoted: Vec<String> = actions.iter().map(|action| format!("\"{action}\"")).collect();
        quoted.join(",")
    };
    let clause = condition.map_or(String::new(), |condition| format!(r#","Condition":{condition}"#));
    let mut statements = vec![format!(
        r#"{{"Effect":"Allow","Action":[{}],"Resource":"{resource}"{clause}}}"#,
        list(allowed)
    )];
    if !denied.is_empty() {
        statements.push(format!(
            r#"{{"Effect":"Deny","Action":[{}],"Resource":"{resource}"}}"#,
            list(denied)
        ));
    }
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{}]}}"#,
        statements.join(",")
    )
}

/// Create `role_name` holding `policy`, assume it, and answer the access key id of the session
/// STS minted for it.
async fn session_of(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
    policy: &str,
) -> String {
    let (status, body) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", &trust_policy()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole must succeed, got {status}: {body}"
    );
    let (status, body) = iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", role_name),
            ("PolicyName", "lifecycle"),
            ("PolicyDocument", policy),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRolePolicy must succeed, got {status}: {body}"
    );
    let (status, body) = query(
        router,
        AMBIENT_KEY,
        "sts",
        format!(
            "Action=AssumeRole&Version=2011-06-15&RoleArn={}&RoleSessionName=lifecycle",
            form_encode(&format!("arn:aws:iam::{ACCOUNT_ID}:role/{role_name}"))
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: AssumeRole must succeed, got {status}: {body}"
    );
    element(&body, "AccessKeyId")
}

/// Whether the ACCOUNT's own credential — which no identity-based policy bounds — finds a
/// bucket of `bucket`'s name, read through `HeadBucket` so no row's own credentials decide it.
async fn bucket_exists(router: &MultiServiceRouter, bucket: &str) -> bool {
    let (status, body) = s3(router, AMBIENT_KEY, "HEAD", &format!("/{bucket}"), &[]).await;
    assert!(
        status == 200 || status == 404,
        "HeadBucket of {bucket:?} must answer 200 or 404, got {status}: {body}"
    );
    status == 200
}

/// One row of the create table.
struct CreateRow {
    case: &'static str,
    /// The actions the session's own policy allows.
    allowed: &'static [&'static str],
    /// Whether that grant names THIS bucket's ARN rather than every resource; a row that names
    /// another bucket's ARN is [`Resource::Other`].
    resource: Resource,
    /// The actions the same policy explicitly denies.
    denied: &'static [&'static str],
    /// The `Condition` block the grant carries, where the row scopes it by one.
    condition: Option<&'static str>,
    /// The headers the create carries beside the bucket's name.
    headers: &'static [(&'static str, &'static str)],
    /// Whether S3 must serve the create.
    served: bool,
}

/// Which resource a row's grant names.
#[derive(Debug, Clone, Copy)]
enum Resource {
    /// Every resource — the `*` an unscoped grant names.
    Everything,
    /// The bucket the row creates.
    ThisBucket,
    /// A bucket that is not the one the row creates.
    AnotherBucket,
}

/// The create permission, which every create needs.
const CREATE: &str = "s3:CreateBucket";
/// The further permission an ACL beyond the owner's own needs.
const PUT_ACL: &str = "s3:PutBucketAcl";
/// The further permission an Object Ownership header needs.
const PUT_OWNERSHIP: &str = "s3:PutBucketOwnershipControls";
/// One of the two further permissions enabling Object Lock needs.
const PUT_VERSIONING: &str = "s3:PutBucketVersioning";
/// The other one.
const PUT_OBJECT_LOCK: &str = "s3:PutBucketObjectLockConfiguration";

/// The rows the create half of the rule is asserted over.
const CREATE_ROWS: &[CreateRow] = &[
    CreateRow {
        case: "the create permission on every resource",
        allowed: &[CREATE],
        resource: Resource::Everything,
        denied: &[],
        condition: None,
        headers: &[],
        served: true,
    },
    CreateRow {
        case: "the create permission on this bucket's own ARN",
        allowed: &[CREATE],
        resource: Resource::ThisBucket,
        denied: &[],
        condition: None,
        headers: &[],
        served: true,
    },
    CreateRow {
        case: "the create permission on another bucket's ARN",
        allowed: &[CREATE],
        resource: Resource::AnotherBucket,
        denied: &[],
        condition: None,
        headers: &[],
        served: false,
    },
    CreateRow {
        case: "every S3 action on every resource",
        allowed: &["s3:*"],
        resource: Resource::Everything,
        denied: &[],
        condition: None,
        headers: &[],
        served: true,
    },
    CreateRow {
        case: "an unrelated S3 action alone",
        allowed: &["s3:PutObject"],
        resource: Resource::Everything,
        denied: &[],
        condition: None,
        headers: &[],
        served: false,
    },
    CreateRow {
        case: "an explicit deny of the create beside a wildcard grant",
        allowed: &["s3:*"],
        resource: Resource::Everything,
        denied: &[CREATE],
        condition: None,
        headers: &[],
        served: false,
    },
    CreateRow {
        case: "the private canned ACL, which needs no further permission",
        allowed: &[CREATE],
        resource: Resource::Everything,
        denied: &[],
        condition: None,
        headers: &[("x-amz-acl", "private")],
        served: true,
    },
    CreateRow {
        case: "a public-read ACL without the ACL permission",
        allowed: &[CREATE, PUT_OWNERSHIP],
        resource: Resource::Everything,
        denied: &[],
        condition: None,
        headers: &[
            ("x-amz-acl", "public-read"),
            ("x-amz-object-ownership", "ObjectWriter"),
        ],
        served: false,
    },
    CreateRow {
        case: "a public-read ACL with the ACL permission",
        allowed: &[CREATE, PUT_OWNERSHIP, PUT_ACL],
        resource: Resource::Everything,
        denied: &[],
        condition: None,
        headers: &[
            ("x-amz-acl", "public-read"),
            ("x-amz-object-ownership", "ObjectWriter"),
        ],
        served: true,
    },
    CreateRow {
        case: "an Object Ownership header without its permission",
        allowed: &[CREATE],
        resource: Resource::Everything,
        denied: &[],
        condition: None,
        headers: &[("x-amz-object-ownership", "BucketOwnerPreferred")],
        served: false,
    },
    CreateRow {
        case: "an Object Ownership header with its permission",
        allowed: &[CREATE, PUT_OWNERSHIP],
        resource: Resource::Everything,
        denied: &[],
        condition: None,
        headers: &[("x-amz-object-ownership", "BucketOwnerPreferred")],
        served: true,
    },
    CreateRow {
        case: "Object Lock enabled with neither of its permissions",
        allowed: &[CREATE],
        resource: Resource::Everything,
        denied: &[],
        condition: None,
        headers: &[("x-amz-bucket-object-lock-enabled", "true")],
        served: false,
    },
    CreateRow {
        case: "Object Lock enabled with one of its two permissions",
        allowed: &[CREATE, PUT_VERSIONING],
        resource: Resource::Everything,
        denied: &[],
        condition: None,
        headers: &[("x-amz-bucket-object-lock-enabled", "true")],
        served: false,
    },
    CreateRow {
        case: "Object Lock enabled with both of its permissions",
        allowed: &[CREATE, PUT_VERSIONING, PUT_OBJECT_LOCK],
        resource: Resource::Everything,
        denied: &[],
        condition: None,
        headers: &[("x-amz-bucket-object-lock-enabled", "true")],
        served: true,
    },
    CreateRow {
        // `s3:x-amz-acl` is a published S3 request-header condition key, so a grant scoped
        // by the ACL the create names is decided by the ACL the create names.
        case: "a grant scoped to the ACL the create names",
        allowed: &[CREATE, PUT_OWNERSHIP, PUT_ACL],
        resource: Resource::Everything,
        denied: &[],
        condition: Some(r#"{"StringEquals":{"s3:x-amz-acl":"public-read"}}"#),
        headers: &[
            ("x-amz-acl", "public-read"),
            ("x-amz-object-ownership", "ObjectWriter"),
        ],
        served: true,
    },
    CreateRow {
        case: "a grant scoped to another ACL than the create names",
        allowed: &[CREATE, PUT_OWNERSHIP, PUT_ACL],
        resource: Resource::Everything,
        denied: &[],
        condition: Some(r#"{"StringEquals":{"s3:x-amz-acl":"authenticated-read"}}"#),
        headers: &[
            ("x-amz-acl", "public-read"),
            ("x-amz-object-ownership", "ObjectWriter"),
        ],
        served: false,
    },
];

/// Assert `body` carries the `AccessDenied` code S3 answers a refused call with.
fn assert_access_denied(case: &str, body: &str) {
    assert!(
        body.contains("<Code>AccessDenied</Code>"),
        "case {case:?}: expected the AccessDenied code, got {body}"
    );
}

/// RULE: a create is served exactly when the session's identity-based documents allow every
/// permission the create needs, and a refused one brings no bucket into being.
#[tokio::test]
async fn a_bucket_create_needs_the_permissions_its_headers_ask_for() {
    let router = build_router();
    for (index, row) in CREATE_ROWS.iter().enumerate() {
        let bucket = format!("lifecycle-create-{index}-{ACCOUNT_ID}");
        let resource = match row.resource {
            Resource::Everything => "*".to_string(),
            Resource::ThisBucket => format!("arn:aws:s3:::{bucket}"),
            Resource::AnotherBucket => format!("arn:aws:s3:::lifecycle-other-{index}"),
        };
        let policy = identity_policy(row.allowed, &resource, row.denied, row.condition);
        let session = session_of(&router, row.case, &format!("Creator{index}"), &policy).await;

        let (status, body) = s3(&router, &session, "PUT", &format!("/{bucket}"), row.headers).await;
        let expected = if row.served { 200 } else { 403 };
        assert_eq!(
            status, expected,
            "case {:?}: expected HTTP {expected} for the create, got {status}: {body}",
            row.case
        );
        if !row.served {
            assert_access_denied(row.case, &body);
        }
        let exists = bucket_exists(&router, &bucket).await;
        assert_eq!(
            exists, row.served,
            "case {:?}: expected the bucket to exist? {}, got {exists}",
            row.case, row.served
        );
    }
}

/// One row of the delete table.
struct DeleteRow {
    case: &'static str,
    allowed: &'static [&'static str],
    resource: Resource,
    denied: &'static [&'static str],
    served: bool,
}

/// The delete permission.
const DELETE: &str = "s3:DeleteBucket";

/// The rows the delete half of the rule is asserted over: which actions the session holds, and
/// which resource they are scoped to.
const DELETE_ROWS: &[DeleteRow] = &[
    DeleteRow {
        case: "the delete permission on every resource",
        allowed: &[DELETE],
        resource: Resource::Everything,
        denied: &[],
        served: true,
    },
    DeleteRow {
        case: "the delete permission on this bucket's own ARN",
        allowed: &[DELETE],
        resource: Resource::ThisBucket,
        denied: &[],
        served: true,
    },
    DeleteRow {
        case: "the delete permission on another bucket's ARN",
        allowed: &[DELETE],
        resource: Resource::AnotherBucket,
        denied: &[],
        served: false,
    },
    DeleteRow {
        case: "the create permission but not the delete one",
        allowed: &[CREATE],
        resource: Resource::Everything,
        denied: &[],
        served: false,
    },
    DeleteRow {
        case: "an explicit deny of the delete beside a wildcard grant",
        allowed: &["s3:*"],
        resource: Resource::Everything,
        denied: &[DELETE],
        served: false,
    },
];

/// RULE: a delete is served exactly when the session's identity-based documents allow the
/// delete permission on the bucket addressed, and a refused one leaves the bucket in place.
#[tokio::test]
async fn a_bucket_delete_needs_the_delete_permission_on_that_bucket() {
    let router = build_router();
    for (index, row) in DELETE_ROWS.iter().enumerate() {
        let bucket = format!("lifecycle-delete-{index}-{ACCOUNT_ID}");
        // The bucket every row deletes is created by the ACCOUNT's own credential, which no
        // identity-based policy bounds, so the row decides only the SESSION's access.
        let (status, body) = s3(&router, AMBIENT_KEY, "PUT", &format!("/{bucket}"), &[]).await;
        assert_eq!(
            status, 200,
            "case {:?}: the account's own credential must create the bucket, got {status}: \
             {body}",
            row.case
        );

        let resource = match row.resource {
            Resource::Everything => "*".to_string(),
            Resource::ThisBucket => format!("arn:aws:s3:::{bucket}"),
            Resource::AnotherBucket => format!("arn:aws:s3:::lifecycle-other-{index}"),
        };
        let policy = identity_policy(row.allowed, &resource, row.denied, None);
        let session = session_of(&router, row.case, &format!("Remover{index}"), &policy).await;

        let (status, body) = s3(&router, &session, "DELETE", &format!("/{bucket}"), &[]).await;
        let expected = if row.served { 204 } else { 403 };
        assert_eq!(
            status, expected,
            "case {:?}: expected HTTP {expected} for the delete, got {status}: {body}",
            row.case
        );
        if !row.served {
            assert_access_denied(row.case, &body);
        }
        let exists = bucket_exists(&router, &bucket).await;
        assert_eq!(
            exists, !row.served,
            "case {:?}: expected the bucket to still exist? {}, got {exists}",
            row.case, !row.served
        );

        // Whatever a bounded session was refused, the ACCOUNT's own credential still ends the
        // bucket's life: the gate bounds the principals that hold identity-based documents,
        // not the account itself.
        if !row.served {
            let (status, body) =
                s3(&router, AMBIENT_KEY, "DELETE", &format!("/{bucket}"), &[]).await;
            assert_eq!(
                status, 204,
                "case {:?}: the account's own credential must delete the bucket, got {status}: \
                 {body}",
                row.case
            );
        }
    }
}
