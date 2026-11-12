// SPDX-License-Identifier: CC0-1.0

//! The CONCURRENCY one invocation holds while it runs: the slot an admitted invocation
//! takes, the admission decision that hands it out, and which kind of environment served
//! it.
//!
//! Concurrency is the number of invocations a function is processing at one instant, and it
//! is BOUNDED: "Lambda returns `TooManyRequestsException` if running the function would
//! cause you to exceed a concurrency limit at either the account level
//! (`ConcurrentInvocationLimitExceeded`) or function level
//! (`ReservedFunctionConcurrentInvocationLimitExceeded`)" (`Invoke`). What a function
//! reserves is that function-level limit: "The number of simultaneous executions to reserve
//! for the function", and "concurrency settings apply to the function as a whole, including
//! all published versions and the unpublished version" (`PutFunctionConcurrency`) — so the
//! reservation is ONE pool, drawn on by every qualifier of the function alike, and a
//! reservation of zero admits nothing at all.
//!
//! Three things follow, and each is observable:
//!
//! * AN ADMITTED INVOCATION HOLDS ITS SLOT UNTIL IT IS DONE. [`ExecutionSlot`] is a guard:
//!   the count goes up where the invocation is admitted and comes down when the slot is
//!   dropped, which happens however that invocation ends — a handler that returned, one
//!   that raised, a run the sandbox killed for its memory or its timeout, a caller that
//!   went away. So a slot cannot leak and strand a function's capacity, and the next
//!   caller's admission sees the pool as it really is.
//! * WHAT IS IN FLIGHT IS WHAT IS COUNTED. The counts live on the function record
//!   ([`super::FunctionRecord::concurrent_executions`] and its per-qualifier breakdown), so
//!   the account's concurrency metering publishes the executions that are really running
//!   rather than a quantity nothing is using (see [`super::concurrency_metering`]). An
//!   admission is also the instant the function's concurrency RISES, so what it took the
//!   count to is reported back to the caller ([`Admitted::concurrent_executions`]) as the
//!   reading of that gauge to publish.
//! * A PROVISIONED ENVIRONMENT IS REPORTED AS ONE. Provisioned concurrency "initializes a
//!   requested number of execution environments so that they are prepared to respond
//!   immediately to your function's invocations", allocated per alias or version
//!   (`PutProvisionedConcurrencyConfig`), and an invocation one of those initialised
//!   environments serves reports `AWS_LAMBDA_INITIALIZATION_TYPE=provisioned-concurrency` to
//!   the runtime. So the admission that finds the addressed qualifier's allocation ready and
//!   not yet full is the one served that way, and everything past the allocation is served
//!   on demand.
//!
//! Admission is ONE critical section — the pool is read and the slot taken under the same
//! store guard — so two invocations arriving at once cannot both be admitted into the last
//! slot. Nothing here waits: an invocation that cannot be admitted is refused at once with
//! the modelled throttle, which is what a client retries on.

use std::time::SystemTime;

use ::engine::ServiceState;

use super::{ComAmazonawsLambdaAwsgirapiserviceState, FunctionRecord};

/// What `AWS_LAMBDA_INITIALIZATION_TYPE` reports for an invocation served by an environment
/// Lambda initialised on demand: the default for every invocation of a function that has no
/// provisioned concurrency allocated for the qualifier it addressed.
pub const INITIALIZATION_TYPE_ON_DEMAND: &str = "on-demand";

/// What it reports for an invocation served by one of the environments provisioned
/// concurrency keeps initialised.
pub const INITIALIZATION_TYPE_PROVISIONED_CONCURRENCY: &str = "provisioned-concurrency";

/// The `Reason` the modelled throttle names when the FUNCTION's own reservation is full,
/// one of the model's `ThrottleReason` values
/// (`smithy_query(query="shape", shape="ThrottleReason")`).
pub const RESERVED_FUNCTION_CONCURRENCY_REASON: &str =
    "ReservedFunctionConcurrentInvocationLimitExceeded";

