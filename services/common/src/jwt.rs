// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of a compact JWS — a JSON Web Token — for every service that is handed
//! one as a caller-supplied string and has to decide whether it is a token at all.
//!
//! A JWT is three `.`-separated base64url segments: a JOSE header and a claim set, each
//! base64url-encoded JSON, and a signature (RFC 7515 §3.1, RFC 7519 §3). Several emulated
//! services are handed such a value — an HTTP api's JWT authorizer evaluates a presented
//! bearer token, a cluster's authentication reads a service-account token, a Linux
//! subscription registration reads the provider access token held in a secret — and each
//! has to answer the same first question before it can look at a single claim: is this a
//! token, and what does it claim? That reading lives here exactly once, so two services
//! handed the same value cannot disagree about whether it is a token.
//!
//! What is checked here is the FORM, and the form only. Each caller then applies its own
//! rule to the claims it cares about ([`CompactJws::claim_str`]), and words its own
//! refusal: this module reports WHY a value is not a token ([`FormDefect`], which renders
//! the reason), never what any one service should answer for it.
//!
//! [`read`] checks the FORM only and verifies no signature: verifying one needs the
//! issuer's verification key, published at that issuer's own key-set endpoint, and this
//! emulator makes no outbound connection to fetch one. What IS decidable from the token
//! alone is refused there — a token whose header names the unsecured algorithm of
//! RFC 7519 §6 (`alg` `none`), or whose signature segment is empty, is one no issuer
//! signed.
//!
//! A service that CAN read the issuer's key set — because the issuer is one this emulator
//! serves, so the key set it publishes is held in this same process — verifies the
//! signature with [`verify`], which is the RFC 7515 §5.2 check: find the key the header's
//! `kid` names in the published set and check the signature over the token's own encoded
//! header and payload. That check lives here, beside the reading, so two services handed
//! one token and one key set cannot disagree about whether the issuer signed it.
//!
//! A service that MINTED the token it is reading is the one case where that unsecured form
//! is the expected one, because the emulator publishes no verification key to sign with:
//! such a service reads its own token back through [`read_unsecured`], which applies every
//! other rule here and leaves the issuer judgement to the claims.
//!
//! Services with their own copy of this reading — `service/jwt.rs` in
//! `service-com-amazonaws-apigatewayv2-apigatewayv2` and `parse_service_account_token` in
//! `service-com-amazonaws-eksauth-eksauthfrontend` — are to be MIGRATED onto it, each in
//! the change that owns its service (the convention [`crate::iam_client`] records for the
//! same situation). Nothing new should read a token by hand.
//!
//! A presentation prefix is NOT this module's concern: a token travelling in an
//! `Authorization` header carries the RFC 6750 `Bearer` scheme in front of it, which
//! [`without_bearer_scheme`] strips before [`read`] sees it.
//!
//! What a token CLAIMS is the consuming service's rule, but three readings of the claim set are
//! the same wherever they are made and live here beside the form: the AUDIENCES it names
//! ([`CompactJws::audiences`], both `aud` forms and the `client_id` an access token writes
//! instead), the SCOPES it claims ([`CompactJws::claimed_scopes`], both spellings), and its own
//! validity WINDOW ([`CompactJws::outside_window`], RFC 7519 §4.1.4/§4.1.5). Which audience,
//! which scope and what to do about a token carrying no expiry stay the caller's decision.
//!
//! # The JOSE primitives underneath
//!
//! A JWS is not always a compact token: the same signature, over the same
//! `base64url(header) '.' base64url(payload)` octets, travels in the JSON serialization an
//! ACME client posts (RFC 8555 §6.2), and the key it is checked against travels in the
//! request itself rather than in a published key set. The pieces that reading needs are
//! therefore public here, beside the compact reading that uses them, so no second copy of
//! any of them is written:
//!
//! * [`jwk_signature_checks_out`] — the signature check for a key published as a JWK, over
//!   [`VERIFIABLE_SIGNATURE_ALGORITHMS`];
//! * [`hs256_checks_out`] — the same for the MAC-keyed algorithm of RFC 7518 §3.2;
//! * [`jwk_thumbprint`] / [`canonical_jwk`] — RFC 7638, which is how a public key is
//!   IDENTIFIED;
//! * [`encode_base64url`] / [`decode_base64url`] — the one coding every JOSE value travels
//!   in.

/// The largest token this module reads, in bytes.
///
/// A CHOSEN bound, not a documented one: it caps the base64 decode and the JSON parse one
/// request can ask for, so a caller-supplied value cannot be turned into unbounded work. It
/// is far above every token an issuer mints (an access token is roughly one kilobyte) and
/// far below the request budget any front door accepts.
pub const MAX_TOKEN_BYTES: usize = 8 * 1024;

/// The JOSE header parameter naming the algorithm a JWS was signed with (RFC 7515 §4.1.1,
/// the one header JWS makes mandatory).
const ALGORITHM_HEADER: &str = "alg";

/// The `alg` value of an UNSECURED JWT — "a JWS using the 'alg' value of 'none'"
/// (RFC 7519 §6.1) — which is a token with no signature at all.
const UNSECURED_ALGORITHM: &str = "none";

/// Why a value is not a compact JWS carrying a JSON claim set.
///
/// `Display` renders the reason as a clause about the value ("its header segment is not
/// base64url-encoded JSON"), so a service can state its own refusal without holding a
/// second copy of these reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormDefect {
    /// There is nothing to read: the value is empty, or all whitespace.
    Empty,
    /// Longer than [`MAX_TOKEN_BYTES`], so it is not read at all.
    TooLarge,
    /// Not three `.`-separated segments.
    Segments,
    /// The header segment is not base64url-encoded JSON, or does not decode to a JSON
    /// object — neither is a JOSE header.
    Header,
    /// The header segment names no signing algorithm.
    Algorithm,
    /// The claim-set segment is not base64url-encoded JSON, or does not decode to a JSON
    /// object.
    Claims,
    /// Unsecured: the header names `alg` `none`, or the signature segment is empty, so
    /// nothing signed the token.
    Unsecured,
}

impl ::std::fmt::Display for FormDefect {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match self {
            Self::Empty => f.write_str("it is empty"),
            Self::TooLarge => write!(f, "it is longer than {MAX_TOKEN_BYTES} bytes"),
            Self::Segments => f.write_str(
                "it is not a JSON Web Token (three '.'-separated base64url segments)",
            ),
            Self::Header => {
                f.write_str("its header segment is not base64url-encoded JSON")
            }
            Self::Algorithm => f.write_str("its header segment names no signing algorithm"),
            Self::Claims => {
                f.write_str("its payload segment is not base64url-encoded JSON")
            }
            Self::Unsecured => {
                f.write_str("it is unsecured, so no issuer signed it")
            }
        }
    }
}

