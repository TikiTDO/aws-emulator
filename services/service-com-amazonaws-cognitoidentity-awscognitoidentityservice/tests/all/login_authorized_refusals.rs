// SPDX-License-Identifier: CC0-1.0

//! What AUTHORIZES a call of the unsigned, login-authorized operations — the credential
//! exchange a client performs before it holds any Amazon Web Services credential — and how
//! each refusal is worded.
//!
//! Four rules, each asserted over a table of inputs rather than over one recorded call.
//!
//! * THE TOKEN ITSELF. A `Logins` entry made under a provider whose logins are OpenID
//!   Connect identity tokens (an Amazon Cognito user pool, named
//!   `cognito-idp.<region>.amazonaws.com/<UserPoolId>`, or an OpenID Connect issuer the pool
//!   is configured with) has to carry one, issued BY that provider: a value that is no such
//!   token, and one whose `iss` claim names another issuer than the provider name it is
//!   presented under, each authorize nothing. The rows span the operations that take
//!   `Logins` and the forms a presented value can take.
//! * THE IDENTITY. An identity a login has been linked to is a federated user's, so a call
//!   naming it is authorized by a login OF it. A call presenting logins none of which is one
//!   of that identity's has proven only that it holds some other user's login; one presenting
//!   no login at all has proven nothing. An UNAUTHENTICATED (guest) identity has no login to
//!   present, so its id is what names it — and it has no linked login to unlink either.
//! * `CustomRoleArn`. The member substitutes one of the SEVERAL roles the identity provider's
//!   answer offered, so only an exchange of a SAML provider or of a provider the pool holds a
//!   `RoleMappings` entry for may name one at all.
//! * THE DEVELOPER LINK. `GetOpenIdTokenForDeveloperIdentity` links logins that name ONE
//!   account: a developer user identifier the pool has registered against another identity is
//!   not a login of the identity the request names, and an identity an identity PROVIDER's
//!   login already proves is that provider's user, so a developer user is not joined to it.
//!
//! Every refusal of this service travels with HTTP 400, whatever `@httpError` its shape is
//! annotated with (`crate::service` owns that rule); each assertion below names the case and
//! the value it got.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Map, Value};

use crate::developer_identity_tokens::{pool_with_developer_provider, token_request};
use crate::identity_pool_flow::{error_code, Emulator};
use crate::mapped_role_resolution::identity_token;

/// The status every refusal of this service travels with.
const REFUSED_STATUS: u16 = 400;

/// The `Logins` provider name of an Amazon Cognito user pool the fixture pools below trust,
/// and of one no pool is configured with: a user pool is named
/// `cognito-idp.<region>.amazonaws.com/<UserPoolId>`
/// (`smithy_query(query="doc",shape="GetIdInput",member="Logins")`), and its login is that
/// user pool's id token.
const USER_POOL_PROVIDER: &str = "cognito-idp.us-east-1.amazonaws.com/us-east-1_authz001";
const OTHER_USER_POOL_PROVIDER: &str = "cognito-idp.us-east-1.amazonaws.com/us-east-1_authz002";

/// A public provider of the fixture pools: one of the names `GetId$Logins` publishes, whose
/// login is an opaque OAuth access token rather than an identity token.
const PUBLIC_PROVIDER: &str = "graph.facebook.com";

/// The fixed sentences AWS words these refusals with, spelled out here so the wire-visible
/// text is pinned by the test rather than read back out of the crate.
const NOT_AN_IDENTITY_TOKEN: &str = "Invalid login token. Not a valid OpenId Connect identity token.";
const MISMATCHED_ISSUER: &str = "Invalid login token. Issuer doesn't match providerName";
const UNMATCHED_LOGINS: &str = "Logins don't match. Please include at least one valid login for \
                                this identity or identity pool.";
const CANNOT_UNLINK_UNAUTHENTICATED: &str = "Cannot unlink unauthenticated user.";
const UNAUTHENTICATED_ACCESS: &str = "Unauthenticated access is not supported for this identity pool.";
const CANNOT_LINK_LOGINS: &str = "Cannot link these logins.";
const CANNOT_MERGE_IDENTITIES: &str = "Cannot merge these identities.";
const CUSTOM_ROLE_ARN_UNSUPPORTED: &str =
    "Only SAML providers and providers with RoleMappings support custom role ARN.";
