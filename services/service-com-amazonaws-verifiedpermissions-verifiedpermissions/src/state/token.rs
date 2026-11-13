// SPDX-License-Identifier: CC0-1.0

//! The identity tokens an authorization call presents, as this service reads them.
//!
//! `IsAuthorizedWithToken` takes its principal from a token an identity source's
//! provider issued rather than from the request: "Verified Permissions validates each
//! token that is specified in a request by checking its expiration date and its
//! signature" (the operation's own model documentation), and the principal it decides
//! for, the parents that principal has and the attributes it carries all come from THAT
//! token's claims.
//!
//! Two consequences of reading the token rather than the provider follow, and both are
//! the documented behaviour:
//!
//! * a claim is in force exactly while a token carrying it is — "token revocation and
//!   resource deletion have no effect on the validity of a token in your policy store"
//!   (same documentation) — so a group membership the provider has since changed applies
//!   only from the next token it issues; and
//! * a claim is worth nothing unless the issuer signed it, so the signature is checked
//!   against the key set the issuer PUBLISHES ([`published_key_set`]) before a single
//!   claim is used for anything but finding that key set — wherever that set can be read
//!   at all, which is for the issuers this emulator itself hosts. A provider an
//!   `openIdConnectConfiguration` names publishes its key set at a URL outside the
//!   emulator, which makes no outbound connection to fetch one, so no key exists here to
//!   check such a token's signature against and what decides it is every rule its own
//!   value settles: the form (nothing unsecured), the issuer it claims, the member it
//!   arrived in, the client it was issued to and its validity window.
//!
//! Reading a presented token takes two stages, and a refusal says WHICH of them the token
//! failed at, because the two send a caller to different repairs: the body is first DECODED
//! into the claim set this reading requires ([`DecodedClaims`], a token the caller has to
//! repair), and only a token that decoded is then VERIFIED against the policy store's
//! identity sources — signature, kind, client and window (a token the caller presented in
//! good faith, which the store's trust configuration does not accept).
//!
//! Everything here is a READ: no token is recorded, consumed or counted, so two calls
//! presenting one token are answered alike.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

use super::{
    CognitoUserPoolSource, IdentitySource, IdentitySourceConfiguration, OpenIdConnectSource,
    OpenIdConnectTokens, PolicyStore, ScopeEntity,
};
use crate::ops::paths;

/// The clause an authorization call refuses a token whose signature does not check out
/// with: whatever the signature check reports becomes the reason, except that a key set
/// holding no matching key and a signature that does not check out are ONE answer to the
/// caller — the issuer did not sign this token — rather than a report of which key the
/// service looked for.
const SIGNATURE_INVALID: &str = "signature is invalid";

/// The clause a token whose BODY is not a claim set is refused with, in front of what the
/// claim-set deserialization itself reported: which half of the token failed to decode —
/// its body, where the claims are, rather than its header — and that what failed was
/// reading JSON out of it.
const INVALID_BODY: &str = "Invalid body: JSON error";

/// The claim naming the issuer that minted a token (RFC 7519 §4.1.1), which is what says
/// WHICH identity source — and so which published key set — a presented token belongs to.
const ISSUER_CLAIM: &str = "iss";

/// The claim naming a token's subject (RFC 7519 §4.1.2): the immutable id of the user an
/// Amazon Cognito user pool issued the token for, and so the id its principal carries.
const SUBJECT_CLAIM: &str = "sub";

/// The claim naming who a token was issued FOR (RFC 7519 §4.1.3), which for an Amazon
/// Cognito ID token is the app client that requested it.
const AUDIENCE_CLAIM: &str = "aud";

/// The claim an Amazon Cognito ACCESS token names its app client in, which is that
/// token's audience by another name.
const CLIENT_ID_CLAIM: &str = "client_id";

/// The claim stating what a token may be used FOR, which Amazon Cognito stamps on every
/// token it issues.
const TOKEN_USE_CLAIM: &str = "token_use";

/// The claim after which a token is no longer valid (RFC 7519 §4.1.4), in seconds since
/// the epoch — the "expiration date" the operation documents checking.
const EXPIRY_CLAIM: &str = "exp";

/// The claim before which a token is not yet valid (RFC 7519 §4.1.5), same units.
const NOT_BEFORE_CLAIM: &str = "nbf";

/// The claim an Amazon Cognito user pool carries a user's group memberships in: "a user
/// who is in a group ... populates a `cognito:groups` claim to their access and identity
/// tokens" (the user-pool service's own model).
const GROUPS_CLAIM: &str = "cognito:groups";

