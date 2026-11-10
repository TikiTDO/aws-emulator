// SPDX-License-Identifier: CC0-1.0

//! The `COGNITO_USER_POOLS` authorizer, as the `execute-api` data plane evaluates it for one
//! invoke.
//!
//! `Method$authorizationType` gives `COGNITO_USER_POOLS` "for using a Cognito user pool", and
//! the `Authorizer` the method names carries the two members that decide such a call:
//!
//! * `providerARNs` — "a list of the Amazon Cognito user pool ARNs for the
//!   `COGNITO_USER_POOLS` authorizer", which is WHICH pools' tokens this authorizer accepts;
//! * `identitySource` — "the identity source for which authorization is requested … for a
//!   `TOKEN` or `COGNITO_USER_POOLS` authorizer, this is required and specifies the request
//!   header mapping expression for the custom header holding the authorization token".
//!
//! Unlike a `TOKEN` or `REQUEST` authorizer, whose verdict is a Lambda function's return
//! value, this one is a FUNCTION OF THE TOKEN AND OF THE POOLS the authorizer names — and
//! every user pool this emulator serves is held in this same process, publishing its own key
//! set (`UserPool::jwks`, the document a pool serves at `<issuer>/.well-known/jwks.json`). So
//! the decision is computable here in full, and it is computed the way any consumer of a pool
//! token computes it:
//!
//! 1. the token is a compact JWS at all ([`::service_common::jwt::read`], the tree's one
//!    bounded reading of one, which also refuses the unsecured form nothing signed);
//! 2. its `iss` names a pool the authorizer's `providerARNs` list — a token minted by any
//!    other issuer, a pool of another account, or a pool this emulator does not hold, is a
//!    token this authorizer does not accept;
//! 3. the SIGNATURE checks out against that pool's published key set
//!    ([`::service_common::jwt::verify`], the RFC 7515 §5.2 check): only PUBLIC key
//!    parameters are read, so a token that verifies here is one any holder of the same key
//!    set could have verified;
//! 4. the token is inside its own validity window — `exp`, and `nbf` where it carries one
//!    (RFC 7519 §4.1.4/§4.1.5);
//! 5. where the authorizer declares an `identityValidationExpression`, the token's audience
//!    matches it: "for `COGNITO_USER_POOLS` authorizers, API Gateway will match the `aud`
//!    field of the incoming token from the client against the specified regular expression"
//!    (`Authorizer$identityValidationExpression`);
//! 6. where the METHOD declares `authorizationScopes`, the token claims one of them: "the
//!    authorization works by matching the method scopes against the scopes parsed from the
//!    access token in the incoming request. The method invocation is authorized if any method
//!    scopes matches a claimed scope in the access token. Otherwise, the invocation is not
//!    authorized" (`Method$authorizationScopes`) — which is also why "when the method scope is
//!    configured, the client must provide an access token instead of an identity token": an
//!    identity token claims no scope at all, so it matches none.
//!
//! A token that fails any of those is refused, and WHICH refusal it earns is the class the
//! failure belongs to ([`Refusal`]) — never the `500` of an authorizer whose decision could
//! not be obtained: the distinction between "fix the token" and "the authorizer is broken" is
//! the whole point of the two response types. Almost every failure is one refusal: the request
//! authorized nobody, which is `401`. Two classes are their own, because the recorded answers
//! of this endpoint distinguish them: a token whose own `exp` has elapsed carries the sentence
//! of an EXPIRED token at that same `401`, and a token that claims a pool this authorizer
//! TRUSTS as its issuer and does not VERIFY against that pool's published key set — a value
//! presenting itself as a credential that pool never signed — is refused as the authorizer
//! denying the call, `403`. A token that passes all of the rules AUTHORIZES the call, and its
//! claim set is published to the integration as `$context.authorizer.claims.<name>`
//! ([`Authorized::claims`]), which is where a mapping template reads the signed-in user from.
//!
//! Three of the rules above — the RFC 6750 presentation prefix, the `aud`/`client_id` audience
//! reading and the `exp`/`nbf` window — are also implemented by the JWT authorizer of
//! `service-com-amazonaws-apigatewayv2-apigatewayv2` (`src/service/jwt.rs`) and by
//! `service-com-amazonaws-verifiedpermissions-verifiedpermissions`, and
//! `services/common/src/jwt.rs` records both as copies to be MIGRATED onto that shared module.
//! This change adds them THERE — [`jwt::without_bearer_scheme`], [`jwt::CompactJws::audiences`]
//! and [`jwt::CompactJws::outside_window`], beside the scopes reading
//! [`jwt::CompactJws::claimed_scopes`] — so nothing in this module reads a claim by hand; the
//! other two copies are migrated by the changes that own those services.
//!
//! Nothing here is written: the pool's state is READ for the key set it publishes, and a token
//! is not consumed by being presented.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::service_common::jwt;

