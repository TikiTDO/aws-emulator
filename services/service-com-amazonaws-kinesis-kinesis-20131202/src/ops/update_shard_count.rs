// SPDX-License-Identifier: CC0-1.0

//! @op UpdateShardCount
//! Changes the number of shards of a PROVISIONED stream by re-tiling its hash key space,
//! reading and writing the `streams` store this service's [`crate::state`] owns.
//!
//! RULE: an accepted call closes every open shard of the stream and covers the whole hash
//! key space again with `TargetShardCount` contiguous equal slices — "uniform scaling
//! creates shards of equal size" (`ScalingType`) — each new shard naming the shards it was
//! produced from, leaves the stream `UPDATING` until the transition back to `ACTIVE` falls
//! due, and answers the shard count the stream had beside the one it is being taken to.
//! The call is only served for a stream in the provisioned capacity mode, only while that
//! stream is `ACTIVE`, and only inside the documented scaling limits.
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateShardCount`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json")
//! ```
//!
//! Updates the shard count of the specified stream to the specified number of shards. This
//! API is only supported for the data streams with the provisioned capacity mode. When
//! invoking this API, you must use either the StreamARN or the StreamName parameter, or
//! both. It is recommended that you use the StreamARN input parameter when you invoke this
//! API. Updating the shard count is an asynchronous operation. Upon receiving the request,
//! Kinesis Data Streams returns immediately and sets the status of the stream to UPDATING.
//! After the update is complete, Kinesis Data Streams sets the status of the stream back to
//! ACTIVE. Depending on the size of the stream, the scaling action could take a few minutes
//! to complete. You can continue to read and write data to your stream while its status is
//! UPDATING. To update the shard count, Kinesis Data Streams performs splits or merges on
//! individual shards. This can cause short-lived shards to be created, in addition to the
//! final shards. These short-lived shards count towards your total shard limit for your
//! account in the Region. When using this operation, we recommend that you specify a target
//! shard count that is a multiple of 25% (25%, 50%, 75%, 100%). You can specify any target
//! value within your shard limit. However, if you specify a target that isn't a multiple of
//! 25%, the scaling action might take longer to complete. This operation has the following
//! default limits. By default, you cannot do the following: Scale more than ten times per
//! rolling 24-hour period per stream Scale up to more than double your current shard count
//! for a stream Scale down below half your current shard count for a stream Scale up to
//! more than 10000 shards in a stream Scale a stream with more than 10000 shards down
//! unless the result is less than 10000 shards Scale up to more than the shard limit for
//! your account Make over 10 TPS. TPS over 10 will trigger the LimitExceededException For
//! the default limits for an Amazon Web Services account, see Streams Limits in the Amazon
//! Kinesis Data Streams Developer Guide. To request an increase in the call rate limit, the
//! shard limit for this API, or your overall shard limit, use the limits form.
//!
//! ## Input
//!
//! * `ScalingType` (ScalingType, one of: UNIFORM_SCALING) **required** — The scaling type.
//!   Uniform scaling creates shards of equal size.
//! * `StreamARN` (StreamARN, len 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$) —
//!   The ARN of the stream.
//! * `StreamId` (StreamId, len 1..24, pattern ^[a-z0-9]{20}-[a-z0-9]{3}$) — Not Implemented.
//!   Reserved for future use.
//! * `StreamName` (StreamName, len 1..128, pattern ^[a-zA-Z0-9_.-]+$) — The name of the
//!   stream.
//! * `TargetShardCount` (PositiveIntegerObject, range 1..) **required** — The new number of
//!   shards. This value has the following default limits. By default, you cannot do the
//!   following: Set this value to more than double your current shard count for a stream. Set
//!   this value below half your current shard count for a stream. Set this value to more than
//!   10000 shards in a stream (the default limit for shard count per stream is 10000 per
//!   account per region), unless you request a limit increase. Scale a stream with more than
//!   10000 shards down unless you set this value to less than 10000 shards.
//!
//! ## Output
//!
//! * `CurrentShardCount` (PositiveIntegerObject, range 1..) — The current number of shards.
//! * `StreamARN` (StreamARN, len 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$) —
//!   The ARN of the stream.
//! * `StreamName` (StreamName, len 1..128, pattern ^[a-zA-Z0-9_.-]+$) — The name of the
//!   stream.
//! * `TargetShardCount` (PositiveIntegerObject, range 1..) — The updated number of shards.
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
// `UpdateShardCountError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-kinesis-kinesis-20131202.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateShardCountError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.kinesis#UpdateShardCount",
    // `InternalFailureException` is widened in with `extra_errors`: this SERVICE models it
    // ("The processing of the request failed because of an unknown error, exception, or
    // failure"), the operation shape does not list it, and it is the only honest answer
    // for a service mounted on an engine that holds no state for it.
    extra_errors = ["InternalFailureException"]
)]
#[derive(Debug)]
pub enum UpdateShardCountErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InvalidArgumentException(InvalidArgumentException),
    LimitExceededException(LimitExceededException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
    InternalFailureException(InternalFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_shard_count::UpdateShardCountError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateShardCountError = crate::scaffolded::Scaffolded<UpdateShardCountErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateShardCountErrorModeled> for UpdateShardCountError {
    fn from(modeled: UpdateShardCountErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateShardCount` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateShardCount<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

/// Record this state-changing call for attribution: the access key id the caller presented
/// (a public identifier, never a secret), the region the stream lives in, and how the call
/// ended. No stream name and no ARN: a log sink needs neither to attribute the change.
fn audit(akid: &str, region: &str, outcome: &str) {
    ::tracing::info!(
        target: "emulator::audit",
        operation = "UpdateShardCount",
        akid = %akid,
        region = %region,
        outcome = %outcome,
        "state-changing operation"
    );
}

/// How many times the stream's current shard count a scaling action may take it to: "you
/// cannot do the following: ... scale up to more than double your current shard count for
/// a stream[,] scale down below half your current shard count for a stream"
/// (`UpdateShardCount`), so the target lies between half and double the current count.
const SCALING_FACTOR: u32 = 2;

/// Refuse the request with the error this operation models for a parameter that "exceeds
/// its restrictions, is not supported, or can't be used".
fn invalid_argument(message: String) -> UpdateShardCountError {
    UpdateShardCountErrorModeled::InvalidArgumentException(InvalidArgumentException {
        message: Some(message),
    })
    .into()
}

/// Refuse the request because serving it would take the stream past one of this
/// operation's documented scaling limits, or this emulator past a ceiling it keeps on
/// itself: "the requested resource exceeds the maximum number allowed"
/// (`LimitExceededException`).
fn limit_exceeded(message: String) -> UpdateShardCountError {
    UpdateShardCountErrorModeled::LimitExceededException(LimitExceededException {
        message: Some(message),
    })
    .into()
}

/// The modelled refusal for a request whose `StreamName` / `StreamARN` pair names no stream
/// of this account and region, worded exactly as the reads word it: one rule, one phrasing,
/// for every operation that addresses a stream.
fn lookup_refusal(
    failure: crate::state::StreamLookupFailure,
    identifier: &str,
) -> UpdateShardCountError {
    match failure {
        crate::state::StreamLookupFailure::Unspecified => {
            invalid_argument(crate::state::STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string())
        }
        crate::state::StreamLookupFailure::Mismatch { name, arn } => {
            invalid_argument(crate::state::stream_identifier_mismatch_message(&name, &arn))
        }
        crate::state::StreamLookupFailure::NotFound => {
            UpdateShardCountErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(crate::state::stream_not_found_message(identifier)),
            })
            .into()
        }
    }
}

/// Change the shard count of the stream the request names, or answer the modelled refusal
/// that stands in the way. The whole write lives here so the caller holds the store's lock
/// for exactly this long.
fn update_shard_count(
    store: &mut crate::state::ComAmazonawsKinesisKinesis20131202State,
    region: &str,
    now: ::std::time::SystemTime,
    input: &UpdateShardCountInput,
) -> Result<UpdateShardCountOutput, UpdateShardCountError> {
    // `ScalingType` is a modelled enum, so a value outside `SCALING_TYPE_VALUES` is a
    // client error rather than a scaling action. The model's own constraint walk refuses
    // one before this handler runs; this line states the same refusal for a caller that
    // reaches the handler another way.
    if !SCALING_TYPE_VALUES.contains(&input.scaling_type.as_str()) {
        return Err(invalid_argument(format!(
            "ScalingType {} is not supported; the supported values are {}",
            input.scaling_type,
            SCALING_TYPE_VALUES.join(" | ")
        )));
    }

    // The account and region's shard quota is counted across the whole store, so it is read
    // BEFORE the stream is borrowed for writing.
    let (shard_limit, open_shard_count) = store.shard_quota_in(now, region);

    // `StreamId` is documented "Not Implemented. Reserved for future use", so no stream
    // carries one and a request naming only that member names no stream.
    let identifier = input
        .stream_name
        .as_deref()
        .or(input.stream_arn.as_deref())
        .unwrap_or_default()
        .to_string();
    let stream = store
        .resolve_stream_mut(
            now,
            region,
            input.stream_name.as_deref(),
            input.stream_arn.as_deref(),
        )
        .map_err(|failure| lookup_refusal(failure, &identifier))?;

    // "This API is only supported for the data streams with the provisioned capacity
    // mode": an on-demand stream's shard count is the service's to manage, so the request
    // is refused rather than served, and the stored shard map is left exactly as it was.
    if stream.stream_mode != crate::state::STREAM_MODE_PROVISIONED {
        return Err(UpdateShardCountErrorModeled::ValidationException(
            ValidationException {
                message: Some(crate::state::stream_on_demand_message(&stream.name)),
            },
        )
        .into());
    }

    // "The resource is not available for this operation. For successful operation, the
    // resource must be in the ACTIVE state" (`ResourceInUseException`): a stream whose
    // create, whose removal or whose PREVIOUS scaling action is still due has a shard map
    // nothing else may re-tile.
    let status = stream.status(now);
    if status != crate::state::STREAM_ACTIVE {
        return Err(UpdateShardCountErrorModeled::ResourceInUseException(
            ResourceInUseException {
                message: Some(crate::state::stream_not_active_message(
                    &stream.name,
                    status,
                )),
            },
        )
        .into());
    }

    // Modelled `range 1..`, so the boundary validator has already refused a non-positive
    // target; every ceiling below is checked BEFORE a shard map is sized from it.
    let target = u32::try_from(input.target_shard_count).unwrap_or(u32::MAX);
    let current_open = stream.open_shard_count();
    let current = u32::try_from(current_open).unwrap_or(u32::MAX);

    // "You cannot do the following: ... scale up to more than double your current shard
    // count for a stream[, or] scale down below half your current shard count for a
    // stream." Both bounds are taken against the stream's LIVE open shard count, and a
    // target outside them is a parameter that "can't be used" rather than capacity the
    // account has run out of — so the refusal is the modelled `InvalidArgumentException`,
    // and the stored shard map is left exactly as it was.
    if target > current.saturating_mul(SCALING_FACTOR) {
        return Err(invalid_argument(crate::state::scale_up_over_double_message(
            current_open,
            input.target_shard_count,
        )));
    }
    if target.saturating_mul(SCALING_FACTOR) < current {
        return Err(invalid_argument(
            crate::state::scale_down_below_half_message(current_open, input.target_shard_count),
        ));
    }

    // "You cannot ... scale more than ten times per rolling 24-hour period per stream."
    let scaled = stream.reshards_in_window(now);
    if scaled >= crate::state::MAX_RESHARDS_PER_WINDOW {
        return Err(limit_exceeded(format!(
            "stream {identifier} has been scaled {scaled} times in the last 24 hours; you \
             cannot scale more than {} times per rolling 24-hour period per stream",
            crate::state::MAX_RESHARDS_PER_WINDOW
        )));
    }

    // "You cannot ... scale up to more than the shard limit for your account", and "to
    // update the shard count, Kinesis Data Streams performs splits or merges on individual
    // shards. This can cause short-lived shards to be created, in addition to the final
    // shards. These short-lived shards count towards your total shard limit for your account
    // in the Region" — so the shards this re-tiling lays down are counted against the room
    // left under the quota `DescribeLimits` publishes, and a target past it is refused
    // rather than grown into.
    let laid_down = i32::try_from(target).unwrap_or(i32::MAX);
    if open_shard_count.saturating_add(laid_down) > shard_limit {
        return Err(limit_exceeded(crate::state::shard_limit_exceeded_message(
            region,
            open_shard_count,
            shard_limit,
            laid_down,
        )));
    }

    // "To update the shard count, Kinesis Data Streams performs splits or merges on
    // individual shards", and the status goes to `UPDATING` until the re-tiling the call
    // armed falls due — which the store records as an instant rather than sleeping on it.
    let current_shard_count = stream.reshard_uniformly(target, now);
    Ok(UpdateShardCountOutput {
        stream_name: Some(stream.name.clone()),
        stream_arn: Some(stream.arn.clone()),
        // "The current number of shards": the count the stream had when the call arrived,
        // beside "the updated number of shards" it is being taken to.
        current_shard_count: Some(current_shard_count),
        target_shard_count: Some(input.target_shard_count),
    })
}

impl<Eng> UpdateShardCount<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateShardCount<Eng>
{
    type Input = UpdateShardCountInput;
    type Output = UpdateShardCountOutput;
    type Error = UpdateShardCountError;

    const NAME: &'static str = "UpdateShardCount";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_SHARD_COUNT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_SHARD_COUNT_OUTPUT_SCHEMA;
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
            UpdateShardCountError::from(UpdateShardCountErrorModeled::InternalFailureException(
                InternalFailureException {
                    message: Some("the stream store is unavailable".to_string()),
                },
            ))
        })?;

        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        let outcome = update_shard_count(&mut store, &region, now, &input);
        drop(store);

        audit(
            &akid,
            &region,
            if outcome.is_ok() { "scaled" } else { "refused" },
        );
        outcome
    }
}