/// The claim an access token carries its scopes in: a space-delimited string
/// (RFC 6749 §3.3), so it maps to a SET of strings rather than to one string.
const SCOPE_CLAIM: &str = "scope";

/// The claims this service READS to decide whether a token is valid and whose principal
/// it names, and which are therefore not mapped to principal attributes: the registered
/// JWT claims the validation uses, the two claims naming the app client and the token's
/// purpose, and the per-token identifiers an issuer stamps on a token so it can be told
/// apart from the next one. Everything else a token claims is an attribute of the
/// principal it names.
const CONSUMED_CLAIMS: [&str; 11] = [
    AUDIENCE_CLAIM,
    CLIENT_ID_CLAIM,
    EXPIRY_CLAIM,
    ISSUER_CLAIM,
    NOT_BEFORE_CLAIM,
    TOKEN_USE_CLAIM,
    "auth_time",
    "event_id",
    "iat",
    "jti",
    "origin_jti",
];

/// The separator between the id of the provider a principal came from and the id that
/// provider knows it by, in the entity id Verified Permissions generates for it
/// (`<user pool id>|<sub>` for an Amazon Cognito user pool,
/// `<entityIdPrefix>|<claim>` for an OpenID Connect provider).
const PROVIDER_SEPARATOR: char = '|';

/// Which member of an authorization request a presented token arrived in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// `accessToken`: "Must be an access token. Verified Permissions returns an error if
    /// the `token_use` claim in the submitted token isn't `access`."
    Access,
    /// `identityToken`: "Must be an ID token. Verified Permissions returns an error if the
    /// `token_use` claim in the submitted token isn't `id`."
    Identity,
}

impl TokenKind {
    /// How a refusal NAMES this token. The service words a token it could not verify as a
    /// sentence about the member ("Failed to verify IdentityToken: ..."), in which the
    /// member is spelled in the API's own capitalisation of it.
    pub fn reported_name(self) -> &'static str {
        match self {
            Self::Access => "AccessToken",
            Self::Identity => "IdentityToken",
        }
    }

    /// The JSON pointer of the request member this token arrived in, which a
    /// `ValidationException`'s `fieldList` names.
    pub fn path(self) -> &'static str {
        match self {
            Self::Access => paths::ACCESS_TOKEN,
            Self::Identity => paths::IDENTITY_TOKEN,
        }
    }

    /// The `token_use` claim a token presented in this member must carry.
    pub fn token_use(self) -> &'static str {
        match self {
            Self::Access => "access",
            Self::Identity => "id",
        }
    }
}

/// One token an authorization call presented: the member it arrived in and its value.
#[derive(Debug, Clone, Copy)]
pub struct PresentedToken<'a> {
    pub kind: TokenKind,
    pub value: &'a str,
}

/// What a verified token names: the principal to decide for, the parent entities its
/// group claim maps to, and the attributes its remaining claims map to.
#[derive(Debug, Clone)]
pub struct TokenIdentity {
    /// The principal entity, whose type is the identity source's `principalEntityType`
    /// and whose id is the provider's id for the user.
    pub principal: ScopeEntity,
    /// The groups the token claims, as the entities the identity source's group
    /// configuration maps them to. Empty when the source maps none — then the claim is a
    /// principal ATTRIBUTE instead, because nothing says which entity type it would name.
    pub groups: Vec<ScopeEntity>,
    /// The principal's attributes: one per claim the validation did not consume.
    pub attributes: Map<String, Value>,
    /// The entity types the identity SOURCE generates: its `principalEntityType` and, when
    /// it maps groups, the entity type it maps them to.
    ///
    /// They are principal types of the store, so an authorization request may not supply
    /// entities of them — this call builds those entities itself, out of the token. They
    /// come from the SOURCE's configuration and not from the groups this token happens to
    /// carry, so the rule holds for a token naming no group too;
    /// [`crate::state::cedar::check_token_request_entities`] is what applies it, together
    /// with the principal types the store's schema declares.
    pub owned_entity_types: Vec<String>,
}

impl TokenIdentity {
    /// The Cedar entity store this identity contributes: the principal, carrying its
    /// attributes and its groups as parents, plus one entity per group so the hierarchy
    /// the principal is `in` is present in the slice.
    pub fn entities(&self) -> Vec<Value> {
        let parents: Vec<Value> = self
            .groups
            .iter()
            .map(|group| uid_json(&group.entity_type, &group.entity_id))
            .collect();
        let mut entities = vec![serde_json::json!({
            "uid": uid_json(&self.principal.entity_type, &self.principal.entity_id),
            "attrs": Value::Object(self.attributes.clone()),
            "parents": Value::Array(parents),
        })];
        entities.extend(self.groups.iter().map(|group| {
            serde_json::json!({
                "uid": uid_json(&group.entity_type, &group.entity_id),
                "attrs": {},
                "parents": [],
            })
        }));
        entities
    }