/// A compact JWS whose FORM this module accepted: the algorithm its header names, that
/// header, and its claim set. Nothing has been read out of the claims yet — which claims
/// matter is the calling service's rule, not this module's.
#[derive(Debug, Clone)]
pub struct CompactJws {
    /// The `alg` the JOSE header names, trimmed and non-empty.
    pub algorithm: String,
    /// The JOSE header, as the JSON object it decoded to.
    pub header: ::serde_json::Map<String, ::serde_json::Value>,
    /// The claim set, as the JSON object it decoded to.
    pub claims: ::serde_json::Map<String, ::serde_json::Value>,
}

impl CompactJws {
    /// The claim `name`, when the token carries it as a string.
    #[must_use]
    pub fn claim_str(&self, name: &str) -> Option<&str> {
        self.claims.get(name).and_then(::serde_json::Value::as_str)
    }

    /// The AUDIENCES the token names: who it was issued FOR.
    ///
    /// "The `aud` value is … either an array of case-sensitive strings or … a single
    /// case-sensitive string" (RFC 7519 §4.1.3), so both forms are read. An OAuth 2.0 ACCESS
    /// token names its client in [`CLIENT_ID_CLAIM`] where an identity token writes
    /// [`AUDIENCE_CLAIM`], so both claims are read and the audiences of either are reported —
    /// a consumer matching an expected audience matches against this whole list.
    #[must_use]
    pub fn audiences(&self) -> Vec<&str> {
        let mut audiences = Vec::new();
        for claim in [AUDIENCE_CLAIM, CLIENT_ID_CLAIM] {
            match self.claims.get(claim) {
                Some(::serde_json::Value::String(single)) => audiences.push(single.as_str()),
                Some(::serde_json::Value::Array(entries)) => audiences.extend(
                    entries
                        .iter()
                        .filter_map(::serde_json::Value::as_str),
                ),
                _ => {}
            }
        }
        audiences
    }

    /// The SCOPES the token claims, from either spelling in [`SCOPE_CLAIMS`]: the
    /// space-delimited string OAuth 2.0 defines ("a JSON string containing a space-separated
    /// list of scopes", RFC 8693 §4.2) and the list form an issuer may write instead.
    ///
    /// A token carrying neither claims nothing, which is what an identity token does.
    #[must_use]
    pub fn claimed_scopes(&self) -> Vec<&str> {
        let mut claimed = Vec::new();
        for claim in SCOPE_CLAIMS {
            match self.claims.get(*claim) {
                Some(::serde_json::Value::String(text)) => {
                    claimed.extend(text.split_whitespace());
                }
                Some(::serde_json::Value::Array(entries)) => claimed.extend(
                    entries
                        .iter()
                        .filter_map(::serde_json::Value::as_str)
                        .map(str::trim),
                ),
                _ => {}
            }
        }
        claimed
    }

    /// A claim written as RFC 7519 §2's NumericDate, in whole seconds.
    ///
    /// "Non-integer values can be used" (§2), so a fractional instant is read and truncated to
    /// the whole second; a claim that is not a JSON number is no NumericDate and reads as
    /// absent.
    #[must_use]
    pub fn numeric_date(&self, claim: &str) -> Option<i64> {
        let value = self.claims.get(claim)?;
        if let Some(seconds) = value.as_i64() {
            return Some(seconds);
        }
        let seconds = value.as_f64()?;
        if !seconds.is_finite() {
            return None;
        }
        // A float-to-integer cast saturates at the integer's bounds rather than wrapping, so
        // an instant beyond them reads as the nearest representable one.
        Some(seconds.trunc() as i64)
    }

    /// Why the token is outside its own validity window at `now`, or `None` while it is inside
    /// it.
    ///
    /// The two claims RFC 7519 defines it by: `exp`, "the expiration time on or after which
    /// the JWT MUST NOT be accepted for processing" (§4.1.4), and `nbf`, "the time before
    /// which the JWT MUST NOT be accepted for processing" (§4.1.5). `nbf` is optional, so a
    /// token carrying none is valid from its issuance; whether a token carrying no `exp` is
    /// acceptable is the CONSUMER's rule, so that case is reported
    /// ([`WindowDefect::NoExpiry`]) rather than decided here.
    #[must_use]
    pub fn outside_window(&self, now: ::std::time::SystemTime) -> Option<WindowDefect> {
        let seconds = now
            .duration_since(::std::time::UNIX_EPOCH)
            .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or_default();
        let Some(expiry) = self.numeric_date(EXPIRY_CLAIM) else {
            return Some(WindowDefect::NoExpiry);
        };
        if seconds >= expiry {
            return Some(WindowDefect::Expired);
        }
        if self
            .numeric_date(NOT_BEFORE_CLAIM)
            .is_some_and(|starts| seconds < starts)
        {
            return Some(WindowDefect::NotYetValid);
        }
        None
    }
}

/// The claim naming a token's ISSUER — "the principal that issued the JWT" (RFC 7519 §4.1.1).
pub const ISSUER_CLAIM: &str = "iss";

/// The claim naming who a token was issued FOR (RFC 7519 §4.1.3); see
/// [`CompactJws::audiences`].
pub const AUDIENCE_CLAIM: &str = "aud";

/// The claim an OAuth 2.0 ACCESS token names its client in, where an identity token writes
/// [`AUDIENCE_CLAIM`]; see [`CompactJws::audiences`].
pub const CLIENT_ID_CLAIM: &str = "client_id";

/// The claim naming when a token stops being accepted (RFC 7519 §4.1.4).
pub const EXPIRY_CLAIM: &str = "exp";

/// The claim naming when a token starts being accepted (RFC 7519 §4.1.5).
pub const NOT_BEFORE_CLAIM: &str = "nbf";

/// The claims a token's SCOPES are read from; see [`CompactJws::claimed_scopes`].
pub const SCOPE_CLAIMS: &[&str] = &["scope", "scp"];

/// The RFC 6750 §2.1 authentication scheme a bearer token is presented under, lowercased —
/// an HTTP auth scheme is case-insensitive (RFC 7235 §2.1), so the comparison is too.
pub const BEARER_SCHEME: &str = "bearer";

/// `presented` with the RFC 6750 `Bearer` credential prefix removed, and trimmed.
///
/// A presentation prefix is not part of the token: a token travelling in an `Authorization`
/// header carries that scheme in front of it, and the service reading that header strips it
/// before reading the token. A value carrying no scheme is the token itself.
#[must_use]
pub fn without_bearer_scheme(presented: &str) -> &str {
    let trimmed = presented.trim();
    let (scheme, rest) = match trimmed.split_once(' ') {
        Some(split) => split,
        None => return trimmed,
    };
    if scheme.eq_ignore_ascii_case(BEARER_SCHEME) {
        rest.trim()
    } else {
        trimmed
    }
}

