// SPDX-License-Identifier: CC0-1.0

//! Lake Formation resolves the principal a permission is granted TO against this account's
//! IAM — through the WHOLE assembled router.
//!
//! RULE: a Lake Formation permission is held BY a principal, so a grant to an identity that
//! does not exist is a grant to nobody: `GrantPermissions` resolves its `Principal` against
//! this account's IAM before it records anything and refuses the request with the
//! operation's modelled `InvalidInputException` (HTTP 400), recording nothing, when IAM
//! answers that the identity is absent. A refusal has to be grounded in what IAM ANSWERED,
//! so a `DataLakePrincipalIdentifier` that names no IAM role or user of this account — the
//! `IAM_ALLOWED_PRINCIPALS` group, an identity of ANOTHER account whose IAM this emulator is
//! not the one to ask, a principal of another identity system — is resolved by nothing and
//! refuses nothing.
//!
//! The principal is resolved through the in-process callback, so a single-service router has
//! no IAM to ask: that is why this suite lives here rather than in the service's own crate.
//! The identities are made with IAM's OWN wire API (`awsQuery`) and the grants with Lake
//! Formation's (`restJson1`), so a response can only be what the handlers computed from
//! stored state.
//!
//! The rows span what the rule generalises over: the IAM entity type the identifier names
//! (role, user), the path the entity lives under, whether the entity was ever created, which
//! account it belongs to, and the identifier forms that name no IAM entity at all.
//!
//! Each row grants on an LF-tag resource of its own, which is this service's own record
//! rather than a Data Catalog entity, so no row's grant can be refused for the resource it
//! names and no two rows share a record.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The form encoder the awsQuery calls to IAM need is the one
// `all/cloudfront_key_value_store_handshake.rs` already owns.
use crate::cloudfront_key_value_store_handshake::segment as form_encode;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// A syntactically valid account id that is NOT the account this emulator serves.
const OTHER_ACCOUNT: &str = "123456789012";

/// The group a Lake Formation grant may name instead of an identity: it stands for every
/// principal IAM itself allows, so it is no IAM role or user and IAM is not asked about it.
const IAM_ALLOWED_PRINCIPALS: &str = "IAM_ALLOWED_PRINCIPALS";

/// The permission every row grants, which an LF-tag resource carries.
const ASSOCIATE: &str = "ASSOCIATE";

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never checked: this router runs with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// Send one buffered request and answer its status with the body, parsed when it is JSON.
fn send(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    path: &str,
    content_type: &str,
    body: String,
) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://{signing_name}.{REGION}.amazonaws.com{path}"
        ))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type)
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Drive one IAM operation (`awsQuery`: the action and its parameters go in the form body).
fn iam(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    parameters: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    send(
        router,
        runtime,
        "iam",
        "/",
        "application/x-www-form-urlencoded",
        body,
    )
}

/// Drive one Lake Formation operation (`restJson1`: the operation names the path).
fn lakeformation(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let (status, text) = send(
        router,
        runtime,
        "lakeformation",
        &format!("/{operation}"),
        "application/json",
        body.to_string(),
    );
    let parsed = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code, without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("")
        .to_string()
}

/// The error message, whichever casing the envelope carries it under.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// The ARN of an IAM entity: `arn:aws:iam::<account>:<resource>`.
fn iam_arn(account: &str, resource: &str) -> String {
    format!("arn:aws:iam::{account}:{resource}")
}

/// A trust policy any service may assume, so the fixture roles are ordinary roles: what
/// this suite asks IAM is whether the entity EXISTS, not what may assume it.
fn trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "lakeformation.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }]
    })
    .to_string()
}

/// Create the IAM identities the rows name: two roles (one under the default path, one
/// under a path) and one user.
fn create_identities(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime) {
    let document = trust_policy();
    for (role_name, path) in [("analyst", None), ("path-analyst", Some("/team/"))] {
        let mut parameters = vec![
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", document.as_str()),
        ];
        if let Some(path) = path {
            parameters.push(("Path", path));
        }
        let (status, body) = iam(router, runtime, "CreateRole", &parameters);
        assert_eq!(
            status, 200,
            "setup: CreateRole {role_name} must succeed, got {status} {body}"
        );
    }
    let (status, body) = iam(router, runtime, "CreateUser", &[("UserName", "auditor")]);
    assert_eq!(
        status, 200,
        "setup: CreateUser auditor must succeed, got {status} {body}"
    );
}

/// One row: the `DataLakePrincipalIdentifier` a grant names, and whether the grant must be
/// recorded.
struct PrincipalCase {
    case: &'static str,
    principal: String,
    recorded: bool,
}