    /// The identity BOTH tokens of a call name, when a call presented two: the one
    /// principal they agree on, the union of their groups, and their claims merged — with
    /// `later`'s value kept where both carry a claim.
    ///
    /// A call may present an access token and an identity token, and "the
    /// `IsAuthorizedWithToken` operation takes principal attributes from only the
    /// `identityToken` or `accessToken` passed to the operation", so the attributes of
    /// both are the principal's. Two tokens naming DIFFERENT principals describe no one
    /// caller, which is the `None`.
    pub fn merged_with(mut self, later: Self) -> Option<Self> {
        if self.principal != later.principal {
            return None;
        }
        for group in later.groups {
            if !self.groups.contains(&group) {
                self.groups.push(group);
            }
        }
        for (claim, value) in later.attributes {
            self.attributes.insert(claim, value);
        }
        for entity_type in later.owned_entity_types {
            if !self.owned_entity_types.contains(&entity_type) {
                self.owned_entity_types.push(entity_type);
            }
        }
        Some(self)
    }
}

/// `{"type": T, "id": I}` — the uid form Cedar's entity JSON carries.
fn uid_json(entity_type: &str, entity_id: &str) -> Value {
    serde_json::json!({ "type": entity_type, "id": entity_id })
}

/// Why a presented token was refused.
#[derive(Debug, Clone)]
pub enum TokenRefusal {
    /// The token's BODY is not the claim set this service reads one as: a claim it has to
    /// carry is absent, or one it carries is not of that claim's type. Nothing about the
    /// token was verified — the claim set a verification reads is not there to read — so
    /// the refusal reports the stage that failed, which is DECODING the token rather than
    /// verifying it: a caller repairs the TOKEN, not the trust the policy store configures.
    /// `kind` is the member it arrived in, and `reason` what deserializing that body
    /// reported: the claim at fault and its position in the body, which is what says where
    /// to repair it.
    NotDecoded { kind: TokenKind, reason: String },
    /// The token did not verify. `kind` is the member it arrived in, and `reason` the
    /// clause saying what about it failed; the operation words the sentence those two make
    /// and answers the modelled `ValidationException`.
    NotVerified { kind: TokenKind, reason: String },
    /// The token was issued to a client the identity source does not accept, so its
    /// audience is none of the ones the policy store takes. The service answers ONE fixed
    /// sentence for this ([`crate::ops::is_authorized_with_token`]), so the refusal
    /// carries nothing read off the rejected token — not the claim it was read from and
    /// not the value it held — only the member the token arrived in, which `fieldList`
    /// names.
    AudienceNotAccepted { kind: TokenKind },
    /// No identity source of this policy store names the issuer the token claims, so the
    /// call references an identity source the store does not hold. The request is well
    /// formed and what it names is MISSING, which is the modelled
    /// `ResourceNotFoundException` rather than a bad request. Carries the issuer the token
    /// claimed, which is how the request referenced that identity source.
    NoIdentitySource(String),
    /// The signature could not be CHECKED at all for an issuer this emulator itself HOSTS:
    /// the key set that issuer publishes in-process cannot be read, or it holds no key this
    /// service can check a signature under. That is not something the caller sent — the
    /// token may be perfectly good and this service simply cannot say — so it is answered
    /// as a service fault rather than as a token that failed to verify. An issuer OUTSIDE
    /// the emulator is not this case: nothing in this process ever publishes a key set for
    /// it, so there is no check to make and the token is judged by what its own value
    /// decides ([`verify`]). Carries the issuer and why.
    NotCheckable { issuer: String, reason: String },
}

impl TokenRefusal {
    /// Refuse the token in `kind` because `reason`.
    fn not_verified(kind: TokenKind, reason: impl Into<String>) -> Self {
        Self::NotVerified {
            kind,
            reason: reason.into(),
        }
    }

    /// Refuse the token in `kind` because its body is not a claim set, as `error` — the
    /// report of the deserialization that read that body — says.
    fn not_decoded(kind: TokenKind, error: &::serde_json::Error) -> Self {
        Self::NotDecoded {
            kind,
            reason: format!("{INVALID_BODY}: {error}"),
        }
    }
}

