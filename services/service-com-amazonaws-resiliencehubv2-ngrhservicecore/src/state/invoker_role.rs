// SPDX-License-Identifier: CC0-1.0

//! Whether this service can assume the invoker role a `permissionModel` names.
//!
//! A service does not assess itself: its assessments, tests and discoveries run UNDER the
//! IAM role `PermissionModel$invokerRoleName` names, which is why `CreateService` carries a
//! `@references` entry to the service's IAM resource and why the member is `@required`
//! (`smithy_query(query="shape", shape="PermissionModel")`). A role whose trust policy
//! withholds `sts:AssumeRole` from [`SERVICE_PRINCIPAL`] is therefore a service that could
//! never be assessed, so a request naming one is refused — with
//! [`unassumable_message`] — before anything is written.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **the role** — resolved by NAME against the account's IAM records, the very records
//!    `CreateRole` and `UpdateAssumeRolePolicy` wrote, read through the IAM service's own
//!    state struct in the SAME shared engine this service's store lives in. IAM entity
//!    names are unique within the account and "are not distinguished by case", so the
//!    lookup goes through IAM's own key ([`::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::name_key`]);
//!    the member admits a path (`^([A-Za-z0-9_+=,.@\-]+/)*[A-Za-z0-9_+=,.@\-]+$`), and IAM
//!    holds a role under the last segment of one.
//! 2. **the decision** — [`::service_common::iam_trust_policy::admits_service_principal`],
//!    this tree's ONE reading of a trust document, so this service and every other one
//!    handed the same document answer alike: a statement must name the principal and
//!    `sts:AssumeRole`, an explicit `Deny` wins over any `Allow`, and the document is
//!    percent-decoded before it is parsed.
//!
//! The verdict is taken on EVERY call and never cached on a record, because a trust policy
//! is edited outside this service: that is what makes an `UpdateAssumeRolePolicy` repairing
//! the role flip an otherwise identical request from refused to served, with no further
//! call to this service.
//!
//! Two cases are left unjudged, because neither is evidence about the role:
//!
//! * an engine carrying no IAM store at all — there is nothing to ask, and a refusal has to
//!   be grounded in what the account's records say;
//! * a name those records hold no role under — an invoker role is routinely provisioned out
//!   of band, so "this emulator's IAM holds no such role" is evidence about the store and
//!   not about the role, the same reading
//!   `service_common::iam_role::held_role_is_unassumable` takes for an execution role.
//!
//! A role the records DO hold is judged by the document they hold for it: one carrying no
//! trust document, and one that cannot be read as a policy at all, show no grant this
//! service could act on, so they are refused alongside a document that withholds or denies
//! the assume.

use crate::ops::types::ValidationException;

/// The IAM store an `invokerRoleName` is resolved against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine this
/// service's store lives in, so the role read here is the one IAM's handlers wrote rather
/// than a second copy of it.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The service principal an invoker role has to trust for this service to assume it: this
/// service's own endpoint prefix under `amazonaws.com`, which is how AWS names a service
/// principal.
pub const SERVICE_PRINCIPAL: &str = "resiliencehub.amazonaws.com";

/// The IAM store in the SAME shared engine, which is where the account's roles live.
///
/// This is the IAM service's OWN accessor, called rather than copied. `None` means the
/// engine carries no such store — one of the two cases this module leaves unjudged (see the
/// module docs).
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// The sentence a request naming an invoker role this service cannot assume is refused
/// with.
///
/// Fixed text around ONE variable part: the role name, taken verbatim from the request's
/// `permissionModel.invokerRoleName`.
pub fn unassumable_message(invoker_role_name: &str) -> String {
    format!(
        "Cannot assume invoker role '{invoker_role_name}'. Ensure the role exists and its \
         trust policy allows access."
    )
}

