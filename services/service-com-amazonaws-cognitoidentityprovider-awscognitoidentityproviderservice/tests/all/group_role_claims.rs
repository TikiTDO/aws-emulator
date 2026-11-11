// SPDX-License-Identifier: CC0-1.0

//! The ROLE claims a user pool's groups put in the identity token of a sign-in.
//!
//! RULE: a group's `RoleArn` and `Precedence` decide the `cognito:roles` and
//! `cognito:preferred_role` claims of the identity token a sign-in issues. `CreateGroup`'s
//! own `Precedence` documentation states it (this service's model): "Zero is the highest
//! precedence value. Groups with lower `Precedence` values take precedence over groups with
//! higher or null `Precedence` values. If a user belongs to two or more groups, it is the
//! group with the lowest precedence value whose role ARN is given in the user's tokens for
//! the `cognito:roles` and `cognito:preferred_role` claims. Two groups can have the same
//! `Precedence` value. If this happens, neither group takes precedence over the other. If
//! two groups with the same `Precedence` have the same role ARN, that role is used in the
//! `cognito:preferred_role` claim in tokens for users in each group. If the two groups have
//! different role ARNs, the `cognito:preferred_role` claim isn't set. The default
//! `Precedence` value is null. The maximum `Precedence` value is `2^31-1`."
//!
//! So: every role the user's groups grant appears in `cognito:roles`, once, best precedence
//! first; the preference is the best-precedence group's role, and is WITHHELD exactly when
//! the groups tied at that precedence name different roles; a group granting no role
//! contributes to neither claim; and a user whose groups grant no role at all carries
//! neither claim rather than an empty list.
//!
//! Both claims are the IDENTITY token's alone — "Amazon Cognito ID tokens have a
//! `cognito:preferred_role` claim that presents the highest-precedence group that a user
//! belongs to. Both ID and access tokens also contain a `cognito:groups` claim"
//! (`CreateGroup`'s `RoleArn`, this service's model) — so the access and refresh tokens of
//! the same sign-in carry neither.
//!
//! The rows differ along what the rule generalises over: the COUNT of groups (none, one,
//! two, three), whether each grants a role, the precedences (absent, zero, distinct,
//! equal, the documented maximum `2^31-1`), the ORDER the precedences put the roles in, and
//! whether the groups tied on the best precedence agree about the role.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service; the signature
/// itself is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The documented maximum of `CreateGroup`'s `Precedence`: "The maximum `Precedence` value
/// is `2^31-1`" (this service's model).
const MAX_PRECEDENCE: i64 = 2_147_483_647;

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// A base64url JWS segment's bytes (RFC 7515 §2): the alphabet substitutions and the
/// stripped padding are restored before decoding.
fn decode_base64url(segment: &str) -> Vec<u8> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|error| panic!("{segment:?} must be base64url ({error})"))
}

/// The claim set of a compact-serialized JWS.
fn claims_of(token: &str) -> Value {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a JWS compact serialization has three segments, got {token}"
    );
    serde_json::from_slice(&decode_base64url(segments[1]))
        .unwrap_or_else(|error| panic!("the claims must be JSON ({error}): {token}"))
}

/// An IAM role ARN naming `role_name`, satisfying `ArnType` (length 20..2048 and the
/// pattern `CreateGroup`'s `RoleArn` carries).
fn role_arn(role_name: &str) -> String {
    format!("arn:aws:iam::000000000000:role/{role_name}")
}

/// A pool with one app client that admits `ADMIN_USER_PASSWORD_AUTH`, returned as
/// (`UserPoolId`, `ClientId`).
async fn pool_with_client(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let pool = ok(router, "CreateUserPool", &json!({"PoolName": name})).await;
    let user_pool_id = pool["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": format!("{name}-client"),
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    (user_pool_id, client_id)
}

/// Create `username` with a permanent password so it can sign in.
async fn user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) {
    ok(
        router,
        "AdminCreateUser",
        &json!({"UserPoolId": user_pool_id, "Username": username}),
    )
    .await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
}

/// Sign `username` in and return the `AuthenticationResult`.
async fn sign_in(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
) -> Value {
    let result = ok(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": {"USERNAME": username, "PASSWORD": GOOD_PASSWORD},
        }),
    )
    .await;
    result["AuthenticationResult"].clone()
}

/// One group a row creates: its name, the role it grants (`None` for a group that grants
/// none) and its `Precedence` (`None` for the documented default).
struct GroupSpec {
    group_name: &'static str,
    role: Option<&'static str>,
    precedence: Option<i64>,
}

