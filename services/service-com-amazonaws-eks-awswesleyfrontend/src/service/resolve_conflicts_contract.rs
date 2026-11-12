// SPDX-License-Identifier: CC0-1.0

//! How the two add-on operations that take `resolveConflicts` WORD the refusal of a
//! value outside the modelled enum.
//!
//! `CreateAddon$resolveConflicts` and `UpdateAddon$resolveConflicts` target the
//! enum `ResolveConflicts`
//! (`smithy_query(query="shape",shape="ResolveConflicts",
//! model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")`),
//! so the framework's validating walk refuses an off-enum value BEFORE any handler
//! runs and words it as one more constrained leaf — `Value '<value>' at
//! 'resolveConflicts' failed to satisfy constraint: …`. This service refuses that
//! value in ITS own sentence instead, which names the value and the operation it
//! was sent to, because the message is part of the contract a client matches on.
//!
//! WHEN that refusal is answered is not this module's question: `resolveConflicts`
//! says what to do with a copy of the add-on already running ON the cluster, so it
//! is a question about the add-on the request asks for and is answered AFTER the
//! cluster the request is addressed to has been resolved — a request against a
//! cluster that does not exist is that 404 whatever its `resolveConflicts` says.
//! The order lives in [`super::precedence`], the layer that owns the order the
//! called operation answers its questions in, and it reads the sentence from here.
//!
//! Only the enum membership of that one member is judged here. Every other rule of
//! those two requests is the handler's, and a request whose `resolveConflicts` is
//! absent or inside the enum earns no refusal from this module at all.

use aws_smithy_schema_server::constraints::Violation;

use crate::ops::types::{InvalidParameterException, RESOLVE_CONFLICTS_VALUES};

/// The routing key of the operation that INSTALLS an add-on. On this service's
/// restJson1 protocol a routing key is the operation's own shape name
/// (`smithy_query(query="digest",…)` reports `restJson1`), so these two constants are
/// the only place this module names the operations it answers for.
pub const CREATE_ADDON: &str = "CreateAddon";
/// The routing key of the operation that UPDATES one; see [`CREATE_ADDON`].
pub const UPDATE_ADDON: &str = "UpdateAddon";

/// The member this contract judges, as a violation path spells it — which is how the
/// framework's own walk names it too.
const RESOLVE_CONFLICTS_MEMBER: &str = "resolveConflicts";

/// The value as the sentence may echo it: elided at the cap every refusal this
/// service words shares ([`crate::state::elided_value`]), because the member targets
/// an enum and so carries no modelled `length` bound at all.
///
/// Rendering only — the enum membership below is decided on the ORIGINAL value, so
/// eliding can never turn a declared value into a refused one.
fn echoed(value: &str) -> String {
    crate::state::elided_value(value, crate::state::MAX_ECHOED_VALUE)
}

/// The sentence Amazon EKS refuses an off-enum `resolveConflicts` with.
///
/// Fixed text around the two variable parts: the value the request sent, and the
/// operation it sent it to — which is why this cannot be worded by a per-service
/// refusal seam.
fn refusal_message(value: &str, operation: &str) -> String {
    format!(
        "Resolve conflicts option {} is not supported for operation {operation}",
        echoed(value)
    )
}

/// The value of `resolveConflicts` when the request carries one the enum does not
/// declare, or `None` when it carries none or carries a declared one.
///
/// The declared set is the generated `RESOLVE_CONFLICTS_VALUES` and the membership
/// test is the service-wide one ([`super::off_enum_member_contract::off_enum`]), so
/// this layer, the layer that answers the other members judged this way, and the
/// framework's own walk all read the SAME enum the model declares.
fn off_enum(value: Option<&str>) -> Option<&str> {
    super::off_enum_member_contract::off_enum(RESOLVE_CONFLICTS_VALUES, value)
}

