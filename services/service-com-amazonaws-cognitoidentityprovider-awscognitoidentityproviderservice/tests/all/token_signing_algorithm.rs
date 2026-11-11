// SPDX-License-Identifier: CC0-1.0

//! The ALGORITHM every token a user pool mints is signed under, over every
//! operation of this service that mints one.
//!
//! RULE: a user pool signs each JWS it issues with RSASSA-PKCS1-v1_5 over
//! SHA-256, so the token's JOSE header names `alg` `RS256` beside the `kid` of
//! the key it was signed with, and the pool publishes that key as an RSA JWK
//! (`kty` `RSA`, `alg` `RS256`, `use` `sig`, the `n`/`e` public parameters of
//! RFC 7518 §6.3.1) in the key set its issuer serves. That is the one thing
//! every published way of verifying such a token does: fetch the key set the
//! token's `iss` leads to, find the RSA key the `kid` names, and check the
//! signature under `RS256`.
//!
//! The rule is about the SIGNER, not about one operation, so the table spans
//! every path through this service that mints a token — the two password flows,
//! the legacy admin flow, both refresh exchanges, and a challenge answered
//! through each of the two respond operations — and runs each on two pools,
//! whose users differ in the claims their tokens carry (so the signed payload
//! differs row by row) and each of which signs with its own keys, one per token
//! type.
//!
//! Every row is verified twice from PUBLIC parameters only: once by `ring`'s
//! RSA verifier, a library that did not make the signature, over the key set as
//! SERVED at the pool's `/.well-known/jwks.json`, and once through
//! `service_common::jwt::verify`, the reading every emulated consumer of such a
//! token (a JWT authorizer, a policy store, a web-identity exchange) checks it
//! with. A tampered payload must fail both.

use std::sync::Arc;

use bytes::Bytes;
use ring::signature::{RsaPublicKeyComponents, RSA_PKCS1_2048_8192_SHA256};
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// This service's real regional endpoint host, which the suite's requests
/// address and the pools therefore issue on.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// A request's SigV4 credential scope is what routes it to this service; the
/// signature itself is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The temporary password an administrator-created user signs in with once,
/// which is what leaves it owing a `NEW_PASSWORD_REQUIRED` challenge.
const TEMPORARY_PASSWORD: &str = "Temp0rary!2026";

/// The password a challenge response sets, which completes the sign-in.
const REPLACEMENT_PASSWORD: &str = "Repl@cement2026";

/// The challenge an administrator-created user has to answer.
const NEW_PASSWORD_REQUIRED: &str = "NEW_PASSWORD_REQUIRED";

/// The one-time-password challenge of the choice-based flow, which the
/// caller-side respond operation answers.
const EMAIL_OTP: &str = "EMAIL_OTP";

/// The `alg` every token of a user pool carries, and every key it publishes:
/// RSASSA-PKCS1-v1_5 using SHA-256 (RFC 7518 §3.1 registers the name).
const EXPECTED_ALGORITHM: &str = "RS256";

/// The `kty` of the key family that algorithm is defined over (RFC 7518 §6.3).
const EXPECTED_KEY_TYPE: &str = "RSA";

/// The smallest modulus RFC 7518 §3.3 allows for `RS256` ("A key of size 2048
/// bits or larger MUST be used with these algorithms"), in octets.
const MINIMUM_MODULUS_BYTES: usize = 2048 / 8;

/// The public exponent an RSA key pair is generated with, big-endian: 65537,
/// which a JWK publishes as `AQAB`.
const PUBLIC_EXPONENT: [u8; 3] = [0x01, 0x00, 0x01];

/// The members of `AuthenticationResult` that carry a SIGNED token. The refresh
/// token beside them is an opaque credential that states no claim, so it is not
/// signed and not among these.
const TOKEN_MEMBERS: [&str; 2] = ["AccessToken", "IdToken"];

/// One emulator with just this service mounted, beside the engine its state
/// lives in.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.set_request_authentication(false);
    (router, engine)
}