/// One row of the table: the case, the user that signs in, the groups it belongs to, and
/// the two claims its identity token must then carry — the roles in the order the claim
/// must list them, and the preferred role (`None` for a claim that must be absent).
struct Row {
    case: &'static str,
    username: &'static str,
    groups: Vec<GroupSpec>,
    roles: Vec<&'static str>,
    preferred: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a user in no group",
            username: "claims-none",
            groups: vec![],
            roles: vec![],
            preferred: None,
        },
        Row {
            case: "one group that grants no role",
            username: "claims-roleless",
            groups: vec![GroupSpec {
                group_name: "roleless",
                role: None,
                precedence: Some(0),
            }],
            roles: vec![],
            preferred: None,
        },
        Row {
            case: "one group with a role and the default null Precedence",
            username: "claims-default",
            groups: vec![GroupSpec {
                group_name: "default-precedence",
                role: Some("role-default"),
                precedence: None,
            }],
            roles: vec!["role-default"],
            preferred: Some("role-default"),
        },
        Row {
            case: "one group with a role at Precedence 0, the highest",
            username: "claims-zero",
            groups: vec![GroupSpec {
                group_name: "zero-precedence",
                role: Some("role-zero"),
                precedence: Some(0),
            }],
            roles: vec!["role-zero"],
            preferred: Some("role-zero"),
        },
        Row {
            case: "two groups, distinct precedences: the lower one wins",
            username: "claims-distinct",
            groups: vec![
                GroupSpec {
                    group_name: "distinct-high",
                    role: Some("role-high"),
                    precedence: Some(10),
                },
                GroupSpec {
                    group_name: "distinct-low",
                    role: Some("role-low"),
                    precedence: Some(1),
                },
            ],
            roles: vec!["role-low", "role-high"],
            preferred: Some("role-low"),
        },
        Row {
            case: "the same two roles with the precedences flipped: the winner flips",
            username: "claims-flipped",
            groups: vec![
                GroupSpec {
                    group_name: "flipped-a",
                    role: Some("role-high"),
                    precedence: Some(1),
                },
                GroupSpec {
                    group_name: "flipped-b",
                    role: Some("role-low"),
                    precedence: Some(10),
                },
            ],
            roles: vec!["role-high", "role-low"],
            preferred: Some("role-high"),
        },
        Row {
            case: "two groups at EQUAL Precedence with different roles: no preference",
            username: "claims-tied",
            groups: vec![
                GroupSpec {
                    group_name: "tied-a",
                    role: Some("role-tied-a"),
                    precedence: Some(5),
                },
                GroupSpec {
                    group_name: "tied-b",
                    role: Some("role-tied-b"),
                    precedence: Some(5),
                },
            ],
            roles: vec!["role-tied-a", "role-tied-b"],
            preferred: None,
        },
        Row {
            case: "two groups at EQUAL Precedence with the SAME role: that role is preferred",
            username: "claims-tied-same",
            groups: vec![
                GroupSpec {
                    group_name: "tied-same-a",
                    role: Some("role-shared"),
                    precedence: Some(5),
                },
                GroupSpec {
                    group_name: "tied-same-b",
                    role: Some("role-shared"),
                    precedence: Some(5),
                },
            ],
            roles: vec!["role-shared"],
            preferred: Some("role-shared"),
        },
        Row {
            case: "a group stating a Precedence beside one stating none: the stated one wins",
            username: "claims-mixed",
            groups: vec![
                GroupSpec {
                    group_name: "mixed-null",
                    role: Some("role-null"),
                    precedence: None,
                },
                GroupSpec {
                    group_name: "mixed-stated",
                    role: Some("role-stated"),
                    precedence: Some(MAX_PRECEDENCE),
                },
            ],
            roles: vec!["role-stated", "role-null"],
            preferred: Some("role-stated"),
        },
        Row {
            case: "a group granting a role beside one granting none",
            username: "claims-partial",
            groups: vec![
                GroupSpec {
                    group_name: "partial-roleless",
                    role: None,
                    precedence: Some(0),
                },
                GroupSpec {
                    group_name: "partial-role",
                    role: Some("role-partial"),
                    precedence: Some(7),
                },
            ],
            roles: vec!["role-partial"],
            preferred: Some("role-partial"),
        },
        Row {
            case: "three groups spanning Precedence 0, 5 and the maximum 2^31-1",
            username: "claims-three",
            groups: vec![
                GroupSpec {
                    group_name: "three-last",
                    role: Some("role-three-c"),
                    precedence: Some(MAX_PRECEDENCE),
                },
                GroupSpec {
                    group_name: "three-first",
                    role: Some("role-three-a"),
                    precedence: Some(0),
                },
                GroupSpec {
                    group_name: "three-middle",
                    role: Some("role-three-b"),
                    precedence: Some(5),
                },
            ],
            roles: vec!["role-three-a", "role-three-b", "role-three-c"],
            preferred: Some("role-three-a"),
        },
        Row {
            case: "two groups at the maximum Precedence with different roles: no preference",
            username: "claims-max-tied",
            groups: vec![
                GroupSpec {
                    group_name: "max-tied-a",
                    role: Some("role-max-a"),
                    precedence: Some(MAX_PRECEDENCE),
                },
                GroupSpec {
                    group_name: "max-tied-b",
                    role: Some("role-max-b"),
                    precedence: Some(MAX_PRECEDENCE),
                },
            ],
            roles: vec!["role-max-a", "role-max-b"],
            preferred: None,
        },
    ]
}

