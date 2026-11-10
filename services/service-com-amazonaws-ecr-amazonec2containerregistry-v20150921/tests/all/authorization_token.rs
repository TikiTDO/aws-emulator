// SPDX-License-Identifier: CC0-1.0

//! `GetAuthorizationToken`, driven end to end over the wire.
//!
//! The rule under test: every call MINTS one `docker login` credential for the one
//! registry this emulator serves, reported as a single `authorizationData` entry whose
//! `authorizationToken` is the base64 of `AWS:<freshly minted password>`, whose
//! `expiresAt` is the call's instant plus the documented 12-hour validity, and whose
//! `proxyEndpoint` is the registry endpoint THIS EMULATOR serves — `proxyEndpoint` is
//! where `docker login` and every pull after it go, and this emulator is what answers the
//! registry API (`service/registry_v2.rs`), so an address naming AWS's own
//! `<account>.dkr.ecr.<region>.amazonaws.com` host would send the next client out of the
//! emulator entirely. `registryIds` is `@deprecated` — "specifying a
//! registry ID doesn't change the permissions scope of the authorization token" — so it
//! selects nothing: a request carrying any accepted value for it is answered exactly like
//! one carrying none.
//!
//! The class the rule generalises over is therefore the `registryIds` value (absent, this
//! registry's own id, another account's id, several ids, a repeated id, the modelled
//! maximum of ten) crossed with the Region the call is signed for, plus the state of the
//! registry the call is made against. Each row below is one member of that class, not a
//! recorded request; the recorded case (no `registryIds` at all) is the first row.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    call, emulator, ok, ok_in_region, ANNOUNCED_REGISTRY_URL, REGION, REGISTRY_ID,
};

/// The user half of the `user:password` pair the token decodes to, as the handler's
/// `AUTHORIZATION_TOKEN_USER` documents it from `AuthorizationData$authorizationToken`:
/// "When the string is decoded, it is presented in the format `user:password` for private
/// registry authentication using `docker login`".
const EXPECTED_USER: &str = "AWS";

/// The documented validity of an issued token, in seconds: `AuthorizationData$expiresAt`
/// — "Authorization tokens are valid for 12 hours."
const LIFETIME_SECONDS: f64 = 12.0 * 60.0 * 60.0;

/// How far `expiresAt` may sit from `now + 12h` and still be the same instant: the slack
/// covers only the wall-clock time the call itself takes.
const CLOCK_SLACK_SECONDS: f64 = 120.0;

/// The modelled member names of `AuthorizationData`, as the awsJson1_1 wire carries them.
const MEMBER_TOKEN: &str = "authorizationToken";
const MEMBER_EXPIRY: &str = "expiresAt";
const MEMBER_PROXY: &str = "proxyEndpoint";

/// An account id that is NOT this emulator's registry.
const OTHER_REGISTRY_ID: &str = "123456789012";

/// The registry URL an issued token must name: the endpoint this emulator serves the
/// registry at for the calling client (see the module docs). `crate::wire`'s calls are
/// addressed to `api.ecr.<region>.amazonaws.com`, a host that names a service and so is an
/// endpoint no registry client can use, which is why every row — whatever Region it is
/// signed for — is answered with the address this emulator is served at
/// ([`ANNOUNCED_REGISTRY_URL`]).
const EXPECTED_PROXY_ENDPOINT: &str = ANNOUNCED_REGISTRY_URL;

/// Ten distinct 12-digit account ids — the modelled maximum of
/// `GetAuthorizationTokenRegistryIdList` (`@length(min: 1, max: 10)`), every entry
/// matching `RegistryId`'s `^[0-9]{12}$`.
fn ten_registry_ids() -> Vec<String> {
    (0..10).map(|n| format!("{:012}", 900_000_000_000u64 + n)).collect()
}

/// Seconds since the Unix epoch, the unit `expiresAt` is carried in.
fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs_f64()
}