/// The claim set a presented token's body is DECODED into, before a single rule is applied
/// to what it claims.
///
/// The ISSUER is what this reading requires, because it is the one claim read before any
/// identity source is known: it says WHICH source — and so which published key set, which
/// accepted clients and which principal type — a presented token belongs to, so there is
/// nothing to read the remaining claims under until it is in hand. Every other claim is
/// read under the rule of the source the issuer selects ([`verify`]), so a token missing one
/// of those HAS decoded, and is refused by the rule that wanted it.
///
/// Deserialization is what REPORTS a body that is not this claim set, and its report is the
/// refusal's reason: serde names the claim at fault and `serde_json` the position of it in
/// the body being read. A token mostly carries claims this service does not require, so
/// every other member is ignored here and read later as an attribute of the principal.
#[derive(::serde::Deserialize)]
struct DecodedClaims {
    /// [`ISSUER_CLAIM`], under the name it travels under on the wire (an attribute takes a
    /// literal, so that claim's own constant cannot be named here).
    #[serde(rename = "iss")]
    issuer: String,
}

/// The bytes of a presented token's BODY: the middle segment of a compact JWS, base64url
/// decoded, which is the JSON a claim set is read out of.
///
/// The octets the CALLER sent, because the position a deserialization reports is a position
/// in them — re-encoding the claims this service already parsed would report a position in
/// that re-encoding instead. A value whose body cannot be isolated or decoded is no token
/// at all, which [`::service_common::jwt::read`] refuses before this is reached; it reads
/// here as an empty body, which is no claim set either.
fn token_body(presented: &str) -> Vec<u8> {
    presented
        .trim()
        .split('.')
        .nth(1)
        .and_then(::service_common::jwt::decode_base64url)
        .unwrap_or_default()
}

/// Verify one presented token against the policy store's identity sources and report the
/// identity it names.
///
/// The order is the one a verifier has to apply: the value is a token at all, its body
/// DECODES into the claim set this reading requires ([`DecodedClaims`]), it names an issuer
/// this store trusts, the issuer SIGNED it where that signature can be checked at
/// all — and only then are its remaining claims read. `engine` is the shared engine the
/// issuer's published key set is read from.
///
/// A signature is checked against the key set the token's ISSUER publishes, and the key
/// sets readable at all are the ones issuers this emulator HOSTS publish in this same
/// process ([`published_key_set`]). An identity source may name a provider the emulator
/// does not host, which publishes its key set at a URL outside the emulator — and the
/// emulator makes no outbound connection to fetch one — so for such an issuer there is no
/// key here to check any signature against, and the token is judged by every rule its own
/// value decides instead: it is a compact JWS, nothing unsecured (`alg` `none` or an empty
/// signature segment, which [`::service_common::jwt::read`] refuses), it names that issuer,
/// it arrived in the member the source processes, it was issued to a client the source
/// accepts and the clock has not run out on it.
pub fn verify<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &PolicyStore,
    presented: PresentedToken<'_>,
    now: SystemTime,
) -> Result<TokenIdentity, TokenRefusal> {
    let kind = presented.kind;
    let refuse = |reason: String| TokenRefusal::not_verified(kind, reason);
    let read = ::service_common::jwt::read(presented.value)
        .map_err(|defect| refuse(defect.to_string()))?;

    // The body is DECODED into the claim set this service reads one as before any rule is
    // applied to the claims: a body that is not that claim set failed to DECODE, and is
    // refused as such rather than as a token that failed to verify.
    let decoded: DecodedClaims = ::serde_json::from_slice(&token_body(presented.value))
        .map_err(|error| TokenRefusal::not_decoded(kind, &error))?;

    let issuer = Some(decoded.issuer.trim())
        .filter(|issuer| !issuer.is_empty())
        .ok_or_else(|| refuse("the token names no issuer".to_string()))?
        .to_string();
    let source = matching_source(store, &issuer, kind)
        .ok_or_else(|| TokenRefusal::NoIdentitySource(issuer.clone()))?;

    match published_key_set(engine, &issuer) {
        Some(key_set) => {
            ::service_common::jwt::verify(presented.value, &key_set).map_err(|defect| {
                match defect {
                    // Both say the same thing to a caller: no key of this issuer signed
                    // this token.
                    ::service_common::jwt::SignatureDefect::Invalid
                    | ::service_common::jwt::SignatureDefect::UnknownKey => {
                        refuse(SIGNATURE_INVALID.to_string())
                    }
                    // The check could not be MADE under the algorithm the token names or
                    // the key the issuer published for it, so nothing was decided about the
                    // token: reported as the service fault it is, never as the caller's
                    // token being bad.
                    defect @ (::service_common::jwt::SignatureDefect::UnsupportedAlgorithm(_)
                    | ::service_common::jwt::SignatureDefect::UnsupportedKey) => {
                        TokenRefusal::NotCheckable {
                            issuer: issuer.clone(),
                            reason: defect.to_string(),
                        }
                    }
                    // The form rules, which are decided from the token alone.
                    defect @ ::service_common::jwt::SignatureDefect::Form(_) => {
                        refuse(defect.to_string())
                    }
                }
            })?;
        }
        // An issuer this emulator HOSTS has its key set in this same process, so failing to
        // read it is this service's own fault and nothing was decided about the token.
        None if emulator_hosts_issuer(source) => {
            return Err(TokenRefusal::NotCheckable {
                issuer,
                reason: "it publishes no key set this service can read".to_string(),
            });
        }
        // An issuer outside the emulator publishes its key set where the emulator never
        // reaches, so there is no signature check to make: the rules the token's own value
        // decides are the ones that apply, and they continue below.
        None => {}
    }

    // Past here the token has met every check this service could make of it: the claims of
    // a token an issuer whose key set is readable here signed, and otherwise the claims of
    // a well-formed token naming an issuer the store's own identity source trusts.
    if let Some(expected) = expected_token_use(source, kind) {
        let claimed = read.claim_str(TOKEN_USE_CLAIM).unwrap_or_default();
        if claimed != expected {
            return Err(refuse(format!(
                "its {TOKEN_USE_CLAIM} claim is {claimed:?}, not {expected:?}"
            )));
        }
    }
    if !audience_accepted(source, &read) {
        return Err(TokenRefusal::AudienceNotAccepted { kind });
    }
    check_window(&read, now).map_err(refuse)?;

    let principal = principal_entity(source, &read, kind).map_err(refuse)?;
    let mapping = group_mapping(source);
    let groups = group_entities(mapping.as_ref(), &read);
    let attributes = principal_attributes(mapping.as_ref(), &read);
    // From the SOURCE's configuration, so an `entities` payload of the source's own group
    // type is refused whether or not THIS token names a group.
    let mut owned_entity_types = vec![source.principal_entity_type.clone()];
    if let Some(mapped) = mapping.as_ref() {
        if !owned_entity_types.iter().any(|owned| owned == mapped.entity_type) {
            owned_entity_types.push(mapped.entity_type.to_string());
        }
    }
    Ok(TokenIdentity {
        principal,
        groups,
        attributes,
        owned_entity_types,
    })
}

