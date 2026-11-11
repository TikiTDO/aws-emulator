// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHICH identity a developer-identity call resolves, and how the
//! refusals of that resolution are worded.
//!
//! Every case asserts a RULE over a table of inputs rather than one recorded call:
//!
//! * a `Logins` map carrying NO entry presents no login at all, which is a different
//!   missing input from a map that names a provider the pool's own developer provider is
//!   not — two refusals of the same class with two fixed sentences;
//! * an `IdentityId` no login has ever been linked to is named by its id alone, so nothing
//!   a request carries binds the call to it: a developer user identifier the pool has
//!   already registered keeps naming the identity it is registered under, which is the
//!   identity the call answers, and the identity the request named is left untouched. An
//!   identity logins HAVE been linked to is a federated user's, so a presented login
//!   proving another identity is the authorization failure instead;
//! * a pool-scoped request decides OWNERSHIP of the identity it names before resolving it
//!   in the named pool's scope, so an identity of another pool is refused as one the caller
//!   may not reach while an id no call ever minted is reported missing;
//! * the developer-identity lookup words its missing-member and pair-mismatch refusals with
//!   AWS's fixed sentences, which name neither value the request carried.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Value};

use crate::developer_identity_tokens::{identity_count, pool_with_developer_provider, token_request};
use crate::identity_pool_flow::{error_code, Emulator};

/// The operation that registers or retrieves a developer identity.
const EXCHANGE: &str = "GetOpenIdTokenForDeveloperIdentity";

/// Every refused call of this service answers HTTP 400, whatever `@httpError` the error
/// shape annotates (`src/service/modeled_error_status.rs`).
const REFUSED_STATUS: u16 = 400;

/// How AWS words a `Logins` map that carries no entry at all.
const NO_LOGIN_PROVIDER: &str = "Please specify at least one login provider";

/// How AWS words a `Logins` map whose developer provider is not the pool's own.
const UNSUPPORTED_DEVELOPER_PROVIDER: &str =
    "This identity pool does not support the specified developer provider.";

/// How AWS words a call presenting no valid login of the identity it names.
const UNMATCHED_LOGINS: &str = "Logins don't match. Please include at least one valid login \
                                for this identity or identity pool.";

/// How AWS words a developer-identity lookup that names neither member it resolves from.
const NO_IDENTITY_OR_DEVELOPER_USER: &str =
    "Please specify either an identity id or developer user identifier";

/// How AWS words a developer-user-identifier / `IdentityId` pair the pool does not link.
const MISMATCHED_PAIR: &str = "The provided user identifier and identity id do not match.";

/// How this operation's developer-identity refusals are worded for an identity the caller
/// may not resolve a mapping for.
const NO_DEVELOPER_ACCESS: &str = "You do not have access to this identity";

/// A public identity provider, used where a row needs an identity a PROVIDER's login
/// proves.
const PUBLIC_PROVIDER: &str = "graph.facebook.com";

/// A row of the invalid-`Logins` table: the case name, the pool's `DeveloperProviderName`
/// (absent for a pool configured with none), the `Logins` entries the call carries, and the
/// fixed sentence the refusal owes.
type InvalidLoginsRow<'a> = (&'a str, Option<&'a str>, Vec<(&'a str, &'a str)>, &'a str);

/// The `message` an error response carries.
fn error_message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an error response carries a message: {body}"))
        .to_string()
}

/// AWS's wording for an identity the caller may not reach, around the id the REQUEST
/// carried.
fn forbidden_identity(identity_id: &str) -> String {
    format!("Access to Identity '{identity_id}' is forbidden.")
}

/// AWS's wording for an identity this service holds no record of.
fn identity_not_found(identity_id: &str) -> String {
    format!("Identity '{identity_id}' not found.")
}

/// A well-formed `IdentityId` (`len 1..55, pattern ^[\w-]+:[0-9a-f-]+$`) that no call ever
/// minted.
const ABSENT_IDENTITY_ID: &str = "us-east-1:00000001-0000-0000-0000-000000000000";

/// Register `identifier` under the pool's developer provider and answer the identity it
/// resolves to.
async fn register(
    emulator: &Emulator,
    pool_id: &str,
    developer_provider: &str,
    identifier: &str,
) -> String {
    emulator
        .ok(
            EXCHANGE,
            &token_request(pool_id, &[(developer_provider, identifier)], None, None),
        )
        .await["IdentityId"]
        .as_str()
        .expect("the exchange answers an IdentityId")
        .to_string()
}

