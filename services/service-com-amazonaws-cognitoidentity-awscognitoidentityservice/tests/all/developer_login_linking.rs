// SPDX-License-Identifier: CC0-1.0

//! WHICH identity `GetOpenIdTokenForDeveloperIdentity` links the logins of one call into,
//! and the only thing that stops the link.
//!
//! RULE: the logins one call presents name ONE account, so the call links every one of them
//! to the identity it is about — the one the request names when the call PROVES it (a
//! presented login is already one of that identity's, or no login has ever been linked to it),
//! else the one the presented developer user identifier is already registered under — and it
//! is refused only where a presented login is bound to a DIFFERENT identity than that one, or
//! where it would join a developer user to an identity another provider's login proves while
//! presenting no login of it. An `IdentityId` that names an identity nothing of the request
//! binds the call to is ignored rather than refused: the developer user identifier keeps
//! naming the identity it is registered under, and the identity the request named is left
//! untouched ("Supplying multiple logins will create an implicit linked account … When you
//! want to associate a new login with an existing authenticated/unauthenticated identity, you
//! can do so by providing the existing IdentityId",
//! `smithy_query(query="operation",operation="GetOpenIdTokenForDeveloperIdentity")`).
//!
//! The rows differ along what the rule generalises over: whether the request names an
//! `IdentityId` at all and what state that identity is in (a developer identity, an identity a
//! user-pool login proves, an identity a public provider's access token proves, an
//! unauthenticated guest), which logins the call presents beside the developer one and which
//! identity each is bound to, and whether the developer user identifier is new or already
//! registered.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Value};

use crate::developer_identity_tokens::token_request;
use crate::identity_pool_flow::{error_code, Emulator};
use crate::mapped_role_resolution::identity_token;

/// The status every refusal of this service travels with, whatever `@httpError` the shape it
/// answers is annotated with (`crate::service` owns that rule).
const REFUSED_STATUS: u16 = 400;

/// The pool's `DeveloperProviderName` — the "domain" its developer logins are made under.
const DEVELOPER_PROVIDER: &str = "login.example.backend";

/// The `Logins` provider name of the Amazon Cognito user pool the fixture pools trust: a user
/// pool is named `cognito-idp.<region>.amazonaws.com/<UserPoolId>`
/// (`smithy_query(query="doc",shape="GetIdInput",member="Logins")`), and its login is that
/// user pool's id token.
const USER_POOL_PROVIDER: &str = "cognito-idp.us-east-1.amazonaws.com/us-east-1_linking01";

/// A public provider of the fixture pools: one of the names `GetId$Logins` publishes, whose
/// login is an opaque OAuth access token rather than an identity token.
const PUBLIC_PROVIDER: &str = "graph.facebook.com";

/// The fixed sentences AWS words these two refusals with, spelled out here so the
/// wire-visible text is pinned by the test rather than read back out of the crate.
const CANNOT_LINK_LOGINS: &str = "Cannot link these logins.";
const CANNOT_MERGE_IDENTITIES: &str = "Cannot merge these identities.";

/// One `Logins` entry as a row carries it: the provider name and the login (for the developer
/// provider, the developer user identifier).
type Login<'a> = (&'a str, &'a str);

/// The `message` an error response carries.
fn error_message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an error response carries a message: {body}"))
        .to_string()
}

/// A pool carrying a `DeveloperProviderName` AND the identity providers whose logins the rows
/// present beside the developer one, with unauthenticated identities allowed so a guest
/// identity is one of the states a row can name.
async fn pool_with_developer_and_identity_providers(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": name,
                "AllowUnauthenticatedIdentities": true,
                "DeveloperProviderName": DEVELOPER_PROVIDER,
                "CognitoIdentityProviders": [{
                    "ProviderName": USER_POOL_PROVIDER,
                    "ClientId": "linkingclient",
                    "ServerSideTokenCheck": false,
                }],
                "SupportedLoginProviders": {PUBLIC_PROVIDER: "0123456789"},
            }),
        )
        .await;
    created["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string()
}

/// The id token the fixture user pool issues for `subject`.
fn user_pool_token(subject: &str) -> String {
    identity_token(&json!({
        "iss": format!("https://{USER_POOL_PROVIDER}"),
        "sub": subject,
        "aud": "linkingclient",
        "token_use": "id",
        "exp": 4_102_444_800u64,
    }))
}

