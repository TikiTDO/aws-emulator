// SPDX-License-Identifier: CC0-1.0

//! The tokens this service issues, as a verifier sees them: the access and id
//! tokens are three NON-EMPTY base64url segments whose third one is an RS256
//! signature that checks out against the PUBLIC key the issuing pool's key set
//! publishes, and whose claims come from stored state. The refresh token beside
//! them is an opaque credential and is checked to be exactly that.
//!
//! The encoding of a segment is pinned by PUBLISHED vectors rather than by this
//! emulator's own output: RFC 4648 §10's base64 test strings, plus inputs whose
//! standard encoding carries both characters RFC 7515 §2 substitutes.
//!
//! The signature suite signs in over the wire and then verifies each issued
//! token the way any consumer does (Verified Permissions, an API Gateway JWT
//! authorizer, a JOSE library): read `alg` and `kid` from the header, find that
//! key in the pool's JWKS, and check the signature over
//! `base64url(header) '.' base64url(claims)` with the algorithm the key set
//! advertises (RS256 — RSASSA-PKCS1-v1_5 with SHA-256, RFC 7518 §3.3). Nothing
//! here touches a private key — the verification path uses only the `n`/`e`
//! components the key set publishes — so a token that passes is one a third
//! party could have verified. The rows span the signed tokens of a sign-in,
//! users with and without groups and an email, two pools, a table of tamperings
//! each of which must FAIL to verify, and the key set as served over the wire at
//! the pool's `/.well-known/jwks.json`.

use std::sync::Arc;

use bytes::Bytes;
use ring::signature::{RsaPublicKeyComponents, RSA_PKCS1_2048_8192_SHA256};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// A request's SigV4 credential scope is what routes it to this service; the
/// signature itself is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The members of `AuthenticationResult` that carry a SIGNED token, with the
/// `token_use` claim each one's token states. The refresh token beside them is an
/// opaque credential that states no claim at all, so it is not signed and not
/// among these.
const TOKEN_MEMBERS: [(&str, &str); 2] = [("AccessToken", "access"), ("IdToken", "id")];

/// One emulator with just this service mounted, beside the engine its state
/// lives in.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.set_request_authentication(false);
    (router, engine)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
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

/// A base64url segment's bytes, or `None` when the segment is not base64url at
/// all: decoded with the codec the wire serializer uses
/// (`aws_smithy_types::base64`), with the two alphabet substitutions and the
/// padding RFC 7515 §2 strips restored first.
fn try_decode_base64url(segment: &str) -> Option<Vec<u8>> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    aws_smithy_types::base64::decode(&standard).ok()
}

/// [`try_decode_base64url`] for a segment this emulator itself produced, which
/// must therefore decode.
fn decode_base64url(segment: &str) -> Vec<u8> {
    try_decode_base64url(segment).unwrap_or_else(|| panic!("{segment:?} must be base64url"))
}

/// A JWS segment is base64url without padding (RFC 7515 §2): the standard
/// base64 of RFC 4648 §10 with `+`/`/` replaced and `=` dropped.
#[tokio::test]
async fn a_jws_segment_is_base64url_without_padding() {
    // The first rows are RFC 4648 §10's own base64 test vectors with the
    // padding stripped; the last two are byte strings whose standard encoding
    // contains the two characters RFC 7515 §2 substitutes.
    let cases: &[(&str, &[u8], &str)] = &[
        ("RFC 4648 \"\"", b"", ""),
        ("RFC 4648 \"f\"", b"f", "Zg"),
        ("RFC 4648 \"fo\"", b"fo", "Zm8"),
        ("RFC 4648 \"foo\"", b"foo", "Zm9v"),
        ("RFC 4648 \"foob\"", b"foob", "Zm9vYg"),
        ("RFC 4648 \"fooba\"", b"fooba", "Zm9vYmE"),
        ("RFC 4648 \"foobar\"", b"foobar", "Zm9vYmFy"),
        ("bytes whose encoding uses + and /", &[0xfb, 0xff], "-_8"),
        ("bytes whose encoding uses /", &[0x00, 0xff, 0xff], "AP__"),
    ];

    for (case, bytes, expected) in cases {
        let actual = state::base64_url(bytes);
        assert_eq!(
            &actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            !actual.contains('+') && !actual.contains('/') && !actual.contains('='),
            "case {case:?}: a base64url segment carries none of + / =, got {actual:?}"
        );
        let decoded = decode_base64url(&actual);
        assert_eq!(
            decoded, *bytes,
            "case {case:?}: the segment must decode to the bytes it encoded, got {decoded:?}"
        );
    }
}