/// Why a token is outside its own validity window; see [`CompactJws::outside_window`].
///
/// `Display` renders the reason as a clause about the token, the same way [`FormDefect`] does,
/// so a service states its own refusal without holding a second copy of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowDefect {
    /// The token names no expiry at all, so its window cannot be checked.
    NoExpiry,
    /// `now` is at or after the token's `exp`.
    Expired,
    /// `now` is before the token's `nbf`.
    NotYetValid,
}

impl ::std::fmt::Display for WindowDefect {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match self {
            Self::NoExpiry => f.write_str("it names no expiry"),
            Self::Expired => f.write_str("it has expired"),
            Self::NotYetValid => f.write_str("it is not yet valid"),
        }
    }
}

/// Read `presented` as a compact JWS and report its header and claim set.
///
/// An UNSECURED token — one whose header names `alg` `none`, or whose signature segment is
/// empty — is refused here ([`FormDefect::Unsecured`]): a value a caller presents as an
/// ISSUER's token is one no issuer signed. A service reading back a token it minted ITSELF
/// is the other case, and calls [`read_unsecured`].
///
/// Total: a value that is not a token is a [`FormDefect`], never a panic — nothing is
/// indexed, sliced or unwrapped on the way through.
pub fn read(presented: &str) -> Result<CompactJws, FormDefect> {
    let (token, unsecured) = read_form(presented)?;
    // Decidable from the token alone: an unsecured JWT (RFC 7519 §6) is one nothing
    // signed, whatever its claims say.
    if unsecured {
        return Err(FormDefect::Unsecured);
    }
    Ok(token)
}

/// Read `presented` as a compact JWS and report its header and claim set, WITHOUT refusing
/// the unsecured form.
///
/// The one reading a service makes of a token of its OWN minting. This emulator publishes no
/// verification key, so a service that mints a token mints the form RFC 7515 §6 defines for
/// a JWS carrying no signature; asked later what one of those tokens claims — the window it
/// was minted for, the subject it names — that service reads it back through here, where
/// every other form rule and the size bound are the ones [`read`] applies. Nothing is
/// VERIFIED by either function: a claim read out of a value a caller supplied is evidence
/// for refusing it, never for granting anything.
pub fn read_unsecured(presented: &str) -> Result<CompactJws, FormDefect> {
    read_form(presented).map(|(token, _)| token)
}

/// The form rule both readers apply: the token, with whether it is an UNSECURED one.
fn read_form(presented: &str) -> Result<(CompactJws, bool), FormDefect> {
    let token = presented.trim();
    if token.is_empty() {
        return Err(FormDefect::Empty);
    }
    if token.len() > MAX_TOKEN_BYTES {
        return Err(FormDefect::TooLarge);
    }
    let mut segments = token.split('.');
    let (Some(header), Some(claims), Some(signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(FormDefect::Segments);
    };
    let header = json_segment(header).ok_or(FormDefect::Header)?;
    let algorithm = header
        .get(ALGORITHM_HEADER)
        .and_then(::serde_json::Value::as_str)
        .map(str::trim)
        .filter(|algorithm| !algorithm.is_empty())
        .ok_or(FormDefect::Algorithm)?
        .to_string();
    let claims = json_segment(claims).ok_or(FormDefect::Claims)?;
    let unsecured = algorithm.eq_ignore_ascii_case(UNSECURED_ALGORITHM) || signature.is_empty();
    Ok((
        CompactJws {
            algorithm,
            header,
            claims,
        },
        unsecured,
    ))
}

/// The JOSE header parameter naming WHICH of an issuer's keys signed a JWS — "a hint
/// indicating which key was used to secure the JWS" (RFC 7515 §4.1.4).
const KEY_ID_HEADER: &str = "kid";

/// The `alg` a token's signature is checked under by [`verify`]: the one the token's
/// own header names (RFC 7515 §5.2 step 8), which has to be one of
/// [`VERIFIABLE_SIGNATURE_ALGORITHMS`]. A token naming any other algorithm is reported as
/// one that check cannot make rather than as one that failed to check out.
fn verifiable_algorithm(algorithm: &str) -> Option<&'static str> {
    VERIFIABLE_SIGNATURE_ALGORITHMS
        .iter()
        .find(|name| algorithm.eq_ignore_ascii_case(name))
        .copied()
}

/// The JOSE header parameter naming a key's family (RFC 7517 §4.1).
const KEY_TYPE: &str = "kty";

/// The `kty` of an elliptic-curve key (RFC 7518 §6.2).
const EC_KEY_TYPE: &str = "EC";

/// The `kty` of an RSA key (RFC 7518 §6.3).
const RSA_KEY_TYPE: &str = "RSA";

/// The `kty` of an octet-key-pair key — the family Edwards-curve keys are published
/// under (RFC 8037 §2).
const OKP_KEY_TYPE: &str = "OKP";

/// The `crv` of a P-256 key (RFC 7518 §6.2.1.1).
const P256_CURVE: &str = "P-256";

/// The `crv` of a P-384 key (the same member, the other NIST curve RFC 7518 §6.2.1.1
/// registers).
const P384_CURVE: &str = "P-384";

/// The `crv` of an Ed25519 key (RFC 8037 §3.1).
const ED25519_CURVE: &str = "Ed25519";

/// The width of one P-256 coordinate: RFC 7518 §6.2.1.2 publishes each as the field
/// element's full-width octet string, which on this curve is 32 bytes.
const P256_COORDINATE_BYTES: usize = 32;

/// The width of one P-384 coordinate, by the same rule.
const P384_COORDINATE_BYTES: usize = 48;

/// The width of an Ed25519 public key, which RFC 8037 §2 publishes in the `x` member.
const ED25519_KEY_BYTES: usize = 32;

/// The JWK member each published key carries its own key id in (RFC 7517 §4.5).
const JWK_KEY_ID: &str = "kid";

/// Why a token is not one the issuer whose key set was presented signed.
///
/// `Display` renders the reason as a clause about the token, the same way [`FormDefect`]
/// does, so a service states its own refusal without holding a second copy of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureDefect {
    /// The value is not a compact JWS at all, so there is no signature to check.
    Form(FormDefect),
    /// The presented key set holds no key the token's header names, so nothing in it
    /// signed the token.
    UnknownKey,
    /// The token names a signature algorithm this module cannot check, so whether the
    /// issuer signed it is UNDECIDED here — not decided against it.
    UnsupportedAlgorithm(String),
    /// The key the issuer published for this token is not one this module can check a
    /// signature under (another key type or curve). Also UNDECIDED, and about the KEY
    /// rather than about the token, which named an algorithm this module does check.
    UnsupportedKey,
    /// The key the header names is in the set, and the signature over the token's own
    /// bytes does not check out against it.
    Invalid,
}