const NO_DEVELOPER_ACCESS: &str = "You do not have access to this identity";

/// The `message` an error response carries.
fn error_message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an error response carries a message: {body}"))
        .to_string()
}

/// The identity token `issuer` issues, naming `subject`: a compact JWS whose header names a
/// signing algorithm, whose claim set names that issuer and subject, and whose signature
/// segment is filled (RFC 7515 §3.1, RFC 7519 §4.1). Nothing here verifies the signature —
/// that needs the issuer's published key — so what a presented value is decided by is this
/// form and these claims.
fn id_token_of(issuer: &str, subject: &str) -> String {
    identity_token(&json!({
        "iss": format!("https://{issuer}"),
        "sub": subject,
        "token_use": "id",
        "exp": 4_102_444_800u64,
    }))
}

/// A `Logins` map of one entry.
fn logins(provider: &str, login: &str) -> Value {
    let mut map = Map::new();
    map.insert(provider.to_string(), json!(login));
    Value::Object(map)
}

/// A pool trusting one Amazon Cognito user pool and one public provider, with its two roles
/// seeded so a vend can be served at all, and with unauthenticated identities allowed.
async fn pool_trusting_a_user_pool(emulator: &Emulator, name: &str) -> String {
    let pool_id = emulator.pool_with_roles(name, true).await;
    emulator
        .ok(
            "UpdateIdentityPool",
            &json!({
                "IdentityPoolId": &pool_id,
                "IdentityPoolName": name,
                "AllowUnauthenticatedIdentities": true,
                "CognitoIdentityProviders": [
                    {"ProviderName": USER_POOL_PROVIDER, "ClientId": "client_1"},
                ],
                "SupportedLoginProviders": {PUBLIC_PROVIDER: "0123456789"},
            }),
        )
        .await;
    pool_id
}

/// RULE: a `Logins` entry made under a provider whose logins are OpenID Connect identity
/// tokens has to carry one, issued BY that provider — so a value that is no such token and
/// one carrying another issuer are each refused `NotAuthorizedException`, with the sentence
/// that says which it was, on every operation that takes `Logins`.
#[tokio::test]
async fn a_presented_login_is_the_token_its_provider_issues() {
    let emulator = Emulator::new();
    let pool_id = pool_trusting_a_user_pool(&emulator, "presented-token").await;

    // The rows differ along what the rule generalises over: the FORM of the value (an opaque
    // string, a two-segment value, an unsecured JWS, an identity token) and the ISSUER its
    // claim set names (the provider it is presented under, another user pool, this service
    // itself).
    let unsecured = format!(
        "{}.{}.",
        identity_token(&json!({"iss": format!("https://{USER_POOL_PROVIDER}")}))
            .split('.')
            .next()
            .expect("a first segment"),
        "e30"
    );
    // (case, the value presented for the user-pool provider, the refusal it earns)
    let rows: Vec<(&str, String, Option<&str>)> = vec![
        (
            "an opaque value that is no token at all",
            "an-opaque-value".to_string(),
            Some(NOT_AN_IDENTITY_TOKEN),
        ),
        (
            "two segments instead of three",
            "e30.e30".to_string(),
            Some(NOT_AN_IDENTITY_TOKEN),
        ),
        (
            "an unsecured value, so nothing signed it",
            unsecured,
            Some(NOT_AN_IDENTITY_TOKEN),
        ),
        (
            "an identity token of another user pool",
            id_token_of(OTHER_USER_POOL_PROVIDER, "someone-else"),
            Some(MISMATCHED_ISSUER),
        ),
        (
            "a token this service issued, presented as the user pool's",
            id_token_of("cognito-identity.amazonaws.com", "a-pool-identity"),
            Some(MISMATCHED_ISSUER),
        ),
        (
            "the identity token that user pool issues",
            id_token_of(USER_POOL_PROVIDER, "the-user-pools-own-user"),
            None,
        ),
    ];

    for (row, (case, presented, refusal)) in rows.iter().enumerate() {
        // Each row gets its own identity, reached through the login that row presents where
        // the call must be served and through the user pool's own token otherwise, so what
        // the row varies is the value the calls below present.
        let linked = match refusal {
            None => presented.clone(),
            Some(_) => id_token_of(USER_POOL_PROVIDER, &format!("linked-user-{row}")),
        };
        let identity_id = emulator
            .identity(&pool_id, Some(logins(USER_POOL_PROVIDER, &linked)))
            .await;
        let presented_logins = logins(USER_POOL_PROVIDER, presented);

        // Every operation a client presents a login to decides it the same way.
        let calls: Vec<(&str, Value)> = vec![
            (
                "GetId",
                json!({"IdentityPoolId": &pool_id, "Logins": &presented_logins}),
            ),
            (
                "GetOpenIdToken",
                json!({"IdentityId": &identity_id, "Logins": &presented_logins}),
            ),
            (
                "GetCredentialsForIdentity",
                json!({"IdentityId": &identity_id, "Logins": &presented_logins}),
            ),
            (
                "UnlinkIdentity",
                json!({
                    "IdentityId": &identity_id,
                    "Logins": &presented_logins,
                    "LoginsToRemove": [USER_POOL_PROVIDER],
                }),
            ),
        ];
        for (operation, request) in calls {
            let (status, body) = emulator.call(operation, &request).await;
            match refusal {
                None => assert_eq!(
                    status, 200,
                    "case {case:?} on {operation}: the token the provider issues is served, \
                     got {status} with {body}"
                ),
                Some(message) => {
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (REFUSED_STATUS, "NotAuthorizedException"),
                        "case {case:?} on {operation}: expected \
                         ({REFUSED_STATUS}, NotAuthorizedException), got ({status}, {body})"
                    );
                    assert_eq!(
                        error_message(case, &body),
                        *message,
                        "case {case:?} on {operation}: expected the fixed sentence \
                         {message:?}, got {body}"
                    );
                }
            }
        }
    }
}