/// Why a token failed to verify against a key set, for an assertion message.
#[derive(Debug, PartialEq, Eq)]
enum Rejected {
    /// Not three non-empty segments, or a segment that is not base64url JSON.
    Malformed(String),
    /// The header names no algorithm or key this key set can check it under.
    UnusableHeader(String),
    /// No key in the set carries the `kid` the header names.
    UnknownKey(String),
    /// The signature does not check out against the named key.
    BadSignature,
}

/// Verify `token` exactly as a third-party consumer does, using ONLY the public
/// parameters in `jwks`, and return its claims.
///
/// The steps are the ones RFC 7515 §5.2 and RFC 7518 §3.3 define: split the
/// compact serialization, read `alg`/`kid` from the header, find that key in
/// the set, rebuild the public key from its `n`/`e` (RFC 7518 §6.3.1), and
/// check RSASSA-PKCS1-v1_5 over SHA-256 against the two encoded segments.
fn verify_against_jwks(token: &str, jwks: &Value) -> Result<Value, Rejected> {
    let segments: Vec<&str> = token.split('.').collect();
    if segments.len() != 3 || segments.iter().any(|segment| segment.is_empty()) {
        return Err(Rejected::Malformed(format!(
            "expected three non-empty segments, got {token:?}"
        )));
    }
    let header_bytes = try_decode_base64url(segments[0])
        .ok_or_else(|| Rejected::Malformed("the header is not base64url".to_string()))?;
    let header: Value = serde_json::from_slice(&header_bytes)
        .map_err(|error| Rejected::Malformed(format!("the header is not JSON: {error}")))?;
    if header["alg"] != "RS256" {
        return Err(Rejected::UnusableHeader(format!(
            "unsupported alg in {header}"
        )));
    }
    let Some(kid) = header["kid"].as_str() else {
        return Err(Rejected::UnusableHeader(format!("no kid in {header}")));
    };
    let key = jwks["keys"]
        .as_array()
        .unwrap_or_else(|| panic!("a key set has a keys list, got {jwks}"))
        .iter()
        .find(|jwk| jwk["kid"] == kid)
        .ok_or_else(|| Rejected::UnknownKey(kid.to_string()))?;
    assert_eq!(key["kty"], "RSA", "the published JWK must be RSA, got {key}");
    assert_eq!(key["alg"], "RS256", "the published JWK must be RS256");
    assert_eq!(key["use"], "sig", "the published JWK must be for signing");
    // RFC 7518 §6.3.1: an RSA public key travels as its modulus and its public
    // exponent, each the unsigned big-endian octet string of the value.
    let component = |member: &str| -> Vec<u8> {
        let bytes = decode_base64url(
            key[member]
                .as_str()
                .unwrap_or_else(|| panic!("the JWK carries {member}, got {key}")),
        );
        assert!(
            !bytes.is_empty(),
            "the JWK's {member} must carry the component's octets, got none"
        );
        bytes
    };
    let public = RsaPublicKeyComponents {
        n: component("n"),
        e: component("e"),
    };
    // A signature segment that is not base64url, or not the modulus-width
    // PKCS#1 block RFC 7518 §3.3 defines, is no signature of this key: the
    // token is refused the same way one carrying wrong bytes is.
    let signature_bytes = try_decode_base64url(segments[2]).ok_or(Rejected::BadSignature)?;
    let signing_input = format!("{}.{}", segments[0], segments[1]);
    public
        .verify(
            &RSA_PKCS1_2048_8192_SHA256,
            signing_input.as_bytes(),
            &signature_bytes,
        )
        .map_err(|_| Rejected::BadSignature)?;
    let claim_bytes = try_decode_base64url(segments[1])
        .ok_or_else(|| Rejected::Malformed("the claims are not base64url".to_string()))?;
    serde_json::from_slice(&claim_bytes)
        .map_err(|error| Rejected::Malformed(format!("the claims are not JSON: {error}")))
}

