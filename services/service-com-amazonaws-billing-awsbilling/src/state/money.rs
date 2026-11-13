// SPDX-License-Identifier: CC0-1.0

//! The money and rate arithmetic this service's charge reports share.
//!
//! Every amount is held in minor units ([`MINOR_UNITS_PER_UNIT`]) and every
//! rate in basis points (hundredths of one percent), so a percentage of an
//! amount is an exact integer computation and a stored amount round-trips to
//! the modeled decimal string unchanged. `ListBusinessSupportAccountCharges`
//! and `GetEnterpriseSupportChargeSummary` both price stored spend this way, so
//! the arithmetic lives here rather than beside either handler.

use super::MINOR_UNITS_PER_UNIT;

/// Basis points in one whole unit: a rate is held in hundredths of one percent,
/// so 10_000 of them is the whole amount.
pub const BASIS_POINTS_PER_UNIT: i64 = 10_000;

/// A basis-point rate applied to an amount of minor units, rounded half up —
/// integer arithmetic throughout, so a charge is never a binary float's
/// approximation of one. Computed at a width the product cannot overflow.
pub fn apply_rate(amount_minor: i64, rate_basis_points: i64) -> i64 {
    let scaled = i128::from(amount_minor) * i128::from(rate_basis_points)
        + i128::from(BASIS_POINTS_PER_UNIT / 2);
    i64::try_from(scaled.div_euclid(i128::from(BASIS_POINTS_PER_UNIT))).unwrap_or(i64::MAX)
}

/// An amount of minor units as the decimal string the modeled money members
/// carry (`totalCharge`, `supportCharge`, `supportDiscount`, ...), with the two
/// fraction digits [`MINOR_UNITS_PER_UNIT`] gives it.
pub fn amount(minor: i64) -> String {
    let sign = if minor < 0 { "-" } else { "" };
    let magnitude = minor.unsigned_abs();
    let scale = MINOR_UNITS_PER_UNIT.unsigned_abs();
    format!("{sign}{}.{:02}", magnitude / scale, magnitude % scale)
}

/// A basis-point rate as the decimal fraction the modeled rate members carry.
///
/// From the `discountPercentage` doc: "expressed as a decimal (for example,
/// 0.12 for a 12% discount)" — the form `tierRate`, the rate applied to the
/// spend inside a tier, carries too. Two fraction digits for a rate that is a
/// whole percent, four for a finer one, so no rate is reported rounded, and the
/// sign of a negative rate is kept the way [`amount`] keeps one.
pub fn rate_decimal(rate_basis_points: i64) -> String {
    let sign = if rate_basis_points < 0 { "-" } else { "" };
    let magnitude = rate_basis_points.unsigned_abs();
    let scale = BASIS_POINTS_PER_UNIT.unsigned_abs();
    let whole = magnitude / scale;
    let fraction = magnitude % scale;
    if fraction.is_multiple_of(100) {
        format!("{sign}{whole}.{:02}", fraction / 100)
    } else {
        format!("{sign}{whole}.{fraction:04}")
    }
}

/// The rate, in basis points, that `charge_minor` works out to as a share of
/// `base_minor`, rounded half up — the percentage a charge computed from a
/// spend was effectively charged at.
///
/// `0` when `base_minor` is zero: nothing charged on nothing is no rate, and
/// there is no share of zero to report.
pub fn effective_rate_basis_points(charge_minor: i64, base_minor: i64) -> i64 {
    if base_minor == 0 {
        return 0;
    }
    prorate(charge_minor, BASIS_POINTS_PER_UNIT, base_minor)
}

/// `amount_minor` in the proportion `part_minor` is of `whole_minor`, rounded
/// half up — one account's share of a charge computed across many.
///
/// `0` when `whole_minor` is zero: there is no share of nothing. Computed at a
/// width the product cannot overflow.
pub fn prorate(amount_minor: i64, part_minor: i64, whole_minor: i64) -> i64 {
    if whole_minor == 0 {
        return 0;
    }
    let whole = i128::from(whole_minor);
    let scaled = i128::from(amount_minor) * i128::from(part_minor) + whole / 2;
    i64::try_from(scaled.div_euclid(whole)).unwrap_or(i64::MAX)
}