use crate::state::AuthorizerEntry;

/// The service namespace a user pool's ARN names, which is the user-pool service's own
/// `arnNamespace`.
const USER_POOL_ARN_VENDOR: &str = "cognito-idp";

/// The resource prefix a user pool's ARN carries:
/// `arn:aws:cognito-idp:{region}:{account}:userpool/{pool-id}`, the form
/// `Authorizer$providerARNs` lists.
const USER_POOL_ARN_RESOURCE: &str = "userpool/";

/// What this authorizer made of the token the request presented.
#[derive(Debug)]
pub enum Decision {
    /// The token is one a pool the authorizer names signed, inside its window, and its
    /// audience satisfies the authorizer's validation expression. The call is authorized and
    /// these are the claims the integration sees.
    Authorized(Authorized),
    /// The token is not one this authorizer accepts: the CLASS of refusal the request earns,
    /// and the reason — which is logged, never answered, because what the caller is told about
    /// a token it presented is the refusal's own sentence and nothing about the token.
    Refused(Refusal, String),
}

/// Which refusal a token this authorizer does not accept earns — the question the data plane
/// answers the request by ([`super::cognito_refusal`]).
///
/// A request this authorizer does not admit authorized nobody, which is the `401` every one of
/// its refusals is answered through except the two this endpoint words differently: the token
/// whose own `exp` has elapsed, which carries the sentence of an expired token at that same
/// `401`, and the token that claims a TRUSTED pool and does not verify against it, which is
/// answered as the authorizer denying the call. Which of the three a failed check belongs to is
/// what this names, so the rule lives beside the checks and not in the serving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The authorizer does not admit what the request presented, and nothing about the failure
    /// is worded differently from any other: no value at the identity source, a value that is
    /// not a signed token, a token naming no issuer, a token whose issuer is no user pool this
    /// authorizer's `providerARNs` name, a window that cannot be read or has not begun, an
    /// audience the `identityValidationExpression` does not match, or none of the method's
    /// `authorizationScopes` claimed.
    Unauthorized,
    /// The request presented a signed token whose own `exp` has passed: a credential that was
    /// one, and is not one now.
    Expired,
    /// The request presented a token claiming a pool this authorizer TRUSTS as its issuer whose
    /// signature does not check out against that pool's published key set: a value presenting
    /// itself as a credential of a trusted pool that the pool never signed.
    Unverified,
}

/// The authorization a presented token earned: its claim set, as the integration reads it.
#[derive(Debug, Default, Clone)]
pub struct Authorized {
    /// The token's claims, by claim name, each rendered as the text a mapping template's
    /// `$context.authorizer.claims.<name>` reference stands for.
    pub claims: BTreeMap<String, String>,
}