/// One row of the sign-in table: a pool to build and the user to sign in.
struct SignInCase {
    case: &'static str,
    pool_name: &'static str,
    username: &'static str,
    /// The groups the user is added to, which land in the `cognito:groups`
    /// claim the signature must bind.
    groups: &'static [&'static str],
    /// The `email` attribute the user is created with, when the row has one.
    email: Option<&'static str>,
}

/// What a signed-in row left behind: the pool it belongs to and its tokens.
struct SignedIn {
    case: &'static str,
    user_pool_id: String,
    client_id: String,
    sub: String,
    issuer: String,
    /// The groups the signed-in user belongs to, in the order they were added.
    groups: &'static [&'static str],
    result: Value,
}

/// Build `case`'s pool, app client, groups and user, then sign the user in.
async fn sign_in(router: &MultiServiceRouter, case: &SignInCase) -> SignedIn {
    let pool = ok(
        router,
        "CreateUserPool",
        &json!({ "PoolName": case.pool_name }),
    )
    .await;
    let user_pool_id = pool["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": format!("{}-client", case.pool_name),
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    let attributes: Vec<Value> = case
        .email
        .map(|email| vec![json!({ "Name": "email", "Value": email })])
        .unwrap_or_default();
    let created = ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": case.username,
            "UserAttributes": attributes,
        }),
    )
    .await;
    let sub = created["User"]["Attributes"]
        .as_array()
        .expect("the stored attributes")
        .iter()
        .find(|attribute| attribute["Name"] == "sub")
        .and_then(|attribute| attribute["Value"].as_str())
        .expect("a generated sub")
        .to_string();
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": case.username,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    for group in case.groups {
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
                "Username": case.username,
                "GroupName": group,
            }),
        )
        .await;
    }
    let result = ok(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": case.username, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    SignedIn {
        case: case.case,
        issuer: format!("https://cognito-idp.us-east-1.amazonaws.com/{user_pool_id}"),
        user_pool_id,
        client_id,
        sub,
        groups: case.groups,
        result: result["AuthenticationResult"].clone(),
    }
}

