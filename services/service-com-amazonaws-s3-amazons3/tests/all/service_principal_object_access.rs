// SPDX-License-Identifier: CC0-1.0

//! What authorizes a request ANOTHER Amazon Web Services service makes to S3 as its own
//! SERVICE PRINCIPAL, on behalf of one of its resources — the position a CloudFront origin
//! fetch under an origin access control is in.
//!
//! THE RULE: such a call is authorized as that service principal, which holds no
//! identity-based policies, so the addressed bucket's OWN policy is the only document in
//! force: the call is served only where a statement of that policy names the principal, covers
//! the action and holds under the `aws:SourceArn` / `aws:SourceAccount` keys the call carries,
//! and is refused HTTP 403 `AccessDenied` (`Access Denied`) otherwise. Because the action a
//! read is judged under follows what the address RESOLVED to — "if the object that you request
//! doesn't exist, the error that Amazon S3 returns depends on whether you also have the
//! `s3:ListBucket` permission. If you have the `s3:ListBucket` permission on the bucket,
//! Amazon S3 returns an HTTP status code 404 Not Found error. If you don't have the
//! `s3:ListBucket` permission, Amazon S3 returns an HTTP status code 403 Access Denied error"
//! (`GetObject`'s own documentation) — a key such a caller's policy grants only the read of is
//! answered 403 rather than `NoSuchKey`, so object existence is not disclosed to a principal
//! that may not list the bucket.
//!
//! WHICH requests are in that class is not a client's choice: the calling service mints a
//! credential for the one call and records, under its access key id, the principal it is
//! calling as and the resource it is calling for
//! (`service_common::service_principal_calls`) — the same directory CloudFront's origin-access
//! path writes. A request presenting any other credential is decided as before, which the last
//! test pins.
//!
//! The table's rows differ along what the rule generalises over: whether the ADDRESS resolves
//! to an object (a key the bucket holds, a prefix-shaped path with no object behind it, a plain
//! absent key), which ACTIONS the policy grants (the read alone, the read and the listing, the
//! listing alone, every S3 action, none at all), the PRINCIPAL the grant names (this service
//! principal, another one, the wildcard), the CONDITION it is scoped by (this resource's ARN,
//! another resource's, the console's own key casing, no condition at all), the EFFECT (a grant,
//! and a deny beside it), and the METHOD the call makes (`GET`, `HEAD`, `PUT`).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{put_bucket_policy, router_with_engine, send_bytes_signed, Answer};
use super::object_and_tagging_roundtrip::{bucket, put_object, text};

/// The Region every request below is signed for, and the endpoint host it is addressed to.
const REGION: &str = "us-east-1";

/// See [`REGION`]: the Regional endpoint, which carries no bucket, so every request is
/// path-style.
const ENDPOINT_HOST: &str = "s3.us-east-1.amazonaws.com";

/// The access key id the calling service's own request presents — the credential it minted
/// for that one call and recorded itself under.
const SERVICE_KEY_ID: &str = "EMULATORAKSERVICEPRINCIPAL";

/// An access key id no record was written for, which neither STS nor IAM holds either: it
/// speaks for the account itself, the position the emulator's own published credential is in.
const ACCOUNT_KEY_ID: &str = "AKIDEXAMPLE";

/// The service principal the calls below are made as: the one a bucket policy written for a
/// distribution's origin access control grants the read to.
const SERVICE_PRINCIPAL: &str = "cloudfront.amazonaws.com";

/// Another service principal, which no call below is made as.
const OTHER_PRINCIPAL: &str = "logs.amazonaws.com";

/// The resource the calls below are made ON BEHALF OF: the value of `aws:SourceArn`.
fn source_arn() -> String {
    format!(
        "arn:aws:cloudfront::{}:distribution/E1SERVICEPRINCIPAL",
        engine::emulator_account_id()
    )
}

/// Another resource of the same kind, which no call below is made for.
fn other_source_arn() -> String {
    format!(
        "arn:aws:cloudfront::{}:distribution/E2SOMEONEELSEXX",
        engine::emulator_account_id()
    )
}

/// The bytes the object every row reads holds. A policy is about who may reach an object,
/// not about what is in it, so one payload serves every row.
const BODY: &[u8] = b"tenant-index-payload";

/// The key the setup WRITES, so a read of it resolves an object.
const HELD_KEY: &str = "tenants/acme/index.html";