/// The refusal a request naming `invoker_role_name` is answered with, or `None` when
/// nothing of the kind is shown.
///
/// `Some(..)` — the modelled `ValidationException`, carried by every operation that takes a
/// `permissionModel`, so the two handlers refuse one thing one way — exactly when the
/// account's IAM records HOLD a role under that name and the trust document they hold for it
/// does not admit [`SERVICE_PRINCIPAL`] for `sts:AssumeRole`. `None` for the two unjudged
/// cases and for a role the records show trusting this service. The IAM guard is taken and
/// released inside this call, before any caller locks this service's own store, so no caller
/// holds two stores at once.
pub fn invoker_role_rejection<Eng: Send + Sync + 'static>(
    engine: &Eng,
    invoker_role_name: &str,
) -> Option<ValidationException> {
    let iam = iam_state(engine)?;
    let held = iam.lock();
    if held_role_admits_invoker(invoker_role_name, &held) {
        return None;
    }
    Some(ValidationException {
        message: unassumable_message(invoker_role_name),
        // The value the request sent names a role this service cannot act as, which is
        // this member's value being invalid rather than missing or duplicated — one of the
        // modelled `ValidationExceptionReason` values. `fieldList` is left out: its entries
        // carry a per-field sentence, and nothing in the model or the service's own wording
        // fixes what that sentence is for this refusal.
        reason: Some("INVALID_FIELD_VALUE".to_string()),
        field_list: None,
    })
}

/// Whether `iam` shows a role under `invoker_role_name` trusting [`SERVICE_PRINCIPAL`] for
/// `sts:AssumeRole`.
///
/// `true` as well for a name the store holds no role under — the reading is "admitted, or
/// nothing is shown": that is the unjudged case the module docs give, and this reports only
/// what the records say about a role they DO hold.
pub fn held_role_admits_invoker(invoker_role_name: &str, iam: &IamState) -> bool {
    let key = ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::name_key(
        role_name(invoker_role_name),
    );
    let Some(role) = iam.roles.get(&key) else {
        return true;
    };
    // The rule itself is IAM's, not this service's, so it is read through the ONE reading of
    // a trust document every service that resolves a caller-supplied role shares. A role
    // carrying no document, and a document this emulator cannot read as a policy, show no
    // grant to act on.
    role.assume_role_policy_document
        .as_deref()
        .and_then(|document| {
            ::service_common::iam_trust_policy::admits_service_principal(
                document,
                SERVICE_PRINCIPAL,
            )
        })
        == Some(true)
}

/// The role name IAM holds a value of this member under: its last path segment.
///
/// The member's `@pattern` admits a path (`^([A-Za-z0-9_+=,.@\-]+/)*[A-Za-z0-9_+=,.@\-]+$`),
/// while IAM holds a role under its NAME and reports the path separately — the same reading
/// `service_common::iam_role::role_name` takes of the resource field of a role ARN.
fn role_name(invoker_role_name: &str) -> &str {
    match invoker_role_name.rsplit_once('/') {
        Some((_path, name)) => name,
        None => invoker_role_name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the role a request names is the one IAM holds under the last segment of the
    /// value, whatever path the value carries and whatever case it is written in.
    #[test]
    fn the_value_resolves_to_the_name_iam_holds_the_role_under() {
        for (case, value, expected) in [
            ("a bare name", "ResilienceHubRole", "ResilienceHubRole"),
            (
                "a name under a path",
                "service-role/ResilienceHubRole",
                "ResilienceHubRole",
            ),
            (
                "a name under a deeper path",
                "a/b/c/ResilienceHubRole",
                "ResilienceHubRole",
            ),
        ] {
            let resolved = role_name(value);
            assert_eq!(
                resolved, expected,
                "case {case:?}: expected {expected:?}, got {resolved:?}"
            );
        }
    }

    /// RULE: the refusal's sentence is fixed text around the role name the request sent.
    #[test]
    fn the_refusal_names_the_role_the_request_sent() {
        for (case, role) in [
            ("a bare name", "ResilienceHubRole"),
            ("a name under a path", "service-role/ResilienceHubRole"),
        ] {
            let message = unassumable_message(role);
            assert_eq!(
                message,
                format!(
                    "Cannot assume invoker role '{role}'. Ensure the role exists and its \
                     trust policy allows access."
                ),
                "case {case:?}: unexpected wording {message:?}"
            );
        }
    }
}