/// The key set the stored pool publishes, read from this service's state the
/// way the endpoint that serves it does.
fn pool_jwks(engine: &engine::InMemoryEngine, user_pool_id: &str) -> Value {
    let service_state = state::service_state(engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .unwrap_or_else(|| panic!("the stored pool {user_pool_id}"))
        .jwks()
}

/// Every token a sign-in returns is a signed JWS whose signature
/// verifies against the issuing pool's published key set, over claims that come
/// from stored state — and a token one pool issued verifies against no other
/// pool's key set.
#[tokio::test]
async fn an_issued_token_is_signed_and_verifies_against_its_pools_key_set() {
    // The rows differ along what the signing rule generalises over: whether the
    // user is in no group, one group or several (so the `cognito:groups` claim
    // is absent, scalar-sized or longer), and whether it carries an extra
    // `email` claim — the payload the signature is computed over differs in
    // every row, and each pool signs with its OWN key.
    let cases = [
        SignInCase {
            case: "a user in no group and with no email",
            pool_name: "plain-pool",
            username: "alice",
            groups: &[],
            email: None,
        },
        SignInCase {
            case: "a user in two groups with an email",
            pool_name: "group-pool",
            username: "bob",
            groups: &["admins", "auditors"],
            email: Some("bob@example.test"),
        },
    ];

    let (router, engine) = router_and_engine();
    let mut signed_in: Vec<SignedIn> = Vec::new();
    for case in &cases {
        signed_in.push(sign_in(&router, case).await);
    }

    for row in &signed_in {
        let case = row.case;
        let jwks = pool_jwks(&engine, &row.user_pool_id);
        let keys = jwks["keys"].as_array().expect("a keys list");
        assert_eq!(
            keys.len(),
            TOKEN_MEMBERS.len(),
            "case {case:?}: the pool publishes the key of each token type it signs, got {jwks}"
        );
        for key in keys {
            assert!(
                key["n"].as_str().is_some_and(|n| !n.is_empty())
                    && key["e"].as_str().is_some_and(|e| !e.is_empty()),
                "case {case:?}: an RSA JWK publishes its modulus and exponent, got {key}"
            );
            assert!(
                ["d", "p", "q", "dp", "dq", "qi"]
                    .iter()
                    .all(|private| key.get(*private).is_none()),
                "case {case:?}: a published JWK carries no private parameter, got {key}"
            );
        }
        let published: Vec<&Value> = keys.iter().collect();

        let mut signatures: Vec<String> = Vec::new();
        let mut jtis: Vec<String> = Vec::new();
        let mut signing_kids: Vec<String> = Vec::new();
        for (member, token_use) in TOKEN_MEMBERS {
            let token = row.result[member].as_str().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the sign-in must return {member}, got {}",
                    row.result
                )
            });
            let segments: Vec<&str> = token.split('.').collect();
            assert_eq!(
                segments.len(),
                3,
                "case {case:?} / {member}: a JWS compact serialization has three segments, \
                 got {token}"
            );
            assert!(
                !segments[2].is_empty(),
                "case {case:?} / {member}: a signed JWS has a non-empty signature segment, \
                 got {token}"
            );
            // The whole token must satisfy what a consumer's model constrains
            // an identity token to: three non-empty base64url runs (the
            // `Token` shape of Verified Permissions' `IsAuthorizedWithToken`
            // is `^[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+$`).
            assert!(
                segments.iter().all(|segment| !segment.is_empty()
                    && segment
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '=')),
                "case {case:?} / {member}: every segment must be non-empty base64url, got {token}"
            );
            signatures.push(segments[2].to_string());

            let header: Value = serde_json::from_slice(&decode_base64url(segments[0]))
                .unwrap_or_else(|error| {
                    panic!("case {case:?} / {member}: the header must be JSON ({error})")
                });
            assert_eq!(
                header["alg"], "RS256",
                "case {case:?} / {member}: expected alg RS256, got {header}"
            );
            assert_eq!(
                header["typ"], "JWT",
                "case {case:?} / {member}: expected typ JWT, got {header}"
            );
            let signing_kid = header["kid"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?} / {member}: the header must name a key, got {header}")
                })
                .to_string();
            assert!(
                published.iter().any(|key| key["kid"] == signing_kid),
                "case {case:?} / {member}: the header must name a key the pool publishes, \
                 got {header} against {jwks}"
            );
            assert!(
                !signing_kids.contains(&signing_kid),
                "case {case:?} / {member}: each token type is signed by its own key, \
                 {signing_kid} repeated"
            );
            signing_kids.push(signing_kid);

            let claims = verify_against_jwks(token, &jwks).unwrap_or_else(|rejected| {
                panic!(
                    "case {case:?} / {member}: the signature must verify against the pool's \
                     key set, got {rejected:?}"
                )
            });
            assert_eq!(
                claims["sub"], row.sub,
                "case {case:?} / {member}: the sub claim must be the stored user's, got {claims}"
            );
            assert_eq!(
                claims["iss"], row.issuer,
                "case {case:?} / {member}: the iss claim must be the pool's issuer, got {claims}"
            );
            assert_eq!(
                claims["token_use"], token_use,
                "case {case:?} / {member}: expected token_use {token_use}, got {claims}"
            );
            // Each token of a sign-in is its own token, so each carries its
            // own `jti`, and the signature covers that claim too.
            let jti = claims["jti"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?} / {member}: a token must carry a jti, got {claims}")
                })
                .to_string();
            assert!(
                !jtis.contains(&jti),
                "case {case:?} / {member}: every token gets its own jti, {jti} repeated"
            );
            jtis.push(jti);
            // `AdminAddUserToGroup`: a user in a group "populates a
            // `cognito:groups` claim to their access and identity tokens", so
            // the claim the signature covers is the stored membership on those
            // two, and absent for a user in no group.
            let expected_groups = (!row.groups.is_empty()).then(|| json!(row.groups));
            match expected_groups {
                Some(expected) => assert_eq!(
                    claims["cognito:groups"], expected,
                    "case {case:?} / {member}: the signed claims must carry the stored \
                     membership {expected}, got {claims}"
                ),
                None => assert!(
                    claims["cognito:groups"].is_null(),
                    "case {case:?} / {member}: expected no cognito:groups claim, got {claims}"
                ),
            }
        }
        assert!(
            signatures[0] != signatures[1],
            "case {case:?}: each token is signed over its own claims, so no two of a \
             sign-in's signatures are equal, got {signatures:?}"
        );
        // The refresh token beside them is a credential, not a statement: it is
        // NOT a JWS, so nothing in it is readable by its holder.
        let refresh = row.result["RefreshToken"].as_str().unwrap_or_else(|| {
            panic!(
                "case {case:?}: the sign-in must return RefreshToken, got {}",
                row.result
            )
        });
        assert!(
            !refresh.contains('.'),
            "case {case:?}: a refresh token is opaque, not a compact-serialized JWS, \
             got {refresh}"
        );
        assert!(
            try_decode_base64url(refresh)
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .is_none(),
            "case {case:?}: a refresh token decodes to no readable document, got {refresh}"
        );
    }

    // Each pool signs with its own key, so neither pool's key set verifies the
    // other's token and neither publishes the other's `kid`.
    for signer in &signed_in {
        for verifier in &signed_in {
            if signer.user_pool_id == verifier.user_pool_id {
                continue;
            }
            let token = signer.result["IdToken"].as_str().expect("an id token");
            let jwks = pool_jwks(&engine, &verifier.user_pool_id);
            let outcome = verify_against_jwks(token, &jwks);
            assert!(
                matches!(
                    outcome,
                    Err(Rejected::UnknownKey(_)) | Err(Rejected::BadSignature)
                ),
                "case {:?}: a token from another pool must not verify against {:?}'s key set, \
                 got {outcome:?}",
                signer.case,
                verifier.case
            );
        }
    }

    // The id token's audience is the app client it was issued for, and the
    // signature covers that too.
    let first = &signed_in[0];
    let claims = verify_against_jwks(
        first.result["IdToken"].as_str().expect("an id token"),
        &pool_jwks(&engine, &first.user_pool_id),
    )
    .expect("the id token verifies");
    assert_eq!(
        claims["aud"], first.client_id,
        "the id token's aud must be the app client it was issued for, got {claims}"
    );
}