/// A PREFIX-shaped path under which objects live but which holds no object itself — what a
/// viewer asking for a "directory" of a distribution with no default root object addresses.
const PREFIX_KEY: &str = "tenants/acme/";

/// A plain key the setup never writes.
const ABSENT_KEY: &str = "tenants/acme/missing.json";

/// The code and message S3 refuses an unauthorized request with (Amazon S3 "Error responses":
/// `AccessDenied`, HTTP 403, "Access Denied").
const ACCESS_DENIED: &str = "AccessDenied";
/// See [`ACCESS_DENIED`].
const ACCESS_DENIED_MESSAGE: &str = "Access Denied";

/// The code and message a read of a key the bucket holds nothing at answers with
/// (`NoSuchKey`: "The specified key does not exist.", HTTP 404).
const NO_SUCH_KEY: &str = "NoSuchKey";
/// See [`NO_SUCH_KEY`].
const NO_SUCH_KEY_MESSAGE: &str = "The specified key does not exist.";

/// The `Authorization` header a request signed with `access_key_id` carries: the credential
/// scope names the Region of the endpoint it is addressed to, and this harness verifies no
/// signature (see [`super::bucket_lifecycle`]).
fn authorization(access_key_id: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/s3/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// Dispatch one object request signed with `access_key_id`. A write carries [`BODY`]; a read
/// and a HEAD carry no payload at all, the way a client sends them.
async fn call(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    bucket: &str,
    key: &str,
) -> Answer {
    let body: &[u8] = match method {
        "PUT" => BODY,
        _ => b"",
    };
    send_bytes_signed(
        router,
        &authorization(access_key_id),
        method,
        ENDPOINT_HOST,
        &format!("/{bucket}/{key}"),
        &[],
        body,
    )
    .await
}

/// One bucket-policy statement: the effect, the `Principal` element as JSON, the action, the
/// resource ARN and the `Condition` clause (when any).
fn statement(
    effect: &str,
    principal: &str,
    action: &str,
    resource: &str,
    condition: Option<&str>,
) -> String {
    let condition = match condition {
        Some(clause) => format!(",\"Condition\":{clause}"),
        None => String::new(),
    };
    format!(
        "{{\"Effect\":\"{effect}\",\"Principal\":{principal},\"Action\":\"{action}\",\
         \"Resource\":\"{resource}\"{condition}}}"
    )
}

/// A policy document carrying `statements`.
fn policy(statements: &[String]) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{}]}}",
        statements.join(",")
    )
}

/// The `Principal` element naming one service principal.
fn service(principal: &str) -> String {
    format!("{{\"Service\":\"{principal}\"}}")
}

/// A `StringEquals` clause over one condition key, so a row can state the key the way the AWS
/// console writes it and can scope a grant by the resource or by its account.
fn string_equals_clause(key: &str, value: &str) -> String {
    format!("{{\"StringEquals\":{{\"{key}\":\"{value}\"}}}}")
}

/// The policy a distribution's origin access control is published with: the read of every key
/// of the bucket, granted to [`SERVICE_PRINCIPAL`] and scoped to the one resource the call is
/// made for.
fn read_grant(bucket: &str) -> String {
    policy(&[statement(
        "Allow",
        &service(SERVICE_PRINCIPAL),
        "s3:GetObject",
        &format!("arn:aws:s3:::{bucket}/*"),
        Some(&string_equals_clause("aws:SourceArn", &source_arn())),
    )])
}

/// What one request must answer: the status, and the error document's code and message (both
/// empty for a request that is served).
struct Expected {
    status: u16,
    code: &'static str,
    message: &'static str,
}

/// One row: a bucket carrying one policy, the request the service principal then makes of it,
/// and what that request must answer.
struct Row {
    case: &'static str,
    /// This row's own bucket, so one row's policy cannot authorize another's request.
    bucket: &'static str,
    /// The document the bucket carries, or `None` for a bucket carrying none.
    policy: Option<String>,
    method: &'static str,
    key: &'static str,
    expected: Expected,
}

/// The served answer: no error document at all.
const SERVED: Expected = Expected {
    status: 200,
    code: "",
    message: "",
};

/// The refusal every unauthorized row answers with.
const REFUSED: Expected = Expected {
    status: 403,
    code: ACCESS_DENIED,
    message: ACCESS_DENIED_MESSAGE,
};

/// The answer a row whose policy lets the caller list the bucket gets for a key it holds
/// nothing at.
const ABSENT: Expected = Expected {
    status: 404,
    code: NO_SUCH_KEY,
    message: NO_SUCH_KEY_MESSAGE,
};

