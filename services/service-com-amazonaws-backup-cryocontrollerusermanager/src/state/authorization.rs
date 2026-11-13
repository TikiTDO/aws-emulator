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
//! account is refused for lack of privileges, with the sentence AWS states when a
//! resource grants the calling principal nothing
//! ([`::service_common::iam_authorization::not_authorized_by_resource_policy`]):
//! the principal that signed, the action asked for and the resource ARN as the
//! request carried it. That sentence is one rule across this tree, so the
//! namespace is the only part this service supplies.
//!
//! Refusing it as ABSENT would be wrong twice over: a caller would learn from the
//! refusal what another account does or does not hold, and a client that branches
//! on the authorization refusal — to re-assume a role, to surface a permissions
//! error, to stop retrying — would instead read a call the service refused as a
//! point that is already gone. Naming the principal, the action and the ARN the
//! request carried discloses nothing about the other account either: all three are
//! values the caller already supplied or already holds, which is why the sentence
//! can be stated before anything is resolved.
//!
//! The decision is taken from the REQUEST alone, so it needs no store and runs
//! ahead of every lookup. It is stated once here, for every call that addresses a
//! recovery point by ARN, so the delete, the read and the two jobs that take one
//! as input cannot drift into different answers for one condition.

/// The IAM action namespace AWS Backup publishes its actions under, which is the
/// same namespace its ARNs carry (`OPERATIONS.md`: "ARN namespace: `backup`").
const ACTION_NAMESPACE: &str = super::ARN_NAMESPACE;

/// The authorization decision for a call that addresses the recovery point
/// `recovery_point_arn` names: `Ok(())` when the ARN names a recovery point of
/// the account this emulator serves, and the refusal otherwise.
///
/// `api_name` is the API name of the calling operation — its `Operation::NAME`,
/// so no handler spells an action string — and the action the decision is taken
/// on is that name under [`ACTION_NAMESPACE`].
pub fn authorized_recovery_point(
    api_name: &str,
    recovery_point_arn: &str,
) -> Result<(), crate::scaffolded::AccessDeniedException> {
    let own_account = ::service_common::arn::parse(recovery_point_arn)
        .is_some_and(|arn| arn.account == ::engine::emulator_account_id());
    if own_account {
        return Ok(());
    }
    Err(crate::scaffolded::AccessDeniedException::new(
        ::service_common::iam_authorization::not_authorized_by_resource_policy(
            &::service_common::caller_principal::ambient_user_arn(),
            &format!("{ACTION_NAMESPACE}:{api_name}"),
            recovery_point_arn,
        ),
    ))
}