/// The signature BINDS the claims: every tampering of a signed token fails to
/// verify against the pool's key set, so a rewritten claim cannot be passed off
/// as this pool's.
#[tokio::test]
async fn a_tampered_token_does_not_verify() {
    let (router, engine) = router_and_engine();
    let row = sign_in(
        &router,
        &SignInCase {
            case: "a group member whose claims are then rewritten",
            pool_name: "tamper-pool",
            username: "carol",
            groups: &["readers"],
            email: None,
        },
    )
    .await;
    let jwks = pool_jwks(&engine, &row.user_pool_id);
    let token = row.result["IdToken"]
        .as_str()
        .expect("an id token")
        .to_string();
    let segments: Vec<String> = token.split('.').map(str::to_string).collect();
    let claims: Value =
        serde_json::from_slice(&decode_base64url(&segments[1])).expect("the claims are JSON");

    /// `value` as a JWS segment, the encoding `state::base64_url` performs.
    fn segment(value: &Value) -> String {
        state::base64_url(value.to_string().as_bytes())
    }

    let mut forged_claims = claims.clone();
    forged_claims["cognito:groups"] = json!(["admins"]);
    let mut other_subject = claims.clone();
    other_subject["sub"] = json!("00000000-0000-4000-8000-000000000000");
    let flipped = {
        let mut signature = segments[2].clone();
        let last = signature.pop().expect("a non-empty signature");
        signature.push(if last == 'A' { 'B' } else { 'A' });
        signature
    };

    // Each row: a token built from the genuine one, and what the verifier must
    // answer for it. The rows differ along WHICH part was rewritten — the
    // claims, the header, the signature — and in which direction.
    let cases: Vec<(&str, String, Rejected)> = vec![
        (
            "a forged cognito:groups claim with the original signature",
            format!("{}.{}.{}", segments[0], segment(&forged_claims), segments[2]),
            Rejected::BadSignature,
        ),
        (
            "another subject with the original signature",
            format!("{}.{}.{}", segments[0], segment(&other_subject), segments[2]),
            Rejected::BadSignature,
        ),
        (
            "the last character of the signature flipped",
            format!("{}.{}.{}", segments[0], segments[1], flipped),
            Rejected::BadSignature,
        ),
        (
            "the signature truncated by one character",
            format!(
                "{}.{}.{}",
                segments[0],
                segments[1],
                &segments[2][..segments[2].len() - 1]
            ),
            Rejected::BadSignature,
        ),
        (
            "the signature dropped, as an unsecured JWS",
            format!("{}.{}.", segments[0], segments[1]),
            Rejected::Malformed(String::new()),
        ),
        (
            "the header downgraded to alg none",
            format!(
                "{}.{}.{}",
                segment(&json!({ "alg": "none", "typ": "JWT" })),
                segments[1],
                segments[2]
            ),
            Rejected::UnusableHeader(String::new()),
        ),
        (
            "the header naming a key the pool does not publish",
            format!(
                "{}.{}.{}",
                segment(&json!({ "alg": "RS256", "kid": "not-this-pools-key", "typ": "JWT" })),
                segments[1],
                segments[2]
            ),
            Rejected::UnknownKey(String::new()),
        ),
    ];

    for (case, tampered, expected) in cases {
        let outcome = verify_against_jwks(&tampered, &jwks);
        let rejected = match outcome {
            Ok(claims) => panic!("case {case:?}: must not verify, got the claims {claims}"),
            Err(rejected) => rejected,
        };
        assert_eq!(
            ::std::mem::discriminant(&rejected),
            ::std::mem::discriminant(&expected),
            "case {case:?}: expected {expected:?}, got {rejected:?}"
        );
    }

    // The genuine token still verifies, so the rows above failed on their
    // tampering and not on the fixture.
    assert!(
        verify_against_jwks(&token, &jwks).is_ok(),
        "the untampered token must still verify against the pool's key set"
    );
}

