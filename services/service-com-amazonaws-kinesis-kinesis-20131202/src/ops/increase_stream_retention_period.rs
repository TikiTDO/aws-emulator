// SPDX-License-Identifier: CC0-1.0

//! @op IncreaseStreamRetentionPeriod
//!
//! Lengthens the retention window of the stream the request names, reading and writing the
//! `streams` store this service's [`crate::state`] owns.
//!
//! RULE: an accepted call stores the requested `RetentionPeriodHours` on the stream, so
//! every later read of that stream reports it and the stream's trim horizon is measured
//! against it. A window outside the documented 24..8760 hours, and one that is not longer
//! than the window the stream already carries, are refused with the modelled client error
//! and leave the stored window exactly as it was; the call is served only while the stream
//! is `ACTIVE`.
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `IncreaseStreamRetentionPeriod`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json")
//! ```
//!
//! Increases the Kinesis data stream's retention period, which is the length of time data
//! records are accessible after they are added to the stream. The maximum value of a
//! stream's retention period is 8760 hours (365 days). When invoking this API, you must use
//! either the StreamARN or the StreamName parameter, or both. It is recommended that you
//! use the StreamARN input parameter when you invoke this API. If you choose a longer
//! stream retention period, this operation increases the time period during which records
//! that have not yet expired are accessible. However, it does not make previous, expired
//! data (older than the stream's previous retention period) accessible after the operation
//! has been called. For example, if a stream's retention period is set to 24 hours and is
//! increased to 168 hours, any data that is older than 24 hours remains inaccessible to
//! consumer applications.
//!
//! ## Input
//!
//! * `RetentionPeriodHours` (RetentionPeriodHours) **required** — The new retention period of
//!   the stream, in hours. Must be more than the current retention period.
//! * `StreamARN` (StreamARN, len 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$) —
//!   The ARN of the stream.
//! * `StreamId` (StreamId, len 1..24, pattern ^[a-z0-9]{20}-[a-z0-9]{3}$) — Not Implemented.
//!   Reserved for future use.
//! * `StreamName` (StreamName, len 1..128, pattern ^[a-zA-Z0-9_.-]+$) — The name of the
//!   stream to modify.
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

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `IncreaseStreamRetentionPeriodError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-kinesis-kinesis-20131202.json");

#[derive(Debug, Clone, Default)]
pub struct IncreaseStreamRetentionPeriodOutput;

impl IncreaseStreamRetentionPeriodOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static INCREASE_STREAM_RETENTION_PERIOD_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &IncreaseStreamRetentionPeriodOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for IncreaseStreamRetentionPeriodOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `IncreaseStreamRetentionPeriodError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.kinesis#IncreaseStreamRetentionPeriod",
    // `InternalFailureException` is widened in with `extra_errors`: this SERVICE models it
    // ("The processing of the request failed because of an unknown error, exception, or
    // failure"), the operation shape does not list it, and it is the only honest answer
    // for a service mounted on an engine that holds no state for it.
    extra_errors = ["InternalFailureException"]
)]
#[derive(Debug)]
pub enum IncreaseStreamRetentionPeriodErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InvalidArgumentException(InvalidArgumentException),
    LimitExceededException(LimitExceededException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    InternalFailureException(InternalFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::increase_stream_retention_period::IncreaseStreamRetentionPeriodError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type IncreaseStreamRetentionPeriodError = crate::scaffolded::Scaffolded<IncreaseStreamRetentionPeriodErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<IncreaseStreamRetentionPeriodErrorModeled> for IncreaseStreamRetentionPeriodError {
    fn from(modeled: IncreaseStreamRetentionPeriodErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `IncreaseStreamRetentionPeriod` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct IncreaseStreamRetentionPeriod<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

/// Record this state-changing call for attribution: the access key id the caller presented
/// (a public identifier, never a secret), the region the stream lives in, and how the call
/// ended. No stream name and no ARN: a log sink needs neither to attribute the change.
fn audit(akid: &str, region: &str, outcome: &str) {
    ::tracing::info!(
        target: "emulator::audit",
        operation = "IncreaseStreamRetentionPeriod",
        akid = %akid,
        region = %region,
        outcome = %outcome,
        "state-changing operation"
    );
}

/// Refuse the request with the error this operation models for a parameter that "exceeds
/// its restrictions, is not supported, or can't be used".
fn invalid_argument(message: String) -> IncreaseStreamRetentionPeriodError {
    IncreaseStreamRetentionPeriodErrorModeled::InvalidArgumentException(InvalidArgumentException {
        message: Some(message),
    })
    .into()
}

/// The identifier the request addressed the stream by, which is what a refusal names.
/// `StreamId` is documented "Not Implemented. Reserved for future use", so no stream
/// carries one and a request naming only that member names no stream.
fn stream_identifier(input: &IncreaseStreamRetentionPeriodInput) -> &str {
    input
        .stream_name
        .as_deref()
        .or(input.stream_arn.as_deref())
        .unwrap_or_default()
}

/// The modelled refusal for a request whose `StreamName` / `StreamARN` pair names no stream
/// of this account and region, worded exactly as the reads word it: one rule, one phrasing,
/// for every operation that addresses a stream.
fn lookup_refusal(
    failure: crate::state::StreamLookupFailure,
    identifier: &str,
) -> IncreaseStreamRetentionPeriodError {
    match failure {
        crate::state::StreamLookupFailure::Unspecified => {
            invalid_argument(crate::state::STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string())
        }
        crate::state::StreamLookupFailure::Mismatch { name, arn } => {
            invalid_argument(crate::state::stream_identifier_mismatch_message(&name, &arn))
        }
        crate::state::StreamLookupFailure::NotFound => {
            IncreaseStreamRetentionPeriodErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::stream_not_found_message(identifier)),
                },
            )
            .into()
        }
    }
}

/// Lengthen the retention window of the stream the request names, or answer the modelled
/// refusal that stands in the way. The whole write lives here so the caller holds the
/// store's lock for exactly this long.
fn increase_retention_period(
    store: &mut crate::state::ComAmazonawsKinesisKinesis20131202State,
    region: &str,
    now: ::std::time::SystemTime,
    input: &IncreaseStreamRetentionPeriodInput,
) -> Result<(), IncreaseStreamRetentionPeriodError> {
    // "The maximum value of a stream's retention period is 8760 hours (365 days)" and the
    // shortest window a stream carries is the 24 hours it starts with, so a request outside
    // that window is refused BEFORE anything is stored — `RetentionPeriodHours` carries no
    // modelled range, so this is the only thing that bounds it.
    let requested = input.retention_period_hours;
    if requested > crate::state::MAX_RETENTION_PERIOD_HOURS {
        return Err(invalid_argument(
            crate::state::retention_period_too_long_message(requested),
        ));
    }
    if requested < crate::state::MIN_RETENTION_PERIOD_HOURS {
        return Err(invalid_argument(
            crate::state::retention_period_too_short_message(requested),
        ));
    }

    let identifier = stream_identifier(input).to_string();
    let stream = store
        .resolve_stream_mut(
            now,
            region,
            input.stream_name.as_deref(),
            input.stream_arn.as_deref(),
        )
        .map_err(|failure| lookup_refusal(failure, &identifier))?;

    // "The resource is not available for this operation. For successful operation, the
    // resource must be in the ACTIVE state" (`ResourceInUseException`): a stream whose
    // create, whose removal or whose previous update is still due is not one whose
    // retention window anything may change.
    let status = stream.status(now);
    if status != crate::state::STREAM_ACTIVE {
        return Err(
            IncreaseStreamRetentionPeriodErrorModeled::ResourceInUseException(
                ResourceInUseException {
                    message: Some(crate::state::stream_not_active_message(&stream.name, status)),
                },
            )
            .into(),
        );
    }

    // This operation INCREASES the window: its member "must be more than the current
    // retention period", so a window that does not lengthen it is refused and the stored
    // one is left as it was — shortening it is the other operation's to serve.
    let current = stream.retention_period_hours;
    if requested <= current {
        // A refusal about the stream this request RESOLVED names it by its `StreamName`,
        // which is the variable part the service's messages carry: whichever identifier
        // addressed the stream, the STORED name is what the message reports.
        return Err(invalid_argument(
            crate::state::retention_period_not_longer_message(&stream.name, requested, current),
        ));
    }

    // "If you choose a longer stream retention period, this operation increases the time
    // period during which records that have not yet expired are accessible": the window is
    // what the stream's trim horizon is measured against, so storing it is the whole change.
    stream.retention_period_hours = requested;
    Ok(())
}

impl<Eng> IncreaseStreamRetentionPeriod<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for IncreaseStreamRetentionPeriod<Eng>
{
    type Input = IncreaseStreamRetentionPeriodInput;
    type Output = IncreaseStreamRetentionPeriodOutput;
    type Error = IncreaseStreamRetentionPeriodError;

    const NAME: &'static str = "IncreaseStreamRetentionPeriod";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::INCREASE_STREAM_RETENTION_PERIOD_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = INCREASE_STREAM_RETENTION_PERIOD_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The credential scope's access key id is read only to ATTRIBUTE the change
        // ([`audit`]); the region it names is the region whose streams this request
        // addresses, since a stream name "is also scoped by Amazon Web Services Region".
        let akid = ::emulator_server::extract_akid(&ctx.parts().headers);
        let region = crate::state::request_region(ctx);

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            audit(&akid, &region, "unavailable");
            IncreaseStreamRetentionPeriodError::from(
                IncreaseStreamRetentionPeriodErrorModeled::InternalFailureException(
                    InternalFailureException {
                        message: Some("the stream store is unavailable".to_string()),
                    },
                ),
            )
        })?;

        let now = ::std::time::SystemTime::now();
        let outcome = {
            let mut store = state.lock();
            increase_retention_period(&mut store, &region, now, &input)
        };
        audit(
            &akid,
            &region,
            if outcome.is_ok() { "increased" } else { "refused" },
        );
        outcome?;

        // The operation models no output members: the window it stored is what
        // `DescribeStreamSummary` and `DescribeStream` report for the stream.
        Ok(IncreaseStreamRetentionPeriodOutput)
    }
}
