// SPDX-License-Identifier: CC0-1.0

//! Whether the ROLE a plan's `executionRole` names is one a plan execution could act as.
//!
//! A plan execution does not act as whoever started it: Region switch performs a plan's
//! workflows as the plan's own `executionRole`, so a plan whose role does not resolve to
//! a role of the account cannot execute at all — nothing can be assumed to drive its
//! steps. That is a MISCONFIGURATION of the plan rather than a failure of any one call,
//! which is why it belongs to the plan's evaluation
//! ([`super::StoredPlan::evaluation_warnings`]) and is reported as an active warning there
//! before a caller starts a switch that cannot run.
//!
//! The role is resolved through the IAM service's OWN store, in the SAME shared engine
//! this service's store lives in, so what is read is the very record `iam:CreateRole`
//! wrote and deleting the role raises the warning again on the next read.
//!
//! Nothing is flagged on evidence this emulator does not have: a role ARN naming ANOTHER
//! account is not this account's to hold, and a blank `executionRole` is the absence of a
//! role rather than a role that cannot be resolved. Only EXISTENCE is judged here — what
//! a role may do once assumed is the business of the calls the workflow's own execution
//! blocks would make, which this emulator does not perform.
//!
//! LOCK ORDER: this service's own store is taken first and IAM's second, and the IAM guard
//! is never held while another store is taken.

/// The IAM store a plan's `executionRole` is resolved against: the IAM service's own state
/// struct, reached through the shared engine.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// Why the plan's `executionRole` cannot be acted as, or `None` when nothing of the kind
/// can be shown.
///
/// Called with the engine a handler holds, so the IAM store is reached the same way this
/// service's own store is.
pub fn execution_role_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    role_arn: &str,
) -> Option<String> {
    let iam = ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(
        engine,
    )?;
    let held = iam.lock();
    refusal(&held, role_arn)
}

/// [`execution_role_refusal`] against an IAM store already locked.
pub fn refusal(iam: &IamState, role_arn: &str) -> Option<String> {
    let role_arn = role_arn.trim();
    // No role named at all: there is no ARN to resolve, and the plan's own required
    // member is what would be missing.
    if role_arn.is_empty() {
        return None;
    }
    // A role of another account is not among this account's roles, so its absence here
    // shows nothing about it.
    if !names_this_account(role_arn) {
        return None;
    }
    if iam.roles.values().any(|role| role.arn == role_arn) {
        return None;
    }
    Some(format!(
        "Region switch cannot assume the plan's execution role {role_arn}: this account \
         holds no role with that Amazon Resource Name. A plan execution performs the \
         plan's workflows as that role, so the role has to exist before a Region switch \
         can run."
    ))
}

/// Whether `arn`'s account segment is the account this emulator serves.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`, so the account is
/// its fifth colon-separated segment.
fn names_this_account(arn: &str) -> bool {
    arn.split(':')
        .nth(4)
        .is_some_and(|account| account == ::engine::emulator_account_id())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: only a role ARN of THIS account that the IAM store resolves to no role is
    /// flagged; everything else is evidence of nothing.
    #[test]
    fn only_an_unresolvable_role_of_this_account_is_flagged() {
        let account = ::engine::emulator_account_id();
        let iam = IamState::default();
        // (case, the stored `executionRole`, whether a warning is owed)
        let rows: &[(&str, String, bool)] = &[
            (
                "a role of this account the store holds no role for",
                format!("arn:aws:iam::{account}:role/ArcRegionSwitchRole"),
                true,
            ),
            (
                "a role under a path, of this account",
                format!("arn:aws:iam::{account}:role/service-role/Switcher"),
                true,
            ),
            (
                "a role of another account",
                "arn:aws:iam::123456789012:role/ArcRegionSwitchRole".to_string(),
                false,
            ),
            ("no role named at all", String::new(), false),
            ("a blank role", "   ".to_string(), false),
        ];
        for (case, role_arn, owed) in rows {
            let answered = refusal(&iam, role_arn);
            assert_eq!(
                answered.is_some(),
                *owed,
                "case {case:?}: expected a warning owed = {owed}, got {answered:?}"
            );
            if let Some(message) = answered {
                assert!(
                    message.contains(role_arn.trim()),
                    "case {case:?}: expected the warning to name the role, got {message:?}"
                );
            }
        }
    }
}