/// The identity source of `store` that issued a token claiming `issuer`, when the source
/// also accepts a token presented in `kind`'s member.
///
/// An identity source IS an issuer this store trusts, so matching one is what makes a
/// token's claims usable here and what decides which entity types its principal carries.
/// A token whose issuer matches none references an identity source the store does not
/// hold, which is a resource that does not exist rather than a token that is wrong.
/// An OpenID Connect source "processes either identity (ID) or access tokens"
/// (`OpenIdConnectTokenSelection`), never both, so one presented in the other member
/// matches no source rather than being read under the wrong rule.
///
/// A user-pool source trusts the POOL its ARN names, and a pool issues on the endpoint
/// each of its callers reached it at, so the token's issuer is read with the user-pool
/// service's own reading of which issuers that pool publishes rather than compared to the
/// one URL this source reports.
fn matching_source<'s>(
    store: &'s PolicyStore,
    issuer: &str,
    kind: TokenKind,
) -> Option<&'s IdentitySource> {
    store.identity_sources.values().find(|source| {
        match &source.configuration {
            IdentitySourceConfiguration::CognitoUserPool(cognito) => {
                ::service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice
                    ::state::is_pool_issuer(&cognito.region, &cognito.user_pool_id, issuer)
            }
            IdentitySourceConfiguration::OpenIdConnect(oidc) => {
                oidc.issuer == issuer
                    && match (&oidc.token_selection, kind) {
                        (OpenIdConnectTokens::AccessToken { .. }, TokenKind::Access)
                        | (OpenIdConnectTokens::IdentityToken { .. }, TokenKind::Identity) => true,
                        (OpenIdConnectTokens::AccessToken { .. }, TokenKind::Identity)
                        | (OpenIdConnectTokens::IdentityToken { .. }, TokenKind::Access) => false,
                    }
            }
        }
    })
}