/// What `authorizer` makes of `presented` at `now`, for a method declaring `method_scopes`,
/// reading the key sets the user pools it names publish in this same process from `engine`.
///
/// `method_scopes` is the method's own `authorizationScopes`, empty for a method declaring
/// none — which is every method whose client presents an identity token.
///
/// LOCK ORDER: this takes the user-pool service's state lock and nothing else. Nothing in that
/// service reads this one's state, so the order this service's data plane takes — its own
/// store, then the user pool's — is the only one taken.
pub fn evaluate<Eng: Send + Sync + 'static>(
    engine: &Eng,
    authorizer: &AuthorizerEntry,
    method_scopes: &[String],
    presented: &str,
    now: SystemTime,
) -> Decision {
    // A presentation prefix is not part of the token: a bearer token travelling in
    // `Authorization` carries the RFC 6750 scheme in front of it, which the shared reader
    // strips — in whatever case the scheme was written, since an HTTP auth scheme is
    // case-insensitive.
    let token = jwt::without_bearer_scheme(presented);
    let read = match jwt::read(token) {
        Ok(read) => read,
        Err(defect) => {
            return Decision::Refused(
                Refusal::Unauthorized,
                format!("the presented value is not a token: {defect}"),
            )
        }
    };
    let Some(issuer) = read.claim_str(jwt::ISSUER_CLAIM) else {
        return Decision::Refused(
            Refusal::Unauthorized,
            "the presented token names no issuer".to_string(),
        );
    };
    let Some(key_set) = accepted_issuer_key_set(engine, authorizer, issuer) else {
        return Decision::Refused(
            Refusal::Unauthorized,
            "the presented token's issuer is no user pool this authorizer accepts".to_string(),
        );
    };
    // The signature over the token's OWN encoded bytes, against the key its header names in
    // the set the pool publishes: a token minted by one pool does not verify against another's
    // key set, and a claim set rewritten after signing no longer checks out.
    let verified = match jwt::verify(token, &key_set) {
        Ok(verified) => verified,
        Err(defect) => {
            return Decision::Refused(
                Refusal::Unverified,
                format!("the presented token is not one its issuer signed: {defect}"),
            )
        }
    };
    // Its own validity window, through the shared reading of RFC 7519 §4.1.4/§4.1.5. A token
    // carrying NO expiry is refused here rather than accepted without end: a pool mints no such
    // token, so a value presenting itself as one is not a token this authorizer accepts.
    if let Some(defect) = verified.outside_window(now) {
        return Decision::Refused(
            window_refusal(defect),
            format!("the presented token is outside its window: {defect}"),
        );
    }
    if let Some(unmatched) = audience_unmatched(authorizer, &verified) {
        return Decision::Refused(Refusal::Unauthorized, unmatched);
    }
    if !scopes_satisfied(&verified, method_scopes) {
        return Decision::Refused(
            Refusal::Unauthorized,
            "the presented token claims none of the method's authorization scopes".to_string(),
        );
    }
    Decision::Authorized(Authorized {
        claims: claims_of(&verified),
    })
}

/// The refusal a token outside its own validity window earns.
///
/// A token whose `exp` has PASSED is the one failure of this rule a holder can tell from the
/// refusal — "the time on or after which the JWT MUST NOT be accepted" (RFC 7519 §4.1.4) is
/// behind it, and the holder's remedy is to obtain another token — so that class is answered
/// with the sentence API Gateway words an expired token with
/// ([`super::EXPIRED_IDENTITY_TOKEN`]). The other two readings of the window are not a token
/// that has expired: one names no `exp` at all, which no pool mints, and one has not begun,
/// so each is a value that authenticates nobody at this instant.
fn window_refusal(defect: jwt::WindowDefect) -> Refusal {
    match defect {
        jwt::WindowDefect::Expired => Refusal::Expired,
        jwt::WindowDefect::NoExpiry | jwt::WindowDefect::NotYetValid => Refusal::Unauthorized,
    }
}

/// Whether `token` satisfies the method's `required` scopes, which is what
/// `Method$authorizationScopes` is matched against: "the method invocation is authorized if any
/// method scopes matches a claimed scope in the access token. Otherwise, the invocation is not
/// authorized".
///
/// A method requiring NONE is satisfied by every token. The scopes a token claims are the
/// shared reading of both spellings ([`jwt::CompactJws::claimed_scopes`]); a token carrying
/// neither — which is every IDENTITY token, and why "when the method scope is configured, the
/// client must provide an access token instead of an identity token" — claims nothing and so
/// matches no method scope.
fn scopes_satisfied(token: &jwt::CompactJws, required: &[String]) -> bool {
    if required.is_empty() {
        return true;
    }
    let claimed = token.claimed_scopes();
    required
        .iter()
        .any(|scope| claimed.contains(&scope.as_str()))
}

