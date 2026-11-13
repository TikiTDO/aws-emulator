// SPDX-License-Identifier: CC0-1.0

//! How `GetOpenIdTokenForDeveloperIdentity` refuses an implicit link it cannot make.
//!
//! RULE: a call whose presented logins cannot all name ONE account is refused as the CONFLICT
//! between stored identities that is — the modelled `ResourceConflictException`, "Thrown when
//! a user tries to use a login which is already linked to another account"
//! (`smithy_query(query="doc",shape="ResourceConflictException")`), named in the body's
//! `__type` and in the `x-amzn-errortype` header — and never as a failure of the caller's
//! authority, which no login the caller could add would clear. Which fixed sentence it
//! carries follows how many of the link's two sides the REQUEST itself identifies: a request
//! that names an `IdentityId` AND presents a developer user identifier the pool has already
//! registered identifies both, so the link it asks for would have to fuse two stored
//! identities into one and is refused as the merge of them that is ("Cannot merge these
//! identities."); where one of the two sides is not identified — a developer user identifier
//! the pool has registered against nothing, or no `IdentityId` at all — there is no pair to
//! merge and what cannot be done is the LINK of the logins presented ("Cannot link these
//! logins."). Neither sentence has a request- or state-derived part, so neither names an
//! identity, a provider or a login, and a refused call writes nothing.
//!
//! The rows differ along what the rule generalises over: whether the request names an
//! `IdentityId` at all, what state the identity it names is in (a developer identity of the
//! same backend, the identifier's OWN identity, an identity a user-pool id token proves, an
//! identity a public provider's access token proves, an identity an unlink left linked to
//! nothing, an unauthenticated guest), which logins the call presents beside the developer
//! one and which identity each is bound to, and whether the developer user identifier is new
//! or already registered.
//!
//! The harness is `response_request_id`'s, because this rule is about a response HEADER as
//! well as a response body; it drives the same `MultiServiceRouter` the production router
//! builds over one shared engine.

use serde_json::{json, Value};

use crate::developer_identity_tokens::token_request;
use crate::mapped_role_resolution::identity_token;
use crate::response_request_id::{Caller, Emulator};

/// The operation under test, spelled once.
const OPERATION: &str = "GetOpenIdTokenForDeveloperIdentity";

/// The status every refusal of this service travels with, whatever `@httpError` the shape it
/// answers is annotated with (`crate::service` owns that rule).
const REFUSED_STATUS: u16 = 400;

/// The exception shape every refusal below names.
const CONFLICT: &str = "ResourceConflictException";

/// The header a failure names its exception shape in.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The pool's `DeveloperProviderName` — the "domain" its developer logins are made under.
const DEVELOPER_PROVIDER: &str = "login.example.conflicts";

/// The `Logins` provider name of the Amazon Cognito user pool the fixture pool trusts: a user
/// pool is named `cognito-idp.<region>.amazonaws.com/<UserPoolId>`
/// (`smithy_query(query="doc",shape="GetIdInput",member="Logins")`), and its login is that
/// user pool's id token.
const USER_POOL_PROVIDER: &str = "cognito-idp.us-east-1.amazonaws.com/us-east-1_conflict1";

/// The app client the fixture user pool's id tokens are issued to.
const CLIENT_ID: &str = "conflictclient";

/// A public provider of the fixture pool: one of the names `GetId$Logins` publishes, whose
/// login is an opaque OAuth access token rather than an identity token.
const PUBLIC_PROVIDER: &str = "graph.facebook.com";

/// The two fixed sentences AWS words these refusals with, spelled out here so the
/// wire-visible text is pinned by the test rather than read back out of the crate.
const CANNOT_MERGE_IDENTITIES: &str = "Cannot merge these identities.";
const CANNOT_LINK_LOGINS: &str = "Cannot link these logins.";