/// The `token_use` claim a token of `kind` must carry for `source`, when the provider
/// stamps one.
///
/// An Amazon Cognito user pool stamps every token it issues with what it may be used for,
/// and the operation documents refusing a token presented in the wrong member. An OpenID
/// Connect provider is not required to stamp anything — which of its tokens a policy store
/// processes is the source's own `tokenSelection`, already applied by
/// [`matching_source`] — so no claim is required of it.
fn expected_token_use(source: &IdentitySource, kind: TokenKind) -> Option<&'static str> {
    match &source.configuration {
        IdentitySourceConfiguration::CognitoUserPool(_) => Some(kind.token_use()),
        IdentitySourceConfiguration::OpenIdConnect(_) => None,
    }
}

/// Whether the client a token was issued to is one the identity source accepts.
///
/// A source names the clients whose tokens the policy store accepts — `clientIds` for an
/// Amazon Cognito user pool, and for the two arms of an OpenID Connect token selection the
/// `audiences` / `clientIds` the model calls "the access token `aud` claim values that you
/// want to accept" and "the ID token audience, or client ID, claim values that you want to
/// accept" — and a token names who it was issued FOR in its own audience claims
/// ([`token_audiences`]). All three configurations are therefore the one rule: one of the
/// token's audiences must be one the source lists. A source that lists none accepts any
/// client of its issuer, which is what an empty list means on the wire.
fn audience_accepted(source: &IdentitySource, read: &::service_common::jwt::CompactJws) -> bool {
    let allowed: &[String] = match &source.configuration {
        IdentitySourceConfiguration::CognitoUserPool(cognito) => &cognito.client_ids,
        IdentitySourceConfiguration::OpenIdConnect(OpenIdConnectSource {
            token_selection: OpenIdConnectTokens::AccessToken { audiences, .. },
            ..
        }) => audiences,
        IdentitySourceConfiguration::OpenIdConnect(OpenIdConnectSource {
            token_selection: OpenIdConnectTokens::IdentityToken { client_ids, .. },
            ..
        }) => client_ids,
    };
    if allowed.is_empty() {
        return true;
    }
    let claimed = token_audiences(read);
    claimed
        .iter()
        .any(|claimed| allowed.iter().any(|allowed| allowed == claimed))
}

/// Who a token was issued FOR, as its own claims name them.
///
/// [`AUDIENCE_CLAIM`] is "either a single case-sensitive string or an array of strings"
/// (RFC 7519 §4.1.3), so a token naming several audiences names each of them; and a token
/// carrying no `aud` at all names its client in [`CLIENT_ID_CLAIM`] instead — the audience
/// of an access token by another name (RFC 9068 §2.2's `client_id`, which is the claim an
/// Amazon Cognito access token carries). Empty for a token that names neither, which no
/// non-empty list of accepted audiences matches.
///
/// Each value is the claim's own string, taken as the issuer wrote it: §4.1.3 makes an
/// audience a case-sensitive string compared as a `StringOrURI`, so nothing here is
/// trimmed or folded — only a value that is empty, and so names nobody, is dropped.
fn token_audiences(read: &::service_common::jwt::CompactJws) -> Vec<&str> {
    let claimed: Vec<&str> = match read.claims.get(AUDIENCE_CLAIM) {
        Some(Value::String(single)) => vec![single.as_str()],
        Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let claimed: Vec<&str> = claimed
        .into_iter()
        .filter(|audience| !audience.is_empty())
        .collect();
    if claimed.is_empty() {
        return read
            .claim_str(CLIENT_ID_CLAIM)
            .filter(|client| !client.is_empty())
            .into_iter()
            .collect();
    }
    claimed
}

/// Refuse a token the clock has run out on, or one that is not yet valid.
///
/// "Verified Permissions validates each token that is specified in a request by checking
/// its expiration date and its signature", so a token past its `exp` is refused however
/// good its signature is. `nbf` is the same rule at the other end of the window, and is
/// checked only when the token carries it.
fn check_window(read: &::service_common::jwt::CompactJws, now: SystemTime) -> Result<(), String> {
    let seconds = |claim: &str| read.claims.get(claim).and_then(Value::as_i64);
    let now = now
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default();
    match seconds(EXPIRY_CLAIM) {
        Some(expiry) if expiry <= now => return Err("the token is expired".to_string()),
        Some(_) => {}
        None => return Err(format!("the token names no {EXPIRY_CLAIM} claim")),
    }
    if seconds(NOT_BEFORE_CLAIM).is_some_and(|not_before| now < not_before) {
        return Err("the token is not yet valid".to_string());
    }
    Ok(())
}

/// The principal entity a verified token names.
///
/// The TYPE is the identity source's `principalEntityType`, which is what the policy store
/// writes its policies against. The ID is the provider's own id for the user, prefixed by
/// what identifies the provider: a user pool generates
/// `<user pool id>|<sub>`, and an OpenID Connect source
/// `<entityIdPrefix>|<the principalIdClaim's value>` — the `entityIdPrefix` being "a
/// string that will be prepended to the entity id" of such a principal, absent for a
/// source that configured none.
fn principal_entity(
    source: &IdentitySource,
    read: &::service_common::jwt::CompactJws,
    kind: TokenKind,
) -> Result<ScopeEntity, String> {
    let (claim, prefix) = match &source.configuration {
        IdentitySourceConfiguration::CognitoUserPool(cognito) => {
            (SUBJECT_CLAIM, Some(cognito.user_pool_id.clone()))
        }
        IdentitySourceConfiguration::OpenIdConnect(oidc) => {
            let claim = match (&oidc.token_selection, kind) {
                (OpenIdConnectTokens::AccessToken {
                    principal_id_claim, ..
                }, _)
                | (OpenIdConnectTokens::IdentityToken {
                    principal_id_claim, ..
                }, _) => principal_id_claim.as_str(),
            };
            (claim, oidc.entity_id_prefix.clone())
        }
    };
    let value = read
        .claim_str(claim)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            format!("the token names no {claim} claim, so it identifies no principal")
        })?;
    Ok(ScopeEntity {
        entity_type: source.principal_entity_type.clone(),
        entity_id: prefixed_id(prefix.as_deref(), value),
    })
}