/// RULE: a call naming an identity is authorized by a LOGIN of that identity — one the pool
/// has indexed against it, or a token this service minted for it — so a call presenting
/// another user's login is refused the login mismatch, a call presenting none is refused
/// access to the identity, and an unlink of an identity no login was ever linked to is
/// refused as the unauthenticated user it is. A vend presenting no login at all is the
/// unauthenticated exchange, which a pool whose `AllowUnauthenticatedIdentities` does not
/// admit one refuses in those words. Every refusal leaves the identity's logins as they
/// were.
#[tokio::test]
async fn an_identity_is_reached_only_through_a_login_of_its_own() {
    let emulator = Emulator::new();
    let pool_id = pool_trusting_a_user_pool(&emulator, "identity-access").await;
    let own = id_token_of(USER_POOL_PROVIDER, "the-identitys-own-user");
    let identity_id = emulator
        .identity(&pool_id, Some(logins(USER_POOL_PROVIDER, &own)))
        .await;
    // A second identity of the same pool, whose login is nobody else's.
    let other = id_token_of(USER_POOL_PROVIDER, "another-user-of-the-pool");
    let other_identity = emulator
        .identity(&pool_id, Some(logins(USER_POOL_PROVIDER, &other)))
        .await;
    // And an identity no login was ever linked to: the one `GetId` mints for a call
    // presenting none.
    let guest = emulator.identity(&pool_id, None).await;

    // The rows differ along what the rule generalises over: the operation, which identity is
    // named (a federated one, a guest one) and what the call presents for it (its own login,
    // another identity's, a public provider's the pool has linked to nobody, none at all).
    // (case, operation, identity, the logins presented, the refusal owed)
    type Row<'a> = (&'a str, &'a str, &'a str, Option<Value>, Option<&'a str>);
    let rows: Vec<Row<'_>> = vec![
        (
            "a token exchange presenting the identity's own login",
            "GetOpenIdToken",
            identity_id.as_str(),
            Some(logins(USER_POOL_PROVIDER, &own)),
            None,
        ),
        (
            "a token exchange presenting another identity's login",
            "GetOpenIdToken",
            identity_id.as_str(),
            Some(logins(USER_POOL_PROVIDER, &other)),
            Some(UNMATCHED_LOGINS),
        ),
        (
            "a token exchange presenting a login of nobody",
            "GetOpenIdToken",
            identity_id.as_str(),
            Some(logins(PUBLIC_PROVIDER, "an-unlinked-access-token")),
            Some(UNMATCHED_LOGINS),
        ),
        (
            "a vend presenting the identity's own login",
            "GetCredentialsForIdentity",
            identity_id.as_str(),
            Some(logins(USER_POOL_PROVIDER, &own)),
            None,
        ),
        (
            "a vend presenting another identity's login",
            "GetCredentialsForIdentity",
            identity_id.as_str(),
            Some(logins(USER_POOL_PROVIDER, &other)),
            Some(UNMATCHED_LOGINS),
        ),
        (
            "a vend presenting no login at all for a federated identity",
            "GetCredentialsForIdentity",
            identity_id.as_str(),
            None,
            Some("forbidden"),
        ),
        (
            "a vend for a guest identity, which its id is all of",
            "GetCredentialsForIdentity",
            guest.as_str(),
            None,
            None,
        ),
        (
            "an unlink presenting an empty Logins map",
            "UnlinkIdentity",
            identity_id.as_str(),
            Some(json!({})),
            Some("forbidden"),
        ),
        (
            "an unlink presenting another identity's login",
            "UnlinkIdentity",
            identity_id.as_str(),
            Some(logins(USER_POOL_PROVIDER, &other)),
            Some(UNMATCHED_LOGINS),
        ),
        (
            "an unlink of an identity no login was ever linked to",
            "UnlinkIdentity",
            guest.as_str(),
            Some(logins(PUBLIC_PROVIDER, "an-unlinked-access-token")),
            Some(CANNOT_UNLINK_UNAUTHENTICATED),
        ),
    ];

    for (case, operation, identity, presented, refusal) in &rows {
        let mut request = Map::new();
        request.insert("IdentityId".to_string(), json!(identity));
        if let Some(presented) = presented {
            request.insert("Logins".to_string(), presented.clone());
        }
        if *operation == "UnlinkIdentity" {
            request.insert("LoginsToRemove".to_string(), json!([USER_POOL_PROVIDER]));
        }
        let (status, body) = emulator.call(operation, &Value::Object(request)).await;
        match refusal {
            None => assert_eq!(
                status, 200,
                "case {case:?}: a call a login of the identity authorizes is served, got \
                 {status} with {body}"
            ),
            Some(message) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (REFUSED_STATUS, "NotAuthorizedException"),
                    "case {case:?}: expected ({REFUSED_STATUS}, NotAuthorizedException), got \
                     ({status}, {body})"
                );
                // The refusal of an identity the caller has not authorized itself for names
                // the identity the REQUEST named and nothing else of what the pool holds.
                let expected = if *message == "forbidden" {
                    format!("Access to Identity '{identity}' is forbidden.")
                } else {
                    (*message).to_string()
                };
                assert_eq!(
                    error_message(case, &body),
                    expected,
                    "case {case:?}: expected the fixed sentence {expected:?}, got {body}"
                );
            }
        }
    }

    // Every refusal above wrote nothing: each identity is still reached through its own
    // login, and the guest identity is still linked to none.
    for (case, identity, login) in [
        ("the identity the rows named", &identity_id, &own),
        ("the other identity", &other_identity, &other),
    ] {
        let resolved = emulator
            .identity(&pool_id, Some(logins(USER_POOL_PROVIDER, login)))
            .await;
        assert_eq!(
            &resolved, identity,
            "case {case:?}: expected the login to still resolve to {identity}, got {resolved}"
        );
    }
    let described = emulator
        .ok("DescribeIdentity", &json!({"IdentityId": &guest}))
        .await;
    assert_eq!(
        described["Logins"],
        json!([]),
        "the guest identity is still linked to no provider, got {described}"
    );

    // And the unauthenticated exchange itself: an identity no login is linked to is reached
    // by its id alone, so the vend for it is the unauthenticated one — which a pool that no
    // longer admits an unauthenticated identity refuses in the words that case has.
    let closed_pool = emulator.pool_with_roles("identity-access-closed", true).await;
    let closed_guest = emulator.identity(&closed_pool, None).await;
    emulator
        .ok(
            "UpdateIdentityPool",
            &json!({
                "IdentityPoolId": &closed_pool,
                "IdentityPoolName": "identity-access-closed",
                "AllowUnauthenticatedIdentities": false,
            }),
        )
        .await;
    let (status, body) = emulator
        .call(
            "GetCredentialsForIdentity",
            &json!({"IdentityId": &closed_guest}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (REFUSED_STATUS, "NotAuthorizedException"),
        "a vend presenting no login against a pool that admits none is refused, got \
         ({status}, {body})"
    );
    assert_eq!(
        error_message("the unauthenticated exchange of a closed pool", &body),
        UNAUTHENTICATED_ACCESS,
        "expected the fixed unauthenticated-access sentence, got {body}"
    );
}