/// A pool carrying a `DeveloperProviderName` AND the identity providers whose logins the rows
/// present beside the developer one, with unauthenticated identities allowed so a guest
/// identity is one of the states a row can name.
async fn conflict_pool(emulator: &Emulator, name: &str) -> String {
    emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": name,
                "AllowUnauthenticatedIdentities": true,
                "DeveloperProviderName": DEVELOPER_PROVIDER,
                "CognitoIdentityProviders": [{
                    "ProviderName": USER_POOL_PROVIDER,
                    "ClientId": CLIENT_ID,
                    "ServerSideTokenCheck": false,
                }],
                "SupportedLoginProviders": {PUBLIC_PROVIDER: "0123456789"},
            }),
        )
        .await["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string()
}

/// The id token the fixture user pool issues for `subject`.
fn user_pool_token(subject: &str) -> String {
    identity_token(&json!({
        "iss": format!("https://{USER_POOL_PROVIDER}"),
        "sub": subject,
        "aud": CLIENT_ID,
        "token_use": "id",
        "exp": 4_102_444_800u64,
    }))
}

/// The identity `GetId` mints (or resolves) for the logins given, which is how an identity a
/// provider's login proves — and a guest identity, for `None` — comes to exist.
async fn federated_identity(emulator: &Emulator, pool_id: &str, logins: Option<Value>) -> String {
    let mut request = json!({"IdentityPoolId": pool_id});
    if let Some(logins) = logins {
        request["Logins"] = logins;
    }
    emulator.ok("GetId", &request).await["IdentityId"]
        .as_str()
        .expect("GetId returns an IdentityId")
        .to_string()
}

/// The identity the pool registers `identifier` under, through the operation under test.
async fn registered_identity(emulator: &Emulator, pool_id: &str, identifier: &str) -> String {
    emulator
        .ok(
            OPERATION,
            &token_request(pool_id, &[(DEVELOPER_PROVIDER, identifier)], None, None),
        )
        .await["IdentityId"]
        .as_str()
        .expect("a served exchange answers an IdentityId")
        .to_string()
}