/// The refusal an add-on request sent to `operation` earns for a `resolveConflicts`
/// outside the modelled enum, or `None` when the member carries no such value or the
/// walk refuses something else as well.
///
/// A body that ALSO breaks a modelled constraint keeps the framework's whole
/// violation report, which one sentence about one member cannot carry: this contract
/// rewords a refusal, it does not outrank the walk. The violations handed in are the
/// ones the dispatch sequence would have reported, so this cannot disagree with the
/// walk that would otherwise follow it.
///
/// The payload names the cluster and the add-on the request asked for, exactly as the
/// refusals the handlers themselves word do.
pub fn off_enum_refusal(
    operation: &str,
    value: Option<&str>,
    cluster_name: &str,
    addon_name: &str,
    violations: &[Violation],
) -> Option<InvalidParameterException> {
    let value = off_enum(value)?;
    if violations
        .iter()
        .any(|violation| violation.path != RESOLVE_CONFLICTS_MEMBER)
    {
        return None;
    }
    Some(InvalidParameterException {
        message: Some(refusal_message(value, operation)),
        cluster_name: Some(cluster_name.to_string()),
        addon_name: Some(addon_name.to_string()),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use aws_smithy_schema_server::constraints::ViolationKind;

    /// The enum's own value set decides what this contract refuses, and the sentence
    /// it refuses with names the value and the operation.
    ///
    /// Source for the accepted rows: the `ResolveConflicts` enum values the model
    /// declares, read through the generated `RESOLVE_CONFLICTS_VALUES`.
    #[test]
    fn the_modelled_enum_decides_which_values_this_contract_refuses() {
        for value in RESOLVE_CONFLICTS_VALUES {
            assert!(
                off_enum(Some(value)).is_none(),
                "case {value:?}: a declared enum value is not this contract's refusal"
            );
        }
        assert!(
            off_enum(None).is_none(),
            "case absent: a request that names no value is not refused"
        );
        // Rows differ along HOW the value misses the enum: unknown, the right word in
        // the wrong case, a declared value with padding, and the empty string.
        for value in ["BANANA", "overwrite", " NONE", ""] {
            assert_eq!(
                off_enum(Some(value)),
                Some(value),
                "case {value:?}: a value outside the enum must be refused"
            );
        }
    }

    #[test]
    fn the_refusal_sentence_names_the_value_and_the_operation() {
        for (operation, value, expected) in [
            (
                CREATE_ADDON,
                "BANANA",
                "Resolve conflicts option BANANA is not supported for operation CreateAddon",
            ),
            (
                UPDATE_ADDON,
                "overwrite",
                "Resolve conflicts option overwrite is not supported for operation UpdateAddon",
            ),
        ] {
            let message = refusal_message(value, operation);
            assert_eq!(
                message, expected,
                "case {operation}/{value:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    /// RULE: the refusal carries the cluster and the add-on the request named, and a
    /// body that breaks another modelled rule as well keeps the framework's report.
    #[test]
    fn the_refusal_names_the_cluster_and_the_addon_and_yields_to_another_violation() {
        let refusal = off_enum_refusal(CREATE_ADDON, Some("BANANA"), "probe", "vpc-cni", &[])
            .expect("an off-enum value is refused");
        assert_eq!(
            refusal.cluster_name.as_deref(),
            Some("probe"),
            "expected the requested cluster, got {:?}",
            refusal.cluster_name
        );
        assert_eq!(
            refusal.addon_name.as_deref(),
            Some("vpc-cni"),
            "expected the requested add-on, got {:?}",
            refusal.addon_name
        );

        // A violation on the member itself is this contract's own, so it is still
        // worded; one on any other member is not.
        for (case, path, expected) in [
            ("the member itself", RESOLVE_CONFLICTS_MEMBER, true),
            ("another member", "tags", false),
        ] {
            let violations = vec![Violation {
                path: path.to_string(),
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            }];
            let refused = off_enum_refusal(
                CREATE_ADDON,
                Some("BANANA"),
                "probe",
                "vpc-cni",
                &violations,
            );
            assert_eq!(
                refused.is_some(),
                expected,
                "case {case:?}: expected worded={expected}, got {refused:?}"
            );
        }
    }

    /// A value the caller chose the length of is echoed only up to the cap, whatever
    /// its bytes are: the rows differ along the length and along whether the cut
    /// would land inside a multi-byte character.
    #[test]
    fn a_caller_sized_value_is_echoed_only_up_to_the_cap() {
        let cap = crate::state::MAX_ECHOED_VALUE;
        for (case, value) in [
            ("at the cap", "x".repeat(cap)),
            ("one character past the cap", "x".repeat(cap + 1)),
            ("far past the cap", "x".repeat(cap * 40)),
            (
                "multi-byte characters across the cap",
                "\u{00e9}".repeat(cap * 2),
            ),
        ] {
            // The ORIGINAL value decides the refusal, however long it is.
            assert_eq!(
                off_enum(Some(&value)),
                Some(value.as_str()),
                "case {case}: the value itself is what the enum is checked against"
            );
            let message = refusal_message(&value, CREATE_ADDON);
            assert!(
                message.chars().count() < cap * 2,
                "case {case}: the sentence must not grow with the value, got {} characters",
                message.chars().count()
            );
        }
    }
}