/// The one-time password a challenge session delivered, read out of the
/// emulator's own state because nothing in the response carries it.
fn session_code(engine: &Arc<engine::InMemoryEngine>, session: &str) -> String {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .auth_sessions
        .get(session)
        .and_then(|stored| stored.code.as_ref())
        .map(|code| code.code.clone())
        .expect("the challenge delivered a code")
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", ENDPOINT_HOST)
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
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or_else(|error| {
        panic!(
            "{operation} answered a body that is not JSON ({error}): {:?}",
            String::from_utf8_lossy(response.body())
        )
    });
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The key set the pool publishes, fetched the way a verifier does: an
/// ANONYMOUS GET of `/<UserPoolId>/.well-known/jwks.json` on the endpoint the
/// token's `iss` claim names.
async fn served_jwks(router: &MultiServiceRouter, user_pool_id: &str) -> Value {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/{user_pool_id}/.well-known/jwks.json"))
        .header("host", ENDPOINT_HOST)
        .body(Bytes::new())
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    assert_eq!(
        status,
        200,
        "the pool {user_pool_id} must serve its key set, got {status} with {:?}",
        String::from_utf8_lossy(response.body())
    );
    serde_json::from_slice(response.body()).unwrap_or_else(|error| {
        panic!(
            "the key set must be JSON ({error}): {:?}",
            String::from_utf8_lossy(response.body())
        )
    })
}

/// A base64url segment's bytes, or `None` when the value is not base64url: the
/// decoding `service_common::jwt` performs, which is the one coding every JOSE
/// value travels in.
fn decode(segment: &str) -> Option<Vec<u8>> {
    service_common::jwt::decode_base64url(segment)
}

/// A JWK member read as the big-endian integer RFC 7518 §6.3.1 publishes.
fn integer_member(case: &str, jwk: &Value, member: &str) -> Vec<u8> {
    let encoded = jwk[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the published JWK carries {member}, got {jwk}"));
    let bytes = decode(encoded)
        .unwrap_or_else(|| panic!("case {case:?}: the JWK's {member} must be base64url, got {jwk}"));
    assert!(
        !bytes.is_empty() && bytes[0] != 0,
        "case {case:?}: the JWK's {member} is an unsigned big-endian integer with no leading \
         zero octet, got {bytes:?}"
    );
    bytes
}

/// One pool and the fixture a row signs in with.
struct Pool {
    user_pool_id: String,
    client_id: String,
    /// The issuer this pool's tokens claim, which is the endpoint the suite's
    /// requests reach it on with the pool's id appended.
    issuer: String,
}

/// Create a pool, one app client allowing every flow this suite drives, a
/// confirmed user, two administrator-created users owing a challenge, and the
/// groups and email attributes the row's claims come from.
async fn pool_with_users(
    router: &MultiServiceRouter,
    pool_name: &str,
    groups: &[&str],
    email: Option<&str>,
) -> Pool {
    let created = ok(
        router,
        "CreateUserPool",
        &json!({
            "PoolName": pool_name,
            // The choice-based flow the one-time-password challenge belongs to.
            "Policies": {
                "SignInPolicy": { "AllowedFirstAuthFactors": ["PASSWORD", "EMAIL_OTP"] }
            },
        }),
    )
    .await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": format!("{pool_name}-client"),
            "ExplicitAuthFlows": [
                "ALLOW_USER_PASSWORD_AUTH",
                "ALLOW_ADMIN_USER_PASSWORD_AUTH",
                "ALLOW_REFRESH_TOKEN_AUTH",
                "ALLOW_USER_AUTH",
            ],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();

    let attributes: Vec<Value> = email
        .map(|email| vec![json!({ "Name": "email", "Value": email })])
        .unwrap_or_default();
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "signer",
            "UserAttributes": attributes,
        }),
    )
    .await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "signer",
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    for group in groups {
        ok(
            router,
            "CreateGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": group }),
        )
        .await;
        ok(
            router,
            "AdminAddUserToGroup",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": "signer",
                "GroupName": group,
            }),
        )
        .await;
    }
    // A user that owes a password challenge, answered through the admin respond
    // operation, and one that owes the one-time-password challenge the
    // caller-side respond operation answers.
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "admin-challenged",
            "TemporaryPassword": TEMPORARY_PASSWORD,
        }),
    )
    .await;
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "otp-challenged",
            "UserAttributes": [
                { "Name": "email", "Value": "otp@example.test" },
                { "Name": "email_verified", "Value": "true" },
            ],
        }),
    )
    .await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "otp-challenged",
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    Pool {
        issuer: format!("https://{ENDPOINT_HOST}/{user_pool_id}"),
        user_pool_id,
        client_id,
    }
}