/// The provider names `DescribeIdentity` reports for one identity.
async fn described_logins(emulator: &Emulator, identity_id: &str) -> Vec<String> {
    emulator
        .ok("DescribeIdentity", &json!({"IdentityId": identity_id}))
        .await["Logins"]
        .as_array()
        .map(|logins| {
            logins
                .iter()
                .map(|login| login.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// How many identities the pool holds.
async fn identity_count(emulator: &Emulator, pool_id: &str) -> usize {
    emulator
        .ok(
            "ListIdentities",
            &json!({"IdentityPoolId": pool_id, "MaxResults": 60}),
        )
        .await["Identities"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// The identity the pool resolves a developer user identifier to.
async fn resolved_identity(emulator: &Emulator, pool_id: &str, identifier: &str) -> String {
    emulator
        .ok(
            "LookupDeveloperIdentity",
            &json!({"IdentityPoolId": pool_id, "DeveloperUserIdentifier": identifier}),
        )
        .await["IdentityId"]
        .as_str()
        .expect("LookupDeveloperIdentity answers an IdentityId")
        .to_string()
}

/// One answered call of the operation under test: its status, its parsed body and the
/// exception shape its `x-amzn-errortype` header names.
async fn answer(
    emulator: &Emulator,
    request: &Value,
) -> (u16, Value, Option<String>) {
    let response = emulator.dispatch(Caller::Signed, OPERATION, request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get(ERROR_TYPE_HEADER)
        .map(|value| {
            value
                .to_str()
                .expect("the error-type header is ASCII")
                .to_string()
        });
    let bytes = response.into_body();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body, error_type)
}

/// The exception shape the body names (`__type`, on the awsJson protocols), reduced to the
/// bare shape name.
fn body_shape(case: &str, body: &Value) -> String {
    let raw = body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an error response names a __type: {body}"));
    let bare = raw.rsplit('#').next().unwrap_or(raw);
    bare.split(':').next().unwrap_or(bare).to_string()
}

/// The `message` an error response carries.
fn error_message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an error response carries a message: {body}"))
        .to_string()
}

/// RULE: every link this operation cannot make is the modelled link conflict, in the body and
/// in the header — carrying the merge sentence where the request identifies both sides of the
/// link, and the link sentence where it identifies one — and a refused call writes nothing.
#[tokio::test]
async fn a_link_that_cannot_be_made_is_refused_as_the_conflict_it_is() {
    let emulator = Emulator::new(false);
    let pool_id = conflict_pool(&emulator, "developer-link-conflicts").await;

    // The developer user identifiers the pool has registered, and the identities they are
    // registered under.
    let registered_user = "the-owners-backend-user";
    let registered = registered_identity(&emulator, &pool_id, registered_user).await;
    let other_registered =
        registered_identity(&emulator, &pool_id, "another-backend-user").await;

    // The identities a provider's login proves, a guest identity, and one an unlink left
    // reachable through no login at all: the states the identity a request names can be in.
    let user_pool_login = user_pool_token("a-user-pool-user");
    let idp_proven = federated_identity(
        &emulator,
        &pool_id,
        Some(json!({USER_POOL_PROVIDER: &user_pool_login})),
    )
    .await;
    let facebook_login = "a-facebook-access-token";
    let facebook_proven = federated_identity(
        &emulator,
        &pool_id,
        Some(json!({PUBLIC_PROVIDER: facebook_login})),
    )
    .await;
    let guest = federated_identity(&emulator, &pool_id, None).await;
    let unlinked_login = "a-second-facebook-access-token";
    let unlinked = federated_identity(
        &emulator,
        &pool_id,
        Some(json!({PUBLIC_PROVIDER: unlinked_login})),
    )
    .await;
    emulator
        .ok(
            "UnlinkIdentity",
            &json!({
                "IdentityId": &unlinked,
                "Logins": {PUBLIC_PROVIDER: unlinked_login},
                "LoginsToRemove": [PUBLIC_PROVIDER],
            }),
        )
        .await;

    // (case, the IdentityId sent, the logins presented, the sentence owed)
    type Row<'a> = (&'a str, Option<&'a str>, Vec<(&'a str, &'a str)>, &'a str);
    let rows: Vec<Row<'_>> = vec![
        // Both sides identified: the request names an identity and presents an identifier
        // the pool has already registered, so the link would fuse two stored identities.
        (
            "an identity a user-pool id token proves, with an identifier registered \
             elsewhere",
            Some(idp_proven.as_str()),
            vec![(DEVELOPER_PROVIDER, registered_user)],
            CANNOT_MERGE_IDENTITIES,
        ),
        (
            "that identity, with its own id token presented beside the registered identifier",
            Some(idp_proven.as_str()),
            vec![
                (USER_POOL_PROVIDER, user_pool_login.as_str()),
                (DEVELOPER_PROVIDER, registered_user),
            ],
            CANNOT_MERGE_IDENTITIES,
        ),
        (
            "the identifier's OWN identity, beside a login bound to another identity",
            Some(registered.as_str()),
            vec![
                (USER_POOL_PROVIDER, user_pool_login.as_str()),
                (DEVELOPER_PROVIDER, registered_user),
            ],
            CANNOT_MERGE_IDENTITIES,
        ),
        (
            "an identity a public provider's access token proves, with its own token beside \
             the registered identifier",
            Some(facebook_proven.as_str()),
            vec![
                (PUBLIC_PROVIDER, facebook_login),
                (DEVELOPER_PROVIDER, registered_user),
            ],
            CANNOT_MERGE_IDENTITIES,
        ),
        (
            "another developer identity of the same backend",
            Some(other_registered.as_str()),
            vec![(DEVELOPER_PROVIDER, registered_user)],
            CANNOT_MERGE_IDENTITIES,
        ),
        (
            "an identity an unlink left linked to nothing",
            Some(unlinked.as_str()),
            vec![(DEVELOPER_PROVIDER, registered_user)],
            CANNOT_MERGE_IDENTITIES,
        ),
        // One side unidentified: a brand-new developer user identifier, or no IdentityId at
        // all, so there is no pair of identities to merge.
        (
            "no identity named, a user-pool login proving one identity beside a new \
             identifier",
            None,
            vec![
                (USER_POOL_PROVIDER, user_pool_login.as_str()),
                (DEVELOPER_PROVIDER, "a-brand-new-backend-user"),
            ],
            CANNOT_LINK_LOGINS,
        ),
        (
            "no identity named, a user-pool login proving one identity beside an identifier \
             registered under another",
            None,
            vec![
                (USER_POOL_PROVIDER, user_pool_login.as_str()),
                (DEVELOPER_PROVIDER, registered_user),
            ],
            CANNOT_LINK_LOGINS,
        ),
        (
            "a guest identity, with a public login bound to another identity",
            Some(guest.as_str()),
            vec![
                (PUBLIC_PROVIDER, facebook_login),
                (DEVELOPER_PROVIDER, "a-second-brand-new-backend-user"),
            ],
            CANNOT_LINK_LOGINS,
        ),
        (
            "an identity a user-pool id token proves, with a new identifier and none of its \
             logins",
            Some(idp_proven.as_str()),
            vec![(DEVELOPER_PROVIDER, "a-third-brand-new-backend-user")],
            CANNOT_LINK_LOGINS,
        ),
    ];

    // What the pool held before the refusals, so they can be shown to have written nothing.
    let watched = [
        ("the identity the identifier is registered under", registered.as_str()),
        ("the other developer identity", other_registered.as_str()),
        ("the identity a user-pool id token proves", idp_proven.as_str()),
        ("the identity a public token proves", facebook_proven.as_str()),
        ("the guest identity", guest.as_str()),
        ("the identity an unlink left linked to nothing", unlinked.as_str()),
    ];
    let mut before: Vec<(&str, &str, Vec<String>)> = Vec::new();
    for (case, identity_id) in watched {
        before.push((case, identity_id, described_logins(&emulator, identity_id).await));
    }
    let identities_before = identity_count(&emulator, &pool_id).await;

    for (case, identity_id, presented, sentence) in &rows {
        let (status, body, error_type) = answer(
            &emulator,
            &token_request(&pool_id, presented, *identity_id, None),
        )
        .await;
        assert_eq!(
            (status, body_shape(case, &body).as_str()),
            (REFUSED_STATUS, CONFLICT),
            "case {case:?}: expected ({REFUSED_STATUS}, {CONFLICT}), got ({status}, {body})"
        );
        assert_eq!(
            error_type.as_deref(),
            Some(format!("{CONFLICT}:").as_str()),
            "case {case:?}: expected the header to name {CONFLICT}, got {error_type:?}"
        );
        assert_eq!(
            error_message(case, &body),
            *sentence,
            "case {case:?}: expected the fixed sentence {sentence:?}, got {body}"
        );
        // The sentences are fixed text: a refusal reports no identity of the pool, not even
        // one the caller named itself.
        let message = error_message(case, &body);
        for (watched_case, watched_identity, _) in &before {
            assert!(
                !message.contains(watched_identity),
                "case {case:?}: the refusal must name no identity, got {watched_case:?} in \
                 {body}"
            );
        }
    }

    // A refused call wrote nothing: every identity carries the logins it carried, the pool
    // holds the identities it held, and the registered identifier still resolves to its own.
    for (case, identity_id, logins) in &before {
        assert_eq!(
            &described_logins(&emulator, identity_id).await,
            logins,
            "case {case:?}: a refused call linked nothing, expected {logins:?}"
        );
    }
    assert_eq!(
        identity_count(&emulator, &pool_id).await,
        identities_before,
        "a refused call mints no identity, expected {identities_before} of them"
    );
    assert_eq!(
        resolved_identity(&emulator, &pool_id, registered_user).await,
        registered,
        "the registered identifier still resolves to the identity it was registered under"
    );

    // And the boundary of the rule: the same logins that cannot be linked into two accounts
    // ARE linked where they name one, so the conflict is decided by what the pool holds and
    // not by the shape of the request.
    let served = emulator
        .ok(
            OPERATION,
            &token_request(
                &pool_id,
                &[
                    (USER_POOL_PROVIDER, user_pool_login.as_str()),
                    (DEVELOPER_PROVIDER, "the-user-pool-users-backend-id"),
                ],
                Some(idp_proven.as_str()),
                None,
            ),
        )
        .await;
    assert_eq!(
        served["IdentityId"],
        json!(idp_proven),
        "a call presenting a login of the identity it names links into it, got {served}"
    );
    assert_eq!(
        resolved_identity(&emulator, &pool_id, "the-user-pool-users-backend-id").await,
        idp_proven,
        "the new identifier resolves to the identity it was registered against"
    );
}