/// RULE: `CustomRoleArn` substitutes one of the SEVERAL roles the identity provider's answer
/// offered — "the role to be assumed when multiple roles were received in the token from the
/// identity provider" — so only an exchange whose presented login is of a SAML provider or of
/// a provider the pool holds a `RoleMappings` entry for may name one; any other exchange that
/// names one is refused the bad parameter it sent.
#[tokio::test]
async fn a_custom_role_arn_is_only_for_a_provider_that_offered_several_roles() {
    let emulator = Emulator::new();
    let pool_id = pool_trusting_a_user_pool(&emulator, "custom-role-support").await;
    let role_arn = crate::identity_pool_flow::seeded_role_arn("custom-role-support", "auth");
    // A mapping for ONE of the providers the rows present, so the rows differ in whether the
    // login they present is of a mapped provider.
    emulator
        .ok(
            "SetIdentityPoolRoles",
            &json!({
                "IdentityPoolId": &pool_id,
                "Roles": {"authenticated": &role_arn, "unauthenticated": &role_arn},
                "RoleMappings": {
                    PUBLIC_PROVIDER: {
                        "Type": "Token",
                        "AmbiguousRoleResolution": "AuthenticatedRole",
                    },
                },
            }),
        )
        .await;

    // (case, the logins the exchange presents, whether it may name a CustomRoleArn)
    let rows: Vec<(&str, Option<Value>, bool)> = vec![
        (
            "a login of the provider the pool holds a mapping for",
            Some(logins(PUBLIC_PROVIDER, "a-facebook-access-token")),
            true,
        ),
        (
            "a login of a provider the pool holds no mapping for",
            Some(logins(
                USER_POOL_PROVIDER,
                &id_token_of(USER_POOL_PROVIDER, "a-user-pool-user"),
            )),
            false,
        ),
        ("no login at all", None, false),
    ];

    for (case, presented, supported) in &rows {
        let identity_id = emulator.identity(&pool_id, presented.clone()).await;
        let mut request = Map::new();
        request.insert("IdentityId".to_string(), json!(&identity_id));
        request.insert("CustomRoleArn".to_string(), json!(&role_arn));
        if let Some(presented) = presented {
            request.insert("Logins".to_string(), presented.clone());
        }
        let (status, body) = emulator
            .call("GetCredentialsForIdentity", &Value::Object(request))
            .await;
        if *supported {
            assert_eq!(
                status, 200,
                "case {case:?}: an exchange that may name a CustomRoleArn is served, got \
                 {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (REFUSED_STATUS, "InvalidParameterException"),
            "case {case:?}: expected ({REFUSED_STATUS}, InvalidParameterException), got \
             ({status}, {body})"
        );
        assert_eq!(
            error_message(case, &body),
            CUSTOM_ROLE_ARN_UNSUPPORTED,
            "case {case:?}: expected the fixed custom-role sentence, got {body}"
        );
    }
}

/// RULE: the logins `GetOpenIdTokenForDeveloperIdentity` links name ONE account, and a call
/// whose logins cannot all name one is refused as the link CONFLICT it is — a developer user
/// identifier the pool has registered against another identity than the one the request names
/// makes the link a merge of two stored identities, and an identity an identity PROVIDER's
/// login already proves is that provider's user, so a developer user not yet registered
/// anywhere is not linked into it — and `LookupDeveloperIdentity` resolves a
/// developer-identity mapping of the pool, so an identity it has registered none against
/// resolves nothing.
#[tokio::test]
async fn a_developer_login_links_only_logins_that_name_one_account() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";
    let pool_id =
        pool_with_developer_provider(&emulator, "developer-link", Some(developer_provider)).await;

    // A developer identity, an identity an identity provider's login proves, and a guest
    // identity: the three states the identity a request names can be in.
    let registered = emulator
        .ok(
            "GetOpenIdTokenForDeveloperIdentity",
            &token_request(&pool_id, &[(developer_provider, "the-backends-user")], None, None),
        )
        .await["IdentityId"]
        .as_str()
        .expect("GetOpenIdTokenForDeveloperIdentity returns an IdentityId")
        .to_string();
    let provider_proven = emulator
        .identity(
            &pool_id,
            Some(logins(PUBLIC_PROVIDER, "a-facebook-access-token")),
        )
        .await;
    let guest = emulator.identity(&pool_id, None).await;

    // (case, the identity named, the identifier presented, the refusal owed)
    type DeveloperLinkRow<'a> = (&'a str, &'a str, &'a str, Option<(&'a str, &'a str)>);
    let rows: Vec<DeveloperLinkRow<'_>> = vec![
        (
            "the identity the identifier is already registered against",
            registered.as_str(),
            "the-backends-user",
            None,
        ),
        (
            "an identifier registered against another identity",
            provider_proven.as_str(),
            "the-backends-user",
            Some(("ResourceConflictException", CANNOT_MERGE_IDENTITIES)),
        ),
        (
            "a new identifier, for an identity a provider's login proves",
            provider_proven.as_str(),
            "a-new-backend-user",
            Some(("ResourceConflictException", CANNOT_LINK_LOGINS)),
        ),
        (
            "a new identifier, for a guest identity",
            guest.as_str(),
            "another-new-backend-user",
            None,
        ),
    ];

    for (case, identity, identifier, refusal) in &rows {
        let (status, body) = emulator
            .call(
                "GetOpenIdTokenForDeveloperIdentity",
                &token_request(
                    &pool_id,
                    &[(developer_provider, identifier)],
                    Some(identity),
                    None,
                ),
            )
            .await;
        match refusal {
            None => assert_eq!(
                status, 200,
                "case {case:?}: a link of logins naming one account is served, got {status} \
                 with {body}"
            ),
            Some((code, message)) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (REFUSED_STATUS, *code),
                    "case {case:?}: expected ({REFUSED_STATUS}, {code}), got ({status}, {body})"
                );
                assert_eq!(
                    error_message(case, &body),
                    *message,
                    "case {case:?}: expected the fixed sentence {message:?}, got {body}"
                );
                // The refusal names no identity the caller did not send.
                assert!(
                    !error_message(case, &body).contains(registered.as_str()),
                    "case {case:?}: the refusal must not name the identity holding the \
                     identifier, got {body}"
                );
            }
        }
    }

    // And the lookup: the identity the pool registered an identifier against answers it,
    // while one it registered none against resolves no mapping at all. A second identity of
    // each kind, so what the rows above linked does not decide this.
    let unregistered_provider_proven = emulator
        .identity(
            &pool_id,
            Some(logins(PUBLIC_PROVIDER, "another-facebook-access-token")),
        )
        .await;
    let unregistered_guest = emulator.identity(&pool_id, None).await;
    let looked_up = emulator
        .ok(
            "LookupDeveloperIdentity",
            &json!({"IdentityPoolId": &pool_id, "IdentityId": &registered}),
        )
        .await;
    assert_eq!(
        looked_up["DeveloperUserIdentifierList"],
        json!(["the-backends-user"]),
        "the registered identifier is what the lookup answers, got {looked_up}"
    );
    for (case, identity) in [
        (
            "an identity a provider's login proves",
            &unregistered_provider_proven,
        ),
        ("a guest identity", &unregistered_guest),
    ] {
        let (status, body) = emulator
            .call(
                "LookupDeveloperIdentity",
                &json!({"IdentityPoolId": &pool_id, "IdentityId": identity}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (REFUSED_STATUS, "NotAuthorizedException"),
            "case {case:?}: expected ({REFUSED_STATUS}, NotAuthorizedException), got \
             ({status}, {body})"
        );
        assert_eq!(
            error_message(case, &body),
            NO_DEVELOPER_ACCESS,
            "case {case:?}: expected the fixed no-access sentence, got {body}"
        );
    }
}