/// The `Logins` provider names `DescribeIdentity` reports for one identity of this
/// harness's emulator.
///
/// `iam_oidc_federation::described_logins` asks the same question, but of ITS OWN harness
/// (`iam_oidc_federation::Emulator`), which is a different type from the one every suite
/// here drives, so it cannot be called with this emulator.
async fn described_logins(emulator: &Emulator, case: &str, identity_id: &str) -> Vec<String> {
    let described = emulator
        .ok("DescribeIdentity", &json!({"IdentityId": identity_id}))
        .await;
    described["Logins"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: an identity carries Logins: {described}"))
        .iter()
        .map(|login| login.as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE: a `Logins` map carrying NO entry presents no login to register, which is refused
/// as the missing input it is; a map that DOES name a provider and names one the pool's own
/// developer provider is not is the other refusal of the same class. Both are
/// `InvalidParameterException` with AWS's own fixed sentence for the condition, and neither
/// mints anything.
#[tokio::test]
async fn an_empty_logins_map_and_a_foreign_provider_are_two_fixed_sentences() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";

    // The rows differ along what the rule generalises over: whether the map carries an
    // entry at all, whether the pool names a developer provider, and HOW a present entry
    // differs from the pool's own (another domain, another case, a public provider, a
    // provider name at the modelled `len 1..128` maximum).
    let at_max_provider = "p".repeat(128);
    // (case, the pool's DeveloperProviderName, the Logins entries, the message owed)
    let rows: Vec<InvalidLoginsRow<'_>> = vec![
        (
            "no entry at all, for a pool with a developer provider",
            Some(developer_provider),
            vec![],
            NO_LOGIN_PROVIDER,
        ),
        (
            "no entry at all, for a pool with none",
            None,
            vec![],
            NO_LOGIN_PROVIDER,
        ),
        (
            "an entry naming another developer domain",
            Some(developer_provider),
            vec![("login.other.app", "a-backend-user")],
            UNSUPPORTED_DEVELOPER_PROVIDER,
        ),
        (
            "an entry naming the developer provider in another case",
            Some(developer_provider),
            vec![("Login.Example.App", "a-backend-user")],
            UNSUPPORTED_DEVELOPER_PROVIDER,
        ),
        (
            "an entry naming a public provider",
            Some(developer_provider),
            vec![(PUBLIC_PROVIDER, "a-facebook-access-token")],
            UNSUPPORTED_DEVELOPER_PROVIDER,
        ),
        (
            "an entry at the modelled maximum provider length",
            Some(developer_provider),
            vec![(at_max_provider.as_str(), "a-backend-user")],
            UNSUPPORTED_DEVELOPER_PROVIDER,
        ),
        (
            "an entry naming a provider, for a pool with none",
            None,
            vec![(developer_provider, "a-backend-user")],
            UNSUPPORTED_DEVELOPER_PROVIDER,
        ),
    ];

    for (row, (case, pool_provider, logins, expected)) in rows.iter().enumerate() {
        let pool_id =
            pool_with_developer_provider(&emulator, &format!("logins-{row}"), *pool_provider).await;
        let (status, body) = emulator
            .call(EXCHANGE, &token_request(&pool_id, logins, None, None))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (REFUSED_STATUS, "InvalidParameterException"),
            "case {case:?}: expected ({REFUSED_STATUS}, InvalidParameterException), got \
             ({status}, {body})"
        );
        assert_eq!(
            error_message(case, &body),
            *expected,
            "case {case:?}: expected the fixed sentence {expected:?}, got {body}"
        );
        assert_eq!(
            identity_count(&emulator, &pool_id).await,
            0,
            "case {case:?}: a refused call must mint no identity"
        );
    }
}