/// Calls `GetAuthorizationToken` with `request` against `region` and asserts everything
/// the rule says about one issued token, bar uniqueness (which only a set of them shows).
/// Answers the encoded token, so a caller can collect it.
///
/// The decoded password is a secret, so nothing here ever prints it — only its length.
async fn issue(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    request: &Value,
) -> String {
    let before = unix_now();
    let body = ok_in_region(router, case, region, "GetAuthorizationToken", request).await;
    let after = unix_now();

    // `GetAuthorizationTokenResponse$authorizationData` targets a LIST, and the one
    // registry this emulator serves is the one thing an entry can describe.
    let entries = body["authorizationData"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: expected authorizationData to be a list, got {body}")
    });
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: expected exactly one authorizationData entry, got {entries:?}"
    );
    let data = &entries[0];

    let members: BTreeSet<&str> = data
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected an AuthorizationData object, got {data}"))
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        members,
        BTreeSet::from([MEMBER_TOKEN, MEMBER_EXPIRY, MEMBER_PROXY]),
        "case {case:?}: expected authorizationData to carry exactly the modelled \
         members, got {members:?}"
    );

    let token = data[MEMBER_TOKEN]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string authorizationToken in {body}"))
        .to_string();
    // `Base64`'s modelled pattern is `^\S+$`: no whitespace anywhere, and never empty.
    assert!(
        !token.is_empty() && !token.chars().any(char::is_whitespace),
        "case {case:?}: expected a non-empty whitespace-free token, got one of length {}",
        token.len()
    );

    let decoded = ::aws_smithy_types::base64::decode(&token).unwrap_or_else(|error| {
        panic!("case {case:?}: expected a base64 authorizationToken, got {error}")
    });
    let decoded = String::from_utf8(decoded)
        .unwrap_or_else(|_| panic!("case {case:?}: expected the token to decode to text"));
    let (user, password) = decoded
        .split_once(':')
        .unwrap_or_else(|| panic!("case {case:?}: expected a `user:password` pair once decoded"));
    assert_eq!(
        user, EXPECTED_USER,
        "case {case:?}: expected the decoded user to be {EXPECTED_USER:?}, got {user:?}"
    );
    assert!(
        !password.is_empty() && !password.chars().any(char::is_whitespace),
        "case {case:?}: expected a non-empty whitespace-free password, got one of length {}",
        password.len()
    );

    let expires_at = data[MEMBER_EXPIRY]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a numeric expiresAt in {body}"));
    let earliest = before + LIFETIME_SECONDS - CLOCK_SLACK_SECONDS;
    let latest = after + LIFETIME_SECONDS + CLOCK_SLACK_SECONDS;
    assert!(
        (earliest..=latest).contains(&expires_at),
        "case {case:?}: expected expiresAt in {earliest}..={latest} \
         (the call's instant plus 12 hours), got {expires_at}"
    );

    let proxy_endpoint = data[MEMBER_PROXY]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string proxyEndpoint in {body}"));
    assert_eq!(
        proxy_endpoint,
        EXPECTED_PROXY_ENDPOINT,
        "case {case:?}: expected the registry URL this emulator serves, got \
         {proxy_endpoint:?}"
    );

    token
}

#[tokio::test]
async fn every_call_mints_one_aws_user_token_for_this_registry() {
    // (case, request body, Region the call is signed for). The rows differ along the
    // `registryIds` value — the only member the operation models — and along the Region,
    // whose endpoint host is what `proxyEndpoint` names.
    let rows: [(&str, Value, &str); 7] = [
        ("no registryIds", json!({}), REGION),
        ("no registryIds, another Region", json!({}), "us-east-1"),
        (
            "this registry's own id",
            json!({"registryIds": [REGISTRY_ID]}),
            REGION,
        ),
        (
            "another account's id",
            json!({"registryIds": [OTHER_REGISTRY_ID]}),
            "ap-southeast-2",
        ),
        (
            "two ids, this registry and another account",
            json!({"registryIds": [REGISTRY_ID, OTHER_REGISTRY_ID]}),
            "us-west-2",
        ),
        (
            "the same id twice",
            json!({"registryIds": [OTHER_REGISTRY_ID, OTHER_REGISTRY_ID]}),
            REGION,
        ),
        (
            "ten ids, the modelled maximum",
            json!({"registryIds": ten_registry_ids()}),
            "eu-central-1",
        ),
    ];

    // Across the whole table: an issued credential is never repeated, not within one
    // registry and not between two of them.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut issued = 0usize;

    for (case, request, region) in rows {
        // A fresh emulator per row, plus a second call on the same one: the registry's
        // state before the call is part of the class, and nothing an earlier call wrote
        // may change what a later one answers.
        let router = emulator();
        for round in 0..2 {
            let token = issue(&router, &format!("{case} (call {round})"), region, &request).await;
            assert!(
                seen.insert(token),
                "case {case:?}: call {round} repeated an already issued token"
            );
            issued += 1;
        }
    }

    assert_eq!(
        seen.len(),
        issued,
        "expected every issued token across the table to be distinct, got {} distinct \
         of {issued} issued",
        seen.len()
    );
}