/// The `IdentityId` a served `GetOpenIdTokenForDeveloperIdentity` answers, with its `Token`.
fn minted_identity(case: &str, body: &Value) -> String {
    let token = body["Token"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a served exchange answers a Token: {body}"));
    assert!(
        !token.is_empty(),
        "case {case:?}: the answered Token is not empty, got {body}"
    );
    body["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a served exchange answers an IdentityId: {body}"))
        .to_string()
}

/// The `Logins` list `DescribeIdentity` reports for one identity, sorted so the comparison is
/// over the SET of providers the identity is linked to.
async fn described_providers(emulator: &Emulator, case: &str, identity_id: &str) -> Vec<String> {
    let described = emulator
        .ok("DescribeIdentity", &json!({"IdentityId": identity_id}))
        .await;
    let mut providers: Vec<String> = described["Logins"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: DescribeIdentity answers a Logins list: {described}")
        })
        .iter()
        .map(|provider| {
            provider
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a Logins element is a string: {described}")
                })
                .to_string()
        })
        .collect();
    providers.sort();
    providers
}

/// The developer user identifiers the pool has registered against one identity, as
/// `LookupDeveloperIdentity` answers them.
async fn registered_identifiers(
    emulator: &Emulator,
    case: &str,
    pool_id: &str,
    identity_id: &str,
) -> Vec<String> {
    let looked_up = emulator
        .ok(
            "LookupDeveloperIdentity",
            &json!({"IdentityPoolId": pool_id, "IdentityId": identity_id}),
        )
        .await;
    looked_up["DeveloperUserIdentifierList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: LookupDeveloperIdentity answers a list: {looked_up}")
        })
        .iter()
        .map(|identifier| {
            identifier
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an identifier is a string: {looked_up}"))
                .to_string()
        })
        .collect()
}

/// The identity the pool resolves a developer user identifier to.
async fn resolved_identity(
    emulator: &Emulator,
    case: &str,
    pool_id: &str,
    identifier: &str,
) -> String {
    let looked_up = emulator
        .ok(
            "LookupDeveloperIdentity",
            &json!({"IdentityPoolId": pool_id, "DeveloperUserIdentifier": identifier}),
        )
        .await;
    looked_up["IdentityId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: LookupDeveloperIdentity answers an IdentityId: {looked_up}")
        })
        .to_string()
}

