// SPDX-License-Identifier: CC0-1.0

//! @op UpdateUsage
//! Grants a temporary extension to the remaining quota of a usage plan associated with a
//! specified API key.
//!
//! HAND-IMPLEMENTED. A MUTATION by its own documentation — the grant it applies is written
//! into the one quota accounting the `execute-api` data plane admits requests against
//! (`crate::state::usage`), so the extension a caller is told about is the one the next
//! request from that API key meets, and the answer is that accounting read back rather than
//! anything synthesised per request.
//!
//! The grant covers the metering period the call falls in and no more: the next period
//! starts at the plan's own `quota` limit again, which is what makes the extension
//! TEMPORARY.
//!
//! ---
//!
//! # Modeled contract for `UpdateUsage`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json")
//! ```
//!
//! Grants a temporary extension to the remaining quota of a usage plan associated with a
//! specified API key.
//!
//! ## Input
//!
//! * `keyId` (String) **required** — The identifier of the API key associated with the usage
//!   plan in which a temporary extension is granted to the remaining quota.
//! * `patchOperations` (ListOfPatchOperation) — For more information about supported patch
//!   operations, see Patch Operations.
//! * `usagePlanId` (String) **required** — The Id of the usage plan associated with the usage
//!   data.
//!
//! ## Output
//!
//! * `endDate` (String) — The ending date of the usage data.
//! * `items` (MapOfKeyUsages) — The usage data, as daily logs of used and remaining quotas,
//!   over the specified time interval indexed over the API keys in a usage plan. For example,
//!   {..., "values" : { "{api_key}" : [ [0, 100], [10, 90], [100, 10]]}, where {api_key}
//!   stands for an API key value and the daily log entry is of the format [used quota,
//!   remaining quota].
//! * `position` (String) — The current pagination position in the paged result set.
//! * `startDate` (String) — The starting date of the usage data.
//! * `usagePlanId` (String) — The plan Id associated with this usage data.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — The submitted request is not valid, for example, the
//!   input is incomplete or incorrect. See the accompanying error message for details.
//! * `ConflictException` (HTTP 409) — The request configuration has conflicts. For details,
//!   see the accompanying error message.
//! * `LimitExceededException` (HTTP 429) — The request exceeded the rate limit. Retry after
//!   the specified time period.
//! * `NotFoundException` (HTTP 404) — The requested resource is not found. Make sure that the
//!   request URI is correct.
//! * `TooManyRequestsException` (HTTP 429) — The request has reached its throttling limit.
//!   Retry after the specified time period.
//! * `UnauthorizedException` (HTTP 401) — The request is denied because the caller has
//!   insufficient permissions.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{
    patch_i32_checked, InvalidPathRefusal, Patch, PatchFailure, SupportedPatchPaths,
};

/// The paths a usage plan key's `Usage` publishes: the remaining quota of the metering
/// period, which is the one thing this operation grants — "a temporary extension to the
/// remaining quota of a usage plan associated with a specified API key" (`UpdateUsage`'s own
/// documentation).
///
/// Published for the two ops that SET a single-valued member, as every other resource of
/// this service publishes such a member: the remaining quota is a count the period always
/// has, so it can be set but not removed.
const PATCH_PATHS: SupportedPatchPaths = SupportedPatchPaths {
    add: &["/remaining"],
    remove: &[],
    replace: &["/remaining"],
};

/// How this operation words a refusal of a patch `path` it cannot apply: the path quoted,
/// the `op` that named it and the paths this resource publishes for that op — the wording
/// the usage plan's own update answers one with, because both are patches of a usage plan's
/// records.
const INVALID_PATH: InvalidPathRefusal = InvalidPathRefusal::PathOpAndSupported(PATCH_PATHS);

