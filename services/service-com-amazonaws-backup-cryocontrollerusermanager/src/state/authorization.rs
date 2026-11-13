// SPDX-License-Identifier: CC0-1.0

//! The authorization decision a call that addresses a RECOVERY POINT by its ARN
//! takes before it resolves that point.
//!
//! # The rule this module implements
//!
//! AWS Backup authorizes such a call on the recovery point the request NAMES,
//! and it authorizes before it resolves: a recovery point belongs to the account
//! its ARN's account field names, and nothing a principal of another account
//! holds can grant access to it — only that account's own resource-based policy
//! could, and this emulator serves exactly one account
//! ([`engine::emulator_account_id`]). So an ARN whose account field is not that
//! account is refused for lack of privileges, with the SAME fixed sentence this
//! service refuses every call it does not grant with
//! ([`crate::scaffolded::AccessDeniedException::insufficient_privileges`]): AWS
//! Backup words its own refusal, built from nothing in the request, so it names
//! neither the principal that signed nor the action asked for nor the ARN.
//!
//! Refusing it as ABSENT would be wrong twice over: a caller would learn from the
//! refusal what another account does or does not hold, and a client that branches
//! on the authorization refusal — to re-assume a role, to surface a permissions
//! error, to stop retrying — would instead read a call the service refused as a
//! point that is already gone.
//!
//! The decision is taken from the REQUEST alone, so it needs no store and runs
//! ahead of every lookup. It is stated once here, for every call that addresses a
//! recovery point by ARN, so the delete, the read and the two jobs that take one
//! as input cannot drift into different answers for one condition.

/// The authorization decision for a call that addresses the recovery point
/// `recovery_point_arn` names: `Ok(())` when the ARN names a recovery point of
/// the account this emulator serves, and the refusal otherwise.
pub fn authorized_recovery_point(
    recovery_point_arn: &str,
) -> Result<(), crate::scaffolded::AccessDeniedException> {
    let own_account = ::service_common::arn::parse(recovery_point_arn)
        .is_some_and(|arn| arn.account == ::engine::emulator_account_id());
    if own_account {
        return Ok(());
    }
    Err(crate::scaffolded::AccessDeniedException::insufficient_privileges())
}