/// RULE: every call whose presented logins name ONE account is served — the identity it
/// answers is the one it proves, else the one the developer user identifier is registered
/// under — and every presented provider ends up linked to that identity.
#[tokio::test]
async fn a_developer_login_links_into_the_identity_the_call_names_or_proves() {
    let emulator = Emulator::new();
    let pool_id = pool_with_developer_and_identity_providers(&emulator, "developer-link").await;

    // The states the identity a request names can be in, each minted through the operation
    // that mints it.
    let backend_user = "the-backends-user";
    let canonical = minted_identity(
        "the registration of the developer user",
        &emulator
            .ok(
                "GetOpenIdTokenForDeveloperIdentity",
                &token_request(&pool_id, &[(DEVELOPER_PROVIDER, backend_user)], None, None),
            )
            .await,
    );
    let user_pool_login = user_pool_token("a-user-pool-user");
    let user_pool_identity = emulator
        .identity(&pool_id, Some(json!({USER_POOL_PROVIDER: &user_pool_login})))
        .await;
    let facebook_login = "a-facebook-access-token";
    let facebook_identity = emulator
        .identity(&pool_id, Some(json!({PUBLIC_PROVIDER: facebook_login})))
        .await;
    let guest = emulator.identity(&pool_id, None).await;

    // (case, the IdentityId sent, the logins presented, the identity owed, the providers that
    // identity must be linked to afterwards)
    type ServedRow<'a> = (
        &'a str,
        Option<&'a str>,
        Vec<Login<'a>>,
        &'a str,
        Vec<&'a str>,
    );
    let rows: Vec<ServedRow<'_>> = vec![
        (
            "a re-login naming no identity at all",
            None,
            vec![(DEVELOPER_PROVIDER, backend_user)],
            canonical.as_str(),
            vec![DEVELOPER_PROVIDER],
        ),
        (
            "a re-login naming the identity the identifier is registered under",
            Some(canonical.as_str()),
            vec![(DEVELOPER_PROVIDER, backend_user)],
            canonical.as_str(),
            vec![DEVELOPER_PROVIDER],
        ),
        (
            "a re-login naming an unrelated identity nothing of the request binds it to",
            Some(guest.as_str()),
            vec![(DEVELOPER_PROVIDER, backend_user)],
            canonical.as_str(),
            vec![DEVELOPER_PROVIDER],
        ),
        (
            "a new developer login onto the user-pool identity whose id token it presents",
            Some(user_pool_identity.as_str()),
            vec![
                (USER_POOL_PROVIDER, user_pool_login.as_str()),
                (DEVELOPER_PROVIDER, "the-user-pool-users-backend-id"),
            ],
            user_pool_identity.as_str(),
            vec![USER_POOL_PROVIDER, DEVELOPER_PROVIDER],
        ),
        (
            "a new developer login onto the public-provider identity whose token it presents",
            Some(facebook_identity.as_str()),
            vec![
                (PUBLIC_PROVIDER, facebook_login),
                (DEVELOPER_PROVIDER, "the-facebook-users-backend-id"),
            ],
            facebook_identity.as_str(),
            vec![PUBLIC_PROVIDER, DEVELOPER_PROVIDER],
        ),
        (
            "a new developer login onto a guest identity no login has been linked to",
            Some(guest.as_str()),
            vec![(DEVELOPER_PROVIDER, "the-guests-backend-id")],
            guest.as_str(),
            vec![DEVELOPER_PROVIDER],
        ),
    ];

    for (case, identity_id, presented, owed, linked) in &rows {
        // What the identity the row NAMES carried before the call, so a row whose named
        // identity is ignored can be shown to leave it untouched.
        let named_before = match identity_id {
            Some(named) => Some(described_providers(&emulator, case, named).await),
            None => None,
        };
        let (status, body) = emulator
            .call(
                "GetOpenIdTokenForDeveloperIdentity",
                &token_request(&pool_id, presented, *identity_id, None),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a call whose logins name one account is served, got {status} with \
             {body}"
        );
        assert_eq!(
            minted_identity(case, &body).as_str(),
            *owed,
            "case {case:?}: expected the identity {owed:?}, got {body}"
        );
        let mut expected_providers: Vec<String> = linked
            .iter()
            .map(|provider| (*provider).to_string())
            .collect();
        expected_providers.sort();
        assert_eq!(
            described_providers(&emulator, case, owed).await,
            expected_providers,
            "case {case:?}: expected the identity linked to {expected_providers:?}"
        );
        // The developer user identifier the call presented is registered against the identity
        // it was served for, so the pool resolves it back to that identity.
        let identifier = presented
            .iter()
            .find(|(provider, _)| *provider == DEVELOPER_PROVIDER)
            .map(|(_, identifier)| *identifier)
            .expect("every row presents a developer login");
        assert_eq!(
            resolved_identity(&emulator, case, &pool_id, identifier)
                .await
                .as_str(),
            *owed,
            "case {case:?}: the identifier {identifier:?} resolves to the identity it was \
             registered against"
        );
        assert!(
            registered_identifiers(&emulator, case, &pool_id, owed)
                .await
                .contains(&identifier.to_string()),
            "case {case:?}: the identity carries the identifier {identifier:?} it was \
             registered with"
        );
        // An identity the call IGNORED keeps exactly the logins it had: nothing of the
        // request bound the call to it, so it took no part in it.
        if let (Some(named), Some(before)) = (identity_id, named_before) {
            if named != owed {
                assert_eq!(
                    described_providers(&emulator, case, named).await,
                    before,
                    "case {case:?}: the identity {named:?} the call ignored is left untouched"
                );
            }
        }
    }
}

