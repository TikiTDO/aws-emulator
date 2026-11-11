// SPDX-License-Identifier: CC0-1.0

//! Whether a pipe's EXECUTION ROLE lets a stage make the call it is about to make.
//!
//! A pipe's stages do not act as whoever created the pipe: EventBridge Pipes polls the
//! source, invokes the enrichment and delivers to the target ACTING AS the pipe's
//! `RoleArn` — "the ARN of the role that allows the pipe to send data to the target"
//! (`Pipe$RoleArn`) — so a call the policies in force on that role do not allow is a
//! stage that FAILS rather than a call that happens. That is the difference between a
//! pipe that ran and was refused and a pipe that never ran, and it is what the stage
//! counters of [`super::metrics`] report.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **who** — the pipe's own stored `RoleArn`, read off the record at the moment of
//!    each call, so a pipe whose role `UpdatePipe` has switched is judged by the role it
//!    carries then and not by one read before the call;
//! 2. **what it may do** — the identity-based documents the IAM store holds for that
//!    role (its inline policies and the default version of each managed policy attached
//!    to it), read through IAM's own
//!    [`principal_policies`](IamState::principal_policies) accessor so this service sees
//!    exactly what `CreateRole`/`PutRolePolicy` wrote, plus the permissions boundary that
//!    caps them;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of policy documents against `(action, resource)`, so this service and
//!    every other one handed the same documents decide alike.
//!
//! The trust document comes first: a role this service cannot assume at all never gets as
//! far as its permissions, and that is read through
//! [`::service_common::iam_trust_policy`], the one reading of a trust document in this
//! tree.
//!
//! Nothing is refused on evidence this emulator does not have. Each of these shows
//! nothing about the role and lets the stage go ahead: an engine carrying no IAM store, a
//! `RoleArn` the store holds no role for (it may belong to another account), a trust
//! document that cannot be read as a policy, and a role carrying an attachment whose
//! document this account does not hold — an AWS-managed policy might be the very one that
//! carries the grant. What the documents DO state is decided in IAM's own order: an
//! explicit `Deny` refuses, a grant allows, and policies that state nothing about the call
//! withhold it, because the documents in force on a role are the authority on what it may
//! do.

use ::service_common::iam_authorization::{self as iam, Decision};

/// The IAM store a pipe's `RoleArn` is resolved against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine this
/// service's store lives in, so a pipe reads the very records IAM's handlers wrote rather
/// than a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The IAM store in the SAME shared engine, which is where the account's roles live.
/// `None` means the engine carries no such store — a deployment with no IAM to ask, which
/// is evidence of nothing.
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// The service principal a pipe's execution role has to trust for this service to act as
/// it, as a trust document names one.
pub const SERVICE_PRINCIPAL: &str = "pipes.amazonaws.com";

/// Why this service cannot assume the role `role_arn` AT ALL, or `None` when nothing of
/// the kind can be shown.
///
/// This is the first half of [`call_refusal`] on its own: a role whose trust document is a
/// policy that withholds [`ASSUME_ROLE_ACTION`](::service_common::iam_trust_policy::ASSUME_ROLE_ACTION)
/// from [`SERVICE_PRINCIPAL`] is one no stage of the pipe can act as, so there is nothing
/// for a pipe carrying it to do and a lifecycle call that stores it cannot complete.
/// Asked by itself because the answer is about the ROLE rather than about any one call,
/// and `CreatePipe`/`UpdatePipe` need it before the pipe has made a call at all.
///
/// The cases that show nothing are the module's: an engine carrying no IAM store, a
/// `RoleArn` the store holds no role for (it may belong to another account), a role
/// carrying no trust document, and a document that cannot be read as a policy.
pub fn assume_refusal(role_arn: &str, iam: &IamState) -> Option<String> {
    let role = iam.roles.values().find(|role| role.arn == role_arn)?;
    let trust = role.assume_role_policy_document.as_deref()?;
    match ::service_common::iam_trust_policy::admits_service_principal(trust, SERVICE_PRINCIPAL) {
        Some(false) => Some(assume_failure_reason(role_arn)),
        _ => None,
    }
}

