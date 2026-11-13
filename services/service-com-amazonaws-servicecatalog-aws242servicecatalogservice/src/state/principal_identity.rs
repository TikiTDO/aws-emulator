// SPDX-License-Identifier: CC0-1.0

//! The IDENTIFIER a portfolio-principal association records for the principal it names.
//!
//! A portfolio association is addressed by the `PrincipalARN` the request carried — that
//! is the value `AssociatePrincipalWithPortfolio` and `DisassociatePrincipalFromPortfolio`
//! name an association by — but the value a READ of the association reports is the
//! identifier of the PRINCIPAL, not a copy of the request string: for a fully defined ARN
//! (`PrincipalType` `IAM`) that is the principal's own unique id in the account's identity
//! records, which is how a client correlates a portfolio's principals with the roles,
//! users and groups IAM reports.
//!
//! `Principal$PrincipalARN` is what draws the line: "This field allows for an ARN with no
//! accountID, with or without wildcard characters if the `PrincipalType` is an
//! `IAM_PATTERN`". An `IAM_PATTERN` therefore names a SET of principals rather than one —
//! it may omit the account and carry `*`/`?` — so there is no principal to resolve and the
//! pattern itself is what the association records and reports.
//!
//! The identity records are IAM's OWN state, reached through the SAME shared engine this
//! service's store lives in, so what is read is the very record `iam:CreateRole`,
//! `iam:CreateUser` or `iam:CreateGroup` wrote, and the id reported here is the one
//! `iam:GetRole` reports for that role. Those records are the AUTHORITY: nothing here
//! mints an identifier of its own, so a value they resolve to no principal of this account
//! is recorded as the request gave it.
//!
//! LOCK ORDER: the IAM store is taken on its own and its guard released before this
//! service's own store is locked, so no two stores are held at once.

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_state;

use iam_state::EntityKind;

/// The identity records a concrete principal ARN is resolved against: the IAM service's
/// own state struct, reached through the shared engine.
pub type IamState = iam_state::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The `PrincipalType` whose ARN names a PATTERN rather than one principal
/// (`smithy_query query="shape" shape="PrincipalType"`: `IAM` | `IAM_PATTERN`).
pub const PRINCIPAL_TYPE_IAM_PATTERN: &str = "IAM_PATTERN";

/// The identifier an association of `principal_arn` as `principal_type` records, resolved
/// against the account's identity records in the shared engine.
///
/// Called by `AssociatePrincipalWithPortfolio` with the engine the handler holds, so the
/// identity records are reached the same way this service's own store is. The resolution
/// happens ONCE, when the association is stored, which is the value every later read of
/// that association reports.
///
/// `None` when this engine carries no identity records to resolve against at all — the
/// same condition `crate::state::service_state` answers `None` for, which is unreachable
/// in the assembled emulator and which the handler answers with the same modelled server
/// error rather than with a value nothing backs.
pub fn association_principal_identifier<Eng: Send + Sync + 'static>(
    engine: &Eng,
    principal_arn: &str,
    principal_type: &str,
) -> Option<String> {
    // A pattern names no single principal, so there is nothing to resolve: what was
    // associated is the pattern, and that is what is recorded.
    if principal_type == PRINCIPAL_TYPE_IAM_PATTERN {
        return Some(principal_arn.to_string());
    }
    let iam = iam_state::service_state(engine)?;
    let held = iam.lock();
    Some(principal_identifier(&held, principal_arn))
}

/// [`association_principal_identifier`] for a CONCRETE principal against identity records
/// already locked: the unique id those records hold for the principal the ARN names.
///
/// The records are the AUTHORITY on a principal's identifier, so a value they resolve to
/// no principal is recorded as given: a principal they hold no record of — provisioned out
/// of band, or deleted since — has no identifier to report, and neither has a value that
/// names no IAM user, role or group at all, which is a `PrincipalARN`
/// `AssociatePrincipalWithPortfolio` has already refused for an `IAM` association. Nothing
/// here mints an id, because an id IAM did not mint is one no `iam:GetRole` would report.
pub fn principal_identifier(iam: &IamState, principal_arn: &str) -> String {
    let Some(entity) = iam_state::parse_entity_arn(principal_arn) else {
        return principal_arn.to_string();
    };
    held_identifier(iam, &entity, principal_arn).unwrap_or_else(|| principal_arn.to_string())
}

