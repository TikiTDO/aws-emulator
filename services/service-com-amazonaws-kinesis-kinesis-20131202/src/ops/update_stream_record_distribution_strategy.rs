// SPDX-License-Identifier: CC0-1.0

//! @op UpdateStreamRecordDistributionStrategy
//!
//! Stores the record distribution strategy the request names on the stream its `StreamARN`
//! addresses, so every later read of that stream reports the new strategy and every later
//! write to it is placed by it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateStreamRecordDistributionStrategy`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json")
//! ```
//!
//! Updates the record distribution strategy for the specified Amazon Kinesis Data Streams
//! on-demand data stream. The record distribution strategy determines how Amazon Kinesis
//! Data Streams distributes records across the shards in a stream. You must specify the
//! stream using the StreamARN parameter. The record distribution strategy is a stream-level
//! setting. You can switch between the following strategies at any time, and the change
//! takes effect immediately without downtime, data loss, or disruption to producer or
//! consumer applications: AUTO – Amazon Kinesis Data Streams distributes records evenly
//! across shards using service-managed algorithms, and ignores any partition key and
//! ExplicitHashKey that a producer provides. Use this strategy for stateless workloads that
//! do not require partition-key ordering. USER_PARTITION_KEY – Producers must provide a
//! partition key, and Amazon Kinesis Data Streams uses the partition key to determine shard
//! placement. Records that share a partition key are sent to the same shard. This is the
//! default strategy. This operation is only supported for data streams that use the
//! on-demand capacity mode. Provisioned capacity mode streams do not support the record
//! distribution strategy setting. Attempting to set AUTO on a provisioned stream results in
//! an InvalidArgumentException. New records that arrive after the change are distributed
//! according to the new strategy. Records already in the stream keep their original shard
//! assignments and are not redistributed.
//!
//! ## Input
//!
//! * `RecordDistributionStrategy` (RecordDistributionStrategy, one of: AUTO |
//!   USER_PARTITION_KEY) **required** — The record distribution strategy to apply to the
//!   stream. Specify one of the following values: AUTO – Amazon Kinesis Data Streams
//!   distributes records evenly across shards and ignores any partition key and
//!   ExplicitHashKey that producers supply. USER_PARTITION_KEY – Producers must supply a
//!   partition key, which Amazon Kinesis Data Streams uses to determine shard placement. This
//!   is the default.
//! * `StreamARN` (StreamARN, len 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$)
//!   **required** — The Amazon Resource Name (ARN) of the stream to update.
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
// `UpdateStreamRecordDistributionStrategyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-kinesis-kinesis-20131202.json");

#[derive(Debug, Clone, Default)]
pub struct UpdateStreamRecordDistributionStrategyOutput;

impl UpdateStreamRecordDistributionStrategyOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static UPDATE_STREAM_RECORD_DISTRIBUTION_STRATEGY_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &UpdateStreamRecordDistributionStrategyOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for UpdateStreamRecordDistributionStrategyOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateStreamRecordDistributionStrategyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.kinesis#UpdateStreamRecordDistributionStrategy",
    // `InternalFailureException` is widened in with `extra_errors`, exactly as
    // `CreateStream` and `UpdateMaxRecordSize` widen it: this SERVICE models it ("The
    // processing of the request failed because of an unknown error, exception, or failure"),
    // the operation shape does not list it, and it is the only honest answer for a service
    // mounted on an engine that holds no state for it.
    extra_errors = ["InternalFailureException"]
)]
#[derive(Debug)]
pub enum UpdateStreamRecordDistributionStrategyErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InvalidArgumentException(InvalidArgumentException),
    LimitExceededException(LimitExceededException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
    InternalFailureException(InternalFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_stream_record_distribution_strategy::UpdateStreamRecordDistributionStrategyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateStreamRecordDistributionStrategyError = crate::scaffolded::Scaffolded<UpdateStreamRecordDistributionStrategyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateStreamRecordDistributionStrategyErrorModeled> for UpdateStreamRecordDistributionStrategyError {
    fn from(modeled: UpdateStreamRecordDistributionStrategyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateStreamRecordDistributionStrategy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateStreamRecordDistributionStrategy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

/// Record this state-changing call for attribution: the access key id the caller presented
/// (a public identifier, never a secret), the region the stream lives in, and how the call
/// ended. No stream ARN: a log sink needs none of it to attribute the change.
fn audit(akid: &str, region: &str, outcome: &str) {
    ::tracing::info!(
        target: "emulator::audit",
        operation = "UpdateStreamRecordDistributionStrategy",
        akid = %akid,
        region = %region,
        outcome = %outcome,
        "state-changing operation"
    );
}

/// Refuse the request with the error this operation models for a parameter that "exceeds
/// its restrictions, is not supported, or can't be used".
fn invalid_argument(message: String) -> UpdateStreamRecordDistributionStrategyError {
    UpdateStreamRecordDistributionStrategyErrorModeled::InvalidArgumentException(
        InvalidArgumentException {
            message: Some(message),
        },
    )
    .into()
}

/// The modelled refusal for a request whose `StreamARN` names no stream of this account and
/// region, worded exactly as every other operation of this service words it: one rule, one
/// phrasing.
fn lookup_refusal(
    failure: crate::state::StreamLookupFailure,
    identifier: &str,
) -> UpdateStreamRecordDistributionStrategyError {
    match failure {
        // `StreamARN` is this operation's ONLY stream identifier, and `StreamId` is
        // documented "Not Implemented. Reserved for future use", so it names nothing either.
        // A pair cannot disagree here, since there is no second identifier.
        crate::state::StreamLookupFailure::Unspecified
        | crate::state::StreamLookupFailure::Mismatch { .. } => {
            invalid_argument(crate::state::STREAM_ARN_REQUIRED_MESSAGE.to_string())
        }
        crate::state::StreamLookupFailure::NotFound => {
            UpdateStreamRecordDistributionStrategyErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::stream_not_found_message(identifier)),
                },
            )
            .into()
        }
    }
}

/// Store the strategy the request names on the addressed stream, or answer the modelled
/// refusal that stands in the way. The whole write lives here so the caller holds the
/// store's lock for exactly this long.
fn apply_update(
    store: &mut crate::state::ComAmazonawsKinesisKinesis20131202State,
    region: &str,
    now: ::std::time::SystemTime,
    input: &UpdateStreamRecordDistributionStrategyInput,
) -> Result<(), UpdateStreamRecordDistributionStrategyError> {
    let identifier = input.stream_arn.clone();
    let stream = store
        .resolve_stream_mut(now, region, None, Some(input.stream_arn.as_str()))
        .map_err(|failure| lookup_refusal(failure, &identifier))?;

    // "This operation is only supported for data streams that use the on-demand capacity
    // mode. Provisioned capacity mode streams do not support the record distribution
    // strategy setting": a provisioned stream places its records by the producer's partition
    // key and carries no such setting at all, so the request is refused rather than served
    // and the stream keeps reporting no strategy. This stands ahead of the lifecycle check
    // below: a stream whose mode excludes the setting is refused whatever state it is in.
    if stream.stream_mode != crate::state::STREAM_MODE_ON_DEMAND {
        return Err(
            UpdateStreamRecordDistributionStrategyErrorModeled::ValidationException(
                ValidationException {
                    message: Some(crate::state::stream_provisioned_message(&stream.name)),
                },
            )
            .into(),
        );
    }

    // "The resource is not available for this operation. For successful operation, the
    // resource must be in the ACTIVE state" (`ResourceInUseException`): a stream whose
    // creation has not fallen due yet, whose removal has been accepted, or whose previous
    // change is still in flight is not available for a configuration change.
    let status = stream.status(now);
    if status != crate::state::STREAM_ACTIVE {
        return Err(
            UpdateStreamRecordDistributionStrategyErrorModeled::ResourceInUseException(
                ResourceInUseException {
                    message: Some(crate::state::stream_not_active_message(&stream.name, status)),
                },
            )
            .into(),
        );
    }

    // "You can switch between the following strategies at any time, and the change takes
    // effect immediately without downtime, data loss, or disruption to producer or consumer
    // applications": the strategy the request names replaces whatever the stream carried —
    // the value its create named, or the default it fell back to — and nothing is owed
    // afterwards, so the stream stays `ACTIVE` rather than going `UPDATING`. The modelled
    // enum of `RecordDistributionStrategy` is refused BEFORE this handler runs, so a value
    // reaching here is one a stream may carry.
    //
    // "New records that arrive after the change are distributed according to the new
    // strategy. Records already in the stream keep their original shard assignments and are
    // not redistributed": the stored shard map and its records are left exactly as they are,
    // so only the placement of a LATER write changes.
    stream.record_distribution_strategy = Some(input.record_distribution_strategy.clone());
    Ok(())
}

impl<Eng> UpdateStreamRecordDistributionStrategy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateStreamRecordDistributionStrategy<Eng>
{
    type Input = UpdateStreamRecordDistributionStrategyInput;
    type Output = UpdateStreamRecordDistributionStrategyOutput;
    type Error = UpdateStreamRecordDistributionStrategyError;

    const NAME: &'static str = "UpdateStreamRecordDistributionStrategy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_STREAM_RECORD_DISTRIBUTION_STRATEGY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = UPDATE_STREAM_RECORD_DISTRIBUTION_STRATEGY_OUTPUT_SCHEMA;
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
            UpdateStreamRecordDistributionStrategyError::from(
                UpdateStreamRecordDistributionStrategyErrorModeled::InternalFailureException(
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
        // `UpdateStreamRecordDistributionStrategy` models a `Unit` output, so a served
        // request answers 200 with the modelled empty body.
        outcome.map(|()| UpdateStreamRecordDistributionStrategyOutput)
    }
}