/// [`assume_refusal`] against the IAM store of the shared engine this service runs in, for
/// a handler that holds the engine rather than a locked IAM state.
pub fn engine_assume_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    role_arn: &str,
) -> Option<String> {
    let iam = iam_state(engine)?;
    let held = iam.lock();
    assume_refusal(role_arn, &held)
}

/// The sentence a pipe reports as its `StateReason` when this service cannot assume the
/// pipe's execution role: WHICH role it is (the one variable part) and WHY assuming it
/// fails.
///
/// Bounded by [`STATE_REASON_LENGTH_MAX`], so the sentence is always a legal value of the
/// member that carries it however long the role ARN the request named is.
pub fn assume_failure_reason(role_arn: &str) -> String {
    within_the_reason_bound(format!(
        "Pipes cannot assume the execution role {role_arn}: its trust policy does not let \
         {SERVICE_PRINCIPAL} perform {}.",
        ::service_common::iam_trust_policy::ASSUME_ROLE_ACTION
    ))
}

/// `reason` cut to [`STATE_REASON_LENGTH_MAX`], so a sentence built from the values a
/// request named is always a legal value of the member that carries it however long those
/// values are.
///
/// Every sentence a pipe reports as its `StateReason` goes through here, so no write of that
/// member has a bound of its own.
pub fn within_the_reason_bound(reason: String) -> String {
    match reason.char_indices().nth(STATE_REASON_LENGTH_MAX) {
        None => reason,
        Some((cut, _)) => reason[..cut].to_string(),
    }
}

/// The longest `StateReason` a pipe can report. Source: the `@length(min: 0, max: 512)` of
/// `PipeStateReason` in `smithy-model/com-amazonaws-pipes-pipes.json`.
pub const STATE_REASON_LENGTH_MAX: usize = 512;

/// Why the role `role_arn` names may not perform `action` on `resource`, or `None` when
/// nothing of the kind can be shown (see the module docs for the cases that show nothing).
///
/// The answer is a sentence for the poller's own log and for the failure the stage
/// reports; it is never handed to a client, because no operation of this service reports
/// what a pipe's stages did.
pub fn call_refusal(
    role_arn: &str,
    action: &str,
    resource: &str,
    iam: &IamState,
) -> Option<String> {
    // A role this account does not hold shows nothing: it may belong to another account.
    iam.roles.values().find(|role| role.arn == role_arn)?;
    // The trust document first: a role this service cannot assume never reaches its
    // permissions. One reading of it, shared with the lifecycle calls that store the role
    // ([`assume_refusal`]).
    if let Some(refusal) = assume_refusal(role_arn, iam) {
        return Some(refusal);
    }
    // The documents in force on the role. The collection is bounded by IAM's own quotas
    // on how many policies one role carries, so this walk is bounded by the store rather
    // than by a number of its own.
    let policies = iam.principal_policies(role_arn)?;
    if !policies.uncarried.is_empty() {
        return None;
    }
    let request = iam::Request::new(action, resource, &[]);
    let decision = iam::identity_decision(&policies.documents, &request);
    if decision != Decision::Allowed {
        let because = if decision == Decision::Denied {
            "an identity-based policy explicitly denies it"
        } else {
            "no identity-based policy allows it"
        };
        return Some(format!(
            "its execution role {role_arn} may not perform {action} on {resource}: {because}"
        ));
    }
    // The ceiling attached to the role caps what its own policies grant, so a call the
    // boundary does not allow is withheld however widely the role is granted it.
    let boundary = iam.permissions_boundary_document(role_arn);
    if let Some(boundary) = boundary {
        if iam::identity_decision(&[boundary], &request) != Decision::Allowed {
            return Some(format!(
                "the permissions boundary of its execution role {role_arn} does not allow \
                 {action} on {resource}"
            ));
        }
    }
    None
}