/// Apply one patch to the remaining quota the metering period has for this API key.
///
/// `remaining` is a count of requests, so a value that is no count of them is refused with
/// the sentence this service states for a quota count that is not one
/// ([`crate::state::invalid_quota_count`]) — the same sentence whichever operation carried
/// it. Every other pointer names no member of this record.
fn apply_patch(current: &mut i32, patch: &Patch) -> Result<(), PatchFailure> {
    match patch.member() {
        "remaining" => patch_i32_checked(current, patch, |value| {
            if value < 0 {
                return Err(crate::state::invalid_quota_count("remaining"));
            }
            Ok(value)
        }),
        _ => Err(patch.unsupported_path()),
    }
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateUsageError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateUsageError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.apigateway#UpdateUsage")]
#[derive(Debug)]
pub enum UpdateUsageErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    LimitExceededException(LimitExceededException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
    UnauthorizedException(UnauthorizedException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_usage::UpdateUsageError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateUsageError = crate::scaffolded::Scaffolded<UpdateUsageErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateUsageErrorModeled> for UpdateUsageError {
    fn from(modeled: UpdateUsageErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateUsage` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateUsage<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateUsage<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateUsage<Eng>
{
    type Input = UpdateUsageInput;
    type Output = UpdateUsageOutput;
    type Error = UpdateUsageError;

    const NAME: &'static str = "UpdateUsage";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_USAGE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_USAGE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(crate::state::state_unavailable)?;

        // The list is read BEFORE the store lock is taken, so a malformed patch never
        // reaches the accounting at all.
        let patches =
            crate::state::parse_patches_refused_with(input.patch_operations.as_ref(), INVALID_PATH)
                .map_err(patch_failure)?;

        // The day the grant is made on: a quota period is counted in whole UTC days, so the
        // period this call falls in is the one that holds today.
        let day = crate::state::usage_today();
        let mut store = state.lock();
        // The plan is the resource the request's URI addresses, so it is resolved first, the
        // way the path itself reads — exactly as the read of the same usage data resolves it.
        if !store.usage_plans.contains_key(&input.usage_plan_id) {
            return Err(UpdateUsageErrorModeled::NotFoundException(NotFoundException {
                message: Some(crate::state::NO_SUCH_USAGE_PLAN.to_string()),
            })
            .into());
        }
        // `keyId` is "the identifier of the API key associated with the usage plan in which a
        // temporary extension is granted to the remaining quota", so it has to name an API
        // key this plan METERS: a pair the plan does not meter has no quota of this plan to
        // extend, and is refused as the API key identifier it is.
        if store
            .usage_plan_key(&input.usage_plan_id, &input.key_id)
            .is_none()
        {
            return Err(UpdateUsageErrorModeled::NotFoundException(NotFoundException {
                message: Some(crate::state::NO_SUCH_METERED_API_KEY.to_string()),
            })
            .into());
        }
        // What the metering period has left for this pair right now, which is the value a
        // patch of `/remaining` replaces. A plan metering no quota in a window has no
        // remaining quota at all, so a grant against it names a setting the plan has not got.
        let remaining_now = store
            .remaining_quota(&input.usage_plan_id, &input.key_id, day)
            .ok_or_else(|| {
                UpdateUsageErrorModeled::BadRequestException(BadRequestException {
                    message: Some(crate::state::NO_USAGE_PLAN_QUOTA.to_string()),
                })
            })?;
        let mut remaining = i32::try_from(remaining_now).unwrap_or(i32::MAX);
        // Applied to the VALUE first, in request order, so a list whose later op is refused
        // leaves the accounting exactly as it was rather than keeping the grant the ops
        // before it would have made.
        for patch in &patches {
            apply_patch(&mut remaining, patch).map_err(patch_failure)?;
        }
        // A request carrying no patch operation grants nothing, and so writes nothing: it
        // reports the usage data as it stands.
        if !patches.is_empty() {
            store.grant_remaining_quota(
                &input.usage_plan_id,
                &input.key_id,
                day,
                i64::from(remaining),
            );
        }
        // The answer is the usage data of the one API key the request names, on the day the
        // grant was made: the same daily log a read of that data reports, taken from the
        // accounting this call just wrote, so what the caller is told the period has left is
        // what the next request from that key meets.
        let items = [(
            input.key_id.clone(),
            store.daily_usage(&input.usage_plan_id, &input.key_id, day, day),
        )]
        .into_iter()
        .collect();
        let body = Self::Output {
            usage_plan_id: Some(input.usage_plan_id.clone()),
            start_date: Some(crate::state::usage_date_string(day)),
            end_date: Some(crate::state::usage_date_string(day)),
            items: Some(items),
            // The report covers the one key the request named, so there is no further page of
            // it for a caller to walk to.
            position: None,
        };
        // The lock covers the state change only; everything below is owned.
        drop(store);
        // The access record for a state-changing operation: who, what, and the outcome.
        // Metadata only — never a patch value.
        ::tracing::info!(
            target: crate::state::AUDIT_TARGET,
            operation = Self::NAME,
            caller = ?crate::state::request_caller(ctx.parts()),
            usage_plan = ?input.usage_plan_id,
            api_key = ?input.key_id,
            patch_operations = patches.len(),
            outcome = if patches.is_empty() { "unchanged" } else { "granted" },
            "usage plan quota extension",
        );
        Ok(body)
    }
}

/// The modelled failure for a patch this operation cannot apply, mapped the way the usage
/// plan's own update maps the same refusals.
fn patch_failure(failure: PatchFailure) -> UpdateUsageError {
    match failure {
        PatchFailure::BadRequest(message) => {
            UpdateUsageErrorModeled::BadRequestException(BadRequestException {
                message: Some(message),
            })
        }
        PatchFailure::NotFound(message) => {
            UpdateUsageErrorModeled::NotFoundException(NotFoundException {
                message: Some(message),
            })
        }
        PatchFailure::Conflict(message) => {
            UpdateUsageErrorModeled::ConflictException(ConflictException {
                message: Some(message),
            })
        }
    }
    .into()
}