/// One minting path's outcome: the case it exercises and the
/// `AuthenticationResult` it answered.
struct Minted {
    case: String,
    result: Value,
}

/// Drive every operation of this service that mints a token on `pool`, in the
/// order each one's prerequisite is available, and return what each answered.
async fn every_minting_path(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    pool: &Pool,
) -> Vec<Minted> {
    let mut minted: Vec<Minted> = Vec::new();
    let label = |case: &str| format!("{} on {}", case, pool.user_pool_id);

    let signed_in = ok(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": pool.client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "signer", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    let client_refresh = signed_in["AuthenticationResult"]["RefreshToken"]
        .as_str()
        .expect("a sign-in issues a refresh token")
        .to_string();
    minted.push(Minted {
        case: label("InitiateAuth with the client-side password flow"),
        result: signed_in["AuthenticationResult"].clone(),
    });

    let admin_signed_in = ok(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": pool.user_pool_id,
            "ClientId": pool.client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "signer", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    let admin_refresh = admin_signed_in["AuthenticationResult"]["RefreshToken"]
        .as_str()
        .expect("a sign-in issues a refresh token")
        .to_string();
    minted.push(Minted {
        case: label("AdminInitiateAuth with the admin password flow"),
        result: admin_signed_in["AuthenticationResult"].clone(),
    });

    let legacy = ok(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": pool.user_pool_id,
            "ClientId": pool.client_id,
            "AuthFlow": "ADMIN_NO_SRP_AUTH",
            "AuthParameters": { "USERNAME": "signer", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    minted.push(Minted {
        case: label("AdminInitiateAuth with the legacy admin flow"),
        result: legacy["AuthenticationResult"].clone(),
    });

    let refreshed = ok(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": pool.client_id,
            "AuthFlow": "REFRESH_TOKEN_AUTH",
            "AuthParameters": { "REFRESH_TOKEN": client_refresh },
        }),
    )
    .await;
    minted.push(Minted {
        case: label("InitiateAuth with the refresh-token flow"),
        result: refreshed["AuthenticationResult"].clone(),
    });

    let exchanged = ok(
        router,
        "GetTokensFromRefreshToken",
        &json!({ "ClientId": pool.client_id, "RefreshToken": admin_refresh }),
    )
    .await;
    minted.push(Minted {
        case: label("GetTokensFromRefreshToken exchanging a refresh token"),
        result: exchanged["AuthenticationResult"].clone(),
    });

    // The two challenge paths: each administrator-created user signs in with
    // its temporary password, which leaves a challenge whose response mints the
    // tokens.
    let admin_challenge = ok(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": pool.user_pool_id,
            "ClientId": pool.client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "admin-challenged", "PASSWORD": TEMPORARY_PASSWORD },
        }),
    )
    .await;
    let answered = ok(
        router,
        "AdminRespondToAuthChallenge",
        &json!({
            "UserPoolId": pool.user_pool_id,
            "ClientId": pool.client_id,
            "ChallengeName": NEW_PASSWORD_REQUIRED,
            "Session": admin_challenge["Session"],
            "ChallengeResponses": {
                "USERNAME": "admin-challenged",
                "NEW_PASSWORD": REPLACEMENT_PASSWORD,
            },
        }),
    )
    .await;
    minted.push(Minted {
        case: label("AdminRespondToAuthChallenge answering a password challenge"),
        result: answered["AuthenticationResult"].clone(),
    });

    let otp_challenge = ok(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": pool.client_id,
            "AuthFlow": "USER_AUTH",
            "AuthParameters": { "USERNAME": "otp-challenged", "PREFERRED_CHALLENGE": EMAIL_OTP },
        }),
    )
    .await;
    let otp_session = otp_challenge["Session"]
        .as_str()
        .expect("a challenge carries a session")
        .to_string();
    let code = session_code(engine, &otp_session);
    let answered = ok(
        router,
        "RespondToAuthChallenge",
        &json!({
            "ClientId": pool.client_id,
            "ChallengeName": EMAIL_OTP,
            "Session": otp_session,
            "ChallengeResponses": { "USERNAME": "otp-challenged", "EMAIL_OTP_CODE": code },
        }),
    )
    .await;
    minted.push(Minted {
        case: label("RespondToAuthChallenge answering a one-time-password challenge"),
        result: answered["AuthenticationResult"].clone(),
    });

    minted
}