/// RULE: the link is refused only where a presented login is bound to a DIFFERENT identity
/// than the one the call is about, or where it would join a developer user to an identity
/// another provider's login proves while presenting no login of that identity — and every
/// such refusal is the link CONFLICT it is, carrying the merge sentence where the request
/// names an identity and presents a developer user identifier the pool has already
/// registered (both sides of the link are identities the pool holds, so the link is a merge
/// of the two) and the link sentence where one of the two sides does not exist yet. A
/// refused call writes nothing.
#[tokio::test]
async fn a_login_bound_to_another_identity_is_what_refuses_the_link() {
    let emulator = Emulator::new();
    let pool_id = pool_with_developer_and_identity_providers(&emulator, "developer-conflict").await;

    let owner_user = "the-owners-backend-user";
    let owner = minted_identity(
        "the registration of the owning developer user",
        &emulator
            .ok(
                "GetOpenIdTokenForDeveloperIdentity",
                &token_request(&pool_id, &[(DEVELOPER_PROVIDER, owner_user)], None, None),
            )
            .await,
    );
    let user_pool_login = user_pool_token("another-user-pool-user");
    let provider_proven = emulator
        .identity(&pool_id, Some(json!({USER_POOL_PROVIDER: &user_pool_login})))
        .await;
    let guest = emulator.identity(&pool_id, None).await;

    // (case, the IdentityId sent, the logins presented, the error code owed, its sentence)
    type RefusedRow<'a> = (&'a str, Option<&'a str>, Vec<Login<'a>>, &'a str, &'a str);
    let rows: Vec<RefusedRow<'_>> = vec![
        (
            "an identifier registered against another identity, for an identity a provider's \
             login proves",
            Some(provider_proven.as_str()),
            vec![(DEVELOPER_PROVIDER, owner_user)],
            "ResourceConflictException",
            CANNOT_MERGE_IDENTITIES,
        ),
        (
            "an identifier registered against another identity, presented beside a login of \
             the identity named",
            Some(provider_proven.as_str()),
            vec![
                (USER_POOL_PROVIDER, user_pool_login.as_str()),
                (DEVELOPER_PROVIDER, owner_user),
            ],
            "ResourceConflictException",
            CANNOT_MERGE_IDENTITIES,
        ),
        (
            "a provider login bound to another identity, for a guest identity",
            Some(guest.as_str()),
            vec![
                (USER_POOL_PROVIDER, user_pool_login.as_str()),
                (DEVELOPER_PROVIDER, "a-brand-new-backend-user"),
            ],
            "ResourceConflictException",
            CANNOT_LINK_LOGINS,
        ),
        (
            "logins proving two different identities, with no identity named",
            None,
            vec![
                (USER_POOL_PROVIDER, user_pool_login.as_str()),
                (DEVELOPER_PROVIDER, owner_user),
            ],
            "ResourceConflictException",
            CANNOT_LINK_LOGINS,
        ),
        (
            "a new identifier for an identity a provider's login proves, presenting none of \
             its logins",
            Some(provider_proven.as_str()),
            vec![(DEVELOPER_PROVIDER, "another-brand-new-backend-user")],
            "ResourceConflictException",
            CANNOT_LINK_LOGINS,
        ),
    ];

    for (case, identity_id, presented, code, sentence) in &rows {
        let (status, body) = emulator
            .call(
                "GetOpenIdTokenForDeveloperIdentity",
                &token_request(&pool_id, presented, *identity_id, None),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (REFUSED_STATUS, *code),
            "case {case:?}: expected ({REFUSED_STATUS}, {code}), got ({status}, {body})"
        );
        assert_eq!(
            error_message(case, &body),
            *sentence,
            "case {case:?}: expected the fixed sentence {sentence:?}, got {body}"
        );
        // The refusal names no identity the caller did not send.
        assert!(
            !error_message(case, &body).contains(owner.as_str()),
            "case {case:?}: the refusal must not name the identity holding the identifier, got \
             {body}"
        );
    }

    // And a refused call wrote nothing: the identities the rows named carry the logins they
    // carried before, and the pool registered none of the new identifiers.
    for (case, identity_id, linked) in [
        (
            "the identity a provider's login proves",
            provider_proven.as_str(),
            vec![USER_POOL_PROVIDER.to_string()],
        ),
        ("the guest identity", guest.as_str(), Vec::new()),
        (
            "the identity the owning identifier is registered under",
            owner.as_str(),
            vec![DEVELOPER_PROVIDER.to_string()],
        ),
    ] {
        let mut expected = linked;
        expected.sort();
        assert_eq!(
            described_providers(&emulator, case, identity_id).await,
            expected,
            "case {case:?}: a refused call linked nothing, expected {expected:?}"
        );
    }
    assert_eq!(
        resolved_identity(&emulator, "the owning identifier", &pool_id, owner_user)
            .await
            .as_str(),
        owner.as_str(),
        "the owning identifier still resolves to the identity it was registered under"
    );
}