fn rows() -> Vec<Row> {
    let listing_grant = |bucket: &str| {
        policy(&[
            statement(
                "Allow",
                &service(SERVICE_PRINCIPAL),
                "s3:GetObject",
                &format!("arn:aws:s3:::{bucket}/*"),
                Some(&string_equals_clause("aws:SourceArn", &source_arn())),
            ),
            statement(
                "Allow",
                &service(SERVICE_PRINCIPAL),
                "s3:ListBucket",
                &format!("arn:aws:s3:::{bucket}"),
                Some(&string_equals_clause("aws:SourceArn", &source_arn())),
            ),
        ])
    };
    vec![
        Row {
            case: "the read grant alone: a key the bucket holds is served",
            bucket: "sp-read-grant-held",
            policy: Some(read_grant("sp-read-grant-held")),
            method: "GET",
            key: HELD_KEY,
            expected: SERVED,
        },
        Row {
            case: "the read grant alone: a prefix with no object behind it is refused",
            bucket: "sp-read-grant-prefix",
            policy: Some(read_grant("sp-read-grant-prefix")),
            method: "GET",
            key: PREFIX_KEY,
            expected: REFUSED,
        },
        Row {
            case: "the read grant alone: a plain absent key is refused",
            bucket: "sp-read-grant-absent",
            policy: Some(read_grant("sp-read-grant-absent")),
            method: "GET",
            key: ABSENT_KEY,
            expected: REFUSED,
        },
        Row {
            case: "the read grant alone: a HEAD of an absent key is refused",
            bucket: "sp-read-grant-head-absent",
            policy: Some(read_grant("sp-read-grant-head-absent")),
            method: "HEAD",
            key: ABSENT_KEY,
            expected: Expected {
                status: 403,
                // A HEAD carries no body, so the status is its whole answer.
                code: "",
                message: "",
            },
        },
        Row {
            case: "the read grant alone: a HEAD of the held key is served",
            bucket: "sp-read-grant-head-held",
            policy: Some(read_grant("sp-read-grant-head-held")),
            method: "HEAD",
            key: HELD_KEY,
            expected: Expected {
                status: 200,
                code: "",
                message: "",
            },
        },
        Row {
            case: "the read and the listing: an absent key is reported absent",
            bucket: "sp-listing-grant-absent",
            policy: Some(listing_grant("sp-listing-grant-absent")),
            method: "GET",
            key: ABSENT_KEY,
            expected: ABSENT,
        },
        Row {
            case: "the read and the listing: the prefix with no object behind it is absent",
            bucket: "sp-listing-grant-prefix",
            policy: Some(listing_grant("sp-listing-grant-prefix")),
            method: "GET",
            key: PREFIX_KEY,
            expected: ABSENT,
        },
        Row {
            case: "the read and the listing: the held key is still served",
            bucket: "sp-listing-grant-held",
            policy: Some(listing_grant("sp-listing-grant-held")),
            method: "GET",
            key: HELD_KEY,
            expected: SERVED,
        },
        Row {
            case: "the listing alone: the held key is refused, since the read is not granted",
            bucket: "sp-listing-only",
            policy: Some(policy(&[statement(
                "Allow",
                &service(SERVICE_PRINCIPAL),
                "s3:ListBucket",
                "arn:aws:s3:::sp-listing-only",
                Some(&string_equals_clause("aws:SourceArn", &source_arn())),
            )])),
            method: "GET",
            key: HELD_KEY,
            expected: REFUSED,
        },
        Row {
            case: "every S3 action granted: an absent key is reported absent",
            bucket: "sp-every-action-absent",
            policy: Some(policy(&[
                statement(
                    "Allow",
                    &service(SERVICE_PRINCIPAL),
                    "s3:*",
                    "arn:aws:s3:::sp-every-action-absent",
                    None,
                ),
                statement(
                    "Allow",
                    &service(SERVICE_PRINCIPAL),
                    "s3:*",
                    "arn:aws:s3:::sp-every-action-absent/*",
                    None,
                ),
            ])),
            method: "GET",
            key: ABSENT_KEY,
            expected: ABSENT,
        },
        Row {
            case: "the grant is scoped to ANOTHER resource, so the held key is refused",
            bucket: "sp-other-source-arn",
            policy: Some(policy(&[statement(
                "Allow",
                &service(SERVICE_PRINCIPAL),
                "s3:GetObject",
                "arn:aws:s3:::sp-other-source-arn/*",
                Some(&string_equals_clause("aws:SourceArn", &other_source_arn())),
            )])),
            method: "GET",
            key: HELD_KEY,
            expected: REFUSED,
        },
        Row {
            case: "the key spelled the way the console writes it still scopes the grant",
            bucket: "sp-console-key-casing",
            policy: Some(policy(&[statement(
                "Allow",
                &service(SERVICE_PRINCIPAL),
                "s3:GetObject",
                "arn:aws:s3:::sp-console-key-casing/*",
                Some(&string_equals_clause("AWS:SourceArn", &source_arn())),
            )])),
            method: "GET",
            key: HELD_KEY,
            expected: SERVED,
        },
        Row {
            case: "a grant scoped by the resource's ACCOUNT rather than its ARN",
            bucket: "sp-source-account",
            policy: Some(policy(&[statement(
                "Allow",
                &service(SERVICE_PRINCIPAL),
                "s3:GetObject",
                "arn:aws:s3:::sp-source-account/*",
                Some(&string_equals_clause(
                    "aws:SourceAccount",
                    engine::emulator_account_id(),
                )),
            )])),
            method: "GET",
            key: HELD_KEY,
            expected: SERVED,
        },
        Row {
            case: "a grant to ANOTHER service principal",
            bucket: "sp-other-principal",
            policy: Some(policy(&[statement(
                "Allow",
                &service(OTHER_PRINCIPAL),
                "s3:GetObject",
                "arn:aws:s3:::sp-other-principal/*",
                None,
            )])),
            method: "GET",
            key: HELD_KEY,
            expected: REFUSED,
        },
        Row {
            case: "a grant to the wildcard, scoped to this resource, reaches this principal too",
            bucket: "sp-wildcard-principal",
            policy: Some(policy(&[statement(
                "Allow",
                "\"*\"",
                "s3:GetObject",
                "arn:aws:s3:::sp-wildcard-principal/*",
                Some(&string_equals_clause("aws:SourceArn", &source_arn())),
            )])),
            method: "GET",
            key: HELD_KEY,
            expected: SERVED,
        },
        Row {
            case: "a deny of the read beside the grant",
            bucket: "sp-deny-beside-grant",
            policy: Some(policy(&[
                statement(
                    "Allow",
                    &service(SERVICE_PRINCIPAL),
                    "s3:GetObject",
                    "arn:aws:s3:::sp-deny-beside-grant/*",
                    None,
                ),
                statement(
                    "Deny",
                    &service(SERVICE_PRINCIPAL),
                    "s3:GetObject",
                    "arn:aws:s3:::sp-deny-beside-grant/*",
                    None,
                ),
            ])),
            method: "GET",
            key: HELD_KEY,
            expected: REFUSED,
        },
        Row {
            case: "the bucket carries no policy at all, so it has granted nothing",
            bucket: "sp-no-policy",
            policy: None,
            method: "GET",
            key: HELD_KEY,
            expected: REFUSED,
        },
        Row {
            case: "a write the read grant does not name",
            bucket: "sp-write-not-granted",
            policy: Some(read_grant("sp-write-not-granted")),
            method: "PUT",
            key: ABSENT_KEY,
            expected: REFUSED,
        },
        Row {
            case: "a write every S3 action is granted for",
            bucket: "sp-write-granted",
            policy: Some(policy(&[statement(
                "Allow",
                &service(SERVICE_PRINCIPAL),
                "s3:*",
                "arn:aws:s3:::sp-write-granted/*",
                None,
            )])),
            method: "PUT",
            key: ABSENT_KEY,
            expected: SERVED,
        },
    ]
}