/// Whether the RSA key `jwk` publishes signed `signing_input` — the check a
/// third party makes, from the `n`/`e` public parameters only, with a library
/// that did not make the signature.
fn rsa_signature_checks_out(case: &str, jwk: &Value, signing_input: &[u8], signature: &[u8]) -> bool {
    let public = RsaPublicKeyComponents {
        n: integer_member(case, jwk, "n"),
        e: integer_member(case, jwk, "e"),
    };
    public
        .verify(&RSA_PKCS1_2048_8192_SHA256, signing_input, signature)
        .is_ok()
}

/// RULE: every token every minting path of this service answers is an RS256 JWS
/// naming the pool's key, and that key is published as an RSA JWK the signature
/// checks out against.
#[tokio::test]
async fn every_minted_token_is_an_rs256_jws_verified_by_the_pools_rsa_jwk() {
    let (router, engine) = router_and_engine();
    // The two pools differ in the claims their user's tokens carry, so the
    // payload each signature covers differs, and each signs with its own key.
    let pools = [
        pool_with_users(&router, "plain-pool", &[], None).await,
        pool_with_users(
            &router,
            "claims-pool",
            &["admins", "auditors"],
            Some("signer@example.test"),
        )
        .await,
    ];

    let mut key_ids: Vec<String> = Vec::new();
    for pool in &pools {
        let jwks = served_jwks(&router, &pool.user_pool_id).await;
        let keys = jwks["keys"]
            .as_array()
            .unwrap_or_else(|| panic!("a key set has a keys list, got {jwks}"))
            .clone();
        let case = format!("the key set of {}", pool.user_pool_id);
        assert!(
            !keys.is_empty(),
            "case {case:?}: a pool publishes the key of every token it signs, got {jwks}"
        );
        for published in &keys {
            assert_eq!(
                (
                    published["kty"].as_str(),
                    published["alg"].as_str(),
                    published["use"].as_str()
                ),
                (
                    Some(EXPECTED_KEY_TYPE),
                    Some(EXPECTED_ALGORITHM),
                    Some("sig")
                ),
                "case {case:?}: every published key is an RSA signing key for \
                 {EXPECTED_ALGORITHM}, got {published}"
            );
            let modulus = integer_member(&case, published, "n");
            assert!(
                modulus.len() >= MINIMUM_MODULUS_BYTES,
                "case {case:?}: expected a modulus of at least {MINIMUM_MODULUS_BYTES} octets, \
                 got {} octets",
                modulus.len()
            );
            assert_eq!(
                integer_member(&case, published, "e"),
                PUBLIC_EXPONENT.to_vec(),
                "case {case:?}: expected the public exponent 65537, got {published}"
            );
            assert!(
                ["d", "p", "q", "dp", "dq", "qi"]
                    .iter()
                    .all(|private| published.get(*private).is_none()),
                "case {case:?}: a published key set carries no private parameter, got {published}"
            );
            let key_id = published["kid"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every published key names itself, got {published}")
                })
                .to_string();
            assert!(
                !key_id.is_empty(),
                "case {case:?}: a published key's kid is not empty"
            );
            // Each key is its own: two keys of one pool, and two pools' keys,
            // never share a kid, so a token names exactly one of them.
            assert!(
                !key_ids.contains(&key_id),
                "case {case:?}: no two published keys share the kid {key_id}"
            );
            key_ids.push(key_id);
        }

        for row in every_minting_path(&router, &engine, pool).await {
            let Minted { case, result } = row;
            // The kid each of the row's tokens names, so the rows also pin that
            // one token names ONE published key.
            let mut named: Vec<String> = Vec::new();
            for member in TOKEN_MEMBERS {
                let token = result[member].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: the path must answer {member}, got {result}")
                });
                let segments: Vec<&str> = token.split('.').collect();
                assert_eq!(
                    segments.len(),
                    3,
                    "case {case:?} / {member}: a compact JWS has three segments, got {token}"
                );
                let header: Value = decode(segments[0])
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .unwrap_or_else(|| {
                        panic!("case {case:?} / {member}: the header must be base64url JSON")
                    });
                assert_eq!(
                    header["alg"],
                    json!(EXPECTED_ALGORITHM),
                    "case {case:?} / {member}: expected alg {EXPECTED_ALGORITHM}, got {header}"
                );
                assert_eq!(
                    header["typ"],
                    json!("JWT"),
                    "case {case:?} / {member}: expected typ JWT, got {header}"
                );
                // The header names a key of THIS pool's published set, which is
                // the key the signature is then checked against.
                let key_id = header["kid"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?} / {member}: the header names a key, got {header}")
                    })
                    .to_string();
                let published = keys
                    .iter()
                    .find(|published| published["kid"].as_str() == Some(key_id.as_str()))
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?} / {member}: the kid {key_id} must name a key the pool \
                             publishes, got {jwks}"
                        )
                    });
                named.push(key_id);

                // The signature checks out under RS256 against the published
                // key, over the token's own encoded header and payload.
                let signature = decode(segments[2]).unwrap_or_else(|| {
                    panic!("case {case:?} / {member}: the signature must be base64url")
                });
                let signing_input = format!("{}.{}", segments[0], segments[1]);
                assert!(
                    rsa_signature_checks_out(
                        &case,
                        published,
                        signing_input.as_bytes(),
                        &signature
                    ),
                    "case {case:?} / {member}: the signature must check out against the \
                     published RSA key, got the header {header}"
                );

                // And through the reading every emulated consumer of such a
                // token verifies it with.
                let verified = service_common::jwt::verify(token, &jwks).unwrap_or_else(|defect| {
                    panic!(
                        "case {case:?} / {member}: the shared verifier must accept the token, \
                         got {defect:?}"
                    )
                });
                assert_eq!(
                    verified.algorithm, EXPECTED_ALGORITHM,
                    "case {case:?} / {member}: the verified algorithm is the one the header \
                     named, got {:?}",
                    verified.algorithm
                );
                assert_eq!(
                    verified.claims.get("iss").and_then(Value::as_str),
                    Some(pool.issuer.as_str()),
                    "case {case:?} / {member}: the signed claims name the pool's issuer, got {:?}",
                    verified.claims
                );

                // The signature BINDS the payload: the same token with a
                // rewritten claim set fails both verifiers.
                let mut forged: Map<String, Value> = verified.claims.clone();
                forged.insert("cognito:groups".to_string(), json!(["forged"]));
                let tampered = format!(
                    "{}.{}.{}",
                    segments[0],
                    service_common::jwt::encode_base64url(
                        Value::Object(forged).to_string().as_bytes()
                    ),
                    segments[2]
                );
                let tampered_input = tampered
                    .rsplit_once('.')
                    .map(|(signed, _)| signed.to_string())
                    .expect("the tampered token has three segments");
                assert!(
                    !rsa_signature_checks_out(
                        &case,
                        published,
                        tampered_input.as_bytes(),
                        &signature
                    ),
                    "case {case:?} / {member}: a rewritten claim set must not check out"
                );
                assert!(
                    service_common::jwt::verify(&tampered, &jwks).is_err(),
                    "case {case:?} / {member}: the shared verifier must refuse a rewritten \
                     claim set"
                );
            }
            // A pool holds one key per token type, so the two tokens of one
            // answer name different published keys.
            assert_eq!(
                named.len(),
                TOKEN_MEMBERS.len(),
                "case {case:?}: every signed member must name a key, got {named:?}"
            );
            assert!(
                named.iter().collect::<::std::collections::BTreeSet<&String>>().len()
                    == named.len(),
                "case {case:?}: each token type names its own key, got {named:?}"
            );
        }
    }
}