/// The key set of the user pool `issuer` names, when one of `authorizer`'s `providerARNs`
/// names that pool.
///
/// The pools the authorizer lists are what is looked UP — not the pool the token claims — so a
/// token naming a pool this emulator holds but the authorizer does not list resolves no key set
/// and is refused. Each listed ARN's `userpool/{id}` resource names the pool, the token's `iss`
/// has to be an issuer that pool publishes (`UserPool::publishes_issuer`, which is the pool's
/// id on an endpoint this emulator serves it at), and what is read out of the pool is the
/// PUBLIC key set it publishes.
fn accepted_issuer_key_set<Eng: Send + Sync + 'static>(
    engine: &Eng,
    authorizer: &AuthorizerEntry,
    issuer: &str,
) -> Option<::serde_json::Value> {
    let pools = ::service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice
        ::state::service_state(engine)?;
    let claimed = issuer.trim().trim_end_matches('/');
    let pools = pools.lock();
    authorizer
        .provider_arns
        .iter()
        .filter_map(|arn| user_pool_id_of(arn))
        .filter_map(|pool_id| pools.pool(pool_id))
        .find(|pool| pool.publishes_issuer(claimed))
        .map(|pool| pool.jwks())
}

/// The user-pool id a `providerARNs` entry names, or `None` for a value that names no user
/// pool.
///
/// The ARN's resource is [`USER_POOL_ARN_RESOURCE`] then the pool's own id, which is the form
/// `Authorizer$providerARNs` lists; the tree's one ARN parse reads the six fields so this
/// agrees with every other service on what an ARN is.
fn user_pool_id_of(arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse(arn.trim())?;
    if parsed.vendor != USER_POOL_ARN_VENDOR {
        return None;
    }
    let id = parsed.resource.strip_prefix(USER_POOL_ARN_RESOURCE)?.trim();
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

/// Why the token's audience does not satisfy `authorizer`'s `identityValidationExpression`, or
/// `None` when it does — including when the authorizer declares none, which matches every
/// token.
///
/// The audiences read are the shared ones ([`jwt::CompactJws::audiences`]): an IDENTITY token
/// names its app client in `aud`, an ACCESS token of the same pool in `client_id`, and a match
/// under either satisfies the expression.
///
/// An expression this engine does not build a regular expression of within its budgets decides
/// nothing: it is logged and the token is left to the checks that CAN be made, rather than
/// refusing a call on a reading of the configuration this emulator could not perform.
fn audience_unmatched(authorizer: &AuthorizerEntry, token: &jwt::CompactJws) -> Option<String> {
    let expression = authorizer
        .identity_validation_expression
        .as_deref()
        .map(str::trim)
        .filter(|expression| !expression.is_empty())?;
    let pattern = match super::stored_pattern(expression) {
        Some(pattern) => pattern,
        None => {
            tracing::warn!(
                authorizer = %authorizer.name,
                "the authorizer's identityValidationExpression is not a regular expression this \
                 emulator evaluates within its compiled-size budget, so it decides nothing \
                 about the presented token"
            );
            return None;
        }
    };
    if token
        .audiences()
        .iter()
        .any(|audience| pattern.is_match(audience))
    {
        return None;
    }
    Some(
        "the presented token's audience does not match the authorizer's identity validation \
         expression"
            .to_string(),
    )
}

/// The token's claims as the integration reads them: one entry per claim, keyed by its name.
///
/// A string claim is its own text. Every other JSON value is rendered as the compact JSON it
/// is — a number and a boolean as the scalar they are, a list (`cognito:groups`) as the array
/// — so what the integration sees is what the token carries rather than a lossy flattening of
/// it.
fn claims_of(token: &jwt::CompactJws) -> BTreeMap<String, String> {
    token
        .claims
        .iter()
        .map(|(name, value)| {
            let rendered = match value {
                ::serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            (name.clone(), rendered)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    //! The pure rules this module owns. What a deployed stage SERVES for a Cognito-authorized
    //! method is pinned end to end in
    //! `core/router/tests/all/apigateway_cognito_user_pools_authorizer.rs`.

    use super::*;

    /// A compact JWS over `claims`, in the shape a pool's token has (the signature is not
    /// checked by any function under test here).
    fn token(claims: ::serde_json::Value) -> jwt::CompactJws {
        let encoded = format!(
            "{}.{}.c2lnbmF0dXJl",
            jwt::encode_base64url(b"{\"alg\":\"ES256\",\"kid\":\"k\"}"),
            jwt::encode_base64url(claims.to_string().as_bytes())
        );
        jwt::read(&encoded).expect("the test builds a well-formed compact JWS")
    }

    /// RULE: a token outside its own validity window is refused as EXPIRED only when its `exp`
    /// has passed; a window that cannot be read or has not begun presents no credential.
    #[test]
    fn only_an_elapsed_expiry_is_refused_as_an_expired_token() {
        // Rows are every reading of the window the shared check reports.
        let rows: &[(&str, jwt::WindowDefect, Refusal)] = &[
            (
                "an expiry that has passed",
                jwt::WindowDefect::Expired,
                Refusal::Expired,
            ),
            (
                "no expiry at all",
                jwt::WindowDefect::NoExpiry,
                Refusal::Unauthorized,
            ),
            (
                "a window that has not begun",
                jwt::WindowDefect::NotYetValid,
                Refusal::Unauthorized,
            ),
        ];
        for (case, defect, expected) in rows {
            let actual = window_refusal(*defect);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a `providerARNs` entry names a user pool by the `userpool/{id}` resource of a
    /// `cognito-idp` ARN, and nothing else is read as one.
    #[test]
    fn a_provider_arn_names_a_user_pool_by_its_resource() {
        // Rows differ along the ARN's service, its resource shape and its spacing.
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "a user pool ARN",
                "arn:aws:cognito-idp:us-east-1:000000000000:userpool/us-east-1_abc123",
                Some("us-east-1_abc123"),
            ),
            (
                "the same ARN with surrounding space",
                "  arn:aws:cognito-idp:eu-west-1:000000000000:userpool/eu-west-1_z9  ",
                Some("eu-west-1_z9"),
            ),
            (
                "another partition",
                "arn:aws-cn:cognito-idp:cn-north-1:000000000000:userpool/cn-north-1_p",
                Some("cn-north-1_p"),
            ),
            (
                "an identity POOL, not a user pool",
                "arn:aws:cognito-identity:us-east-1:000000000000:identitypool/us-east-1:abc",
                None,
            ),
            (
                "a user pool client of a pool",
                "arn:aws:cognito-idp:us-east-1:000000000000:userpool/us-east-1_a/client/c",
                None,
            ),
            (
                "a pool ARN with no id",
                "arn:aws:cognito-idp:us-east-1:000000000000:userpool/",
                None,
            ),
            ("not an ARN at all", "us-east-1_abc123", None),
        ];
        for (case, arn, expected) in rows {
            let actual = user_pool_id_of(arn);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the authorizer's `identityValidationExpression` is matched against the token's
    /// audience — `aud` on an identity token, `client_id` on an access token — and an
    /// authorizer declaring none accepts every audience.
    #[test]
    fn the_validation_expression_is_matched_against_the_audience() {
        let authorizer = |expression: Option<&str>| AuthorizerEntry {
            name: "pool-authorizer".to_string(),
            identity_validation_expression: expression.map(str::to_string),
            ..AuthorizerEntry::default()
        };
        // Rows differ along the expression declared and along which audience spelling the
        // token carries.
        let rows: &[(&str, Option<&str>, ::serde_json::Value, bool)] = &[
            (
                "no expression at all",
                None,
                ::serde_json::json!({ "aud": "anything" }),
                true,
            ),
            (
                "an expression the aud matches",
                Some("^client-1$"),
                ::serde_json::json!({ "aud": "client-1" }),
                true,
            ),
            (
                "an expression the aud does not match",
                Some("^client-1$"),
                ::serde_json::json!({ "aud": "client-2" }),
                false,
            ),
            (
                "an access token, matched on client_id",
                Some("^client-1$"),
                ::serde_json::json!({ "client_id": "client-1" }),
                true,
            ),
            (
                "a token carrying no audience at all",
                Some("^client-1$"),
                ::serde_json::json!({ "sub": "u" }),
                false,
            ),
            (
                "an expression that is not a regular expression",
                Some("("),
                ::serde_json::json!({ "aud": "client-1" }),
                true,
            ),
        ];
        for (case, expression, claims, matched) in rows {
            let unmatched = audience_unmatched(&authorizer(*expression), &token(claims.clone()));
            assert_eq!(
                unmatched.is_none(),
                *matched,
                "case {case:?}: expected matched={matched}, got {unmatched:?}"
            );
        }
    }

    /// RULE: a method declaring `authorizationScopes` admits a token only when it claims one
    /// of them, and a method declaring none admits every token the other rules accept.
    #[test]
    fn a_methods_authorization_scopes_are_matched_against_the_tokens_claimed_scopes() {
        let required = |scopes: &[&str]| -> Vec<String> {
            scopes.iter().map(|scope| (*scope).to_string()).collect()
        };
        // Rows differ along how the token claims its scopes (the space-delimited `scope`
        // string, an `scp` list, neither) and along which scopes the method requires.
        let rows: &[(&str, ::serde_json::Value, &[&str], bool)] = &[
            (
                "a method requiring nothing",
                ::serde_json::json!({ "sub": "u" }),
                &[],
                true,
            ),
            (
                "one of the space-delimited scopes the token claims",
                ::serde_json::json!({ "scope": "things/read things/write" }),
                &["things/write"],
                true,
            ),
            (
                "a scope the token does not claim",
                ::serde_json::json!({ "scope": "things/read" }),
                &["things/write"],
                false,
            ),
            (
                "one of several required scopes",
                ::serde_json::json!({ "scope": "things/read" }),
                &["things/write", "things/read"],
                true,
            ),
            (
                "a scope claimed in an scp list",
                ::serde_json::json!({ "scp": ["things/read", "things/write"] }),
                &["things/write"],
                true,
            ),
            (
                "an identity token, which claims no scope at all",
                ::serde_json::json!({ "token_use": "id", "aud": "client-1" }),
                &["things/read"],
                false,
            ),
            (
                "a prefix of a claimed scope, which is a different scope",
                ::serde_json::json!({ "scope": "things/readonly" }),
                &["things/read"],
                false,
            ),
        ];
        for (case, claims, scopes, admitted) in rows {
            let satisfied = scopes_satisfied(&token(claims.clone()), &required(scopes));
            assert_eq!(
                satisfied, *admitted,
                "case {case:?}: expected admitted={admitted}, got {satisfied}"
            );
        }
    }

    /// RULE: every claim the token carries is published, a string as its own text and every
    /// other JSON value as the JSON it is.
    #[test]
    fn every_claim_the_token_carries_is_published() {
        let claims = claims_of(&token(::serde_json::json!({
            "sub": "user-1",
            "cognito:username": "alice",
            "auth_time": 1_700_000_000_i64,
            "email_verified": true,
            "cognito:groups": ["admins", "readers"],
        })));
        // Rows differ along the JSON type of the claim's value.
        let rows: &[(&str, &str, &str)] = &[
            ("a string claim", "sub", "user-1"),
            ("a namespaced string claim", "cognito:username", "alice"),
            ("a numeric claim", "auth_time", "1700000000"),
            ("a boolean claim", "email_verified", "true"),
            ("a list claim", "cognito:groups", "[\"admins\",\"readers\"]"),
        ];
        for (case, claim, expected) in rows {
            let actual = claims.get(*claim).map(String::as_str);
            assert_eq!(
                actual,
                Some(*expected),
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
