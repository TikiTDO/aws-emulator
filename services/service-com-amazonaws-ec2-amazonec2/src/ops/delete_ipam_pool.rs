// SPDX-License-Identifier: CC0-1.0

//! @op DeleteIpamPool
//! IMPLEMENTED: deletes the IPAM pool the request names by moving it into the transient
//! `delete-in-progress` state with the clock owing it `delete-complete`, and answers the
//! pool as [`crate::state`] then holds it. This is the ONLY file to touch for this
//! operation.
//!
//! A pool is only deletable once nothing is left inside it: "you cannot delete an IPAM pool
//! if there are allocations in it or CIDRs provisioned to it", and `Cascade` is what "enables
//! you to quickly delete an IPAM pool and all resources within that pool, including
//! provisioned CIDRs, allocations, and other pools". So a pool that still holds pools within
//! it, or still has CIDRs provisioned to it, is refused with `DependencyViolation` unless the
//! request carries `Cascade`, in which case the pools within it and the CIDRs provisioned to
//! them go with it.
//!
//! The refusals, in the order the service answers them: something that is not an
//! `IpamPoolId` at all is `InvalidIpamPoolId.Malformed`, before there is anything to look up;
//! a `DryRun` request a caller holds the permissions for is `DryRunOperation` and writes
//! nothing, because the flag asks whether the CALLER MAY make the request ("without actually
//! making the request", `DryRun`) rather than anything about the named pool; then a
//! well-formed id this account holds no pool for is `InvalidIpamPoolId.NotFound`; a pool
//! whose deletion has already been asked for is `IncorrectState`; a `Cascade` the pool's own
//! scope does not admit is `InvalidParameterCombination`; and what the pool still holds is
//! `DependencyViolation`.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `DeleteIpamPool::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. The store is reached through
//! [`crate::state::require_service_state`], the way every handler beside this one reaches
//! it.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteIpamPool`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Delete an IPAM pool. You cannot delete an IPAM pool if there are allocations in it or
//! CIDRs provisioned to it. To release allocations, see ReleaseIpamPoolAllocation. To
//! deprovision pool CIDRs, see DeprovisionIpamPoolCidr. For more information, see Delete a
//! pool in the Amazon VPC IPAM User Guide.
//!
//! ## Input
//!
//! * `Cascade` (Boolean) — Enables you to quickly delete an IPAM pool and all resources
//!   within that pool, including provisioned CIDRs, allocations, and other pools. You can
//!   only use this option to delete pools in the private scope or pools in the public scope
//!   with a source resource. A source resource is a resource used to provision CIDRs to a
//!   resource planning pool.
//! * `DryRun` (Boolean) — A check for whether you have the required permissions for the
//!   action without actually making the request and provides an error response. If you have
//!   the required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `IpamPoolId` (IpamPoolId) **required** — The ID of the pool to delete.
//!
//! ## Output
//!
//! * `IpamPool` (IpamPool) — Information about the results of the deletion.