/// Record that the requests signed with [`SERVICE_KEY_ID`] are [`SERVICE_PRINCIPAL`]'s own
/// calls for [`source_arn`] — exactly what the calling service records for a call it makes
/// itself, under the credential it minted for it.
fn acting_as_the_service_principal(engine: &Arc<engine::InMemoryEngine>) {
    let now = SystemTime::now();
    engine
        .state::<service_common::service_principal_calls::ServicePrincipalCalls>()
        .lock()
        .remember(
            SERVICE_KEY_ID,
            service_common::service_principal_calls::ServicePrincipalCall {
                principal: SERVICE_PRINCIPAL.to_string(),
                source_arn: source_arn(),
                source_account: engine::emulator_account_id().to_string(),
                expires_at: now + Duration::from_secs(600),
            },
            now,
        );
}

/// Create `bucket`, write the one object the rows read as the bucket's own account, and attach
/// `document` when the row carries one.
async fn configured_bucket(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    document: Option<&str>,
) {
    bucket(router, name).await;
    let written = put_object(router, name, HELD_KEY, BODY, &[]).await;
    assert_eq!(
        written.status,
        200,
        "case {case:?}: the object must be stored before the policy is attached, got {} with \
         body {:?}",
        written.status,
        text(&written)
    );
    if let Some(document) = document {
        let attached = put_bucket_policy(router, name, document).await;
        assert_eq!(
            attached.status,
            204,
            "case {case:?}: the policy must be attached, got {} with body {:?}",
            attached.status,
            text(&attached)
        );
    }
}