/// RULE: an identity NO login has ever been linked to is named by its id alone, so nothing
/// the request carries binds the call to it — a developer user identifier the pool has
/// already registered keeps naming the identity it is registered under, and that identity
/// is the one the call answers, with the named identity left exactly as it was.
#[tokio::test]
async fn an_identity_no_login_binds_is_ignored_and_the_registered_identity_answers() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";

    // The rows differ along what the rule generalises over: whether the guest identity was
    // minted before or after the registration, the shape of the developer user identifier
    // (`len 1..1024`: a plain one, the shortest and the longest the model admits), and how
    // many guest identities of the pool there are to name.
    let longest_identifier = "u".repeat(1024);
    // (case, the identifier registered, whether the guest exists before the registration)
    let rows: Vec<(&str, &str, bool)> = vec![
        ("a guest minted after the registration", "backend-user-1", false),
        ("a guest minted before the registration", "backend-user-2", true),
        ("an identifier at the shortest admitted length", "u", false),
        (
            "an identifier at the modelled maximum length",
            longest_identifier.as_str(),
            true,
        ),
    ];

    for (row, (case, identifier, guest_first)) in rows.iter().enumerate() {
        let pool_id =
            pool_with_developer_provider(&emulator, &format!("unbound-{row}"), Some(developer_provider))
                .await;
        let guest_before = if *guest_first {
            Some(emulator.identity(&pool_id, None).await)
        } else {
            None
        };
        let registered = register(&emulator, &pool_id, developer_provider, identifier).await;
        let guest = match guest_before {
            Some(guest) => guest,
            None => emulator.identity(&pool_id, None).await,
        };
        assert_ne!(
            guest, registered,
            "case {case:?}: the fixture needs two distinct identities"
        );

        let before = identity_count(&emulator, &pool_id).await;
        let answered = emulator
            .ok(
                EXCHANGE,
                &token_request(
                    &pool_id,
                    &[(developer_provider, identifier)],
                    Some(&guest),
                    None,
                ),
            )
            .await;
        assert_eq!(
            answered["IdentityId"],
            json!(registered),
            "case {case:?}: expected the identity holding the identifier ({registered:?}), got \
             {answered}"
        );
        assert!(
            answered["Token"].as_str().is_some_and(|token| !token.is_empty()),
            "case {case:?}: the served call mints a token, got {answered}"
        );

        // The identity the request named took no part in the call: it is still the guest it
        // was, carrying no login at all.
        let guest_logins = described_logins(&emulator, case, &guest).await;
        assert!(
            guest_logins.is_empty(),
            "case {case:?}: the named identity must be left untouched, got {guest_logins:?}"
        );
        // And the mapping still resolves to the identity it was registered under.
        let looked_up = emulator
            .ok(
                "LookupDeveloperIdentity",
                &json!({"IdentityPoolId": &pool_id, "DeveloperUserIdentifier": identifier}),
            )
            .await;
        assert_eq!(
            looked_up["IdentityId"],
            json!(registered),
            "case {case:?}: expected {registered:?} to keep the identifier, got {looked_up}"
        );
        assert_eq!(
            identity_count(&emulator, &pool_id).await,
            before,
            "case {case:?}: resolving an existing identity mints nothing"
        );
    }
}

/// RULE: an identity logins HAVE been linked to is a federated user's, so a presented login
/// the pool has proven for ANOTHER identity is not one of its own and the call is refused as
/// the authorization failure that is — the boundary of the rule above.
#[tokio::test]
async fn an_identity_a_login_binds_is_not_ignored_but_refused() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";
    let pool_id =
        pool_with_developer_provider(&emulator, "bound-identity", Some(developer_provider)).await;
    let identifier = "the-backends-user";
    let registered = register(&emulator, &pool_id, developer_provider, identifier).await;

    // The rows differ in WHAT login binds the identity the request names: a public
    // provider's, another developer user identifier of the same backend, and a public one
    // taken off again by an unlink (which leaves the identity reachable through no login at
    // all, and still not a guest).
    let provider_proven = emulator
        .identity(&pool_id, Some(json!({PUBLIC_PROVIDER: "a-facebook-token"})))
        .await;
    let other_developer_user =
        register(&emulator, &pool_id, developer_provider, "another-backend-user").await;
    let unlinked = emulator
        .identity(&pool_id, Some(json!({PUBLIC_PROVIDER: "a-second-facebook-token"})))
        .await;
    emulator
        .ok(
            "UnlinkIdentity",
            &json!({
                "IdentityId": &unlinked,
                "Logins": {PUBLIC_PROVIDER: "a-second-facebook-token"},
                "LoginsToRemove": [PUBLIC_PROVIDER],
            }),
        )
        .await;

    let rows: Vec<(&str, &str)> = vec![
        ("an identity a public provider's login proves", provider_proven.as_str()),
        ("an identity another developer user identifier proves", other_developer_user.as_str()),
        ("an identity an unlink left linked to nothing", unlinked.as_str()),
    ];

    for (case, named) in rows {
        let (status, body) = emulator
            .call(
                EXCHANGE,
                &token_request(
                    &pool_id,
                    &[(developer_provider, identifier)],
                    Some(named),
                    None,
                ),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (REFUSED_STATUS, "NotAuthorizedException"),
            "case {case:?}: expected ({REFUSED_STATUS}, NotAuthorizedException), got ({status}, \
             {body})"
        );
        assert_eq!(
            error_message(case, &body),
            UNMATCHED_LOGINS,
            "case {case:?}: expected the fixed login-mismatch sentence, got {body}"
        );
        assert!(
            !error_message(case, &body).contains(registered.as_str()),
            "case {case:?}: the refusal must not name the identity holding the identifier, got \
             {body}"
        );
    }
}

