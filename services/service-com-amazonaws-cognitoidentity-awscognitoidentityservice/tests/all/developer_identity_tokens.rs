// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetOpenIdTokenForDeveloperIdentity`: the identity a BACKEND
//! authenticated user has in a stored pool, and the OpenID Connect token minted for it.
//!
//! Every case asserts a RULE over a table of inputs rather than one recorded call:
//!
//! * the `Logins` entry keyed by the POOL's own `DeveloperProviderName` names the
//!   developer user identifier, and that pair is one login of the pool — so the call is
//!   idempotent per identifier (the same identifier resolves to the same identity, with a
//!   FRESH token each time), and the identity it mints or resolves is persisted and
//!   readable back through `ListIdentities` and `GetCredentialsForIdentity`;
//! * the token is a JWS in compact serialization whose claims come from that stored
//!   identity: the service's issuer, the pool as audience, the identity as subject, and a
//!   validity window of the requested `TokenDuration` (the documented 15 minutes when the
//!   request names none);
//! * an identifier already registered under ANOTHER identity, presented for an identity
//!   logins have been linked to, is refused (400 `NotAuthorizedException`) and changes
//!   nothing;
//! * `Logins` that names no developer provider of the pool is a bad input parameter (400
//!   `InvalidParameterException`) and changes nothing;
//! * a pool or an identity this service does not hold is the modelled
//!   `ResourceNotFoundException`.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::identity_pool_flow::{
    cognito_trust_policy, error_code, seeded_role_arn, Emulator, ABSENT_POOL_ID,
};

/// The operation under test, spelled once.
const OPERATION: &str = "GetOpenIdTokenForDeveloperIdentity";

/// The issuer of an Amazon Cognito Identity OIDC token, as this service's state module
/// names it (`crate::state::OIDC_TOKEN_ISSUER`) — asserted here against the literal so
/// the wire-visible value is pinned by the test, not by the code under test.
const ISSUER: &str = "https://cognito-identity.amazonaws.com";

/// The Cognito Identity service principal, which an issued token names among its
/// authentication methods.
const PRINCIPAL: &str = "cognito-identity.amazonaws.com";

/// What `TokenDuration`'s own documentation gives a token that does not name one: "If you
/// don't provide an expiration time, the token is valid for 15 minutes."
const DEFAULT_TOKEN_SECONDS: i64 = 900;

/// The shortest window a token held across several wire calls below may have and still be
/// spendable when they are over. A row asking for less mints a fresh token for the exchange
/// instead, so the exchange is decided by the rule under test and not by how loaded the test
/// host was.
const MIN_SPENDABLE_SECONDS: i64 = 60;

/// A well-formed `IdentityId` that nothing ever minted.
const ABSENT_IDENTITY_ID: &str = "us-east-1:22222222-2222-2222-2222-222222222222";

/// One `Logins` entry as a row carries it: the provider name and the token (for a
/// developer provider, the developer user identifier).
type Login<'a> = (&'a str, &'a str);

/// A row of the minting table: the case name, the pool's `DeveloperProviderName`, the
/// PUBLIC logins presented beside the developer one, and the `TokenDuration` (absent when
/// the row sends none).
type MintingCase<'a> = (&'a str, &'a str, Vec<Login<'a>>, Option<i64>);

/// A row of the invalid-`Logins` table: the case name, the pool's `DeveloperProviderName`
/// (absent for a pool that has none) and the `Logins` entries the call carries.
type InvalidLoginsCase<'a> = (&'a str, Option<&'a str>, Vec<Login<'a>>);