fn principal_cases(account: &str) -> Vec<PrincipalCase> {
    let row = |case: &'static str, principal: String, recorded: bool| PrincipalCase {
        case,
        principal,
        recorded,
    };
    vec![
        row(
            "a role of this account that exists",
            iam_arn(account, "role/analyst"),
            true,
        ),
        row(
            "a role of this account under a path",
            iam_arn(account, "role/team/path-analyst"),
            true,
        ),
        row(
            "a user of this account that exists",
            iam_arn(account, "user/auditor"),
            true,
        ),
        row(
            // The corpus' own request: a grant to a role nobody created.
            "a role of this account that was never created",
            iam_arn(account, "role/ghost-analyst"),
            false,
        ),
        row(
            "a role under a path that was never created",
            iam_arn(account, "role/team/ghost-analyst"),
            false,
        ),
        row(
            "a user of this account that was never created",
            iam_arn(account, "user/ghost-auditor"),
            false,
        ),
        row(
            // The group every IAM principal belongs to: no IAM entity, so nothing to ask.
            "the IAM-allowed group",
            IAM_ALLOWED_PRINCIPALS.to_string(),
            true,
        ),
        row(
            // Another account's IAM is not this emulator's to ask.
            "a role of another account that was never created",
            iam_arn(OTHER_ACCOUNT, "role/ghost-analyst"),
            true,
        ),
        row(
            "a user of another account that was never created",
            iam_arn(OTHER_ACCOUNT, "user/ghost-auditor"),
            true,
        ),
        row(
            // A principal of another identity system carries its own type, which names no
            // IAM role or user.
            "a QuickSight user",
            format!("arn:aws:quicksight:{REGION}:{account}:user/default/analyst"),
            true,
        ),
        row(
            "a SAML-decorated principal",
            iam_arn(account, "saml-provider/corp:user/jdoe"),
            true,
        ),
    ]
}

/// The LF-tag resource row `index` grants on: its own record, so no two rows share one.
fn lf_tag_resource(index: usize) -> Value {
    json!({ "LFTag": { "TagKey": format!("principal-row-{index}"), "TagValues": ["prod"] } })
}

/// The LF-tag keys the grants `principal` holds are recorded against.
fn held_tag_keys(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    principal: &str,
) -> Vec<String> {
    let (status, body) = lakeformation(
        router,
        runtime,
        "ListPermissions",
        &json!({ "Principal": { "DataLakePrincipalIdentifier": principal } }),
    );
    assert_eq!(
        status, 200,
        "ListPermissions for {principal} must answer 200, got {status} {body}"
    );
    let mut keys: Vec<String> = body["PrincipalResourcePermissions"]
        .as_array()
        .unwrap_or_else(|| panic!("PrincipalResourcePermissions must be a list, got {body}"))
        .iter()
        .filter_map(|entry| {
            Some(
                entry["Resource"]["LFTag"]["TagKey"]
                    .as_str()?
                    .to_string(),
            )
        })
        .collect();
    keys.sort();
    keys
}

#[test]
fn a_permission_is_recorded_only_for_a_principal_this_account_holds() {
    std::thread::Builder::new()
        .name("lakeformation-grant-principal".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            create_identities(&router, &runtime);

            let account = engine::emulator_account_id().to_string();
            for (index, row) in principal_cases(&account).into_iter().enumerate() {
                let resource = lf_tag_resource(index);
                let expected_key = format!("principal-row-{index}");
                let (status, body) = lakeformation(
                    &router,
                    &runtime,
                    "GrantPermissions",
                    &json!({
                        "Principal": { "DataLakePrincipalIdentifier": &row.principal },
                        "Resource": resource,
                        "Permissions": [ASSOCIATE],
                    }),
                );

                if row.recorded {
                    assert_eq!(
                        status, 200,
                        "case {:?} ({}): the grant must be served, got {status} {body}",
                        row.case, row.principal
                    );
                } else {
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (400, "InvalidInputException"),
                        "case {:?} ({}): a grant to an identity IAM does not hold must be \
                         refused 400 InvalidInputException, got {status} {body}",
                        row.case,
                        row.principal
                    );
                    assert_eq!(
                        error_message(&body),
                        format!("Invalid principal, arn: {}", row.principal),
                        "case {:?}: the refusal must name the principal the request carried, \
                         got {body}",
                        row.case
                    );
                }

                // What the principal holds afterwards: the row's own LF-tag when the grant
                // was served, and nothing at all when it was refused.
                let held = held_tag_keys(&router, &runtime, &row.principal);
                let expected: Vec<String> = if row.recorded {
                    vec![expected_key]
                } else {
                    Vec::new()
                };
                assert_eq!(
                    held, expected,
                    "case {:?} ({}): expected the recorded grants {expected:?}, got {held:?}",
                    row.case, row.principal
                );
            }
        })
        .expect("spawn the grant-principal thread")
        .join()
        .expect("every Lake Formation grantee must be resolved against this account's IAM");
}
