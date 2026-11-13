// SPDX-License-Identifier: CC0-1.0

//! The token lifetimes an app client may be configured with: the duration each
//! validity member names, and the range that duration has to lie in.

use super::*;

/// The message `InvalidParameterException` carries for a token validity whose
/// DURATION is outside the range its member admits: the fixed sentence Amazon
/// Cognito answers, which names neither the member nor the value the request
/// submitted.
pub const INVALID_TOKEN_VALIDITY_RANGE_MESSAGE: &str = "Invalid range for token validity.";

/// The largest `AccessTokenValidity` and `IdTokenValidity` duration, in seconds.
///
/// Both members are modelled `@range 1..86400` and both document "valid range is
/// displayed below in seconds"
/// (`smithy_query(query="operation",operation="UpdateUserPoolClient")` and
/// `smithy_query(query="doc",shape="UpdateUserPoolClientRequest",member="AccessTokenValidity")`),
/// so the range bounds the duration rather than the bare number — and the bound
/// is the one `TokenValidityUnitsType` states in words: the validity "duration
/// can range from five minutes to one day"
/// (`smithy_query(query="shape",shape="TokenValidityUnitsType")`), one day being
/// these 86400 seconds.
const MAX_ACCESS_TOKEN_VALIDITY_SECONDS: u64 = 86_400;

/// The smallest `AccessTokenValidity` and `IdTokenValidity` duration, in
/// seconds: the other end of the window the same documentation states, five
/// minutes.
const MIN_ACCESS_TOKEN_VALIDITY_SECONDS: u64 = 5 * 60;

/// The largest `RefreshTokenValidity` duration, in seconds.
///
/// The member is modelled `@range 0..315360000` and documents the same "valid
/// range is displayed below in seconds", and `TokenValidityUnitsType` states the
/// bound in words: the refresh validity "duration can range from 60 minutes to
/// 10 years" — ten years being these 315,360,000 seconds.
const MAX_REFRESH_TOKEN_VALIDITY_SECONDS: u64 = 315_360_000;

/// The smallest `RefreshTokenValidity` duration, in seconds: the other end of
/// the window the same documentation states, 60 minutes.
const MIN_REFRESH_TOKEN_VALIDITY_SECONDS: u64 = 60 * 60;

/// The token-validity configuration one create or update of an app client asks
/// for, as the request carried it: each validity member beside the unit block
/// that says what unit each is counted in.
#[derive(Debug, Clone, Copy)]
pub struct RequestedTokenValidity<'a> {
    pub refresh_token_validity: i32,
    pub access_token_validity: Option<i32>,
    pub id_token_validity: Option<i32>,
    pub token_validity_units: Option<&'a TokenValidityUnitsType>,
}

/// Why `requested` is not a token-validity configuration an app client may
/// hold, or `None` when every validity it names lies in the range its member
/// admits.
///
/// A validity member and its time unit TOGETHER name a duration — "when you set
/// `AccessTokenValidity` to `10` and `TokenValidityUnits` to `hours`, your user
/// can authorize access with their access token for 10 hours" (this service's
/// model) — and what each member admits is a WINDOW of durations: an access or
/// an ID token's "duration can range from five minutes to one day" and a refresh
/// token's "from 60 minutes to 10 years"
/// (`smithy_query(query="shape",shape="TokenValidityUnitsType")`), whose upper
/// bounds are also each member's modelled `@range` maximum, the range being
/// "displayed below in seconds" while the number is counted in the unit the
/// request chose. So a validity is judged as the duration it names: the same
/// number is inside the window under one unit and outside it under another, and
/// the member's own default unit applies when the request overrides none.
///
/// `RefreshTokenValidity` is judged as the value the app client would HOLD
/// ([`effective_refresh_token_validity`]), since the modelled default `0` is
/// documented as overridden with 30 days rather than stored.
/// `AuthSessionValidity` names minutes with no unit to choose, so its modelled
/// `@range 3..15` bounds the number itself and the boundary validator has
/// already refused one outside it before any handler runs.
pub fn token_validity_defect(requested: &RequestedTokenValidity<'_>) -> Option<&'static str> {
    let units = requested.token_validity_units;
    let out_of_range = [
        (
            requested.access_token_validity,
            units.and_then(|units| units.access_token.as_deref()),
            // "The default time unit for AccessTokenValidity in an API request
            // is hours."
            TokenValidityUnit::Hours,
            MIN_ACCESS_TOKEN_VALIDITY_SECONDS,
            MAX_ACCESS_TOKEN_VALIDITY_SECONDS,
        ),
        (
            requested.id_token_validity,
            units.and_then(|units| units.id_token.as_deref()),
            // "The default time unit for IdTokenValidity in an API request is
            // hours."
            TokenValidityUnit::Hours,
            MIN_ACCESS_TOKEN_VALIDITY_SECONDS,
            MAX_ACCESS_TOKEN_VALIDITY_SECONDS,
        ),
        (
            Some(effective_refresh_token_validity(
                requested.refresh_token_validity,
            )),
            units.and_then(|units| units.refresh_token.as_deref()),
            // "The default time unit for RefreshTokenValidity in an API request
            // is days."
            TokenValidityUnit::Days,
            MIN_REFRESH_TOKEN_VALIDITY_SECONDS,
            MAX_REFRESH_TOKEN_VALIDITY_SECONDS,
        ),
    ]
    .into_iter()
    .any(|(validity, unit, default_unit, min, max)| {
        validity_seconds(validity, unit, default_unit)
            .is_some_and(|seconds| seconds < min || seconds > max)
    });
    out_of_range.then_some(INVALID_TOKEN_VALIDITY_RANGE_MESSAGE)
}

/// The duration, in seconds, that `validity` names when counted in `unit` — or
/// in `default_unit` when the request named none — and `None` for a request that
/// named no validity for this token at all, which asks for no duration to judge.
///
/// The product saturates rather than wrapping, so a number large enough to
/// overflow is reported as the out-of-range duration it is instead of folding
/// back into the range.
fn validity_seconds(
    validity: Option<i32>,
    unit: Option<&str>,
    default_unit: TokenValidityUnit,
) -> Option<u64> {
    let validity = u64::try_from(validity?).ok()?;
    let unit = unit
        .and_then(TokenValidityUnit::parse)
        .unwrap_or(default_unit);
    Some(validity.saturating_mul(unit.seconds()))
}
