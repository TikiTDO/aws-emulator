// SPDX-License-Identifier: CC0-1.0

//! The documents a user pool publishes under its token issuer, fetched the way
//! a relying party fetches them: an ANONYMOUS GET of a path under
//! `<iss>/.well-known/`, where `<iss>` is the `iss` claim of the pool's tokens.
//!
//! The rules under test:
//!
//! * `<iss>/.well-known/openid-configuration` answers 200 `application/json`
//!   with a discovery document whose `issuer` IS that `iss` and whose every
//!   endpoint sits under it — the only part of each value that varies is the
//!   issuer itself — and which advertises the algorithm the pool's tokens are
//!   actually signed with.
//! * A well-formed pool id no pool carries is answered 404 with a body naming
//!   that pool, on every well-known path the endpoint serves, rather than with
//!   the router's "no service claimed this request" refusal.
//! * Each token type of a sign-in is signed under the pool's OWN key of that
//!   type: the `kid` in a token's header resolves in
//!   `<iss>/.well-known/jwks.json` to an RSA RS256 signing key whose `n`/`e`
//!   verify the signature (RFC 7518 §6.3.1), and the access token's key is a
//!   DIFFERENT key of the same set from the id token's.
//!
//! The rows vary the pool, the endpoint the request reaches the service on —
//! the published regional host and a local address, each of which a pool is
//! served at — and the token type.

use std::sync::Arc;

use bytes::Bytes;
use ring::signature::{RsaPublicKeyComponents, RSA_PKCS1_2048_8192_SHA256};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// A request's SigV4 credential scope is what routes a signed call to this
/// service; the well-known documents are fetched with no credential at all.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The password every fixture user holds.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The published regional endpoint host of the region this suite signs for.
const PUBLISHED_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// A local address the emulator is reached at, which serves the same pools: an
/// operator binds the emulator wherever they like, and a pool issues on the
/// endpoint each caller used.
const LOCAL_HOST: &str = "127.0.0.1:9297";

/// The members of `AuthenticationResult` that carry a SIGNED token.
const TOKEN_MEMBERS: [&str; 2] = ["AccessToken", "IdToken"];

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::new(
        engine::InMemoryEngine::new(),
    )));
    router.set_request_authentication(false);
    router
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", PUBLISHED_HOST)
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

/// An ANONYMOUS GET of `path` addressed to `host`: the status, the
/// `content-type` and the body.
async fn get(
    router: &MultiServiceRouter,
    host: &str,
    path: &str,
) -> (u16, Option<String>, Vec<u8>) {
    let request = http::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", host)
        .body(Bytes::new())
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let media_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    (
        response.status().as_u16(),
        media_type,
        response.body().to_vec(),
    )
}

/// A base64url segment's bytes, with the padding RFC 7515 §2 strips restored.
fn decode_base64url(segment: &str) -> Vec<u8> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|error| panic!("{segment:?} must be base64url ({error})"))
}

/// The JSON a token segment carries.
fn segment_json(segment: &str) -> Value {
    serde_json::from_slice(&decode_base64url(segment))
        .unwrap_or_else(|error| panic!("{segment:?} must carry JSON ({error})"))
}

/// A pool with one app client and one signed-in user.
struct SignedIn {
    user_pool_id: String,
    result: Value,
}

/// Create a pool named `name` with an app client and a `CONFIRMED` user, sign
/// that user in on `host`, and return the pool and the tokens.
async fn sign_in(router: &MultiServiceRouter, name: &str, username: &str) -> SignedIn {
    let user_pool_id = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await
        ["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client_id = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "discovery-client",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "TemporaryPassword": "Temp0rary!2024",
            "MessageAction": "SUPPRESS",
        }),
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
    let signed_in = ok(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    SignedIn {
        user_pool_id,
        result: signed_in["AuthenticationResult"].clone(),
    }
}