#[allow(unused_imports)]
use super::types::*;
use crate::state;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteIpamPoolError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteIpamPoolError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#DeleteIpamPool")]
#[derive(Debug)]
pub enum DeleteIpamPoolErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_ipam_pool::DeleteIpamPoolError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteIpamPoolError = crate::scaffolded::Scaffolded<DeleteIpamPoolErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteIpamPoolErrorModeled> for DeleteIpamPoolError {
    fn from(modeled: DeleteIpamPoolErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteIpamPool` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteIpamPool<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteIpamPool<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteIpamPool<Eng>
{
    type Input = DeleteIpamPoolInput;
    type Output = DeleteIpamPoolOutput;
    type Error = DeleteIpamPoolError;

    const NAME: &'static str = "DeleteIpamPool";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_IPAM_POOL_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_IPAM_POOL_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // `ctx` carries request metadata only; this operation reads none of it — the pool is
        // named in the request and carries its own Region. The store is reached through the
        // engine held on `self`.
        let _ = ctx;

        let pool_id = input.ipam_pool_id.as_str();
        // The request's own FORM first, before anything is resolved: "not an id of this kind"
        // is a malformed request rather than a missing pool, and it is not answered by the
        // DryRun arm below — the order `crate::ops::move_byoip_cidr_to_ipam` asks the same
        // two questions about the same id in.
        state::require_well_formed_id(
            state::IPAM_POOL_ID_PREFIX,
            pool_id,
            state::INVALID_IPAM_POOL_ID_MALFORMED,
        )?;

        // "A check for whether you have the required permissions for the action without
        // actually making the request" (`DryRun`): a question about the CALLER, answered once
        // the request's own form has been checked and before anything is resolved or written;
        // every caller of this emulator is permitted.
        if input.dry_run.unwrap_or(false) {
            return Err(state::dry_run_operation());
        }

        let state_handle = state::require_service_state(&*self.engine)?;
        let mut store = state_handle.lock();
        // The clock first, the way every other state-touching handler in this crate starts: a
        // pool whose `create-complete` or `delete-complete` move has fallen due is already in
        // that state, so every question below is asked of the state the store is ACTUALLY in.
        // Those moves were owed at this instant whether or not anybody called.
        store.materialize_due_transitions();

        // The pool the request names, resolved against the account's pools now that the
        // request's own form has been accepted and the DryRun question answered: an id this
        // account holds no pool for is `.NotFound`.
        let pool = store.ipam_pools.get(pool_id).cloned().ok_or_else(|| {
            state::id_not_found(
                state::INVALID_IPAM_POOL_ID_NOT_FOUND,
                "IPAM pool ID",
                pool_id,
            )
        })?;
        // A pool a delete has already been asked for has nothing left to delete, so a second
        // delete is refused rather than restarting the clock on it — in EC2's own wording for
        // a resource refused SOLELY for the state it is in, through the one writer of it.
        if state::ipam_pool_is_deleting(&pool) {
            return Err(state::incorrect_state_of(
                "IPAM pool",
                pool_id,
                pool.state.as_deref().unwrap_or_default(),
            ));
        }

        // The pools created WITHIN the pool, which is half of what decides whether this
        // delete is admissible at all.
        let within = store.ipam_pools_within(pool_id);

        if input.cascade.unwrap_or(false) {
            // "You can only use this option to delete pools in the private scope or pools in
            // the public scope with a source resource" (`Cascade`): a public-scope pool that
            // provisions no resource planning pool is not one this option may be used on.
            let scope_type = pool.ipam_scope_type.as_deref().unwrap_or_default();
            if scope_type != state::IPAM_SCOPE_TYPE_PRIVATE && pool.source_resource.is_none() {
                return Err(state::invalid_parameter_combination(format!(
                    "Cascade may be specified only for a pool in the '{private}' scope or a \
                     pool in the '{public}' scope with a source resource; the IPAM pool \
                     '{pool_id}' is in the '{scope_type}' scope and has none",
                    private = state::IPAM_SCOPE_TYPE_PRIVATE,
                    public = state::IPAM_SCOPE_TYPE_PUBLIC,
                )));
            }
            // "Enables you to quickly delete an IPAM pool and all resources within that pool,
            // including provisioned CIDRs, allocations, and other pools" (`Cascade`): every
            // pool within this one is deleted on the same terms as this one — deepest first,
            // so no pool is left inside a pool that is already going — and the CIDRs
            // provisioned to each of them are deprovisioned.
            for inner_pool_id in &within {
                store.deprovision_ipam_pool_cidrs(inner_pool_id);
                store.begin_ipam_pool_deletion(inner_pool_id);
            }
            store.deprovision_ipam_pool_cidrs(pool_id);
        } else {
            // "You cannot delete an IPAM pool if there are allocations in it or CIDRs
            // provisioned to it. To deprovision pool CIDRs, see DeprovisionIpamPoolCidr"
            // (`DeleteIpamPool`), and a pool within this one is a resource within it that
            // `Cascade` is the one way to delete along with it. EC2 words both refusals the
            // same way: the action, the resource it is not valid for, and what is still in it.
            if !store.ipam_pool_provisioned_cidrs(pool_id).is_empty() {
                return Err(state::dependency_violation(format!(
                    "The specified action is not valid for {pool_id}. There are one or more \
                     pool cidrs provisioned in the pool."
                )));
            }
            if !within.is_empty() {
                return Err(state::dependency_violation(format!(
                    "The specified action is not valid for {pool_id}. There are one or more \
                     pools in the pool."
                )));
            }
        }

        // "Information about the results of the deletion" (`IpamPool`): the pool as it was
        // STORED by the move this delete started, so the caller sees what a later reader of
        // the pools will see rather than a value built here — reported through the one
        // projection every reader of a pool reports it by
        // ([`state::reported_ipam_pool`]), which is what carries the modelled members a pool
        // reports whether or not its record holds a value for them.
        let view = store.begin_ipam_pool_deletion(pool_id).ok_or_else(|| {
            state::internal_error(format!(
                "the IPAM pool '{pool_id}' resolved a moment ago is not in the store any more"
            ))
        })?;

        Ok(DeleteIpamPoolOutput {
            ipam_pool: Some(state::reported_ipam_pool(view)),
        })
    }
}
