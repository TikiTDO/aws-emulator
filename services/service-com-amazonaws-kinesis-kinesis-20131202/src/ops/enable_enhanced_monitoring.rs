// SPDX-License-Identifier: CC0-1.0

//! @op EnableEnhancedMonitoring
//!
//! Turns shard-level metrics ON for the stream the request names, reading and writing the
//! `streams` store this service's [`crate::state`] owns.
//!
//! RULE: a stream carries a SET of enhanced shard-level metrics; an accepted call adds the
//! metrics its request names to that set — the catch-all value standing for every metric the
//! model's enum lists — reports the set as it was before the call beside the set it leaves
//! behind, and persists it, so every later read of that stream reports exactly that set. A
//! value the model's `MetricsName` enum does not accept is refused with a 400
//! `ValidationException` by the framework's constraint walk, before this handler runs, so it
//! leaves the stored set exactly as it was; the call is served only while the stream is
//! `ACTIVE`.
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `EnableEnhancedMonitoring`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json")
//! ```
//!
//! Enables enhanced Kinesis data stream monitoring for shard-level metrics. When invoking
//! this API, you must use either the StreamARN or the StreamName parameter, or both. It is
//! recommended that you use the StreamARN input parameter when you invoke this API.
//!
//! ## Input
//!
//! * `ShardLevelMetrics` (MetricsNameList, len 1..7) **required** — List of shard-level
//!   metrics to enable. The following are the valid shard-level metrics. The value "ALL"
//!   enables every metric. IncomingBytes IncomingRecords OutgoingBytes OutgoingRecords
//!   WriteProvisionedThroughputExceeded ReadProvisionedThroughputExceeded
//!   IteratorAgeMilliseconds ALL For more information, see Monitoring the Amazon Kinesis Data
//!   Streams Service with Amazon CloudWatch in the Amazon Kinesis Data Streams Developer
//!   Guide.
//! * `StreamARN` (StreamARN, len 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$) —
//!   The ARN of the stream.
//! * `StreamId` (StreamId, len 1..24, pattern ^[a-z0-9]{20}-[a-z0-9]{3}$) — Not Implemented.
//!   Reserved for future use.
//! * `StreamName` (StreamName, len 1..128, pattern ^[a-zA-Z0-9_.-]+$) — The name of the
//!   stream for which to enable enhanced monitoring.
//!
//! ## Output
//!
//! * `CurrentShardLevelMetrics` (MetricsNameList, len 1..7) — Represents the current state of
//!   the metrics that are in the enhanced state before the operation.
//! * `DesiredShardLevelMetrics` (MetricsNameList, len 1..7) — Represents the list of all the
//!   metrics that would be in the enhanced state after the operation.
//! * `StreamARN` (StreamARN, len 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$) —
//!   The ARN of the stream.
//! * `StreamName` (StreamName, len 1..128, pattern ^[a-zA-Z0-9_.-]+$) — The name of the
//!   Kinesis data stream.
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
// `EnableEnhancedMonitoringError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-kinesis-kinesis-20131202.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `EnableEnhancedMonitoringError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.kinesis#EnableEnhancedMonitoring",
    // `InternalFailureException` is widened in with `extra_errors`: this SERVICE models it
    // ("The processing of the request failed because of an unknown error, exception, or
    // failure"), the operation shape does not list it, and it is the only honest answer
    // for a service mounted on an engine that holds no state for it.
    extra_errors = ["InternalFailureException"]
)]
#[derive(Debug)]
pub enum EnableEnhancedMonitoringErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InvalidArgumentException(InvalidArgumentException),
    LimitExceededException(LimitExceededException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    InternalFailureException(InternalFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::enable_enhanced_monitoring::EnableEnhancedMonitoringError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type EnableEnhancedMonitoringError = crate::scaffolded::Scaffolded<EnableEnhancedMonitoringErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<EnableEnhancedMonitoringErrorModeled> for EnableEnhancedMonitoringError {
    fn from(modeled: EnableEnhancedMonitoringErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `EnableEnhancedMonitoring` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct EnableEnhancedMonitoring<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

/// Record this state-changing call for attribution: the access key id the caller presented
/// (a public identifier, never a secret), the region the stream lives in, and how the call
/// ended. No stream name and no ARN: a log sink needs neither to attribute the change.
fn audit(akid: &str, region: &str, outcome: &str) {
    ::tracing::info!(
        target: "emulator::audit",
        operation = "EnableEnhancedMonitoring",
        akid = %akid,
        region = %region,
        outcome = %outcome,
        "state-changing operation"
    );
}

/// Refuse the request with the error this operation models for a parameter that "exceeds
/// its restrictions, is not supported, or can't be used".
fn invalid_argument(message: String) -> EnableEnhancedMonitoringError {
    EnableEnhancedMonitoringErrorModeled::InvalidArgumentException(InvalidArgumentException {
        message: Some(message),
    })
    .into()
}

/// The identifier the request addressed the stream by, which is what a refusal names.
/// `StreamId` is documented "Not Implemented. Reserved for future use", so no stream
/// carries one and a request naming only that member names no stream.
fn stream_identifier(input: &EnableEnhancedMonitoringInput) -> &str {
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
) -> EnableEnhancedMonitoringError {
    match failure {
        crate::state::StreamLookupFailure::Unspecified => {
            invalid_argument(crate::state::STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string())
        }
        crate::state::StreamLookupFailure::Mismatch { name, arn } => {
            invalid_argument(crate::state::stream_identifier_mismatch_message(&name, &arn))
        }
        crate::state::StreamLookupFailure::NotFound => {
            EnableEnhancedMonitoringErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::stream_not_found_message(identifier)),
                },
            )
            .into()
        }
    }
}

/// Add the shard-level metrics the request names to the set the stream reports, or answer
/// the modelled refusal that stands in the way. The whole write lives here so the caller
/// holds the store's lock for exactly this long.
fn enable_enhanced_monitoring(
    store: &mut crate::state::ComAmazonawsKinesisKinesis20131202State,
    region: &str,
    now: ::std::time::SystemTime,
    input: &EnableEnhancedMonitoringInput,
) -> Result<EnableEnhancedMonitoringOutput, EnableEnhancedMonitoringError> {
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
    // monitoring anything may change.
    let status = stream.status(now);
    if status != crate::state::STREAM_ACTIVE {
        return Err(
            EnableEnhancedMonitoringErrorModeled::ResourceInUseException(ResourceInUseException {
                message: Some(crate::state::stream_not_active_message(&stream.name, status)),
            })
            .into(),
        );
    }

    // "Represents the current state of the metrics that are in the enhanced state before the
    // operation" beside "the list of all the metrics that would be in the enhanced state
    // after the operation" (`EnhancedMonitoringOutput`): the set the write replaced, and the
    // set it stored, which is what every later read of this stream reports.
    let current = stream.enable_shard_level_metrics(&input.shard_level_metrics);
    Ok(EnableEnhancedMonitoringOutput {
        stream_name: Some(stream.name.clone()),
        stream_arn: Some(stream.arn.clone()),
        current_shard_level_metrics: Some(current),
        desired_shard_level_metrics: Some(stream.shard_level_metrics.clone()),
    })
}

impl<Eng> EnableEnhancedMonitoring<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for EnableEnhancedMonitoring<Eng>
{
    type Input = EnableEnhancedMonitoringInput;
    type Output = EnableEnhancedMonitoringOutput;
    type Error = EnableEnhancedMonitoringError;

    const NAME: &'static str = "EnableEnhancedMonitoring";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ENABLE_ENHANCED_MONITORING_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ENABLE_ENHANCED_MONITORING_OUTPUT_SCHEMA;
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
            EnableEnhancedMonitoringError::from(
                EnableEnhancedMonitoringErrorModeled::InternalFailureException(
                    InternalFailureException {
                        message: Some("the stream store is unavailable".to_string()),
                    },
                ),
            )
        })?;

        let now = ::std::time::SystemTime::now();
        let outcome = {
            let mut store = state.lock();
            enable_enhanced_monitoring(&mut store, &region, now, &input)
        };
        audit(
            &akid,
            &region,
            if outcome.is_ok() { "enabled" } else { "refused" },
        );
        outcome
    }
}