/// The unique id the identity records HOLD for the principal `entity` names, or `None`
/// when they hold no such principal.
///
/// Each of the three kinds of principal is found the way IAM itself addresses it — by the
/// case-folded name its map is keyed on — and only a record whose OWN ARN is the one asked
/// for answers, so a role and a user of the same name cannot be mistaken for each other.
fn held_identifier(
    iam: &IamState,
    entity: &iam_state::EntityArn,
    principal_arn: &str,
) -> Option<String> {
    let key = iam_state::name_key(&entity.name);
    match entity.kind {
        EntityKind::User => iam
            .users
            .get(&key)
            .filter(|user| user.arn == principal_arn)
            .map(|user| user.user_id.clone()),
        EntityKind::Role => iam
            .roles
            .get(&key)
            .filter(|role| role.arn == principal_arn)
            .map(|role| role.role_id.clone()),
        EntityKind::Group => iam
            .groups
            .get(&key)
            .filter(|group| group.arn == principal_arn)
            .map(|group| group.group_id.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a concrete principal the identity records HOLD reports the id those records
    /// hold for it; a value they resolve to no principal is recorded as given, and nothing
    /// mints an id of its own.
    #[test]
    fn a_concrete_principal_reports_the_identifier_the_records_hold() {
        let account = ::engine::emulator_account_id();
        let iam = IamState::default();

        // The user the account holds before any request is served, with the id the
        // identity records themselves hold for it.
        let ambient_arn = ::service_common::caller_principal::ambient_user_arn();
        let held = principal_identifier(&iam, &ambient_arn);
        assert_eq!(
            held,
            ::service_common::caller_principal::ambient_user_id(),
            "case \"a user the records hold\": expected the id IAM holds, got {held:?}"
        );
        assert_ne!(
            held, ambient_arn,
            "case \"a user the records hold\": the request ARN must not be what is recorded"
        );

        // Rows differ along the kind of principal and its path. The records hold none of
        // them, so each is recorded as given: an identifier is theirs to mint, and one
        // this service minted would be an id no `iam:GetRole` would ever report.
        let rows: &[(&str, String)] = &[
            (
                "a role the records do not hold",
                format!("arn:aws:iam::{account}:role/Admin"),
            ),
            (
                "a role under a path",
                format!("arn:aws:iam::{account}:role/service-role/Launcher"),
            ),
            (
                "a user the records do not hold",
                format!("arn:aws:iam::{account}:user/Builder"),
            ),
            (
                "a group the records do not hold",
                format!("arn:aws:iam::{account}:group/Builders"),
            ),
            (
                "a value naming no principal at all",
                format!("arn:aws:iam::{account}:policy/Admin"),
            ),
        ];
        for (case, arn) in rows {
            let answered = principal_identifier(&iam, arn);
            assert_eq!(
                &answered, arn,
                "case {case:?}: expected the value recorded as given, got {answered:?}"
            );
        }
    }

    /// RULE: an `IAM_PATTERN` association records the pattern it was given, while a
    /// concrete `IAM` principal the records hold records that principal's identifier.
    #[test]
    fn a_pattern_association_records_the_pattern_it_was_given() {
        let engine = ::std::sync::Arc::new(::engine::InMemoryEngine::new());
        let ambient_arn = ::service_common::caller_principal::ambient_user_arn();
        // (case, the ARN, the type it is associated under, whether the ARN is recorded)
        let rows: &[(&str, &str, &str, bool)] = &[
            (
                "an account-less wildcard pattern",
                "arn:aws:iam:::role/Team*",
                PRINCIPAL_TYPE_IAM_PATTERN,
                true,
            ),
            (
                "a fully defined ARN as a pattern",
                &ambient_arn,
                PRINCIPAL_TYPE_IAM_PATTERN,
                true,
            ),
            (
                "the same ARN as a concrete principal",
                &ambient_arn,
                "IAM",
                false,
            ),
        ];
        for (case, arn, principal_type, recorded_as_given) in rows {
            let answered = association_principal_identifier(&*engine, arn, principal_type)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the shared engine carries identity records")
                });
            assert_eq!(
                &answered == arn,
                *recorded_as_given,
                "case {case:?}: expected the ARN recorded as given = {recorded_as_given}, got \
                 {answered:?}"
            );
        }
    }
}
