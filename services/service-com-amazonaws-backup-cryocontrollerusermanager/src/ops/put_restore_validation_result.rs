// SPDX-License-Identifier: CC0-1.0

//! @op PutRestoreValidationResult
//!
//! Records the validation result a client ran for one restore on that restore's
//! stored job ([`crate::state::RestoreJobRecord::validation_status`]), so
//! `DescribeRestoreJob` reports the `ValidationStatus` and
//! `ValidationStatusMessage` the caller sent rather than any value this service
//! decided.
//!
//! An id naming no stored restore job is the one not-found refusal every call
//! that addresses a restore job shares
//! ([`crate::state::restore_job_not_found`]), and a refused call records
//! nothing. A later result REPLACES the one recorded before it: a restore has
//! one validation result, which is the latest one reported for it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `PutRestoreValidationResult`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! This request allows you to send your independent self-run restore test validation
//! results. RestoreJobId and ValidationStatus are required. Optionally, you can input a
//! ValidationStatusMessage.
//!
//! ## Input
//!
//! * `RestoreJobId` (RestoreJobId) **required** — This is a unique identifier of a restore
//!   job within Backup.
//! * `ValidationStatus` (RestoreValidationStatus, one of: FAILED | SUCCESSFUL | TIMED_OUT |
//!   VALIDATING) **required** — The status of your restore validation.
//! * `ValidationStatusMessage` (string) — This is an optional message string you can input to
//!   describe the validation status for the restore test validation.
//!
//! ## Errors
//!
//! * `InvalidParameterValueException` (HTTP 400) — Indicates that something is wrong with a
//!   parameter's value. For example, the value is out of range.
//! * `InvalidRequestException` (HTTP 400) — Indicates that something is wrong with the input
//!   to the request. For example, a parameter is of the wrong type.
//! * `MissingParameterValueException` (HTTP 400) — Indicates that a required parameter is
//!   missing.
//! * `ResourceNotFoundException` (HTTP 400) — A resource that is required for the action
//!   doesn't exist.
//! * `ServiceUnavailableException` (HTTP 500) — The request failed due to a temporary failure
//!   of the server.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `PutRestoreValidationResultError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

#[derive(Debug, Clone, Default)]
pub struct PutRestoreValidationResultOutput;

impl PutRestoreValidationResultOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "PUT",
        "/restore-jobs/{RestoreJobId}/validations",
        Some(204u16),
    ));
}

pub static PUT_RESTORE_VALIDATION_RESULT_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &PutRestoreValidationResultOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for PutRestoreValidationResultOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutRestoreValidationResultError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.backup#PutRestoreValidationResult")]
#[derive(Debug)]
pub enum PutRestoreValidationResultErrorModeled {
    InvalidParameterValueException(InvalidParameterValueException),
    InvalidRequestException(InvalidRequestException),
    MissingParameterValueException(MissingParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_restore_validation_result::PutRestoreValidationResultError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutRestoreValidationResultError = crate::scaffolded::Scaffolded<PutRestoreValidationResultErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutRestoreValidationResultErrorModeled> for PutRestoreValidationResultError {
    fn from(modeled: PutRestoreValidationResultErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutRestoreValidationResult` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutRestoreValidationResult<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutRestoreValidationResult<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutRestoreValidationResult<Eng>
{
    type Input = PutRestoreValidationResultInput;
    type Output = PutRestoreValidationResultOutput;
    type Error = PutRestoreValidationResultError;

    const NAME: &'static str = "PutRestoreValidationResult";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_RESTORE_VALIDATION_RESULT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = PUT_RESTORE_VALIDATION_RESULT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 204u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let now = ::std::time::SystemTime::now();
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            PutRestoreValidationResultErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException {
                    message: Some("the backup vault store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;
        // This store's lock lives for exactly this block: the publication below
        // awaits, and a guard must not be held across an await.
        {
            let mut store = state.lock();
            // The write is on the job, so the transition the clock owes it is
            // materialised first and the job this call writes is the job as it
            // stands now.
            store.settle_restore_job(&input.restore_job_id, now);
            // A transition this call materialised owes the bus its event just as one
            // the clock pump materialised does (`crate::state::events`).
            store.record_job_state_changes();

            // `ValidationStatus` is `@required` and targets the model's own enum, so
            // an absent or unknown value is refused before this handler runs: what
            // is left to resolve is the job it is reported for.
            let job = store
                .stored_restore_job_mut(&input.restore_job_id)
                .map_err(PutRestoreValidationResultErrorModeled::ResourceNotFoundException)?;
            job.validation_status = Some(input.validation_status);
            job.validation_status_message = input.validation_status_message;
        }
        crate::state::publish_job_state_changes(ctx, &*self.engine).await;
        Ok(PutRestoreValidationResultOutput)
    }
}