/// How an identity source maps a token's groups to entities: the CLAIM that carries them,
/// the entity TYPE they are, and the prefix their entity ids carry. `None` for a source
/// that maps none, whose group claim is then just another claim.
///
/// A group claim becomes a PARENT of the principal only where the source says which entity
/// type its groups are, because an entity needs a type to be one at all. An Amazon Cognito
/// user pool source always has one — the type the write of the source named, or the
/// service's own default ([`super::DEFAULT_COGNITO_GROUP_ENTITY_TYPE`]) resolved into the
/// stored source where it named none — while an OpenID Connect source maps groups only
/// when it configured a `groupConfiguration`, which is also what names the claim they
/// arrive in: a user pool populates [`GROUPS_CLAIM`], and an OpenID Connect provider
/// whichever claim its `groupClaim` names.
///
/// Either way the type comes out of STATE and is not defaulted here, so the type an
/// authorization call maps a group to is the one every read of the identity source
/// reports.
struct GroupMapping<'s> {
    claim: &'s str,
    entity_type: &'s str,
    prefix: Option<&'s str>,
}

fn group_mapping(source: &IdentitySource) -> Option<GroupMapping<'_>> {
    match &source.configuration {
        IdentitySourceConfiguration::CognitoUserPool(CognitoUserPoolSource {
            group_entity_type,
            user_pool_id,
            ..
        }) => Some(GroupMapping {
            claim: GROUPS_CLAIM,
            entity_type: group_entity_type.as_str(),
            prefix: Some(user_pool_id.as_str()),
        }),
        IdentitySourceConfiguration::OpenIdConnect(OpenIdConnectSource {
            group_configuration: Some(groups),
            entity_id_prefix,
            ..
        }) => Some(GroupMapping {
            claim: groups.group_claim.as_str(),
            entity_type: groups.group_entity_type.as_str(),
            prefix: entity_id_prefix.as_deref(),
        }),
        IdentitySourceConfiguration::OpenIdConnect(_) => None,
    }
}

/// The groups a verified token claims, as the entities `mapping` maps them to.
///
/// A claim holding one name rather than a list is read as that one group, so a provider
/// that renders a single membership as a string is not silently ignored.
///
/// How many entities this contributes is bounded by the TOKEN: each group is a name inside
/// the claim set, and `service_common::jwt` reads no token longer than its own
/// `MAX_TOKEN_BYTES`, so the claim cannot name more groups than fit in it.
fn group_entities(
    mapping: Option<&GroupMapping<'_>>,
    read: &::service_common::jwt::CompactJws,
) -> Vec<ScopeEntity> {
    let Some(mapping) = mapping else {
        return Vec::new();
    };
    let names: Vec<&str> = match read.claims.get(mapping.claim) {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        Some(Value::String(name)) => vec![name.as_str()],
        _ => Vec::new(),
    };
    let mut groups: Vec<ScopeEntity> = Vec::new();
    for name in names {
        let group = ScopeEntity {
            entity_type: mapping.entity_type.to_string(),
            entity_id: prefixed_id(mapping.prefix, name),
        };
        if !groups.contains(&group) {
            groups.push(group);
        }
    }
    groups
}