#[tokio::test]
async fn the_identity_token_s_role_claims_follow_the_groups_roles_and_precedences() {
    let router = router();
    let (user_pool_id, client_id) = pool_with_client(&router, "group-role-claims").await;

    for row in rows() {
        let case = row.case;
        for group in &row.groups {
            let mut request = json!({
                "UserPoolId": &user_pool_id,
                "GroupName": group.group_name,
            });
            if let Some(role) = group.role {
                request["RoleArn"] = json!(role_arn(role));
            }
            if let Some(precedence) = group.precedence {
                request["Precedence"] = json!(precedence);
            }
            let (status, payload) = call(&router, "CreateGroup", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateGroup({:?}) must succeed, got {status} with {payload}",
                group.group_name
            );
        }
        user(&router, &user_pool_id, row.username).await;
        for group in &row.groups {
            ok(
                &router,
                "AdminAddUserToGroup",
                &json!({
                    "UserPoolId": &user_pool_id,
                    "Username": row.username,
                    "GroupName": group.group_name,
                }),
            )
            .await;
        }

        let result = sign_in(&router, &user_pool_id, &client_id, row.username).await;
        let id_token = result["IdToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an IdToken must be issued, got {result}"));
        let claims = claims_of(id_token);

        let expected_roles: Option<Value> = (!row.roles.is_empty()).then(|| {
            Value::Array(
                row.roles
                    .iter()
                    .map(|role| Value::String(role_arn(role)))
                    .collect(),
            )
        });
        let actual_roles = claims.get("cognito:roles").cloned();
        assert_eq!(
            actual_roles, expected_roles,
            "case {case:?}: expected cognito:roles {expected_roles:?}, got {actual_roles:?}"
        );

        let expected_preferred = row.preferred.map(|role| Value::String(role_arn(role)));
        let actual_preferred = claims.get("cognito:preferred_role").cloned();
        assert_eq!(
            actual_preferred, expected_preferred,
            "case {case:?}: expected cognito:preferred_role {expected_preferred:?}, got \
             {actual_preferred:?}"
        );

        // Both claims belong to the identity token alone, and the opaque refresh
        // credential beside it states no claim at all.
        let access = result["AccessToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: AccessToken must be issued, got {result}"));
        let other = claims_of(access);
        for claim in ["cognito:roles", "cognito:preferred_role"] {
            assert!(
                other.get(claim).is_none(),
                "case {case:?}: the AccessToken carries no {claim} claim, got {:?}",
                other.get(claim)
            );
        }
        let refresh = result["RefreshToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: RefreshToken must be issued, got {result}"));
        // The refresh token is SEALED: a JWE in compact serialization (RFC 7516 §7.1,
        // five segments) rather than the three of a JWS whose claim segment its holder
        // reads, so it carries no claim at all.
        assert_eq!(
            refresh.split('.').count(),
            5,
            "case {case:?}: the refresh token is sealed in JWE compact serialization, \
             got {refresh}"
        );
        assert!(
            !refresh.contains(row.username),
            "case {case:?}: the refresh token carries no claim to read at all, found the \
             user's own name in {refresh}"
        );

        // The claims are read from STORED state, not minted once: a second sign-in renders
        // the same two claims.
        let again = sign_in(&router, &user_pool_id, &client_id, row.username).await;
        let repeated = claims_of(
            again["IdToken"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an IdToken must be issued")),
        );
        for claim in ["cognito:roles", "cognito:preferred_role"] {
            assert_eq!(
                repeated.get(claim),
                claims.get(claim),
                "case {case:?}: a later sign-in renders the same {claim}, got {:?}",
                repeated.get(claim)
            );
        }
    }
}
