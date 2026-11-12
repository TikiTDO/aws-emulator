// SPDX-License-Identifier: CC0-1.0

//! @op GetPlanInRegion
//!
//! The Region-scoped read of a plan: it reports the stored plan record the request's
//! `arn` names, exactly as [`crate::ops::get_plan`] projects it, but only from a Region
//! the plan is available in — the Region whose endpoint the caller reached, taken from
//! the request's SigV4 credential scope by [`crate::state::request_region`].
//!
//! A plan is available in a Region when its `regions` names that Region: a plan carries
//! exactly two (`RegionList`, `@length(min: 2, max: 2)`) and the service's own
//! `ListPlansInRegion` is documented to list "plans in your account that are available in
//! the current Region", so the plan's Region pair is what availability-per-Region means.
//! An `arn` no plan is stored under, and a stored plan the Region the call reached is not
//! one of, are one refusal — from that Region there is no such plan to report — answered
//! with the modelled `ResourceNotFoundException` and its message at
//! [`crate::state::IN_REGION_NOT_FOUND_STATUS`], the status this service answers a
//! Region-scoped read's unresolvable `arn` with.
//!
//! Nothing Region-specific lives in the `Plan` shape itself (every member of it is a
//! property of the global plan — its ARN carries an empty Region segment), so the
//! projection is the whole stored record, not a narrowed one.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. Reach
//! the store through [`crate::state::service_state`] and read it through the guard it
//! hands back. `ctx` ([`smithy_server_runtime::OperationContext`]) carries request
//! metadata only — which is where the Region comes from; the engine is NOT reached
//! through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetPlanInRegion`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")
//! ```
//!
//! Retrieves information about a Region switch plan in a specific Amazon Web Services
//! Region. This operation is useful for getting Region-specific information about a plan.
//!
//! ## Input
//!
//! * `arn` (PlanArn, pattern
//!   ^arn:aws[a-zA-Z-]*:arc-region-switch::[0-9]{12}:plan/([a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?):([a-z0-9]{6})$)
//!   **required** — The Amazon Resource Name (ARN) of the plan in Region.
//!
//! ## Output
//!
//! * `plan` (Plan) — The details of the Region switch plan.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action. HTTP Status Code: 403
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource was not found. HTTP
//!   Status Code: 404. This Region-scoped read answers it at
//!   [`crate::state::IN_REGION_NOT_FOUND_STATUS`] instead — see the error channel below.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetPlanInRegionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetPlanInRegionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
// `InternalServerException` — a shape the SERVICE models, which this operation does not
// list — is widened in through `extra_errors`, so an unreachable shared store has a
// modelled server failure to answer with.
#[smithy_macros::operation_error(
    "com.amazonaws.arcregionswitch#GetPlanInRegion",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum GetPlanInRegionErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_plan_in_region::GetPlanInRegionError`).
///
/// Two variants, because the status is not always the one the model gives the shape:
/// [`Self::Scaffolded`] is the usual channel (a modelled error, the 501, or a refusal the
/// model cannot express), rendered exactly as every other operation renders it, and
/// [`Self::AtStatus`] is a modelled error this service answers at a status of its own —
/// the shape, its `__type` and its message stay the modelled ones.
#[derive(Debug)]
pub enum GetPlanInRegionError {
    /// A modelled error, the scaffold 501, or a handler refusal, at the status the model
    /// gives its shape.
    Scaffolded(crate::scaffolded::Scaffolded<GetPlanInRegionErrorModeled>),
    /// A modelled error at the HTTP status THIS service answers it with.
    AtStatus(GetPlanInRegionErrorModeled, u16),
}

impl GetPlanInRegionError {
    /// The refusal of an `arn` this read cannot resolve a plan from, in the Region the
    /// call reached: the modelled `ResourceNotFoundException` and its message, at
    /// [`crate::state::IN_REGION_NOT_FOUND_STATUS`].
    pub fn not_resolvable_in_region(arn: &str) -> Self {
        Self::AtStatus(
            GetPlanInRegionErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: crate::state::plan_not_found_message(arn),
            }),
            crate::state::IN_REGION_NOT_FOUND_STATUS,
        )
    }
}

impl ::std::fmt::Display for GetPlanInRegionError {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match self {
            Self::Scaffolded(error) => write!(f, "{error}"),
            Self::AtStatus(error, _) => write!(f, "{error}"),
        }
    }
}

impl ::std::error::Error for GetPlanInRegionError {
    fn source(&self) -> Option<&(dyn ::std::error::Error + 'static)> {
        match self {
            Self::Scaffolded(error) => error.source(),
            Self::AtStatus(error, _) => error.source(),
        }
    }
}

/// Every method delegates to the modelled error the active variant carries, so the wire
/// shape, the error code and the message are the model's in both variants; only
/// [`Self::AtStatus`] answers a status of the service's own.
impl smithy_server_runtime::OperationError for GetPlanInRegionError {
    fn as_serializable(&self) -> &dyn ::aws_smithy_schema::serde::SerializableStruct {
        match self {
            Self::Scaffolded(error) => error.as_serializable(),
            Self::AtStatus(error, _) => error.as_serializable(),
        }
    }

    fn variant_schema(&self) -> &'static ::aws_smithy_schema::Schema {
        match self {
            Self::Scaffolded(error) => error.variant_schema(),
            Self::AtStatus(error, _) => error.variant_schema(),
        }
    }

    fn shape_name(&self) -> &'static str {
        match self {
            Self::Scaffolded(error) => error.shape_name(),
            Self::AtStatus(error, _) => error.shape_name(),
        }
    }

    fn http_status(&self) -> u16 {
        match self {
            Self::Scaffolded(error) => error.http_status(),
            Self::AtStatus(_, status) => *status,
        }
    }

    fn query_error_code(&self) -> Option<&'static str> {
        match self {
            Self::Scaffolded(error) => error.query_error_code(),
            Self::AtStatus(error, _) => error.query_error_code(),
        }
    }
}

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetPlanInRegionErrorModeled> for GetPlanInRegionError {
    fn from(modeled: GetPlanInRegionErrorModeled) -> Self {
        Self::Scaffolded(crate::scaffolded::Scaffolded::Modeled(modeled))
    }
}

/// The same for a refusal the scaffold channel words (`Scaffolded::validation`).
impl From<crate::scaffolded::Scaffolded<GetPlanInRegionErrorModeled>> for GetPlanInRegionError {
    fn from(scaffolded: crate::scaffolded::Scaffolded<GetPlanInRegionErrorModeled>) -> Self {
        Self::Scaffolded(scaffolded)
    }
}

/// The `GetPlanInRegion` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetPlanInRegion<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetPlanInRegion<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetPlanInRegion<Eng>
{
    type Input = GetPlanInRegionInput;
    type Output = GetPlanInRegionOutput;
    type Error = GetPlanInRegionError;

    const NAME: &'static str = "GetPlanInRegion";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PLAN_IN_REGION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PLAN_IN_REGION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // IAM decides before the resource's state is looked at, so a caller no
        // identity-based policy allows is told only that it is not authorized.
        crate::service::authorization::authorize(&*self.engine, ctx, Self::NAME, &input.arn)
            .map_err(GetPlanInRegionErrorModeled::AccessDeniedException)?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetPlanInRegionErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the Region switch plan store is unavailable".to_string(),
                },
            )
            .into());
        };

        // The Region this call reports from. The input names no Region, so it is the one
        // the request is scoped to — the Region whose endpoint the caller reached.
        let region = crate::state::request_region(ctx);

        // `@readonly`: the only write is `settle`, which materialises the state changes the
        // clock already made due — owed at the same instant whether or not anybody called.
        // Every handler settles, so no two readers disagree about the current state.
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        store.settle(now);

        // One resolution step for the whole Region-scoped read: an `arn` no plan is
        // stored under and a stored plan this Region is not one of are the same refusal.
        let Some(record) = store.plan_available_in_region(&input.arn, &region) else {
            return Err(GetPlanInRegionError::not_resolvable_in_region(&input.arn));
        };
        Ok(GetPlanInRegionOutput {
            plan: Some(record.as_plan()),
        })
    }
}