/// The attributes a verified token's claims map to.
///
/// Every claim the validation did not consume ([`CONSUMED_CLAIMS`]) is an attribute of the
/// principal the token names, under the claim's own name. Two claims are not mapped
/// verbatim:
///
/// * the claim `mapping` maps to the principal's PARENTS — it is the hierarchy then, not
///   an attribute; and
/// * `scope`, which travels as one space-delimited string (RFC 6749 §3.3) and is therefore
///   the set of scopes it lists rather than the string that lists them.
fn principal_attributes(
    mapping: Option<&GroupMapping<'_>>,
    read: &::service_common::jwt::CompactJws,
) -> Map<String, Value> {
    let mut attributes = Map::new();
    for (claim, value) in &read.claims {
        if CONSUMED_CLAIMS.contains(&claim.as_str()) {
            continue;
        }
        if mapping.is_some_and(|mapping| mapping.claim == claim) {
            continue;
        }
        if claim == SCOPE_CLAIM {
            if let Some(scopes) = value.as_str() {
                attributes.insert(
                    claim.clone(),
                    Value::Array(
                        scopes
                            .split_whitespace()
                            .map(|scope| Value::String(scope.to_string()))
                            .collect(),
                    ),
                );
                continue;
            }
        }
        attributes.insert(claim.clone(), value.clone());
    }
    attributes
}

/// `<prefix>|<value>`, or `value` alone for a provider that prefixes nothing.
fn prefixed_id(prefix: Option<&str>, value: &str) -> String {
    match prefix.filter(|prefix| !prefix.is_empty()) {
        Some(prefix) => format!("{prefix}{PROVIDER_SEPARATOR}{value}"),
        None => value.to_string(),
    }
}

/// The key set `issuer` publishes, read from the service that OWNS that issuer in this
/// same shared engine.
///
/// A verifier checks a token against the key its header names in the key set the token's
/// ISSUER publishes. The issuers this emulator serves are its own Amazon Cognito user
/// pools: each publishes its key set at `<issuer>/.well-known/jwks.json` and holds it in
/// the user-pool service's state in this engine — the one store that endpoint serves it
/// from — so the set is read from there rather than fetched over a network this emulator
/// never leaves. Only PUBLIC key parameters are read: no private key material crosses
/// this seam.
///
/// `None` when nothing in this process publishes a key set for `issuer`, which is every
/// issuer outside the emulator — the case [`emulator_hosts_issuer`] tells apart from an
/// issuer the emulator does host whose key set went missing.
///
/// LOCK ORDER: this takes the user-pool service's state lock, and its caller holds this
/// service's. Nothing in the user-pool service reads this service's state, so that order
/// is the only one taken — a future reader of ITS state from here must keep it.
pub fn published_key_set<Eng: Send + Sync + 'static>(engine: &Eng, issuer: &str) -> Option<Value> {
    let pools = ::service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice
        ::state::service_state(engine)?;
    // A user pool's issuer URL ends in the pool's own id, which is how the pool that
    // publishes a key set is found; the whole URL still has to agree, because a pool
    // answers only for an issuer it publishes — one naming an endpoint this emulator
    // serves that pool at, which is the pool's own reading.
    let user_pool_id = issuer.rsplit('/').next()?.to_string();
    let pools = pools.lock();
    let pool = pools.pool(&user_pool_id)?;
    pool.publishes_issuer(issuer).then(|| pool.jwks())
}

/// Whether the provider `source` points at is one this EMULATOR hosts, so that the key set
/// its issuer publishes is held in this same process and
/// [`published_key_set`] answering `None` for it is this service's own failure rather than
/// the issuer being unreachable.
///
/// The emulated provider is the Amazon Cognito user pool: a source pointing at one names a
/// pool that had to exist in this engine for the source to be written
/// ([`super::resolve_user_pool_arn`]), and the pool publishes its key set in the user-pool
/// service's state. An `openIdConnectConfiguration` names its provider by an `issuer` URL
/// the write accepts as given — "the issuer URL of an OIDC identity provider" the policy
/// store trusts — and that provider lives wherever that URL points, outside the emulator.
fn emulator_hosts_issuer(source: &IdentitySource) -> bool {
    match &source.configuration {
        IdentitySourceConfiguration::CognitoUserPool(_) => true,
        IdentitySourceConfiguration::OpenIdConnect(_) => false,
    }
}