/// The `Reason` it names when the capacity that was full is the ACCOUNT's concurrent
/// executions rather than one function's reservation, the other of the two the operation
/// documents and one of the same `ThrottleReason` values.
///
/// This is the pool a function that reserves nothing draws on, which in this emulator is the
/// engine memory the substrate runs handlers in
/// (`super::execution::MAX_CONCURRENT_RUN_MEMORY_MB`): an invocation refused because none of
/// it came free is refused on the account's capacity, so that is the reason it carries.
pub const ACCOUNT_CONCURRENCY_REASON: &str = "ConcurrentInvocationLimitExceeded";

/// The message Lambda words a throttled invocation with: the fixed sentence of the throttle,
/// which names nothing about the request.
pub const THROTTLED_INVOCATION_MESSAGE: &str = "Rate Exceeded.";

/// Why an invocation was not admitted: what the modelled `TooManyRequestsException` the
/// calling operation answers says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Throttled {
    /// The `Reason` member of that error.
    pub reason: &'static str,
    /// Its message.
    pub message: String,
}

/// The slot one admitted invocation holds for as long as it is in flight.
///
/// Dropping it releases the capacity, so the slot is held by the operation serving the
/// invocation for exactly as long as that invocation lasts. It carries a handle on the store
/// rather than a borrow of it, because the run it covers happens with no store guard held.
pub struct ExecutionSlot {
    /// The store the counts live in.
    state: ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    /// The function the invocation runs on, which the pool belongs to.
    function_name: String,
    /// The qualifier the invocation addressed, whose own in-flight count it joined.
    qualifier: String,
    /// Whether this slot really claimed capacity: a function that was deleted between
    /// resolving it and admitting the invocation has no pool to charge, and releasing one it
    /// never took would leave the count of a function recreated under the same name short.
    claimed: bool,
}

impl ::std::fmt::Debug for ExecutionSlot {
    /// What the slot holds, without the store handle it releases into (a store handle has
    /// no rendering of its own).
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("ExecutionSlot")
            .field("function_name", &self.function_name)
            .field("qualifier", &self.qualifier)
            .field("claimed", &self.claimed)
            .finish()
    }
}

impl Drop for ExecutionSlot {
    fn drop(&mut self) {
        if !self.claimed {
            return;
        }
        let mut store = self.state.lock();
        let Some(record) = store.functions.get_mut(&self.function_name) else {
            return;
        };
        // Never below nothing: the count a released slot gives back is the one it took, and
        // a count already at nothing stays there rather than going negative.
        record.concurrent_executions = (record.concurrent_executions - 1).max(0);
        if let Some(in_flight) = record.in_flight_by_qualifier.get_mut(&self.qualifier) {
            *in_flight = (*in_flight - 1).max(0);
            if *in_flight == 0 {
                record.in_flight_by_qualifier.remove(&self.qualifier);
            }
        }
    }
}

/// An invocation that was admitted: the slot it holds, and the kind of execution environment
/// serving it.
#[derive(Debug)]
pub struct Admitted {
    /// Held until the invocation is done; dropping it releases the capacity.
    pub slot: ExecutionSlot,
    /// What the run reports as `AWS_LAMBDA_INITIALIZATION_TYPE`:
    /// [`INITIALIZATION_TYPE_PROVISIONED_CONCURRENCY`] for an invocation one of the
    /// qualifier's initialised environments serves, [`INITIALIZATION_TYPE_ON_DEMAND`]
    /// otherwise.
    pub initialization_type: &'static str,
    /// How many executions the function is processing now that this invocation has joined
    /// them — the reading of the function's concurrency gauge AT this admission, which is
    /// what the caller publishes as one observation of it
    /// ([`super::concurrency_metering::publish_function_concurrency`]).
    ///
    /// Zero for an invocation that claimed nothing, which is one whose function was deleted
    /// between resolving it and admitting the invocation: it joined no pool, so it is no
    /// reading of one.
    pub concurrent_executions: i32,
}