/// The `iss` claim of the id token, which is the discovery root every document
/// below is fetched from.
fn issuer_of(signed_in: &SignedIn) -> String {
    let token = signed_in.result["IdToken"]
        .as_str()
        .expect("a sign-in returns an id token");
    let claims = segment_json(token.split('.').nth(1).expect("a claims segment"));
    claims["iss"]
        .as_str()
        .expect("a token names its issuer")
        .to_string()
}

/// A pool's discovery document names the pool's own issuer and every endpoint
/// under it.
#[tokio::test]
async fn a_pools_discovery_document_names_its_issuer_and_its_endpoints() {
    let router = router();

    // The rows differ along the pool and the endpoint the request reaches the
    // service on: a pool is served at every address its callers use, and the
    // document it answers names THAT address.
    let first = sign_in(&router, "discovery-pool-one", "alice").await;
    let second = sign_in(&router, "discovery-pool-two", "bob").await;
    let rows = [
        ("the first pool", &first, PUBLISHED_HOST),
        ("the second pool", &second, PUBLISHED_HOST),
        ("the first pool on a local address", &first, LOCAL_HOST),
    ];

    for (case, signed_in, host) in rows {
        let issuer = format!("https://{host}/{}", signed_in.user_pool_id);
        let (status, media_type, body) = get(
            &router,
            host,
            &format!("/{}/.well-known/openid-configuration", signed_in.user_pool_id),
        )
        .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: the discovery document must be served, got {status} with {:?}",
            String::from_utf8_lossy(&body)
        );
        assert_eq!(
            media_type.as_deref(),
            Some("application/json"),
            "case {case:?}: a discovery document is JSON, got {media_type:?}"
        );
        let document: Value = serde_json::from_slice(&body).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the document must be JSON ({error}): {:?}",
                String::from_utf8_lossy(&body)
            )
        });

        // Every endpoint the document names, and the suffix it carries under the
        // issuer. The only variable part of each value is the issuer itself.
        let endpoints = [
            ("issuer", String::new()),
            ("jwks_uri", "/.well-known/jwks.json".to_string()),
            ("authorization_endpoint", "/authorize".to_string()),
            ("token_endpoint", "/token".to_string()),
            ("userinfo_endpoint", "/userInfo".to_string()),
            ("revocation_endpoint", "/revoke".to_string()),
            ("end_session_endpoint", "/logout".to_string()),
        ];
        for (member, suffix) in endpoints {
            let expected = format!("{issuer}{suffix}");
            assert_eq!(
                document[member].as_str(),
                Some(expected.as_str()),
                "case {case:?}: {member} must be {expected:?}, got {:?}",
                document[member]
            );
        }

        // The document's `issuer` is the `iss` claim of the pool's own tokens
        // when the token was minted on the same endpoint, which is what lets a
        // relying party configured from this document accept them.
        if host == PUBLISHED_HOST {
            assert_eq!(
                document["issuer"].as_str(),
                Some(issuer_of(signed_in).as_str()),
                "case {case:?}: the document's issuer must be the token's iss claim, got {document}"
            );
        }

        // The fixed lists a user pool advertises.
        for (member, expected) in [
            ("id_token_signing_alg_values_supported", json!(["RS256"])),
            ("response_types_supported", json!(["code", "token"])),
            ("subject_types_supported", json!(["public"])),
            (
                "scopes_supported",
                json!(["openid", "email", "phone", "profile"]),
            ),
            (
                "token_endpoint_auth_methods_supported",
                json!(["client_secret_basic", "client_secret_post"]),
            ),
        ] {
            assert_eq!(
                document[member], expected,
                "case {case:?}: {member} must be {expected}, got {:?}",
                document[member]
            );
        }

        // A read changes nothing: the same request answers the same document.
        let (again_status, _, again_body) = get(
            &router,
            host,
            &format!("/{}/.well-known/openid-configuration", signed_in.user_pool_id),
        )
        .await;
        assert_eq!(
            (again_status, again_body),
            (status, body),
            "case {case:?}: the document must be the same on every read"
        );
    }
}

