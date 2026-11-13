// SPDX-License-Identifier: CC0-1.0

//! The pair `RegisterClient` answers with, as the registration's OWN credential.
//!
//! RULE: `RegisterClient` mints one registration per accepted call — its input names no
//! existing registration and carries no idempotency token (members `clientName`,
//! `clientType`, `scopes`, `redirectUris`, `grantTypes`, `issuerUrl`,
//! `entitledApplicationArn`, read with
//! `smithy_query(query="operation",operation="RegisterClient")`), and the secret is one
//! "generated for the client" that "the client will use to get authenticated by the
//! service in subsequent calls" — so the `clientId`/`clientSecret` the response carries
//! is that registration's own persisted credential: the response reports exactly what
//! the service stored for it, the pair goes on authenticating that registration for as
//! long as it is stored (authenticating with it neither rotates nor consumes it), no
//! other registration's secret authenticates it, and registering the same metadata
//! again mints a SEPARATE registration while leaving the earlier one and its pair
//! usable.
//!
//! Every case drives the service through the router the production emulator serves,
//! using the harness the device-flow suite beside this file builds.

use std::collections::HashSet;

use serde_json::{json, Value};

use service_com_amazonaws_ssooidc_awsssooidcservice as ssooidc;
use ssooidc::state::{GRANT_TYPE_DEVICE_CODE, GRANT_TYPE_REFRESH_TOKEN};

/// The fixed `error_description` of an `invalid_client` refusal whose
/// `clientSecret` is no usable secret of the registration its `clientId` names,
/// written out so the case pins the wording itself.
const INVALID_CLIENT_SECRET_TEXT: &str = "Invalid client secret provided";

/// The fixed `error_description` of an `invalid_client` refusal whose
/// `clientSecret` is some OTHER registration's own, so the `clientId` standing
/// beside it is the member at fault.
const INVALID_CLIENT_ID_TEXT: &str = "Invalid client ID provided";

use crate::device_authorization_flow::{
    assert_failure, device_flow_token, start_url, text, Emulator,
};

/// One registration's reported pair.
#[derive(Clone, Debug)]
struct Pair {
    client_id: String,
    client_secret: String,
}

/// Register `body` and report the pair the 200 carries.
async fn register(emulator: &Emulator, case: &str, body: &Value) -> Pair {
    let (status, response) = emulator.register_client(body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {response}"
    );
    Pair {
        client_id: text(&response, "clientId"),
        client_secret: text(&response, "clientSecret"),
    }
}

/// The secret the service PERSISTED for `client_id`, if it holds that registration.
fn stored_secret(emulator: &Emulator, client_id: &str) -> Option<String> {
    emulator
        .state()
        .lock()
        .clients
        .get(client_id)
        .map(|client| client.client_secret.clone())
}

/// Present `pair` to `StartDeviceAuthorization` and report the status and body: the
/// one operation that does nothing but authenticate the pair before it acts.
async fn authenticate(emulator: &Emulator, pair: &Pair) -> (u16, Value) {
    emulator
        .start_device_authorization(&json!({
            "clientId": pair.client_id,
            "clientSecret": pair.client_secret,
            "startUrl": start_url(),
        }))
        .await
}

/// The registration metadata the table spans: what the rule generalises over is the
/// metadata a registration carries — none beyond the required members, one grant, two
/// grants with scopes, redirect URIs with an issuer URL, and a name far longer than
/// any of them (`ClientName` is a plain string, no length or pattern constraint).
fn registrations() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a registration carrying only the required members",
            json!({"clientName": "aws-cli", "clientType": "public"}),
        ),
        (
            "a registration naming the device-code grant",
            json!({
                "clientName": "device-only",
                "clientType": "public",
                "grantTypes": [GRANT_TYPE_DEVICE_CODE],
            }),
        ),
        (
            "a registration naming two grants and a scope",
            json!({
                "clientName": "device-and-refresh",
                "clientType": "public",
                "grantTypes": [GRANT_TYPE_DEVICE_CODE, GRANT_TYPE_REFRESH_TOKEN],
                "scopes": ["sso:account:access"],
            }),
        ),
        (
            "a registration naming redirect URIs and an issuer URL",
            json!({
                "clientName": "pkce-client",
                "clientType": "public",
                "redirectUris": ["http://127.0.0.1:52000/oauth/callback"],
                "issuerUrl": "https://identitycenter.amazonaws.com/ssoins-1234567890abcdef",
            }),
        ),
        (
            "a registration with a name longer than any other row's",
            json!({"clientName": "a".repeat(512), "clientType": "public"}),
        ),
    ]
}