/// Admit one invocation of `function_name` addressed at `qualifier` to the function, or
/// refuse it with the modelled throttle when the function's reservation is already full.
///
/// `qualifier` is the label the CALL addressed — an alias name, a published version number or
/// the unpublished version — because provisioned concurrency is allocated per alias or
/// version; the reservation is the function's as a whole and does not read it.
pub fn admit_invocation(
    state: &ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    function_name: &str,
    qualifier: &str,
    now: SystemTime,
) -> Result<Admitted, Throttled> {
    let mut store = state.lock();
    let Some(record) = store.functions.get_mut(function_name) else {
        // The function was deleted between resolving it and admitting this invocation:
        // there is no pool to charge, so nothing is claimed and nothing will be released.
        return Ok(Admitted {
            slot: ExecutionSlot {
                state: ServiceState::clone(state),
                function_name: function_name.to_string(),
                qualifier: qualifier.to_string(),
                claimed: false,
            },
            initialization_type: INITIALIZATION_TYPE_ON_DEMAND,
            concurrent_executions: 0,
        });
    };
    // "The number of simultaneous executions to reserve for the function": the invocation is
    // admitted only while the function is processing fewer than that, and a function that
    // reserves nothing is bounded by the account's capacity instead — which in this emulator
    // is the engine memory the execution substrate runs handlers in
    // (`super::execution::MAX_CONCURRENT_RUN_MEMORY_MB`), below the account ceiling
    // (`super::CONCURRENT_EXECUTIONS_LIMIT`). An invocation this admission let in WAITS for
    // its share of that memory rather than being refused for it, so that bound is no limit of
    // the API; the throttle a substrate that frees none in the whole wait answers carries the
    // account-level reason ([`ACCOUNT_CONCURRENCY_REASON`]) — which is therefore the
    // substrate's to produce and not this admission's.
    if let Some(reserved) = record.reserved_concurrent_executions {
        if record.concurrent_executions >= reserved {
            return Err(Throttled {
                reason: RESERVED_FUNCTION_CONCURRENCY_REASON,
                message: THROTTLED_INVOCATION_MESSAGE.to_string(),
            });
        }
    }
    // Which environment serves it is decided BEFORE the slot is taken: the invocations that
    // fit inside the qualifier's ready allocation are the ones its initialised environments
    // serve, and this one fits when the qualifier is processing fewer than that already.
    let allocated = record.ready_provisioned_concurrency(qualifier, now);
    let in_flight_on_qualifier = record
        .in_flight_by_qualifier
        .get(qualifier)
        .copied()
        .unwrap_or_default();
    let initialization_type = if in_flight_on_qualifier < allocated {
        INITIALIZATION_TYPE_PROVISIONED_CONCURRENCY
    } else {
        INITIALIZATION_TYPE_ON_DEMAND
    };
    record.concurrent_executions = record.concurrent_executions.saturating_add(1);
    *record
        .in_flight_by_qualifier
        .entry(qualifier.to_string())
        .or_insert(0) += 1;
    Ok(Admitted {
        slot: ExecutionSlot {
            state: ServiceState::clone(state),
            function_name: function_name.to_string(),
            qualifier: qualifier.to_string(),
            claimed: true,
        },
        initialization_type,
        concurrent_executions: record.concurrent_executions,
    })
}

impl FunctionRecord {
    /// The provisioned concurrency ready to serve invocations of `qualifier` at `now`.
    ///
    /// "The amount of provisioned concurrency allocated" is all of what was requested once
    /// the allocation is ready and none of it while Lambda is still making it
    /// ([`super::ProvisionedConcurrencyRecord::allocated`]), and the move to ready is owed by
    /// the CLOCK — so the stored record is brought up to `now` on a copy of it: what an
    /// admission reads is what a read of the configuration would report at the same instant,
    /// and reading it changes nothing a later caller sees.
    ///
    /// Zero for a qualifier nobody has provisioned concurrency for, which is every qualifier
    /// of a function nobody has configured.
    #[must_use]
    pub fn ready_provisioned_concurrency(&self, qualifier: &str, now: SystemTime) -> i32 {
        self.provisioned_concurrency_configs
            .get(qualifier)
            .map_or(0, |configured| {
                let mut settled = configured.clone();
                settled.materialize(now);
                settled.allocated()
            })
    }
}