impl ::std::fmt::Display for SignatureDefect {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match self {
            Self::Form(defect) => defect.fmt(f),
            Self::UnknownKey => {
                f.write_str("no key the token issuer publishes carries the key id it names")
            }
            Self::UnsupportedAlgorithm(algorithm) => write!(
                f,
                "its signature algorithm {algorithm} is not one the token issuer's key set \
                 publishes"
            ),
            Self::UnsupportedKey => f.write_str(
                "the key its issuer publishes for it is not one a signature can be checked \
                 under here",
            ),
            Self::Invalid => f.write_str("signature is invalid"),
        }
    }
}

/// Verify `presented` against `key_set` — the JSON Web Key Set (RFC 7517 §5) its issuer
/// publishes — and report the token's header and claim set.
///
/// The steps are the ones RFC 7515 §5.2 defines, which is what every consumer of such a
/// token does: read `alg` and `kid` out of the header, find THAT key in the set the
/// issuer published, rebuild the public key from the parameters the set carries
/// (RFC 7518 §6.2.1) and check the signature over `base64url(header) '.'
/// base64url(payload)` — the exact bytes the token carries, so a payload rewritten after
/// signing no longer checks out. Only PUBLIC key parameters are read, so a token that
/// verifies here is one any third party holding the same key set could have verified.
///
/// Total: every failure is a [`SignatureDefect`], never a panic — nothing is indexed,
/// sliced or unwrapped on the way through.
pub fn verify(
    presented: &str,
    key_set: &::serde_json::Value,
) -> Result<CompactJws, SignatureDefect> {
    let token = read(presented).map_err(SignatureDefect::Form)?;
    let Some(algorithm) = verifiable_algorithm(&token.algorithm) else {
        return Err(SignatureDefect::UnsupportedAlgorithm(token.algorithm.clone()));
    };
    // `read` already established that the value is three `.`-separated segments; they are
    // re-split here because the SIGNATURE is computed over the encoded ones, which the
    // decoded header and claim set no longer carry.
    let mut segments = presented.trim().split('.');
    let (Some(header), Some(claims), Some(signature)) =
        (segments.next(), segments.next(), segments.next())
    else {
        return Err(SignatureDefect::Form(FormDefect::Segments));
    };
    let named_key = token.header.get(KEY_ID_HEADER).and_then(::serde_json::Value::as_str);
    let keys = key_set
        .get("keys")
        .and_then(::serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let candidates: Vec<&::serde_json::Value> = keys
        .iter()
        .filter(|jwk| match named_key {
            // A token naming a key verifies against that key and no other.
            Some(kid) => jwk.get(JWK_KEY_ID).and_then(::serde_json::Value::as_str) == Some(kid),
            // One naming none verifies against ANY key the set holds — which is every key
            // of an issuer that is rotating, so each is tried before the token is refused.
            None => true,
        })
        .collect();
    if candidates.is_empty() {
        return Err(SignatureDefect::UnknownKey);
    }
    // A signature segment that is not base64url, or not the encoding the token's
    // algorithm puts in that segment, is no signature of any key: each algorithm
    // below accepts only its own encoding.
    let signature = decode_base64url(signature).ok_or(SignatureDefect::Invalid)?;
    let signing_input = format!("{header}.{claims}");
    let mut refused = None;
    for key in candidates {
        match checks_out(algorithm, key, signing_input.as_bytes(), &signature) {
            Ok(()) => return Ok(token),
            // A key this module cannot check a signature under leaves the question
            // UNDECIDED, so a key that could be checked and did not match wins: that is
            // the specific answer.
            Err(defect) => {
                if refused != Some(SignatureDefect::Invalid) {
                    refused = Some(defect);
                }
            }
        }
    }
    Err(refused.unwrap_or(SignatureDefect::Invalid))
}

/// Whether `key` — one entry of a published key set — signed `signing_input` with
/// `signature`, under the algorithm the token named.
///
/// `Err(SignatureDefect::UnsupportedKey)` for a key that check cannot be made under,
/// which says nothing about the token; `Err(SignatureDefect::Invalid)` for a key it can,
/// that did not sign these bytes.
fn checks_out(
    algorithm: &str,
    key: &::serde_json::Value,
    signing_input: &[u8],
    signature: &[u8],
) -> Result<(), SignatureDefect> {
    jwk_signature_checks_out(algorithm, key, signing_input, signature)
}

/// Every JWS `alg` a signature can be checked under here, as RFC 7518 §3.1 registers the
/// names: ECDSA on the two NIST curves, RSASSA-PKCS1-v1_5 at each digest width, and
/// Edwards-curve `EdDSA` (RFC 8037 §3.1).
///
/// A caller that has to tell a client WHICH algorithms a signature may be presented
/// under — RFC 8555 §6.2 asks an ACME server for exactly that list — states this one
/// rather than a second copy of it.
pub const VERIFIABLE_SIGNATURE_ALGORITHMS: &[&str] =
    &["ES256", "ES384", "RS256", "RS384", "RS512", "EdDSA"];

/// Whether the public key `jwk` publishes (RFC 7517 §4) signed `signing_input` with
/// `signature` under the JWS algorithm `algorithm` names.
///
/// The ONE signature check in this workspace for a JWS whose verification key travels as
/// a JWK — the key set a compact token names one key of ([`verify`]), or the key an ACME
/// client embeds in its own request's protected header (RFC 8555 §6.2). Only PUBLIC key
/// parameters are read, so a signature that verifies here is one any third party holding
/// the same JWK could have verified.
///
/// `Err(SignatureDefect::UnsupportedAlgorithm)` for an `alg` outside
/// [`VERIFIABLE_SIGNATURE_ALGORITHMS`] and `Err(SignatureDefect::UnsupportedKey)` for a
/// key of a family that algorithm is not defined over: both leave the question UNDECIDED,
/// rather than deciding it against the signature. `Err(SignatureDefect::Invalid)` is the
/// decided answer — the key could be read and did not sign these bytes.
///
/// Total: every failure is a [`SignatureDefect`], never a panic — nothing is indexed,
/// sliced or unwrapped on the way through.
pub fn jwk_signature_checks_out(
    algorithm: &str,
    jwk: &::serde_json::Value,
    signing_input: &[u8],
    signature: &[u8],
) -> Result<(), SignatureDefect> {
    let member = |name: &str| jwk.get(name).and_then(::serde_json::Value::as_str);
    let key_type = member(KEY_TYPE).unwrap_or_default();
    match (algorithm, key_type) {
        ("ES256", EC_KEY_TYPE) => ecdsa_checks_out(
            jwk,
            P256_CURVE,
            P256_COORDINATE_BYTES,
            &::ring::signature::ECDSA_P256_SHA256_FIXED,
            signing_input,
            signature,
        ),
        ("ES384", EC_KEY_TYPE) => ecdsa_checks_out(
            jwk,
            P384_CURVE,
            P384_COORDINATE_BYTES,
            &::ring::signature::ECDSA_P384_SHA384_FIXED,
            signing_input,
            signature,
        ),
        ("RS256", RSA_KEY_TYPE) => rsa_checks_out(
            jwk,
            &::ring::signature::RSA_PKCS1_2048_8192_SHA256,
            signing_input,
            signature,
        ),
        ("RS384", RSA_KEY_TYPE) => rsa_checks_out(
            jwk,
            &::ring::signature::RSA_PKCS1_2048_8192_SHA384,
            signing_input,
            signature,
        ),
        ("RS512", RSA_KEY_TYPE) => rsa_checks_out(
            jwk,
            &::ring::signature::RSA_PKCS1_2048_8192_SHA512,
            signing_input,
            signature,
        ),
        ("EdDSA", OKP_KEY_TYPE) => ed25519_checks_out(jwk, signing_input, signature),
        // An algorithm this module checks, presented with a key of a family it is not
        // defined over: a fact about the KEY, which says nothing about the signature.
        (algorithm, _) if VERIFIABLE_SIGNATURE_ALGORITHMS.contains(&algorithm) => {
            Err(SignatureDefect::UnsupportedKey)
        }
        _ => Err(SignatureDefect::UnsupportedAlgorithm(algorithm.to_string())),
    }
}

/// Whether an ECDSA key on `curve` signed `signing_input`, its coordinates read at
/// `coordinate_bytes` each.
fn ecdsa_checks_out(
    jwk: &::serde_json::Value,
    curve: &str,
    coordinate_bytes: usize,
    algorithm: &'static ::ring::signature::EcdsaVerificationAlgorithm,
    signing_input: &[u8],
    signature: &[u8],
) -> Result<(), SignatureDefect> {
    let member = |name: &str| jwk.get(name).and_then(::serde_json::Value::as_str);
    if member("crv") != Some(curve) {
        return Err(SignatureDefect::UnsupportedKey);
    }
    // SEC 1 §2.3.3: the uncompressed point a verifier reads the key's two published
    // coordinates into is `0x04 || X || Y`, each coordinate the curve's full width.
    let mut point = Vec::with_capacity(1 + 2 * coordinate_bytes);
    point.push(0x04_u8);
    for coordinate in ["x", "y"] {
        let bytes = member(coordinate)
            .and_then(decode_base64url)
            .filter(|bytes| bytes.len() == coordinate_bytes)
            .ok_or(SignatureDefect::Invalid)?;
        point.extend_from_slice(&bytes);
    }
    ::ring::signature::UnparsedPublicKey::new(algorithm, point)
        .verify(signing_input, signature)
        .map_err(|_| SignatureDefect::Invalid)
}

/// Whether the RSA key `jwk` publishes as its modulus and exponent (RFC 7518 §6.3.1)
/// signed `signing_input` under `algorithm`.
fn rsa_checks_out(
    jwk: &::serde_json::Value,
    algorithm: &'static ::ring::signature::RsaParameters,
    signing_input: &[u8],
    signature: &[u8],
) -> Result<(), SignatureDefect> {
    let member = |name: &str| jwk.get(name).and_then(::serde_json::Value::as_str);
    let component = |name: &str| {
        member(name)
            .and_then(decode_base64url)
            .filter(|bytes| !bytes.is_empty())
            .ok_or(SignatureDefect::Invalid)
    };
    ::ring::signature::RsaPublicKeyComponents {
        n: component("n")?,
        e: component("e")?,
    }
    .verify(algorithm, signing_input, signature)
    .map_err(|_| SignatureDefect::Invalid)
}

/// Whether the Ed25519 key `jwk` publishes in its `x` member (RFC 8037 §2) signed
/// `signing_input`.
fn ed25519_checks_out(
    jwk: &::serde_json::Value,
    signing_input: &[u8],
    signature: &[u8],
) -> Result<(), SignatureDefect> {
    let member = |name: &str| jwk.get(name).and_then(::serde_json::Value::as_str);
    if member("crv") != Some(ED25519_CURVE) {
        return Err(SignatureDefect::UnsupportedKey);
    }
    let key = member("x")
        .and_then(decode_base64url)
        .filter(|bytes| bytes.len() == ED25519_KEY_BYTES)
        .ok_or(SignatureDefect::Invalid)?;
    ::ring::signature::UnparsedPublicKey::new(&::ring::signature::ED25519, key)
        .verify(signing_input, signature)
        .map_err(|_| SignatureDefect::Invalid)
}

/// Whether `tag` is the HMAC-SHA-256 of `signed` under `key` — the `HS256` JWS algorithm
/// (RFC 7518 §3.2), which is how a MAC-keyed JWS is checked.
///
/// The comparison is the MAC crate's own, which is constant-time; nothing here compares
/// tag bytes itself.
#[must_use]
pub fn hs256_checks_out(key: &[u8], signed: &[u8], tag: &[u8]) -> bool {
    let key = ::ring::hmac::Key::new(::ring::hmac::HMAC_SHA256, key);
    ::ring::hmac::verify(&key, signed, tag).is_ok()
}

/// The RFC 7638 thumbprint of the public key `jwk` publishes: the base64url-encoded
/// SHA-256 digest of its canonical form ([`canonical_jwk`]).
///
/// The one value that IDENTIFIES a public key independently of how its holder spelled
/// the JWK — which is what RFC 8555 §8.1 keys an ACME account by. `None` for a key family
/// RFC 7638 §3.2 registers no required-member list for.
#[must_use]
pub fn jwk_thumbprint(jwk: &::serde_json::Value) -> Option<String> {
    use ::sha2::Digest as _;
    let canonical = canonical_jwk(jwk)?;
    Some(encode_base64url(
        ::sha2::Sha256::digest(canonical.as_bytes()).as_slice(),
    ))
}

/// The canonical form of the public key `jwk` publishes: exactly the members RFC 7638
/// §3.2 requires for its key family, in the lexicographic order §3.3 fixes, with no
/// whitespace — the octets a thumbprint is the digest of.
///
/// It carries the whole verification key and nothing else, so a caller that has to REMEMBER
/// a key a client presented stores this rather than the JWK as received: two spellings of
/// one key canonicalise to one value, and a private parameter a client sent along is not
/// kept. Every value is rendered by the JSON serializer, so nothing escapes text by hand.
#[must_use]
pub fn canonical_jwk(jwk: &::serde_json::Value) -> Option<String> {
    let member = |name: &str| jwk.get(name).and_then(::serde_json::Value::as_str);
    // RFC 7638 §3.2's required members per key type, each list already in the
    // lexicographic order §3.3 asks for.
    let required: &[&str] = match member(KEY_TYPE)? {
        EC_KEY_TYPE => &["crv", KEY_TYPE, "x", "y"],
        RSA_KEY_TYPE => &["e", KEY_TYPE, "n"],
        OKP_KEY_TYPE => &["crv", KEY_TYPE, "x"],
        _ => return None,
    };
    let mut canonical = String::from("{");
    for (position, name) in required.iter().enumerate() {
        if position > 0 {
            canonical.push(',');
        }
        let value = member(name)?;
        canonical.push_str(&::serde_json::Value::String((*name).to_string()).to_string());
        canonical.push(':');
        canonical.push_str(&::serde_json::Value::String(value.to_string()).to_string());
    }
    canonical.push('}');
    Some(canonical)
}

/// One base64url segment of a token, read as a JSON object. `None` when the segment is not
/// base64url or does not decode to a JSON object — neither is a JOSE header or a claim set.
fn json_segment(segment: &str) -> Option<::serde_json::Map<String, ::serde_json::Value>> {
    let bytes = decode_base64url(segment)?;
    match ::serde_json::from_slice::<::serde_json::Value>(&bytes) {
        Ok(::serde_json::Value::Object(members)) => Some(members),
        _ => None,
    }
}

/// `bytes` as one base64url segment, unpadded — the encoding every JOSE value travels in
/// (RFC 7515 §2), and the inverse of [`decode_base64url`].
#[must_use]
pub fn encode_base64url(bytes: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(bytes)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

/// Decode one base64url segment of a JWS: base64url without padding (RFC 7515 §2, which
/// is RFC 4648 §5 "with all trailing '=' characters omitted"), accepting a presenter that
/// padded it anyway.
///
/// The codec is `aws_smithy_types::base64`, the standard-alphabet decoder this workspace
/// already uses for every Smithy blob, plus the two alphabet substitutions (`-` -> `+`,
/// `_` -> `/`) and the restored padding that turn §5 back into §4. No second codec is
/// introduced for it.
#[must_use]
pub fn decode_base64url(segment: &str) -> Option<Vec<u8>> {
    let mut standard = segment
        .trim_end_matches('=')
        .replace('-', "+")
        .replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    ::aws_smithy_types::base64::decode(&standard).ok()
}

#[cfg(test)]
mod tests {
    //! The form rule, over the shapes a value handed to this module takes. The SERVED
    //! behaviour — which refusal each defect becomes — is pinned by the suites of the
    //! services that call it.

    use super::{
        read, read_unsecured, verify, without_bearer_scheme, CompactJws, FormDefect,
        SignatureDefect, WindowDefect, MAX_TOKEN_BYTES, P256_COORDINATE_BYTES,
    };

    /// `bytes` as one base64url segment (unpadded, as a compact JWS carries it).
    fn base64url(bytes: &[u8]) -> String {
        ::aws_smithy_types::base64::encode(bytes)
            .trim_end_matches('=')
            .replace('+', "-")
            .replace('/', "_")
    }

    /// One base64url segment (unpadded, as a compact JWS carries it) of `value`.
    fn segment(value: &str) -> String {
        base64url(value.as_bytes())
    }

    /// A token whose header and claim set are `header` and `claims`.
    fn token(header: &str, claims: &str) -> String {
        format!("{}.{}.{}", segment(header), segment(claims), segment("sig"))
    }

    /// A token carrying `claims`, read.
    fn claiming(claims: &str) -> CompactJws {
        read(&token(r#"{"alg":"ES256"}"#, claims)).expect("the test builds a well-formed token")
    }

    /// RULE: the RFC 6750 credential prefix is stripped whatever case the scheme is written
    /// in, and a value carrying no scheme is the token itself.
    #[test]
    fn the_bearer_presentation_scheme_is_stripped_in_any_case() {
        // Rows differ along the case of the scheme, the spacing, and whether there is one.
        let rows: &[(&str, &str, &str)] = &[
            ("the documented spelling", "Bearer abc.def.ghi", "abc.def.ghi"),
            ("all lower case", "bearer abc.def.ghi", "abc.def.ghi"),
            ("all upper case", "BEARER abc.def.ghi", "abc.def.ghi"),
            ("mixed case", "BeArEr abc.def.ghi", "abc.def.ghi"),
            ("surrounding space", "  Bearer   abc.def.ghi  ", "abc.def.ghi"),
            ("no scheme at all", "abc.def.ghi", "abc.def.ghi"),
            ("another scheme", "Basic abc.def.ghi", "Basic abc.def.ghi"),
            ("nothing", "", ""),
        ];
        for (case, presented, expected) in rows {
            let actual = without_bearer_scheme(presented);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a token's audiences are both `aud` forms and the `client_id` an access token
    /// writes instead; its scopes are both spellings; and its window is `exp` with the
    /// optional `nbf`.
    #[test]
    fn a_tokens_audiences_scopes_and_window_are_read_from_either_spelling() {
        let at = |seconds: u64| ::std::time::UNIX_EPOCH + ::std::time::Duration::from_secs(seconds);

        // Audiences: the string form, the array form, the access-token spelling, none.
        let audience_rows: &[(&str, &str, &[&str])] = &[
            ("a single aud", r#"{"aud":"client-1"}"#, &["client-1"]),
            (
                "an aud array",
                r#"{"aud":["client-1","client-2"]}"#,
                &["client-1", "client-2"],
            ),
            ("an access token's client_id", r#"{"client_id":"client-1"}"#, &["client-1"]),
            ("neither claim", r#"{"sub":"u"}"#, &[]),
        ];
        for (case, claims, expected) in audience_rows {
            let read = claiming(claims);
            let actual = read.audiences();
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }

        // Scopes: the space-delimited string, the list, neither.
        let scope_rows: &[(&str, &str, &[&str])] = &[
            (
                "the space-delimited scope string",
                r#"{"scope":"things/read things/write"}"#,
                &["things/read", "things/write"],
            ),
            ("an scp list", r#"{"scp":["things/read"]}"#, &["things/read"]),
            ("neither claim", r#"{"sub":"u"}"#, &[]),
        ];
        for (case, claims, expected) in scope_rows {
            let read = claiming(claims);
            let actual = read.claimed_scopes();
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }

        // The window: each published bound and one past it, plus the fractional NumericDate
        // RFC 7519 §2 admits and a token carrying no expiry at all.
        let window_rows: &[(&str, &str, u64, Option<WindowDefect>)] = &[
            ("inside the window", r#"{"exp":2000}"#, 1_999, None),
            ("at the expiry", r#"{"exp":2000}"#, 2_000, Some(WindowDefect::Expired)),
            ("past the expiry", r#"{"exp":2000}"#, 2_001, Some(WindowDefect::Expired)),
            ("a fractional expiry", r#"{"exp":2000.75}"#, 1_999, None),
            ("no expiry at all", r#"{"sub":"u"}"#, 1, Some(WindowDefect::NoExpiry)),
            (
                "before its not-before",
                r#"{"exp":2000,"nbf":1500}"#,
                1_499,
                Some(WindowDefect::NotYetValid),
            ),
            ("at its not-before", r#"{"exp":2000,"nbf":1500}"#, 1_500, None),
        ];
        for (case, claims, seconds, expected) in window_rows {
            let actual = claiming(claims).outside_window(at(*seconds));
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn a_compact_jws_reports_its_algorithm_and_claims() {
        let presented = token(r#"{"alg":"RS256","typ":"JWT"}"#, r#"{"iss":"issuer","typ":"Offline"}"#);
        let read = read(&presented).expect("a well-formed token reads");
        let CompactJws { algorithm, .. } = &read;
        assert_eq!(
            algorithm, "RS256",
            "the algorithm the header names must be reported, got {algorithm:?}"
        );
        assert_eq!(
            read.claim_str("typ"),
            Some("Offline"),
            "a string claim must be readable, got {:?}",
            read.claims
        );
        assert_eq!(
            read.claim_str("absent"),
            None,
            "a claim the token does not carry must read as absent"
        );
    }

    #[test]
    fn a_value_that_is_not_a_token_reports_which_part_is_wrong() {
        let oversized = format!(
            "{}.{}.{}",
            segment(r#"{"alg":"RS256"}"#),
            "a".repeat(MAX_TOKEN_BYTES),
            segment("sig")
        );
        let rows: Vec<(&str, String, FormDefect)> = vec![
            ("empty", String::new(), FormDefect::Empty),
            ("whitespace only", "  \n".to_string(), FormDefect::Empty),
            ("oversized", oversized, FormDefect::TooLarge),
            ("no segments", "opaque-value".to_string(), FormDefect::Segments),
            (
                "two segments",
                format!("{}.{}", segment(r#"{"alg":"RS256"}"#), segment("{}")),
                FormDefect::Segments,
            ),
            (
                "four segments",
                format!("{}.{}.{}.{}", segment(r#"{"alg":"RS256"}"#), segment("{}"), segment("sig"), segment("extra")),
                FormDefect::Segments,
            ),
            (
                "header not base64url",
                format!("@@@@.{}.{}", segment("{}"), segment("sig")),
                FormDefect::Header,
            ),
            (
                "header not a JSON object",
                format!("{}.{}.{}", segment(r#"["RS256"]"#), segment("{}"), segment("sig")),
                FormDefect::Header,
            ),
            (
                "header names no algorithm",
                token(r#"{"typ":"JWT"}"#, "{}"),
                FormDefect::Algorithm,
            ),
            (
                "claims not JSON",
                format!("{}.{}.{}", segment(r#"{"alg":"RS256"}"#), segment("not json"), segment("sig")),
                FormDefect::Claims,
            ),
            (
                "unsecured algorithm",
                token(r#"{"alg":"none"}"#, "{}"),
                FormDefect::Unsecured,
            ),
            (
                "empty signature segment",
                format!("{}.{}.", segment(r#"{"alg":"RS256"}"#), segment("{}")),
                FormDefect::Unsecured,
            ),
        ];

        for (case, presented, expected) in rows {
            let actual = read(&presented).err();
            assert_eq!(
                actual,
                Some(expected),
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            assert!(
                !expected.to_string().is_empty(),
                "case {case:?}: every defect must render a reason"
            );
        }
    }

    #[test]
    fn an_unsecured_token_reports_its_claims_only_to_the_reader_that_asks_for_one() {
        // The rows differ along what makes a token unsecured, and in what a signed one does
        // instead: the two readers must differ on exactly that and on nothing else.
        let rows: Vec<(&str, String, bool)> = vec![
            (
                "the unsecured algorithm",
                token(r#"{"alg":"none","typ":"JWT"}"#, r#"{"iss":"self","exp":42}"#),
                false,
            ),
            (
                "an empty signature segment",
                format!(
                    "{}.{}.",
                    segment(r#"{"alg":"none","typ":"JWT"}"#),
                    segment(r#"{"iss":"self","exp":42}"#)
                ),
                false,
            ),
            (
                "a signed token",
                token(r#"{"alg":"RS256","typ":"JWT"}"#, r#"{"iss":"self","exp":42}"#),
                true,
            ),
        ];

        for (case, presented, signed) in rows {
            let unsecured_read = read_unsecured(&presented)
                .unwrap_or_else(|defect| panic!("case {case:?}: expected a claim set, got {defect:?}"));
            assert_eq!(
                unsecured_read.claims.get("exp").and_then(::serde_json::Value::as_u64),
                Some(42),
                "case {case:?}: the claim set must be reported, got {:?}",
                unsecured_read.claims
            );
            let signed_read = read(&presented);
            assert_eq!(
                signed_read.is_ok(),
                signed,
                "case {case:?}: expected read() to accept == {signed}, got {signed_read:?}"
            );
            if !signed {
                assert_eq!(
                    read(&presented).err(),
                    Some(FormDefect::Unsecured),
                    "case {case:?}: an unsecured token is refused as unsecured"
                );
            }
        }

        // Every OTHER form defect is the same for both readers: neither reads a value that
        // is no token at all.
        for (case, presented) in [
            ("empty", String::new()),
            ("no segments", "opaque-value".to_string()),
            (
                "claims not JSON",
                format!(
                    "{}.{}.",
                    segment(r#"{"alg":"none"}"#),
                    segment("not json")
                ),
            ),
        ] {
            assert!(
                read_unsecured(&presented).is_err(),
                "case {case:?}: a value that is no token must be refused, got {:?}",
                read_unsecured(&presented)
            );
        }
    }

    /// The signing algorithm a test key is generated for and signs with: the same ECDSA
    /// P-256 / SHA-256 in the fixed `R || S` encoding [`verify`] checks.
    const TEST_SIGNING: &::ring::signature::EcdsaSigningAlgorithm =
        &::ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;

    /// A freshly generated P-256 key pair, with the key id its published JWK carries.
    struct TestKey {
        pair: ::ring::signature::EcdsaKeyPair,
        kid: &'static str,
    }

    impl TestKey {
        fn generate(kid: &'static str) -> Self {
            let rng = ::ring::rand::SystemRandom::new();
            let document = ::ring::signature::EcdsaKeyPair::generate_pkcs8(TEST_SIGNING, &rng)
                .expect("the operating system CSPRNG yields a P-256 key pair");
            let pair =
                ::ring::signature::EcdsaKeyPair::from_pkcs8(TEST_SIGNING, document.as_ref(), &rng)
                    .expect("a freshly generated P-256 key pair parses");
            Self { pair, kid }
        }

        /// This key's PUBLIC half as the JWK an issuer publishes (RFC 7518 §6.2.1).
        fn jwk(&self) -> ::serde_json::Value {
            use ::ring::signature::KeyPair as _;
            let point = self.pair.public_key().as_ref();
            let coordinate = |from: usize| {
                base64url(
                    point
                        .get(from..from + P256_COORDINATE_BYTES)
                        .expect("an uncompressed P-256 point carries both coordinates"),
                )
            };
            ::serde_json::json!({
                "alg": "ES256",
                "crv": "P-256",
                "kid": self.kid,
                "kty": "EC",
                "use": "sig",
                "x": coordinate(1),
                "y": coordinate(1 + P256_COORDINATE_BYTES),
            })
        }

        /// `claims` as a compact JWS this key signed, naming this key in its header.
        fn sign(&self, claims: &str) -> String {
            self.sign_under(
                &format!(r#"{{"alg":"ES256","kid":"{}","typ":"JWT"}}"#, self.kid),
                claims,
            )
        }

        /// The same, under a header naming NO key — which a verifier must then try every
        /// key of the set against.
        fn sign_without_key_id(&self, claims: &str) -> String {
            self.sign_under(r#"{"alg":"ES256","typ":"JWT"}"#, claims)
        }

        /// `claims` as a compact JWS this key signed under `header`.
        fn sign_under(&self, header: &str, claims: &str) -> String {
            let signing_input = format!("{}.{}", segment(header), segment(claims));
            let rng = ::ring::rand::SystemRandom::new();
            let signature = self
                .pair
                .sign(&rng, signing_input.as_bytes())
                .expect("the key signs");
            format!("{signing_input}.{}", base64url(signature.as_ref()))
        }
    }

    /// A token verifies against the key its issuer published for it, and against nothing
    /// else: the rows differ along what a presenter can change — the payload, the
    /// signature bytes, which key signed it, which key set is presented and what the
    /// header claims — and only the untouched token signed by a key in the set verifies.
    #[test]
    fn a_token_verifies_only_against_the_key_its_issuer_published() {
        let key = TestKey::generate("key-1");
        let other = TestKey::generate("key-2");
        let key_set = ::serde_json::json!({ "keys": [key.jwk()] });
        let claims = r#"{"sub":"subject","iss":"issuer"}"#;
        let issued = key.sign(claims);
        let signature = issued
            .rsplit('.')
            .next()
            .expect("a signed token has a signature segment")
            .to_string();

        // The payload rewritten while the signature is kept: the one case an
        // unverified reading of the claims would wave through.
        let rewritten = {
            let mut parts = issued.split('.');
            let header = parts.next().unwrap_or_default();
            format!(
                "{header}.{}.{signature}",
                segment(r#"{"sub":"subject","iss":"issuer","admin":true}"#)
            )
        };
        // One byte of the signature flipped, inside the base64url alphabet.
        let flipped = {
            let mut characters: Vec<char> = signature.chars().collect();
            let last = characters.len() - 1;
            characters[last] = if characters[last] == 'A' { 'B' } else { 'A' };
            let tail: String = characters.into_iter().collect();
            let signing_input = issued
                .rsplit_once('.')
                .map(|(input, _)| input.to_string())
                .unwrap_or_default();
            format!("{signing_input}.{tail}")
        };

        let rows: Vec<(&str, String, ::serde_json::Value, Option<SignatureDefect>)> = vec![
            (
                "the token as issued",
                issued.clone(),
                key_set.clone(),
                None,
            ),
            (
                "its payload rewritten, its signature kept",
                rewritten,
                key_set.clone(),
                Some(SignatureDefect::Invalid),
            ),
            (
                "one signature byte flipped",
                flipped,
                key_set.clone(),
                Some(SignatureDefect::Invalid),
            ),
            (
                "a token another key signed, presented against this key set",
                other.sign(claims),
                key_set.clone(),
                Some(SignatureDefect::UnknownKey),
            ),
            (
                "the key set of another issuer",
                issued.clone(),
                ::serde_json::json!({ "keys": [other.jwk()] }),
                Some(SignatureDefect::UnknownKey),
            ),
            (
                "a key set holding no key",
                issued.clone(),
                ::serde_json::json!({ "keys": [] }),
                Some(SignatureDefect::UnknownKey),
            ),
            (
                "a kid-less token against a set holding the signing key beside another",
                key.sign_without_key_id(claims),
                ::serde_json::json!({ "keys": [other.jwk(), key.jwk()] }),
                None,
            ),
            (
                "a kid-less token against a set holding only another issuer's key",
                key.sign_without_key_id(claims),
                ::serde_json::json!({ "keys": [other.jwk()] }),
                Some(SignatureDefect::Invalid),
            ),
            (
                "a key published under the token's key id with another key's coordinates",
                issued.clone(),
                ::serde_json::json!({ "keys": [{
                    "alg": "ES256", "crv": "P-256", "kid": "key-1", "kty": "EC", "use": "sig",
                    "x": other.jwk()["x"], "y": other.jwk()["y"],
                }] }),
                Some(SignatureDefect::Invalid),
            ),
            (
                "an unsecured token",
                token(r#"{"alg":"none"}"#, claims),
                key_set.clone(),
                Some(SignatureDefect::Form(FormDefect::Unsecured)),
            ),
            (
                "a token naming an algorithm this verifier checks, against a key of another \
                 family",
                token(r#"{"alg":"RS256","kid":"key-1"}"#, claims),
                key_set.clone(),
                Some(SignatureDefect::UnsupportedKey),
            ),
            (
                "a token naming an algorithm no key set publishes",
                token(r#"{"alg":"HS256","kid":"key-1"}"#, claims),
                key_set.clone(),
                Some(SignatureDefect::UnsupportedAlgorithm("HS256".to_string())),
            ),
            (
                "a key published on another curve",
                issued.clone(),
                ::serde_json::json!({ "keys": [{
                    "alg": "ES256", "crv": "P-384", "kid": "key-1", "kty": "EC", "use": "sig",
                    "x": key.jwk()["x"], "y": key.jwk()["y"],
                }] }),
                Some(SignatureDefect::UnsupportedKey),
            ),
        ];

        for (case, presented, published, expected) in rows {
            let outcome = verify(&presented, &published);
            let actual = outcome.as_ref().err().cloned();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            if let Ok(verified) = outcome {
                assert_eq!(
                    verified.claim_str("sub"),
                    Some("subject"),
                    "case {case:?}: a verified token reports its claims, got {:?}",
                    verified.claims
                );
            }
            if let Some(defect) = expected {
                assert!(
                    !defect.to_string().is_empty(),
                    "case {case:?}: every defect must render a reason"
                );
            }
        }
    }

    #[test]
    fn a_padded_segment_reads_the_same_as_an_unpadded_one() {
        let header = r#"{"alg":"RS256","typ":"JWT"}"#;
        let claims = r#"{"typ":"Offline"}"#;
        let padded = format!(
            "{}.{}.{}",
            ::aws_smithy_types::base64::encode(header),
            ::aws_smithy_types::base64::encode(claims),
            ::aws_smithy_types::base64::encode("sig")
        );
        let read_padded = read(&padded).expect("a padded token reads");
        let read_plain = read(&token(header, claims)).expect("an unpadded token reads");
        assert_eq!(
            read_padded.claim_str("typ"),
            read_plain.claim_str("typ"),
            "padding must not change what a token claims, got {:?} and {:?}",
            read_padded.claims,
            read_plain.claims
        );
    }
}