/// A well-formed pool id no pool carries is answered 404 naming that pool, on
/// every well-known path the endpoint serves.
#[tokio::test]
async fn a_well_known_document_of_an_unknown_pool_names_the_pool_it_answers_for() {
    let router = router();
    // One pool exists, so the refusals below are about the pool id each request
    // names rather than about an empty store.
    let present = sign_in(&router, "present-pool", "carol").await;

    // The rows differ along the pool id's form, inside `UserPoolIdType`
    // (pattern `^[\w-]+_[0-9a-zA-Z]+$`, length 1..55, from
    // `smithy_query(query="shape",shape="UserPoolIdType")`): another region's
    // prefix, the shortest shape, the longest suffix, and the present pool's id
    // with its suffix changed.
    let absent_ids = [
        ("another region's prefix", "eu-west-3_nosuchpool".to_string()),
        ("the shortest shape", "a_b".to_string()),
        ("a long suffix", format!("us-east-1_{}", "a".repeat(40))),
        (
            "the stored pool's id with another suffix",
            format!("{}zz", present.user_pool_id),
        ),
    ];
    let documents = [".well-known/openid-configuration", ".well-known/jwks.json"];

    for (case, user_pool_id) in &absent_ids {
        for document in documents {
            for host in [PUBLISHED_HOST, LOCAL_HOST] {
                let case = format!("{case} on {document} at {host}");
                let (status, media_type, body) =
                    get(&router, host, &format!("/{user_pool_id}/{document}")).await;
                assert_eq!(
                    status,
                    404,
                    "case {case:?}: an unknown pool's document is a 404, got {status} with {:?}",
                    String::from_utf8_lossy(&body)
                );
                assert_eq!(
                    media_type.as_deref(),
                    Some("application/json"),
                    "case {case:?}: the refusal is JSON, got {media_type:?}"
                );
                let refusal: Value = serde_json::from_slice(&body).unwrap_or_else(|error| {
                    panic!(
                        "case {case:?}: the refusal must be JSON ({error}): {:?}",
                        String::from_utf8_lossy(&body)
                    )
                });
                let expected = format!("User pool {user_pool_id} does not exist.");
                assert_eq!(
                    refusal["message"].as_str(),
                    Some(expected.as_str()),
                    "case {case:?}: expected {expected:?}, got {refusal}"
                );
            }
        }
    }

    // The pool that DOES exist still serves both documents, so the rows above
    // failed on the pool id and not on the route.
    for document in documents {
        let (status, _, body) = get(
            &router,
            PUBLISHED_HOST,
            &format!("/{}/{document}", present.user_pool_id),
        )
        .await;
        assert_eq!(
            status,
            200,
            "case {document:?}: a stored pool serves its document, got {status} with {:?}",
            String::from_utf8_lossy(&body)
        );
    }
}

