// SPDX-License-Identifier: CC0-1.0

//! The EXECUTION ROLE a budget action is written with, resolved against the account's IAM
//! records before the action is stored.
//!
//! A budget action does not act as whoever created it: "the role passed for action
//! execution and reversion" (`Action$ExecutionRoleArn`) is the role this service assumes to
//! apply the action's `Definition` and to take it back off again, so a role this account's
//! IAM does not hold — or one whose trust policy does not let this service assume it at
//! all — is an action that could never execute. Such a request is refused with the
//! `AccessDeniedException` the write paths model (HTTP 403), carrying the sentence
//! [`unassumable_refusal`] builds — which names the role asked for and the assume-role
//! grant this service's principal is missing on it, not the shape's own documented text —
//! and nothing is written, rather than the action being accepted and left in the account
//! with no role behind it.
//!
//! The question is ONE question — does this account hold that role, and does its trust
//! policy admit this service's principal — so it is asked through the shared reading every
//! emulated service resolving a caller-supplied execution role takes
//! ([`::service_common::iam_role::role_is_unassumable`], over the in-process hop to IAM):
//! this module states only WHICH principal is asked about and HOW the refusal is worded.
//! That reading refuses only what IAM itself ANSWERED — a role it reports this account
//! holds nothing under, or a trust document that withholds the principal — and stays silent
//! where nothing was learned (a value naming no role of this account, an emulator with no
//! IAM mounted), so no request is refused on evidence this emulator does not have.

/// The service principal a budget action's execution role has to trust for this service to
/// assume it, as a trust document names one.
pub const SERVICE_PRINCIPAL: &str = "budgets.amazonaws.com";

/// Why the action a request writes may not be written with the role `execution_role_arn`
/// names, or `None` when IAM showed nothing of the kind.
///
/// `Some` carries the sentence that goes in `AccessDeniedException$Message`: it names the
/// role the request asked for and the grant that is missing, and is the same refusal
/// whichever write path (`CreateBudgetAction`, `UpdateBudgetAction`) named it, so the two
/// cannot drift apart.
///
/// The question goes to IAM over the IN-PROCESS callback
/// ([`smithy_server_runtime::OperationContext::callback`]), the way every emulated service
/// asks another: nothing here opens a socket or leaves the host, and the address that hop
/// is made to is [`super::PARTITION_GLOBAL_REGION`] — this service's own constant — so no
/// caller-supplied value reaches it.
pub async fn unassumable_refusal(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    execution_role_arn: &str,
) -> Option<String> {
    ::service_common::iam_role::role_is_unassumable(
        ctx,
        super::PARTITION_GLOBAL_REGION,
        execution_role_arn,
        SERVICE_PRINCIPAL,
    )
    .await
    .then(|| unassumable_message(execution_role_arn))
}

/// What [`unassumable_refusal`] says: the role the request named, and the assume-role grant
/// this service's principal needs on it for the action to be able to run.
fn unassumable_message(execution_role_arn: &str) -> String {
    format!(
        "Budgets permission required to assume [ExecutionRole: {execution_role_arn}]. \
         Please follow the instruction to grant assumeRole access to [Service Principal: \
         {SERVICE_PRINCIPAL}]."
    )
}