/// RULE (above): the reported pair is the registration's own persisted credential — the
/// response repeats what was stored, the pair keeps authenticating that registration,
/// and nothing but that registration's own secret does.
#[tokio::test]
async fn every_registration_is_reported_the_pair_the_service_persisted_for_it() {
    let emulator = Emulator::new();
    let mut seen_secrets: HashSet<String> = HashSet::new();

    for (case, body) in registrations() {
        let pair = register(&emulator, case, &body).await;
        assert!(
            seen_secrets.insert(pair.client_secret.clone()),
            "case {case:?}: a clientSecret was reported for a second registration"
        );

        // The member is not minted for the wire: it is what the store holds.
        assert_eq!(
            stored_secret(&emulator, &pair.client_id).as_deref(),
            Some(pair.client_secret.as_str()),
            "case {case:?}: the reported clientSecret is not the one persisted for {}",
            pair.client_id
        );

        // Authenticating with it is not what mints it, so it holds across calls.
        for attempt in 1..=3 {
            let (status, body) = authenticate(&emulator, &pair).await;
            assert_eq!(
                status, 200,
                "case {case:?}: attempt {attempt} with the registration's own pair was refused: {body}"
            );
            assert_eq!(
                stored_secret(&emulator, &pair.client_id).as_deref(),
                Some(pair.client_secret.as_str()),
                "case {case:?}: attempt {attempt} changed the stored secret"
            );
        }

        // A secret no registration carries is refused as the secret it is; the
        // `clientId` beside it is a registration this service holds.
        let fabricated = Pair {
            client_id: pair.client_id.clone(),
            client_secret: format!("{}-not-the-stored-secret", pair.client_secret),
        };
        let (status, response) = authenticate(&emulator, &fabricated).await;
        assert_failure(
            case,
            status,
            &response,
            401,
            "InvalidClientException",
            "invalid_client",
        );
        assert_eq!(
            response["error_description"].as_str(),
            Some(INVALID_CLIENT_SECRET_TEXT),
            "case {case:?}: expected the fixed text for a secret of no registration, got {response}"
        );
    }
}

/// RULE (above): the same metadata registered again is a SEPARATE registration — a
/// fresh pair, the earlier registration untouched and still authenticating, and
/// neither registration's secret authenticating the other's `clientId`.
#[tokio::test]
async fn registering_the_same_metadata_again_leaves_the_earlier_registration_intact() {
    let emulator = Emulator::new();

    for (case, body) in registrations() {
        let first = register(&emulator, case, &body).await;
        let second = register(&emulator, case, &body).await;

        assert_ne!(
            first.client_id, second.client_id,
            "case {case:?}: the second registration reused the first clientId {}",
            first.client_id
        );
        assert_ne!(
            first.client_secret, second.client_secret,
            "case {case:?}: the second registration of the same metadata reported the first secret"
        );

        // Both registrations are held, each with its own secret.
        for (which, pair) in [("first", &first), ("second", &second)] {
            assert_eq!(
                stored_secret(&emulator, &pair.client_id).as_deref(),
                Some(pair.client_secret.as_str()),
                "case {case:?}: the {which} registration's stored secret is not the reported one"
            );
            let (status, response) = authenticate(&emulator, pair).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the {which} registration's pair was refused: {response}"
            );
        }

        // A secret this service issued, presented beside the OTHER registration's
        // `clientId`, is refused for the id standing beside it.
        for (which, crossed) in [
            (
                "the first id with the second secret",
                Pair {
                    client_id: first.client_id.clone(),
                    client_secret: second.client_secret.clone(),
                },
            ),
            (
                "the second id with the first secret",
                Pair {
                    client_id: second.client_id.clone(),
                    client_secret: first.client_secret.clone(),
                },
            ),
        ] {
            let (status, response) = authenticate(&emulator, &crossed).await;
            assert_failure(
                &format!("{case}: {which}"),
                status,
                &response,
                401,
                "InvalidClientException",
                "invalid_client",
            );
            assert_eq!(
                response["error_description"].as_str(),
                Some(INVALID_CLIENT_ID_TEXT),
                "case {case:?}: {which}: expected the fixed text for the id beside another registration's secret, got {response}"
            );
        }
    }
}

/// RULE (above): the pair the response carries is the credential the whole flow runs
/// on — the device-code grant and the refresh-token grant both authenticate with it,
/// and it is still the registration's pair afterwards.
#[tokio::test]
async fn the_reported_pair_authenticates_the_grants_that_follow_the_registration() {
    let emulator = Emulator::new();

    for (case, body) in registrations() {
        let pair = register(&emulator, case, &body).await;
        let token = device_flow_token(&emulator, &pair.client_id, &pair.client_secret).await;
        assert!(
            !text(&token, "accessToken").is_empty(),
            "case {case:?}: expected an accessToken for the registration's own pair, got {token}"
        );

        // "This list is used to restrict the token granting flows available to the
        // client" (`RegisterClientRequest$grantTypes`), so a registration is handed
        // a refresh token exactly when it named that grant — or named no grant types
        // at all, which restricts nothing.
        let declares_refresh = body["grantTypes"].as_array().is_none_or(|grants| {
            grants.is_empty() || grants.iter().any(|grant| grant == GRANT_TYPE_REFRESH_TOKEN)
        });
        let refresh_token = token["refreshToken"].as_str().map(str::to_string);
        assert_eq!(
            refresh_token.is_some(),
            declares_refresh,
            "case {case:?}: expected a refreshToken beside the access token: {declares_refresh}, got {token}"
        );

        // The refresh token it was handed is rotated for the SAME pair, which
        // authenticates the refresh too.
        if let Some(refresh_token) = refresh_token {
            let (status, rotated) = emulator
                .create_token(&json!({
                    "clientId": pair.client_id,
                    "clientSecret": pair.client_secret,
                    "grantType": GRANT_TYPE_REFRESH_TOKEN,
                    "refreshToken": refresh_token,
                }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the refresh with the registration's own pair was refused: {rotated}"
            );
        }

        assert_eq!(
            stored_secret(&emulator, &pair.client_id).as_deref(),
            Some(pair.client_secret.as_str()),
            "case {case:?}: the flow changed the registration's stored secret"
        );
    }
}
