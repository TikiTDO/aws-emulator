// SPDX-License-Identifier: CC0-1.0

//! Why a pipe cannot be BROUGHT UP — the question a lifecycle write asks before it reports
//! a pipe on its way to a healthy state.
//!
//! A pipe is accepted synchronously and reaches the state its `DesiredState` asked for a
//! moment later ([`super::STATE_SETTLE_DELAY`]), so what a pipe could not be brought up for
//! is not an answer to the call: it is where the pipe SETTLES. A move that cannot be
//! completed lands in the terminal failure state the model declares for it —
//! `CREATE_FAILED` for a creation, `START_FAILED` for a start, `UPDATE_FAILED` for a
//! reconfiguration — carrying the `StateReason` that says why, and the pipe runs nothing.
//!
//! Two things a pipe cannot poll its source without are decided here, in the order the
//! pipe would need them:
//!
//! 1. its EXECUTION ROLE, which every stage acts as ([`super::execution_role`]): a role
//!    whose trust document withholds the assume is one no stage could act as, and a role
//!    whose policies withhold the call that TAKES the records from the source is one the
//!    source stage could never poll under;
//! 2. the SOURCE itself: a `Source` naming a resource the service that owns it does not
//!    hold is a source with nothing to poll.
//!
//! The TARGET side is deliberately NOT among them. A delivery the target refuses — a role
//! that may not write to it, a target that is not there — is one EXECUTION of a RUNNING
//! pipe failing, counted as such ([`super::metrics`]) with the records left on the source
//! for the next attempt; that is what lets a pipe deliver the record it withheld as soon as
//! the grant it was missing is in force. Only what the SOURCE stage cannot start without
//! decides whether the pipe comes up at all.
//!
//! Nothing is refused on evidence this emulator does not have: a source of a kind no owner
//! here is asked, a source of another account, an owner that did not answer, and (through
//! [`super::execution_role`]) a role the account does not hold, an unreadable trust
//! document or a policy attachment whose document this account does not carry — each shows
//! nothing, and a pipe carrying it comes up.

/// Why the pipe that would run as `role_arn` over the source `source` cannot be brought up,
/// or `None` when nothing of the kind can be shown.
///
/// The one decision behind every lifecycle write that reports a pipe up (`CreatePipe`,
/// `StartPipe`, `UpdatePipe`), so none of them can bring up a pipe another would not.
///
/// The answer is a sentence for the pipe's own `StateReason`, bounded by that member's
/// `@length` ([`super::execution_role::within_the_reason_bound`]). No store is locked while
/// the owner of the source is asked, so no request waits on this decision behind a lock.
pub async fn refusal<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Eng,
    source: &str,
    role_arn: &str,
) -> Option<String> {
    // The trust document first: a role this service cannot assume at all never reaches the
    // question of what it may do.
    if let Some(reason) = super::execution_role::engine_assume_refusal(engine, role_arn) {
        return Some(reason);
    }
    if let Some(reason) = source_read_refusal(engine, source, role_arn) {
        return Some(reason);
    }
    absent_source_refusal(ctx, source).await
}

/// Why the role `role_arn` could not take the records of `source`, or `None` when the
/// policies in force on it show nothing of the kind.
///
/// The call asked about is the one that TAKES the records for that kind of source
/// ([`super::data_plane::source_record_action`]) — a source whose records this emulator
/// polls through no call of its own is a question that is not asked.
fn source_read_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    source: &str,
    role_arn: &str,
) -> Option<String> {
    let action = super::data_plane::source_record_action(source)?;
    let iam = super::execution_role::iam_state(engine)?;
    // Held for the length of this one decision, and no call leaves the process while it is.
    let held = iam.lock();
    let refusal = super::execution_role::call_refusal(role_arn, action, source, &held)?;
    Some(super::execution_role::within_the_reason_bound(format!(
        "Pipes cannot poll the source {source}: {refusal}."
    )))
}

/// Why the source `source` names cannot be polled because it is not there, or `None` when
/// the service that owns it does not say so.
async fn absent_source_refusal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    source: &str,
) -> Option<String> {
    super::data_plane::source_is_absent(ctx.callback(), source)
        .await
        .then(|| {
            super::execution_role::within_the_reason_bound(format!(
                "Pipes cannot poll the source {source}: it does not exist."
            ))
        })
}