/// RULE: a pool-scoped request settles OWNERSHIP of the identity it names before resolving
/// it inside the named pool's scope — an identity of another pool is one the caller may not
/// reach (`NotAuthorizedException`, each operation's own fixed wording for that), while an
/// id no call ever minted is the resource this service does not hold
/// (`ResourceNotFoundException`).
#[tokio::test]
async fn an_identity_of_another_pool_is_forbidden_and_an_unminted_one_is_not_found() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";
    let pool_id =
        pool_with_developer_provider(&emulator, "scope-pool", Some(developer_provider)).await;
    let other_pool_id =
        pool_with_developer_provider(&emulator, "scope-other", Some(developer_provider)).await;
    let identifier = "scoped-backend-user";
    register(&emulator, &pool_id, developer_provider, identifier).await;
    // An identity of the OTHER pool, and one whose pool has since been deleted: both are
    // records this service holds and neither belongs to the pool the requests below name.
    let foreign = emulator.identity(&other_pool_id, None).await;
    let doomed_pool_id =
        pool_with_developer_provider(&emulator, "scope-doomed", Some(developer_provider)).await;
    let orphaned = emulator.identity(&doomed_pool_id, None).await;
    emulator
        .ok(
            "DeleteIdentityPool",
            &json!({"IdentityPoolId": &doomed_pool_id}),
        )
        .await;
    let absent = ABSENT_IDENTITY_ID.to_string();

    // (case, the identity named, the expected code, the expected message)
    let identities: Vec<(&str, &str, &str, String)> = vec![
        (
            "an identity of another pool",
            foreign.as_str(),
            "NotAuthorizedException",
            forbidden_identity(&foreign),
        ),
        (
            "an identity whose pool was deleted",
            orphaned.as_str(),
            "NotAuthorizedException",
            forbidden_identity(&orphaned),
        ),
        (
            "an id no call ever minted",
            absent.as_str(),
            "ResourceNotFoundException",
            identity_not_found(&absent),
        ),
    ];

    for (case, identity_id, code, expected) in &identities {
        // Each pool-scoped operation that names an identity beside its pool, with the
        // message it words an identity it may not resolve with.
        let requests: Vec<(&str, Value, String)> = vec![
            (
                EXCHANGE,
                token_request(
                    &pool_id,
                    &[(developer_provider, identifier)],
                    Some(identity_id),
                    None,
                ),
                expected.clone(),
            ),
            (
                "UnlinkDeveloperIdentity",
                json!({
                    "IdentityPoolId": &pool_id,
                    "IdentityId": identity_id,
                    "DeveloperProviderName": developer_provider,
                    "DeveloperUserIdentifier": identifier,
                }),
                expected.clone(),
            ),
            (
                "LookupDeveloperIdentity",
                json!({"IdentityPoolId": &pool_id, "IdentityId": identity_id}),
                if *code == "NotAuthorizedException" {
                    NO_DEVELOPER_ACCESS.to_string()
                } else {
                    expected.clone()
                },
            ),
        ];
        for (operation, request, expected_message) in requests {
            let row = format!("{operation} on {case}");
            let (status, body) = emulator.call(operation, &request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (REFUSED_STATUS, *code),
                "case {row:?}: expected ({REFUSED_STATUS}, {code}), got ({status}, {body})"
            );
            assert_eq!(
                error_message(&row, &body),
                expected_message,
                "case {row:?}: expected {expected_message:?}, got {body}"
            );
        }
    }

    // The mapping is untouched by every refusal above: the identifier still resolves to the
    // identity of this pool it was registered under.
    let looked_up = emulator
        .ok(
            "LookupDeveloperIdentity",
            &json!({"IdentityPoolId": &pool_id, "DeveloperUserIdentifier": identifier}),
        )
        .await;
    assert_eq!(
        looked_up["DeveloperUserIdentifierList"],
        json!([identifier]),
        "the refusals must leave the mapping registered, got {looked_up}"
    );
}