#[tokio::test]
async fn the_registry_state_a_later_caller_sees_is_unchanged_by_issuing_a_token() {
    let router = emulator();
    ok(
        &router,
        "seed",
        "CreateRepository",
        &json!({"repositoryName": "team/app"}),
    )
    .await;

    let before = ok(&router, "before", "DescribeRepositories", &json!({})).await;
    for round in 0..3 {
        issue(
            &router,
            &format!("issuing token {round}"),
            REGION,
            &json!({}),
        )
        .await;
    }
    let after = ok(&router, "after", "DescribeRepositories", &json!({})).await;

    assert_eq!(
        before, after,
        "expected issuing an authorization token to leave the registry's repositories \
         untouched, got {before} then {after}"
    );
}

#[tokio::test]
async fn a_registry_ids_value_outside_its_modelled_constraints_is_refused() {
    // Each row violates a constraint the model declares for
    // `GetAuthorizationTokenRegistryIdList`: `@length(min: 1, max: 10)` on the list, and
    // `RegistryId`'s `^[0-9]{12}$` on each entry. Modelled constraints are refused before
    // the handler runs, as this operation's own `InvalidParameterException` ("The
    // specified parameter is invalid. Review the available parameters for the API
    // request.") — never with a token.
    let rows: [(&str, Value); 4] = [
        ("the empty list, one below the minimum", json!([])),
        (
            "eleven ids, one past the maximum",
            json!(ten_registry_ids()
                .into_iter()
                .chain(std::iter::once("111111111111".to_string()))
                .collect::<Vec<_>>()),
        ),
        ("an eleven-digit id", json!(["12345678901"])),
        ("an id that is not all digits", json!(["01234567890a"])),
    ];

    let router = emulator();
    for (case, registry_ids) in rows {
        let answer = call(
            &router,
            "GetAuthorizationToken",
            &json!({"registryIds": registry_ids}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException, got {:?}",
            answer.body
        );
        assert!(
            answer.body.get("authorizationData").is_none(),
            "case {case:?}: a refused call must not report authorization data, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn two_independent_registries_issue_different_tokens() {
    // The token is a credential this call mints, so two emulators that share nothing must
    // not hand out the same one.
    let first = issue(&emulator(), "first registry", REGION, &json!({})).await;
    let second = issue(&emulator(), "second registry", REGION, &json!({})).await;
    assert_ne!(
        first, second,
        "expected two registries to issue different tokens"
    );
}

/// The endpoint this emulator serves the registry at is what the issued token's registry
/// URL names, whatever Region the call is signed for: one endpoint serves them all here,
/// so this pins the answer across the published Region naming forms rather than one
/// recorded Region.
#[tokio::test]
async fn the_proxy_endpoint_names_the_registry_endpoint_this_emulator_serves() {
    // Regions spanning the naming forms AWS publishes: a classic three-part name, a
    // numbered one, a four-part one, and a non-`aws` partition's.
    let regions = ["us-east-1", "ap-southeast-2", "us-gov-west-1", "cn-north-1"];
    let router = emulator();
    for region in regions {
        let body = ok_in_region(
            &router,
            region,
            region,
            "GetAuthorizationToken",
            &json!({}),
        )
        .await;
        let endpoint = body["authorizationData"][0][MEMBER_PROXY]
            .as_str()
            .unwrap_or_else(|| panic!("case {region:?}: expected a string proxyEndpoint in {body}"));
        assert_eq!(
            endpoint,
            EXPECTED_PROXY_ENDPOINT,
            "case {region:?}: expected the registry URL this emulator serves, got \
             {endpoint:?}"
        );
    }
}
