// SPDX-License-Identifier: CC0-1.0

//! The authorization decision a call that addresses a RECOVERY POINT by its ARN
//! takes before it resolves that point.
//!
//! # The rule this module implements
//!
//! AWS Backup authorizes such a call on the recovery point the request NAMES,
//! and it authorizes before it resolves: the action is this service's action
//! namespace plus the API name (`backup:DeleteRecoveryPoint`), and the resource
//! is the `RecoveryPointArn` the request carried. A recovery point belongs to the
//! account its ARN's account field names, and nothing a principal of another
//! account holds can grant access to it — only that account's own resource-based
//! policy could, and this emulator serves exactly one account
//! ([`engine::emulator_account_id`]). So an ARN whose account field is not that
//! account is refused for lack of privileges, with THIS SERVICE'S OWN sentence for
//! a refusal of that kind
//! ([`crate::scaffolded::AccessDeniedException::insufficient_privileges`]): AWS
//! Backup states its own fixed sentence rather than IAM's decision sentence, as
//! the recorded refusal of such a call shows, and it is the same sentence this
//! service already answers a vault outside the caller's visibility with
//! (`crate::state::ComAmazonawsBackupCryocontrollerusermanagerState::vault_visible_to_caller`),
//! so one condition has one wording across the service.
//!
//! Refusing it as ABSENT would be wrong twice over: a caller would learn from the
//! refusal what another account does or does not hold, and a client that branches
//! on the authorization refusal — to re-assume a role, to surface a permissions
//! error, to stop retrying — would instead read a delete the service refused as a
//! point that is already gone. The sentence names nothing the request carried for
//! the same reason: a refusal that is taken before anything is resolved has
//! nothing to disclose.
//!
//! The decision is taken from the REQUEST alone, so it needs no store and runs
//! ahead of every lookup. It is stated once here, for every call that addresses a
//! recovery point by ARN, so the delete, the read and the two jobs that take one
//! as input cannot drift into different answers for one condition.

/// The authorization decision for a call that addresses the recovery point
/// `recovery_point_arn` names: `Ok(())` when the ARN names a recovery point of
/// the account this emulator serves, and the refusal otherwise.
///
/// It takes no action name, because the sentence this service answers is fixed:
/// the action and the ARN decide the outcome, and neither appears in the refusal.
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