/// RULE: the developer-identity lookup words its two input refusals with AWS's fixed
/// sentences — a request naming NEITHER member it resolves from is a bad parameter, and a
/// pair the pool's mappings do not link is a conflict — and neither sentence names a value
/// the request carried.
#[tokio::test]
async fn the_lookup_words_its_missing_member_and_pair_refusals_as_aws_does() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";
    let pool_id =
        pool_with_developer_provider(&emulator, "lookup-wording", Some(developer_provider)).await;
    let registered = register(&emulator, &pool_id, developer_provider, "user-a").await;
    let other = register(&emulator, &pool_id, developer_provider, "user-b").await;
    let guest = emulator.identity(&pool_id, None).await;

    // The rows differ along what the rule generalises over: for the missing-member refusal,
    // what else the request carries (nothing, and each published `MaxResults` bound of
    // `range 1..60`); for the pair refusal, HOW the pair fails to match (the identifier is
    // another identity's, the identity is another identifier's, the identifier is not
    // registered at all, the identity holds no developer mapping at all).
    // (case, request, expected code, expected message)
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "neither member, and nothing else",
            json!({"IdentityPoolId": &pool_id}),
            "InvalidParameterException",
            NO_IDENTITY_OR_DEVELOPER_USER,
        ),
        (
            "neither member, with MaxResults at its published minimum",
            json!({"IdentityPoolId": &pool_id, "MaxResults": 1}),
            "InvalidParameterException",
            NO_IDENTITY_OR_DEVELOPER_USER,
        ),
        (
            "neither member, with MaxResults at its published maximum",
            json!({"IdentityPoolId": &pool_id, "MaxResults": 60}),
            "InvalidParameterException",
            NO_IDENTITY_OR_DEVELOPER_USER,
        ),
        (
            "an identifier registered under another identity",
            json!({
                "IdentityPoolId": &pool_id,
                "IdentityId": &other,
                "DeveloperUserIdentifier": "user-a",
            }),
            "ResourceConflictException",
            MISMATCHED_PAIR,
        ),
        (
            "an identity registered under another identifier",
            json!({
                "IdentityPoolId": &pool_id,
                "IdentityId": &registered,
                "DeveloperUserIdentifier": "user-b",
            }),
            "ResourceConflictException",
            MISMATCHED_PAIR,
        ),
        (
            "an identifier this pool has not registered",
            json!({
                "IdentityPoolId": &pool_id,
                "IdentityId": &registered,
                "DeveloperUserIdentifier": "never-registered",
            }),
            "ResourceConflictException",
            MISMATCHED_PAIR,
        ),
        (
            "an identity this pool holds no developer mapping for",
            json!({
                "IdentityPoolId": &pool_id,
                "IdentityId": &guest,
                "DeveloperUserIdentifier": "user-a",
            }),
            "ResourceConflictException",
            MISMATCHED_PAIR,
        ),
    ];

    for (case, request, code, expected) in rows {
        let (status, body) = emulator.call("LookupDeveloperIdentity", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (REFUSED_STATUS, code),
            "case {case:?}: expected ({REFUSED_STATUS}, {code}), got ({status}, {body})"
        );
        let message = error_message(case, &body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the fixed sentence {expected:?}, got {body}"
        );
        for named in [registered.as_str(), other.as_str(), guest.as_str()] {
            assert!(
                !message.contains(named),
                "case {case:?}: the refusal must name no identity of the pool, got {body}"
            );
        }
    }
}