/// Assert one answer against what the row expects.
fn assert_answer(case: &str, expected: &Expected, answer: &Answer) {
    assert_eq!(
        answer.status, expected.status,
        "case {case:?}: expected HTTP {}, got {} with body {:?}",
        expected.status,
        answer.status,
        text(answer)
    );
    if expected.code.is_empty() {
        return;
    }
    assert_eq!(
        answer.error_code(),
        expected.code,
        "case {case:?}: expected the {:?} code, got {:?} with body {:?}",
        expected.code,
        answer.error_code(),
        text(answer)
    );
    assert_eq!(
        answer.error_message(),
        expected.message,
        "case {case:?}: expected the message {:?}, got {:?}",
        expected.message,
        answer.error_message()
    );
}

#[tokio::test]
async fn a_bucket_policy_alone_decides_a_service_principals_call() {
    let (router, engine) = router_with_engine();
    acting_as_the_service_principal(&engine);

    for row in rows() {
        configured_bucket(&router, row.case, row.bucket, row.policy.as_deref()).await;
        let answer = call(&router, SERVICE_KEY_ID, row.method, row.bucket, row.key).await;
        assert_answer(row.case, &row.expected, &answer);
        if row.expected.status == 200 && row.method == "GET" {
            assert_eq!(
                answer.body.as_ref(),
                BODY,
                "case {:?}: a served read must answer the stored bytes, got {:?}",
                row.case,
                text(&answer)
            );
        }
        // A refusal says nothing about whether the key is there: the one thing the 403 must
        // never carry is the code that reports an absence.
        if row.expected.status == 403 {
            assert!(
                !text(&answer).contains(NO_SUCH_KEY),
                "case {:?}: a refusal must not report whether the key is there, got {:?}",
                row.case,
                text(&answer)
            );
        }
    }
}

#[tokio::test]
async fn a_request_no_such_call_was_recorded_for_is_decided_by_its_own_credential() {
    let (router, engine) = router_with_engine();
    acting_as_the_service_principal(&engine);
    let name = "sp-other-credentials";
    configured_bucket(&router, "another credential", name, Some(&read_grant(name))).await;

    // The account's own published credential is not the service principal: the policy above
    // grants it nothing, and nothing the identity-based half of the decision finds bounds it,
    // so it reads the object it owns and is told the truth about a key that is not there.
    let rows: [(&str, &str, u16, &str); 2] = [
        ("the account's own credential over a key it holds", HELD_KEY, 200, ""),
        (
            "the account's own credential over a key it does not",
            ABSENT_KEY,
            404,
            NO_SUCH_KEY,
        ),
    ];
    for (case, key, status, code) in rows {
        let answer = call(&router, ACCOUNT_KEY_ID, "GET", name, key).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        if !code.is_empty() {
            assert_eq!(
                answer.error_code(),
                code,
                "case {case:?}: expected the {code:?} code, got {:?}",
                answer.error_code()
            );
        }
    }

    // And the record answers for the one credential it was written for: once the call it
    // describes is over, the key it was signed with is nobody's service principal again.
    engine
        .state::<service_common::service_principal_calls::ServicePrincipalCalls>()
        .lock()
        .forget(SERVICE_KEY_ID);
    let answer = call(&router, SERVICE_KEY_ID, "GET", name, ABSENT_KEY).await;
    assert_eq!(
        answer.status, 404,
        "case {:?}: expected HTTP 404 once the call is forgotten, got {} with body {:?}",
        "a credential whose call has returned",
        answer.status,
        text(&answer)
    );
    assert_eq!(
        answer.error_code(),
        NO_SUCH_KEY,
        "case {:?}: expected the {NO_SUCH_KEY:?} code, got {:?}",
        "a credential whose call has returned",
        answer.error_code()
    );
}
