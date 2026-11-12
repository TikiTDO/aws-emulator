// SPDX-License-Identifier: CC0-1.0

//! @op UpdateMaxRecordSize
//!
//! Sets the maximum size of ONE data record of the stream the request addresses, on the
//! stored stream record, so every later read of that stream reports the new value and
//! every later write to it is measured against it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateMaxRecordSize`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json")
//! ```
//!
//! This allows you to update the MaxRecordSize of a single record that you can write to,
//! and read from a stream. You can ingest and digest single records up to 10240 KiB.
//!
//! ## Input
//!
//! * `MaxRecordSizeInKiB` (MaxRecordSizeInKiB, range 1024..10240) **required** — The maximum
//!   record size of a single record in KiB that you can write to, and read from a stream.
//!   Specify a value between 1024 and 10240 KiB (1 to 10 MiB). If you specify a value that is
//!   out of this range, UpdateMaxRecordSize sends back an ValidationException message.
//! * `StreamARN` (StreamARN, len 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$) —
//!   The Amazon Resource Name (ARN) of the stream for the MaxRecordSize update.
//! * `StreamId` (StreamId, len 1..24, pattern ^[a-z0-9]{20}-[a-z0-9]{3}$) — Not Implemented.
//!   Reserved for future use.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 400) — Specifies that you do not have the permissions
//!   required to perform this operation.
//! * `InvalidArgumentException` (HTTP 400) — A specified parameter exceeds its restrictions,
//!   is not supported, or can't be used. For more information, see the returned message.
//! * `LimitExceededException` (HTTP 400) — The requested resource exceeds the maximum number
//!   allowed, or the number of concurrent stream requests exceeds the maximum number allowed.
//! * `ResourceInUseException` (HTTP 400) — The resource is not available for this operation.
//!   For successful operation, the resource must be in the ACTIVE state.
//! * `ResourceNotFoundException` (HTTP 400) — The requested resource could not be found. The
//!   stream might not be specified correctly.
//! * `ValidationException` (HTTP 400) — Specifies that you tried to invoke this API for a
//!   data stream with the on-demand capacity mode. This API is only supported for data
//!   streams with the provisioned capacity mode.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateMaxRecordSizeError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-kinesis-kinesis-20131202.json");

#[derive(Debug, Clone, Default)]
pub struct UpdateMaxRecordSizeOutput;

impl UpdateMaxRecordSizeOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static UPDATE_MAX_RECORD_SIZE_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &UpdateMaxRecordSizeOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for UpdateMaxRecordSizeOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateMaxRecordSizeError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.kinesis#UpdateMaxRecordSize",
    // `InternalFailureException` is widened in with `extra_errors`, exactly as
    // `CreateStream` and `TagResource` widen it: this SERVICE models it ("The processing of
    // the request failed because of an unknown error, exception, or failure"), the operation
    // shape does not list it, and it is the only honest answer for a service mounted on an
    // engine that holds no state for it.
    extra_errors = ["InternalFailureException"]
)]
#[derive(Debug)]
pub enum UpdateMaxRecordSizeErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InvalidArgumentException(InvalidArgumentException),
    LimitExceededException(LimitExceededException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
    InternalFailureException(InternalFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_max_record_size::UpdateMaxRecordSizeError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateMaxRecordSizeError = crate::scaffolded::Scaffolded<UpdateMaxRecordSizeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateMaxRecordSizeErrorModeled> for UpdateMaxRecordSizeError {
    fn from(modeled: UpdateMaxRecordSizeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateMaxRecordSize` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateMaxRecordSize<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

/// Record this state-changing call for attribution: the access key id the caller presented
/// (a public identifier, never a secret), the region the stream lives in, and how the call
/// ended. No stream ARN: a log sink needs none of it to attribute the change.
fn audit(akid: &str, region: &str, outcome: &str) {
    ::tracing::info!(
        target: "emulator::audit",
        operation = "UpdateMaxRecordSize",
        akid = %akid,
        region = %region,
        outcome = %outcome,
        "state-changing operation"
    );
}

/// Refuse the request with the error this operation models for a parameter that "exceeds
/// its restrictions, is not supported, or can't be used".
fn invalid_argument(message: String) -> UpdateMaxRecordSizeError {
    UpdateMaxRecordSizeErrorModeled::InvalidArgumentException(InvalidArgumentException {
        message: Some(message),
    })
    .into()
}

/// The modelled refusal for a request whose `StreamARN` names no stream of this account and
/// region, worded exactly as every other operation of this service words it: one rule, one
/// phrasing.
fn lookup_refusal(
    failure: crate::state::StreamLookupFailure,
    identifier: &str,
) -> UpdateMaxRecordSizeError {
    match failure {
        // `StreamARN` is this operation's ONLY stream identifier and the model leaves it
        // optional, so a request that omits it names no stream at all. `StreamId` is
        // documented "Not Implemented. Reserved for future use", so it names nothing
        // either. A pair cannot disagree here, since there is no second identifier.
        crate::state::StreamLookupFailure::Unspecified
        | crate::state::StreamLookupFailure::Mismatch { .. } => {
            invalid_argument(crate::state::STREAM_ARN_REQUIRED_MESSAGE.to_string())
        }
        crate::state::StreamLookupFailure::NotFound => {
            UpdateMaxRecordSizeErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(crate::state::stream_not_found_message(identifier)),
            })
            .into()
        }
    }
}

/// Set the addressed stream's maximum record size, or answer the modelled refusal that
/// stands in the way. The whole write lives here so the caller holds the store's lock for
/// exactly this long.
fn apply_update(
    store: &mut crate::state::ComAmazonawsKinesisKinesis20131202State,
    region: &str,
    now: ::std::time::SystemTime,
    input: &UpdateMaxRecordSizeInput,
) -> Result<(), UpdateMaxRecordSizeError> {
    let identifier = input.stream_arn.clone().unwrap_or_default();
    let stream = store
        .resolve_stream_mut(now, region, None, input.stream_arn.as_deref())
        .map_err(|failure| lookup_refusal(failure, &identifier))?;

    // "The resource is not available for this operation. For successful operation, the
    // resource must be in the ACTIVE state" (`ResourceInUseException`): a stream whose
    // creation has not fallen due yet, or whose removal has been accepted, is not available
    // for a configuration change.
    let status = stream.status(now);
    if status != crate::state::STREAM_ACTIVE {
        return Err(UpdateMaxRecordSizeErrorModeled::ResourceInUseException(
            ResourceInUseException {
                message: Some(crate::state::stream_not_active_message(&stream.name, status)),
            },
        )
        .into());
    }

    // The value the request names becomes the stream's own maximum record size, replacing
    // whatever the create named or the documented default the stream fell back to. The
    // modelled `range 1024..10240` is refused BEFORE this handler runs — "If you specify a
    // value that is out of this range, UpdateMaxRecordSize sends back an ValidationException
    // message" (`UpdateMaxRecordSizeInput$MaxRecordSizeInKiB`) — so a value that reaches
    // here is one a stream may carry.
    stream.max_record_size_in_ki_b = input.max_record_size_in_ki_b;
    Ok(())
}

impl<Eng> UpdateMaxRecordSize<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateMaxRecordSize<Eng>
{
    type Input = UpdateMaxRecordSizeInput;
    type Output = UpdateMaxRecordSizeOutput;
    type Error = UpdateMaxRecordSizeError;

    const NAME: &'static str = "UpdateMaxRecordSize";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_MAX_RECORD_SIZE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = UPDATE_MAX_RECORD_SIZE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The credential scope's access key id is read only to ATTRIBUTE the change
        // ([`audit`]); the region it names is the region whose streams this request
        // addresses, since a stream "is also scoped by Amazon Web Services Region".
        let akid = ::emulator_server::extract_akid(&ctx.parts().headers);
        let region = crate::state::request_region(ctx);

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            audit(&akid, &region, "unavailable");
            UpdateMaxRecordSizeError::from(
                UpdateMaxRecordSizeErrorModeled::InternalFailureException(
                    InternalFailureException {
                        message: Some("the stream store is unavailable".to_string()),
                    },
                ),
            )
        })?;

        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        let outcome = apply_update(&mut store, &region, now, &input);
        drop(store);

        audit(
            &akid,
            &region,
            if outcome.is_ok() { "updated" } else { "refused" },
        );
        // `UpdateMaxRecordSize` models no output members, so a served request answers the
        // modelled empty body.
        outcome.map(|()| UpdateMaxRecordSizeOutput)
    }
}