/// A pool's key set is the same on every read, and the tokens it issued keep
/// verifying against it: the key belongs to the pool, not to the call.
#[tokio::test]
async fn a_pools_key_set_is_stable_across_reads_and_sign_ins() {
    let (router, engine) = router_and_engine();

    // A pool has a key from the moment it is created, before anyone signs in,
    // and reading its key set does not mint one: two reads of a brand-new pool
    // answer the same document.
    let fresh = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "never-signed-in-pool" }),
    )
    .await;
    let fresh_id = fresh["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let fresh_keys = pool_jwks(&engine, &fresh_id);
    assert_eq!(
        fresh_keys,
        pool_jwks(&engine, &fresh_id),
        "a pool that has issued no token still publishes one stable key set, got {fresh_keys}"
    );
    assert!(
        fresh_keys["keys"][0]["kid"].as_str().is_some_and(|kid| !kid.is_empty()),
        "a created pool's key set names its key, got {fresh_keys}"
    );

    let row = sign_in(
        &router,
        &SignInCase {
            case: "two sign-ins of one user",
            pool_name: "stable-pool",
            username: "dave",
            groups: &[],
            email: None,
        },
    )
    .await;
    let first = pool_jwks(&engine, &row.user_pool_id);
    let second = pool_jwks(&engine, &row.user_pool_id);
    assert_eq!(
        first, second,
        "a pool's key set must not change between reads, got {first} then {second}"
    );
    assert_ne!(
        first["keys"][0]["kid"], fresh_keys["keys"][0]["kid"],
        "each pool gets its own key, so two pools never publish one kid"
    );

    let again = ok(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": row.user_pool_id,
            "ClientId": row.client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "dave", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    for (member, _) in TOKEN_MEMBERS {
        let sign_ins = [
            ("the first sign-in", &row.result),
            ("a later sign-in", &again["AuthenticationResult"]),
        ];
        for (case, result) in sign_ins {
            let token = result[member].as_str().unwrap_or_else(|| {
                panic!("case {case:?} / {member}: the sign-in must return it, got {result}")
            });
            assert!(
                verify_against_jwks(token, &first).is_ok(),
                "case {case:?} / {member}: every token of a pool verifies against that pool's \
                 one key set"
            );
        }
    }
}

/// A pool publishes its key set where a token's `iss` claim leads: an ANONYMOUS
/// GET of `/<UserPoolId>/.well-known/jwks.json` answers the document, and every
/// token the pool issued verifies against the document as SERVED.
#[tokio::test]
async fn a_pool_serves_its_key_set_at_its_well_known_path() {
    let (router, engine) = router_and_engine();
    let row = sign_in(
        &router,
        &SignInCase {
            case: "a pool whose key set is fetched over the wire",
            pool_name: "jwks-pool",
            username: "erin",
            groups: &["readers"],
            email: None,
        },
    )
    .await;

    // The location is the one the token's issuer leads a verifier to, composed
    // from the endpoint this suite's requests reach the service on.
    let jwks_uri = {
        let service_state = state::service_state(&*engine).expect("this service's state");
        let guard = service_state.lock();
        guard
            .pool(&row.user_pool_id)
            .expect("the stored pool")
            .jwks_uri("https://cognito-idp.us-east-1.amazonaws.com")
    };
    assert_eq!(
        jwks_uri,
        format!("{}/.well-known/jwks.json", row.issuer),
        "a pool publishes its key set under its own issuer"
    );
    let path = jwks_uri
        .strip_prefix("https://cognito-idp.us-east-1.amazonaws.com")
        .expect("the issuer is this endpoint");

    // Each row: a path on this endpoint, and whether it serves a key set. The
    // request carries NO credential — a key set is public — so it also pins
    // that the document is served before anything authenticates.
    let rows: Vec<(&str, String, bool)> = vec![
        ("the pool's own key set", path.to_string(), true),
        (
            "a key set of a pool that does not exist",
            "/us-east-1_nosuchpool/.well-known/jwks.json".to_string(),
            false,
        ),
        (
            "a well-known document this pool publishes none of",
            format!(
                "/{}/.well-known/oauth-authorization-server",
                row.user_pool_id
            ),
            false,
        ),
    ];

    for (case, requested, serves) in rows {
        let request = http::Request::builder()
            .method("GET")
            .uri(&requested)
            .header("host", "cognito-idp.us-east-1.amazonaws.com")
            .body(Bytes::new())
            .expect("the request is well formed");
        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        if !serves {
            assert_ne!(
                status, 200,
                "case {case:?}: {requested} must not serve a key set, got 200 with {:?}",
                String::from_utf8_lossy(response.body())
            );
            continue;
        }
        assert_eq!(
            status,
            200,
            "case {case:?}: {requested} must serve the key set, got {status} with {:?}",
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/json"),
            "case {case:?}: a key set is served as JSON, got {:?}",
            response.headers()
        );
        let served: Value = serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the key set must be JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        });
        assert_eq!(
            served,
            pool_jwks(&engine, &row.user_pool_id),
            "case {case:?}: the served document must be the pool's stored key set"
        );
        for (member, _) in TOKEN_MEMBERS {
            let token = row.result[member]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the sign-in must return {member}"));
            assert!(
                verify_against_jwks(token, &served).is_ok(),
                "case {case:?} / {member}: the token must verify against the key set as SERVED"
            );
        }
    }
}
