// SPDX-License-Identifier: CC0-1.0

//! The `YYYY-MM` billing-month arithmetic this service's monthly charge
//! reports share.
//!
//! Both `ListBusinessSupportAccountCharges` and
//! `GetEnterpriseSupportChargeSummary` take a modeled billing month
//! (`^\d{4}-(0[1-9]|1[0-2])$`), compare it with the month the clock is in and
//! report the UTC period its charges accrued in, so that reading of a month
//! lives here rather than beside either handler.

/// The year and month of a modeled `YYYY-MM` billing month.
pub fn parse_month(billing_month: &str) -> Option<(i32, i32)> {
    let (year, month) = billing_month.split_once('-')?;
    Some((year.parse().ok()?, month.parse().ok()?))
}

/// A month as the count of whole calendar months before it, so two months
/// compare and subtract.
pub fn month_index((year, month): (i32, i32)) -> i32 {
    year * 12 + month - 1
}

/// The first instant of a month, read off the UTC civil calendar.
///
/// `None` for a year the modeled `Timestamp` cannot render at four digits,
/// which only the month after the last `YYYY-MM` month is.
pub fn first_instant_of(year: i32, month: i32) -> Option<::aws_smithy_types::DateTime> {
    ::aws_smithy_types::DateTime::from_str(
        &format!("{year:04}-{month:02}-01T00:00:00Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()
}

/// The half-open UTC period a billing month's charges accrue in: the first
/// instant of the month, and the first instant of the month after it.
///
/// AWS bills on UTC calendar months, so both bounds come from the month's own
/// position on the civil calendar — never from a fixed month length.
pub fn month_period(
    billing_month: &str,
) -> Option<(::aws_smithy_types::DateTime, ::aws_smithy_types::DateTime)> {
    let (year, month) = parse_month(billing_month)?;
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    Some((
        first_instant_of(year, month)?,
        first_instant_of(next_year, next_month)?,
    ))
}