/// Create a pool, optionally with a `DeveloperProviderName`, and return its minted id.
/// `pub(crate)` so a sibling suite in this test target drives the same flow rather than a
/// second copy of this helper.
pub(crate) async fn pool_with_developer_provider(
    emulator: &Emulator,
    name: &str,
    developer_provider: Option<&str>,
) -> String {
    let mut request = json!({
        "IdentityPoolName": name,
        "AllowUnauthenticatedIdentities": true,
    });
    if let Some(developer_provider) = developer_provider {
        request["DeveloperProviderName"] = json!(developer_provider);
    }
    let created = emulator.ok("CreateIdentityPool", &request).await;
    created["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string()
}

/// The `GetOpenIdTokenForDeveloperIdentity` request for one case: `IdentityId` and
/// `TokenDuration` are sent only when the row carries them, so a row with `None`
/// exercises the member's absence. `pub(crate)` for the same reason as
/// [`pool_with_developer_provider`].
pub(crate) fn token_request(
    pool_id: &str,
    logins: &[Login<'_>],
    identity_id: Option<&str>,
    token_duration: Option<i64>,
) -> Value {
    let mut request = Map::new();
    request.insert("IdentityPoolId".to_string(), json!(pool_id));
    let logins: Map<String, Value> = logins
        .iter()
        .map(|(provider, login)| ((*provider).to_string(), json!(login)))
        .collect();
    request.insert("Logins".to_string(), Value::Object(logins));
    if let Some(identity_id) = identity_id {
        request.insert("IdentityId".to_string(), json!(identity_id));
    }
    if let Some(token_duration) = token_duration {
        request.insert("TokenDuration".to_string(), json!(token_duration));
    }
    Value::Object(request)
}

/// A base64url segment's bytes, decoded with the codec the wire serializer uses
/// (`aws_smithy_types::base64`): the two alphabet substitutions and the padding RFC 7515
/// §2 strips are restored first.
fn decode_base64url(case: &str, segment: &str) -> Vec<u8> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|error| panic!("case {case:?}: {segment:?} must be base64url ({error})"))
}

/// The header and the claims of a token in JWS compact serialization, with the form of the
/// serialization itself asserted: three segments, the third one EMPTY, as RFC 7515 §6
/// defines for a JWS that carries no signature. `pub(crate)` so every suite in this target
/// reads an issued token the same way.
pub(crate) fn jws_header_and_claims(case: &str, token: &str) -> (Value, Value) {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "case {case:?}: a compact JWS has three segments, got {token:?}"
    );
    assert!(
        segments[2].is_empty(),
        "case {case:?}: an unsecured JWS carries an empty signature segment, got {:?}",
        segments[2]
    );
    let header: Value = serde_json::from_slice(&decode_base64url(case, segments[0]))
        .unwrap_or_else(|error| panic!("case {case:?}: the header must be JSON ({error})"));
    let claims: Value = serde_json::from_slice(&decode_base64url(case, segments[1]))
        .unwrap_or_else(|error| panic!("case {case:?}: the claims must be JSON ({error})"));
    (header, claims)
}