/// Each token type is signed under its own RSA key of the pool's published key
/// set, and the signature verifies against the `n`/`e` that set carries.
#[tokio::test]
async fn each_token_type_names_its_own_rsa_signing_key_of_the_pools_key_set() {
    let router = router();

    // The rows differ along the pool and the endpoint the sign-in reached, so
    // the key set is resolved from the issuer each token itself names.
    let rows = [
        ("the first pool", sign_in(&router, "key-pool-one", "dave").await),
        ("the second pool", sign_in(&router, "key-pool-two", "erin").await),
    ];

    let mut all_kids: Vec<String> = Vec::new();
    for (case, signed_in) in &rows {
        let issuer = issuer_of(signed_in);
        let path = issuer
            .strip_prefix(&format!("https://{PUBLISHED_HOST}"))
            .unwrap_or_else(|| panic!("case {case:?}: the issuer is this endpoint, got {issuer}"));
        let (status, media_type, body) =
            get(&router, PUBLISHED_HOST, &format!("{path}/.well-known/jwks.json")).await;
        assert_eq!(
            status,
            200,
            "case {case:?}: the key set must be served at the issuer, got {status} with {:?}",
            String::from_utf8_lossy(&body)
        );
        assert_eq!(
            media_type.as_deref(),
            Some("application/json"),
            "case {case:?}: a key set is JSON, got {media_type:?}"
        );
        let key_set: Value = serde_json::from_slice(&body).expect("the key set is JSON");
        let keys = key_set["keys"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a key set has a keys list, got {key_set}"));

        let mut named: Vec<String> = Vec::new();
        for member in TOKEN_MEMBERS {
            let token = signed_in.result[member]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the sign-in must return {member}"));
            let segments: Vec<&str> = token.split('.').collect();
            assert_eq!(
                segments.len(),
                3,
                "case {case:?} / {member}: a compact JWS has three segments, got {token}"
            );
            let header = segment_json(segments[0]);
            assert_eq!(
                header["alg"], "RS256",
                "case {case:?} / {member}: the header must name RS256, got {header}"
            );
            let kid = header["kid"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?} / {member}: the header must name a key, got {header}")
                })
                .to_string();
            let key = keys
                .iter()
                .find(|key| key["kid"].as_str() == Some(kid.as_str()))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?} / {member}: the kid {kid} must resolve in the pool's key \
                         set, got {key_set}"
                    )
                });
            // The shape a verifier configured for this pool expects of the key.
            assert_eq!(
                json!({ "kty": key["kty"], "alg": key["alg"], "use": key["use"] }),
                json!({ "kty": "RSA", "alg": "RS256", "use": "sig" }),
                "case {case:?} / {member}: expected an RSA RS256 signing key, got {key}"
            );
            // RFC 7518 §6.3.1: the public key travels as its modulus and
            // exponent, and the signature checks out against them alone.
            let public = RsaPublicKeyComponents {
                n: decode_base64url(
                    key["n"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case:?} / {member}: the JWK carries n")),
                ),
                e: decode_base64url(
                    key["e"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case:?} / {member}: the JWK carries e")),
                ),
            };
            let signing_input = format!("{}.{}", segments[0], segments[1]);
            let signature = decode_base64url(segments[2]);
            public
                .verify(
                    &RSA_PKCS1_2048_8192_SHA256,
                    signing_input.as_bytes(),
                    &signature,
                )
                .unwrap_or_else(|error| {
                    panic!(
                        "case {case:?} / {member}: the signature must verify as \
                         RSASSA-PKCS1-v1_5 over SHA-256 under the published key ({error})"
                    )
                });
            named.push(kid);
        }
        assert_ne!(
            named[0], named[1],
            "case {case:?}: the access token and the id token must name DIFFERENT keys of the \
             pool's key set, got {named:?}"
        );
        assert_eq!(
            keys.len(),
            TOKEN_MEMBERS.len(),
            "case {case:?}: the pool publishes the key of each token type, got {key_set}"
        );
        all_kids.extend(named);
    }

    // Each pool keeps its own keys, so no two pools publish one `kid`.
    let mut unique = all_kids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        all_kids.len(),
        "each pool's keys are its own, so no kid is shared, got {all_kids:?}"
    );

    // The key set the wire serves is the pool's stored one: the document is
    // rendered from state, not minted per request.
    let (_, _, body) = get(
        &router,
        PUBLISHED_HOST,
        &format!("/{}/.well-known/jwks.json", rows[0].1.user_pool_id),
    )
    .await;
    let served: Value = serde_json::from_slice(&body).expect("the key set is JSON");
    assert!(
        served["keys"]
            .as_array()
            .is_some_and(|keys| keys.iter().all(|key| key.get("d").is_none())),
        "a published key set carries no private parameter, got {served}"
    );
    assert!(
        state::is_pool_issuer(
            "us-east-1",
            &rows[0].1.user_pool_id,
            &issuer_of(&rows[0].1)
        ),
        "the issuer the tokens name is the pool's own, got {}",
        issuer_of(&rows[0].1)
    );
}