/// The `Logins` provider names `ListIdentities` reports for one identity of a pool.
/// `pub(crate)` for the same reason as [`jws_header_and_claims`].
pub(crate) async fn listed_logins(
    emulator: &Emulator,
    pool_id: &str,
    identity_id: &str,
) -> Vec<String> {
    let listed = emulator
        .ok(
            "ListIdentities",
            &json!({"IdentityPoolId": pool_id, "MaxResults": 60}),
        )
        .await;
    let identities = listed["Identities"]
        .as_array()
        .expect("ListIdentities returns Identities")
        .clone();
    let found = identities
        .iter()
        .find(|identity| identity["IdentityId"] == json!(identity_id))
        .unwrap_or_else(|| panic!("ListIdentities must report identity {identity_id}: {listed}"));
    found["Logins"]
        .as_array()
        .map(|logins| {
            logins
                .iter()
                .map(|login| login.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// How many identities a pool holds. `pub(crate)` for the same reason as
/// [`pool_with_developer_provider`]: a sibling suite in this test target asks the same
/// question rather than keeping a second copy of it.
pub(crate) async fn identity_count(emulator: &Emulator, pool_id: &str) -> usize {
    let listed = emulator
        .ok(
            "ListIdentities",
            &json!({"IdentityPoolId": pool_id, "MaxResults": 60}),
        )
        .await;
    listed["Identities"]
        .as_array()
        .expect("ListIdentities returns Identities")
        .len()
}

/// RULE: the `Logins` entry keyed by the pool's `DeveloperProviderName` identifies a
/// backend-authenticated user; the call persists that user's identity in the pool, is
/// idempotent per developer user identifier, and answers a token minted from the stored
/// identity whose window is the requested `TokenDuration` (15 minutes by default).
#[tokio::test]
async fn a_developer_login_resolves_one_identity_and_mints_a_token_per_call() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: the developer provider's
    // spelling, how many PUBLIC logins are presented beside the developer one (the
    // implicit linked account), and the token duration — absent, and at each published
    // bound of the modelled `range 1..86400`.
    let cases: Vec<MintingCase<'_>> = vec![
        (
            "the developer provider alone, no TokenDuration",
            "login.example.app",
            vec![],
            None,
        ),
        (
            "a developer provider spelled with dashes, TokenDuration at its minimum",
            "my-app-auth.example.com",
            vec![],
            Some(1),
        ),
        (
            "a developer provider spelled with an underscore, one public login beside it",
            "my_app_auth",
            vec![("graph.facebook.com", "facebook-access-token")],
            Some(DEFAULT_TOKEN_SECONDS),
        ),
        (
            "two public logins beside the developer one, TokenDuration at its maximum",
            "backend.example.test",
            vec![
                ("accounts.google.com", "google-id-token"),
                ("www.amazon.com", "amazon-access-token"),
            ],
            Some(86_400),
        ),
        (
            "a developer provider whose name is a single character",
            "d",
            vec![("graph.facebook.com", "another-facebook-token")],
            Some(3_600),
        ),
    ];

    let mut minted_identities: BTreeSet<String> = BTreeSet::new();
    let mut minted_tokens: BTreeSet<String> = BTreeSet::new();

    for (case, developer_provider, public_logins, token_duration) in cases {
        let pool_id = pool_with_developer_provider(&emulator, case, Some(developer_provider)).await;
        // The credentials this case vends below are a session of the pool's role, so the
        // pool is given one that trusts its own users.
        let role_arn = emulator.seed_role(
            &seeded_role_arn(developer_provider, "unauth"),
            cognito_trust_policy(&pool_id),
        );
        emulator
            .ok(
                "SetIdentityPoolRoles",
                &json!({
                    "IdentityPoolId": &pool_id,
                    // Both roles, so the vend below — made with the identity's developer
                    // login, which is an AUTHENTICATED exchange — resolves one.
                    "Roles": {"authenticated": &role_arn, "unauthenticated": &role_arn},
                }),
            )
            .await;
        let developer_user_identifier = format!("backend-user-of-{developer_provider}");
        let mut logins: Vec<Login<'_>> = vec![(developer_provider, &developer_user_identifier)];
        logins.extend(public_logins.iter().copied());

        let answered = emulator
            .ok(
                OPERATION,
                &token_request(&pool_id, &logins, None, token_duration),
            )
            .await;

        let identity_id = answered["IdentityId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an IdentityId is answered: {answered}"))
            .to_string();
        let token = answered["Token"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a Token is answered: {answered}"))
            .to_string();

        // The identity is the pool's, in the pool's region, and nothing else's.
        let pool_region = pool_id.split(':').next().expect("a pool id names a region");
        assert!(
            identity_id.starts_with(&format!("{pool_region}:")),
            "case {case:?}: expected an identity in region {pool_region:?}, got {identity_id:?}"
        );
        assert!(
            minted_identities.insert(identity_id.clone()),
            "case {case:?}: identity {identity_id:?} was already handed to another pool"
        );

        let (header, claims) = jws_header_and_claims(case, &token);
        assert_eq!(
            header,
            json!({"alg": "none", "typ": "JWT"}),
            "case {case:?}: expected the unsecured-JWS header, got {header}"
        );
        assert_eq!(
            claims["iss"],
            json!(ISSUER),
            "case {case:?}: expected issuer {ISSUER}, got {claims}"
        );
        assert_eq!(
            claims["aud"],
            json!(pool_id),
            "case {case:?}: expected the pool as audience, got {claims}"
        );
        assert_eq!(
            claims["sub"],
            json!(identity_id),
            "case {case:?}: expected the identity as subject, got {claims}"
        );

        // The window is exactly what the request asked for, or the documented default.
        let issued_at = claims["iat"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: iat is a number: {claims}"));
        let expires_at = claims["exp"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: exp is a number: {claims}"));
        let expected_window = token_duration.unwrap_or(DEFAULT_TOKEN_SECONDS);
        assert_eq!(
            expires_at - issued_at,
            expected_window,
            "case {case:?}: expected a {expected_window}s window, got {claims}"
        );

        // The authentication methods name the identity as authenticated, by this service
        // and by every provider the call presented.
        let methods: Vec<String> = claims["amr"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: amr is a list: {claims}"))
            .iter()
            .map(|method| method.as_str().unwrap_or_default().to_string())
            .collect();
        for expected in [
            "authenticated".to_string(),
            PRINCIPAL.to_string(),
            developer_provider.to_string(),
        ]
        .into_iter()
        .chain(public_logins.iter().map(|(provider, _)| (*provider).to_string()))
        {
            assert!(
                methods.contains(&expected),
                "case {case:?}: expected amr to name {expected:?}, got {methods:?}"
            );
        }
        assert!(
            minted_tokens.insert(token.clone()),
            "case {case:?}: token {token:?} was issued twice"
        );

        // The identity is PERSISTED: a reader reports it, with every provider presented
        // as one of its logins, exactly once.
        let logins_read_back = listed_logins(&emulator, &pool_id, &identity_id).await;
        let mut expected_logins: Vec<String> = logins
            .iter()
            .map(|(provider, _)| (*provider).to_string())
            .collect();
        expected_logins.sort();
        let mut actual_logins = logins_read_back.clone();
        actual_logins.sort();
        assert_eq!(
            actual_logins, expected_logins,
            "case {case:?}: expected the identity's logins to be {expected_logins:?}, \
             got {actual_logins:?}"
        );
        assert_eq!(
            identity_count(&emulator, &pool_id).await,
            1,
            "case {case:?}: the call must mint ONE identity in its pool"
        );

        // Asked again for the SAME developer user identifier, the service answers the
        // same identity and a fresh token.
        let again = emulator
            .ok(
                OPERATION,
                &token_request(&pool_id, &logins, None, token_duration),
            )
            .await;
        assert_eq!(
            again["IdentityId"],
            json!(identity_id),
            "case {case:?}: the same developer user identifier must resolve to \
             {identity_id:?}, got {again}"
        );
        let second_token = again["Token"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a Token is answered: {again}"))
            .to_string();
        assert_ne!(
            second_token, token,
            "case {case:?}: each call mints its own token, got {second_token:?} twice"
        );
        assert!(
            minted_tokens.insert(second_token),
            "case {case:?}: a re-issued token must still be distinct"
        );
        assert_eq!(
            identity_count(&emulator, &pool_id).await,
            1,
            "case {case:?}: asking twice must not mint a second identity"
        );

        // And the identity it resolved is a real one: credentials can be vended for it,
        // presented with a token minted for that identity, which is what authorizes the
        // exchange for it.
        //
        // A row whose window is only seconds long mints a FRESH token for this call, under
        // the documented default window: the calls above are a mint, four reads and a
        // re-mint over the wire, which on a loaded host take longer than a one-second
        // window, so spending the first token here would fail this test for its timing
        // rather than for the rule it asserts. The window itself is asserted above, from
        // the claims, and that the exchange honours it is `login_token_window`'s rule.
        let spent = if token_duration.unwrap_or(DEFAULT_TOKEN_SECONDS) >= MIN_SPENDABLE_SECONDS {
            token.clone()
        } else {
            let reminted = emulator
                .ok(OPERATION, &token_request(&pool_id, &logins, None, None))
                .await;
            reminted["Token"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a Token is answered: {reminted}"))
                .to_string()
        };
        let credentials = emulator
            .ok(
                "GetCredentialsForIdentity",
                &json!({
                    "IdentityId": identity_id,
                    "Logins": { "cognito-identity.amazonaws.com": &spent },
                }),
            )
            .await;
        assert_eq!(
            credentials["IdentityId"],
            json!(identity_id),
            "case {case:?}: credentials belong to the identity asked for, got {credentials}"
        );
        assert!(
            credentials["Credentials"]["AccessKeyId"].is_string(),
            "case {case:?}: expected an AccessKeyId, got {credentials}"
        );
    }
}

/// RULE: a developer user identifier belongs to ONE identity, and this call links only
/// logins the pool has proven for no identity — so presenting an identifier already
/// registered under another identity for an identity logins have been linked to is refused
/// 400 `NotAuthorizedException`, naming the login mismatch rather than the identity that
/// holds it, and changes nothing.
#[tokio::test]
async fn a_developer_identifier_registered_under_another_identity_is_refused() {
    let emulator = Emulator::new();
    let developer_provider = "backend.example.app";

    // The rows differ in how the OTHER identity came to exist: a `GetId` carrying a public
    // login, and a second developer user identifier. An identity NO login has ever been
    // linked to is not one of them: nothing binds the call to it, so it is ignored rather
    // than refused (`developer_identity_resolution`).
    let cases: Vec<(&str, Option<Value>, Option<&str>)> = vec![
        (
            "an identity minted for a public login",
            Some(json!({"graph.facebook.com": "a-facebook-token"})),
            None,
        ),
        (
            "an identity registered for another developer user",
            None,
            Some("some-other-backend-user"),
        ),
    ];

    for (case, logins_for_other, other_developer_user) in cases {
        let pool_id =
            pool_with_developer_provider(&emulator, case, Some(developer_provider)).await;
        let identifier = "the-registered-backend-user";

        // The identifier is registered under its own, fresh identity.
        let registered = emulator
            .ok(
                OPERATION,
                &token_request(&pool_id, &[(developer_provider, identifier)], None, None),
            )
            .await;
        let registered_identity = registered["IdentityId"]
            .as_str()
            .expect("an IdentityId is answered")
            .to_string();

        // And another identity of the same pool exists beside it.
        let other_identity = match other_developer_user {
            Some(other) => {
                let answered = emulator
                    .ok(
                        OPERATION,
                        &token_request(&pool_id, &[(developer_provider, other)], None, None),
                    )
                    .await;
                answered["IdentityId"]
                    .as_str()
                    .expect("an IdentityId is answered")
                    .to_string()
            }
            None => emulator.identity(&pool_id, logins_for_other.clone()).await,
        };
        assert_ne!(
            other_identity, registered_identity,
            "case {case:?}: the fixture needs two distinct identities"
        );

        let before = identity_count(&emulator, &pool_id).await;
        let (status, body) = emulator
            .call(
                OPERATION,
                &token_request(
                    &pool_id,
                    &[(developer_provider, identifier)],
                    Some(&other_identity),
                    None,
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotAuthorizedException",
            "case {case:?}: expected NotAuthorizedException, got {body}"
        );
        assert!(
            !body["message"]
                .as_str()
                .unwrap_or_default()
                .contains(registered_identity.as_str()),
            "case {case:?}: the refusal must not name the identity holding the identifier, \
             got {body}"
        );
        assert_eq!(
            identity_count(&emulator, &pool_id).await,
            before,
            "case {case:?}: a refused call must change nothing"
        );

        // The identifier still resolves to the identity it was registered under.
        let still = emulator
            .ok(
                OPERATION,
                &token_request(&pool_id, &[(developer_provider, identifier)], None, None),
            )
            .await;
        assert_eq!(
            still["IdentityId"],
            json!(registered_identity),
            "case {case:?}: expected {registered_identity:?} to keep the identifier, got {still}"
        );
    }
}

/// RULE: `Logins` must carry the entry keyed by the POOL's own `DeveloperProviderName`;
/// one that does not is a bad input parameter (400 `InvalidParameterException`) and mints
/// nothing.
#[tokio::test]
async fn logins_that_name_no_developer_provider_of_the_pool_are_invalid() {
    let emulator = Emulator::new();

    // The rows differ along what can make a `Logins` map name no developer provider of the
    // pool: the pool has none; the map names a public provider instead; the map is empty
    // (the model admits `len 0..10`); the map names the provider with different case or
    // with surrounding space, neither of which is the stored key.
    let cases: Vec<InvalidLoginsCase<'_>> = vec![
        (
            "a pool with no DeveloperProviderName",
            None,
            vec![("login.example.app", "a-backend-user")],
        ),
        (
            "a map naming a public provider instead",
            Some("login.example.app"),
            vec![("graph.facebook.com", "a-facebook-token")],
        ),
        ("an empty map", Some("login.example.app"), vec![]),
        (
            "the developer provider in another case",
            Some("login.example.app"),
            vec![("Login.Example.App", "a-backend-user")],
        ),
        (
            "the developer provider with trailing space",
            Some("login.example.app"),
            vec![("login.example.app ", "a-backend-user")],
        ),
        (
            "a public provider beside a developer provider of ANOTHER pool",
            Some("login.example.app"),
            vec![
                ("accounts.google.com", "a-google-token"),
                ("some.other.pool.provider", "a-backend-user"),
            ],
        ),
    ];

    for (case, developer_provider, logins) in cases {
        let pool_id = pool_with_developer_provider(&emulator, case, developer_provider).await;
        let (status, body) = emulator
            .call(OPERATION, &token_request(&pool_id, &logins, None, None))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {body}"
        );
        assert_eq!(
            identity_count(&emulator, &pool_id).await,
            0,
            "case {case:?}: a refused call must mint no identity"
        );
    }
}

/// RULE: the call names two resources — the pool and, optionally, the identity — and one
/// this service does not hold is the modelled `ResourceNotFoundException`. An identity of
/// ANOTHER pool is a record this service DOES hold, so it is an ownership refusal instead
/// (`developer_identity_resolution`).
#[tokio::test]
async fn a_pool_or_identity_this_service_does_not_hold_is_not_found() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";
    let pool_id = pool_with_developer_provider(&emulator, "found", Some(developer_provider)).await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "a pool no CreateIdentityPool ever minted",
            token_request(
                ABSENT_POOL_ID,
                &[(developer_provider, "a-backend-user")],
                None,
                None,
            ),
        ),
        (
            "an identity nothing ever minted",
            token_request(
                &pool_id,
                &[(developer_provider, "a-backend-user")],
                Some(ABSENT_IDENTITY_ID),
                None,
            ),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = emulator.call(OPERATION, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }
    assert_eq!(
        identity_count(&emulator, &pool_id).await,
        0,
        "a refused call must mint no identity"
    );
}

/// RULE: naming an existing identity links the developer user identifier to THAT identity
/// — "you can do so by providing the existing IdentityId" — so the identifier resolves to
/// it afterwards, and the identity keeps the logins it already had. An identity an identity
/// PROVIDER's login already proves is that provider's user, so a developer user is not
/// joined to it this way: such a call is refused `ResourceConflictException`, and the
/// identity is left exactly as it was.
#[tokio::test]
async fn naming_an_existing_identity_links_the_developer_login_to_it() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";

    // The rows differ in what the existing identity already carries: nothing (the guest
    // identity the member exists for), another developer user identifier of the same
    // backend, and one and two logins of a public provider.
    // (case, the logins the identity carries, whether the link is served)
    let cases: Vec<(&str, Option<Value>, bool)> = vec![
        ("an unauthenticated identity", None, true),
        (
            "an identity already linked to one public provider",
            Some(json!({"graph.facebook.com": "a-facebook-token"})),
            false,
        ),
        (
            "an identity already linked to two public providers",
            Some(json!({
                "graph.facebook.com": "another-facebook-token",
                "accounts.google.com": "a-google-token",
            })),
            false,
        ),
    ];

    for (case, existing_logins, served) in cases {
        let pool_id =
            pool_with_developer_provider(&emulator, case, Some(developer_provider)).await;
        let existing = emulator.identity(&pool_id, existing_logins.clone()).await;
        let identifier = "a-backend-user";

        if !served {
            let (status, body) = emulator
                .call(
                    OPERATION,
                    &token_request(
                        &pool_id,
                        &[(developer_provider, identifier)],
                        Some(&existing),
                        None,
                    ),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a developer login joined to a \
                 provider-proven identity, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceConflictException",
                "case {case:?}: expected ResourceConflictException, got {body}"
            );
            // The refusal wrote nothing: the identity still carries exactly its own logins.
            let mut expected: Vec<String> = existing_logins
                .as_ref()
                .and_then(Value::as_object)
                .map(|logins| logins.keys().cloned().collect())
                .unwrap_or_default();
            expected.sort();
            let mut actual = listed_logins(&emulator, &pool_id, &existing).await;
            actual.sort();
            assert_eq!(
                actual, expected,
                "case {case:?}: a refused link leaves the logins {expected:?}, got {actual:?}"
            );
            continue;
        }

        let answered = emulator
            .ok(
                OPERATION,
                &token_request(
                    &pool_id,
                    &[(developer_provider, identifier)],
                    Some(&existing),
                    None,
                ),
            )
            .await;
        assert_eq!(
            answered["IdentityId"],
            json!(existing),
            "case {case:?}: expected the named identity {existing:?}, got {answered}"
        );

        // The identity now carries the developer provider beside what it already had.
        let logins = listed_logins(&emulator, &pool_id, &existing).await;
        let mut expected: Vec<String> = existing_logins
            .as_ref()
            .and_then(Value::as_object)
            .map(|logins| logins.keys().cloned().collect())
            .unwrap_or_default();
        expected.push(developer_provider.to_string());
        expected.sort();
        let mut actual = logins.clone();
        actual.sort();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the identity's logins to be {expected:?}, got {actual:?}"
        );

        // And the identifier resolves to it from then on, without naming it again.
        let resolved = emulator
            .ok(
                OPERATION,
                &token_request(&pool_id, &[(developer_provider, identifier)], None, None),
            )
            .await;
        assert_eq!(
            resolved["IdentityId"],
            json!(existing),
            "case {case:?}: expected the identifier to resolve to {existing:?}, got {resolved}"
        );
        assert_eq!(
            identity_count(&emulator, &pool_id).await,
            1,
            "case {case:?}: linking a login to an existing identity mints none"
        );
    }
}
