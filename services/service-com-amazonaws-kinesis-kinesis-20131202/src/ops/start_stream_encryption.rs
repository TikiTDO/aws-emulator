// SPDX-License-Identifier: CC0-1.0

//! @op StartStreamEncryption
//!
//! Turns server-side encryption ON for the stream the request addresses, with the KMS key
//! it names — and UPDATES that key on a stream that is already encrypted: "Enables or
//! updates server-side encryption using an Amazon Web Services KMS key for a specified
//! stream". The change is asynchronous, so the stream is `UPDATING` until it falls back to
//! `ACTIVE`, from when on records written to it are encrypted.
//!
//! The `KeyId` is RESOLVED against KMS before anything is stored ([`resolve_key`]): a
//! stream reported as encrypted is one whose records are wrapped with the caller's key, so
//! an identifier KMS holds no key for is refused `KMSNotFoundException` — the error this
//! operation models for exactly that — and the stream keeps the configuration it had.
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `StartStreamEncryption`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json")
//! ```
//!
//! Enables or updates server-side encryption using an Amazon Web Services KMS key for a
//! specified stream. When invoking this API, you must use either the StreamARN or the
//! StreamName parameter, or both. It is recommended that you use the StreamARN input
//! parameter when you invoke this API. Starting encryption is an asynchronous operation.
//! Upon receiving the request, Kinesis Data Streams returns immediately and sets the status
//! of the stream to UPDATING. After the update is complete, Kinesis Data Streams sets the
//! status of the stream back to ACTIVE. Updating or applying encryption normally takes a
//! few seconds to complete, but it can take minutes. You can continue to read and write
//! data to your stream while its status is UPDATING. Once the status of the stream is
//! ACTIVE, encryption begins for records written to the stream. API Limits: You can
//! successfully apply a new Amazon Web Services KMS key for server-side encryption 25 times
//! in a rolling 24-hour period. Note: It can take up to 5 seconds after the stream is in an
//! ACTIVE status before all records written to the stream are encrypted. After you enable
//! encryption, you can verify that encryption is applied by inspecting the API response
//! from PutRecord or PutRecords.
//!
//! ## Input
//!
//! * `EncryptionType` (EncryptionType, one of: KMS | NONE) **required** — The encryption type
//!   to use. The only valid value is KMS.
//! * `KeyId` (KeyId, len 1..2048) **required** — The GUID for the customer-managed Amazon Web
//!   Services KMS key to use for encryption. This value can be a globally unique identifier,
//!   a fully specified Amazon Resource Name (ARN) to either an alias or a key, or an alias
//!   name prefixed by "alias/".You can also use a master key owned by Kinesis Data Streams by
//!   specifying the alias aws/kinesis. Key ARN example:
//!   arn:aws:kms:us-east-1:123456789012:key/12345678-1234-1234-1234-123456789012 Alias ARN
//!   example: arn:aws:kms:us-east-1:123456789012:alias/MyAliasName Globally unique key ID
//!   example: 12345678-1234-1234-1234-123456789012 Alias name example: alias/MyAliasName
//!   Master key owned by Kinesis Data Streams: alias/aws/kinesis
//! * `StreamARN` (StreamARN, len 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$) —
//!   The ARN of the stream.
//! * `StreamId` (StreamId, len 1..24, pattern ^[a-z0-9]{20}-[a-z0-9]{3}$) — Not Implemented.
//!   Reserved for future use.
//! * `StreamName` (StreamName, len 1..128, pattern ^[a-zA-Z0-9_.-]+$) — The name of the
//!   stream for which to start encrypting records.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 400) — Specifies that you do not have the permissions
//!   required to perform this operation.
//! * `InvalidArgumentException` (HTTP 400) — A specified parameter exceeds its restrictions,
//!   is not supported, or can't be used. For more information, see the returned message.
//! * `KMSAccessDeniedException` (HTTP 400) — The ciphertext references a key that doesn't
//!   exist or that you don't have access to.
//! * `KMSDisabledException` (HTTP 400) — The request was rejected because the specified
//!   customer master key (CMK) isn't enabled.
//! * `KMSInvalidStateException` (HTTP 400) — The request was rejected because the state of
//!   the specified resource isn't valid for this request. For more information, see How Key
//!   State Affects Use of a Customer Master Key in the Amazon Web Services Key Management
//!   Service Developer Guide.
//! * `KMSNotFoundException` (HTTP 400) — The request was rejected because the specified
//!   entity or resource can't be found.
//! * `KMSOptInRequired` (HTTP 400) — The Amazon Web Services access key ID needs a
//!   subscription for the service.
//! * `KMSThrottlingException` (HTTP 400) — The request was denied due to request throttling.
//!   For more information about throttling, see Limits in the Amazon Web Services Key
//!   Management Service Developer Guide.
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
// `StartStreamEncryptionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-kinesis-kinesis-20131202.json");

#[derive(Debug, Clone, Default)]
pub struct StartStreamEncryptionOutput;

impl StartStreamEncryptionOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static START_STREAM_ENCRYPTION_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &StartStreamEncryptionOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for StartStreamEncryptionOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StartStreamEncryptionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.kinesis#StartStreamEncryption",
    // `InternalFailureException` is widened in with `extra_errors`: this SERVICE models it
    // ("The processing of the request failed because of an unknown error, exception, or
    // failure"), the operation shape does not list it, and it is the only honest answer
    // for a service mounted on an engine that holds no state for it.
    extra_errors = ["InternalFailureException"]
)]
#[derive(Debug)]
pub enum StartStreamEncryptionErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InvalidArgumentException(InvalidArgumentException),
    KMSAccessDeniedException(KMSAccessDeniedException),
    KMSDisabledException(KMSDisabledException),
    KMSInvalidStateException(KMSInvalidStateException),
    KMSNotFoundException(KMSNotFoundException),
    KMSOptInRequired(KMSOptInRequired),
    KMSThrottlingException(KMSThrottlingException),
    LimitExceededException(LimitExceededException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    InternalFailureException(InternalFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_stream_encryption::StartStreamEncryptionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StartStreamEncryptionError = crate::scaffolded::Scaffolded<StartStreamEncryptionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StartStreamEncryptionErrorModeled> for StartStreamEncryptionError {
    fn from(modeled: StartStreamEncryptionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartStreamEncryption` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StartStreamEncryption<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

/// Record this state-changing call for attribution: the access key id the caller presented
/// (a public identifier, never a secret), the region the stream lives in, and how the call
/// ended. No stream name, no ARN and no key id: a log sink needs none of them to attribute
/// the change.
fn audit(akid: &str, region: &str, outcome: &str) {
    ::tracing::info!(
        target: "emulator::audit",
        operation = "StartStreamEncryption",
        akid = %akid,
        region = %region,
        outcome = %outcome,
        "state-changing operation"
    );
}

/// Refuse the request with the error this operation models for a parameter that "exceeds
/// its restrictions, is not supported, or can't be used".
fn invalid_argument(message: String) -> StartStreamEncryptionError {
    StartStreamEncryptionErrorModeled::InvalidArgumentException(InvalidArgumentException {
        message: Some(message),
    })
    .into()
}

/// The identifier the request addressed the stream by, which is what a refusal names.
/// `StreamId` is documented "Not Implemented. Reserved for future use", so no stream
/// carries one and a request naming only that member names no stream.
fn stream_identifier(input: &StartStreamEncryptionInput) -> &str {
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
) -> StartStreamEncryptionError {
    match failure {
        crate::state::StreamLookupFailure::Unspecified => {
            invalid_argument(crate::state::STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string())
        }
        crate::state::StreamLookupFailure::Mismatch => {
            invalid_argument(crate::state::STREAM_IDENTIFIER_MISMATCH_MESSAGE.to_string())
        }
        crate::state::StreamLookupFailure::NotFound => {
            StartStreamEncryptionErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::stream_not_found_message(identifier)),
                },
            )
            .into()
        }
    }
}

/// Apply the request's KMS key to the stream it names, or answer the modelled refusal that
/// stands in the way. The whole write lives here so the caller holds the store's lock for
/// exactly this long, which is what serialises two overlapping calls: the key the later one
/// stores is the one that stands.
fn start_stream_encryption(
    store: &mut crate::state::ComAmazonawsKinesisKinesis20131202State,
    region: &str,
    now: ::std::time::SystemTime,
    input: &StartStreamEncryptionInput,
) -> Result<(), StartStreamEncryptionError> {
    // Made again here, under the write lock: the store can have moved while this request
    // was asking KMS about the key.
    refusal_from_state(store, region, now, input)?;

    let identifier = stream_identifier(input).to_string();
    let stream = store
        .resolve_stream_mut(
            now,
            region,
            input.stream_name.as_deref(),
            input.stream_arn.as_deref(),
        )
        .map_err(|failure| lookup_refusal(failure, &identifier))?;

    // The key the request names is what the stream is encrypted with from here on, whether
    // it had none before (enabling) or another one (updating). The shard map and the
    // records in it are untouched: "you can continue to read and write data to your stream
    // while its status is UPDATING".
    stream.start_encryption(&input.key_id, now);
    Ok(())
}

/// Every refusal this request can be answered from the STORE alone, so that nothing is
/// written — and no key is resolved — for a request the stored streams already refuse.
///
/// Read-only, and made TWICE: once before the awaited KMS lookup (a store guard is not held
/// across an await) and once under the lock that performs the write.
fn refusal_from_state(
    store: &crate::state::ComAmazonawsKinesisKinesis20131202State,
    region: &str,
    now: ::std::time::SystemTime,
    input: &StartStreamEncryptionInput,
) -> Result<(), StartStreamEncryptionError> {
    // "The encryption type to use. The only valid value is KMS"
    // (`StartStreamEncryptionInput$EncryptionType`): `NONE` is a value the enum admits on
    // the wire but not one this operation serves — turning encryption off is
    // `StopStreamEncryption`'s — so it is refused BEFORE any stream is read.
    let encryption_type = input.encryption_type.as_str();
    if encryption_type != crate::state::ENCRYPTION_TYPE_KMS {
        return Err(invalid_argument(
            crate::state::encryption_type_unsupported_message(encryption_type),
        ));
    }

    let identifier = stream_identifier(input).to_string();
    let stream = store
        .resolve_stream(
            now,
            region,
            input.stream_name.as_deref(),
            input.stream_arn.as_deref(),
        )
        .map_err(|failure| lookup_refusal(failure, &identifier))?;
    // A refusal about the stream this request RESOLVED names it by its `StreamName`, which
    // is the variable part the service's messages carry: whichever identifier addressed the
    // stream — its name, its ARN or both — the STORED name is what the message reports.
    let named = stream.name.clone();

    // "The resource is not available for this operation. For successful operation, the
    // resource must be in the ACTIVE state" (`ResourceInUseException`) — and this operation
    // publishes which streams that leaves out: "You can continue to read and write data to
    // your stream while its status is UPDATING", so a stream whose encryption change is
    // still in flight is still serving, and a change that arrives then is served too (the
    // key it stores is the one that stands). What is refused is a stream that is not
    // serving at all: one whose create has not fallen due yet, or whose removal has been
    // accepted.
    let status = stream.status(now);
    if !matches!(
        status,
        crate::state::STREAM_ACTIVE | crate::state::STREAM_UPDATING
    ) {
        return Err(StartStreamEncryptionErrorModeled::ResourceInUseException(
            ResourceInUseException {
                message: Some(crate::state::stream_not_active_message(&named, status)),
            },
        )
        .into());
    }

    // "You can successfully apply a new Amazon Web Services KMS key for server-side
    // encryption 25 times in a rolling 24-hour period": a stream that has used up the
    // window's changes keeps the configuration it has.
    let started = stream.encryption_starts_in_window(now);
    if started >= crate::state::MAX_ENCRYPTION_CHANGES_PER_WINDOW {
        return Err(StartStreamEncryptionErrorModeled::LimitExceededException(
            LimitExceededException {
                message: Some(crate::state::encryption_change_limit_message(
                    &named, started,
                )),
            },
        )
        .into());
    }

    Ok(())
}

/// What KMS answered about the `KeyId` this request named.
#[derive(Debug, PartialEq, Eq)]
enum KeyLookup {
    /// KMS resolved the identifier to a key, so the stream can be reported as encrypted
    /// under it.
    Resolved,
    /// KMS reported that this account and region hold no key the identifier resolves to.
    NoSuchKey,
    /// KMS said nothing about the key itself (it answered about the call, about itself, or
    /// it was not there to ask). Not a negative, so not a refusal.
    Indeterminate,
}

/// The KMS client this service resolves a `KeyId` through: the AWS SDK driven over the
/// emulator's own in-process callback, so this crate names no other service's crate and no
/// socket is opened. `region` is the request's own signing region, which is where the
/// stream this key encrypts lives.
///
/// That region is CALLER-SUPPLIED text and here it ends up in an endpoint URL's AUTHORITY,
/// so it is read as a Region LABEL ([`::service_common::arn::region_label`], the one place
/// this tree states that grammar) rather than interpolated as given. `None` for a value
/// that would name an authority the caller chose, in which case no lookup is made at all
/// and nothing is refused on it.
fn kms_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<::aws_sdk_kms::Client> {
    let region = ::service_common::arn::region_label(region)?;
    let config = ::aws_sdk_kms::Config::builder()
        .behavior_version(::aws_sdk_kms::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_kms::config::Region::new(region.to_string()))
        // The endpoint is the LABEL the in-process transport routes by (and what the
        // request is signed for); the callback above dispatches it inside this process, so
        // no socket is opened and nothing outside the emulator is reached.
        .endpoint_url(format!("https://kms.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled())
        .build();
    Some(::aws_sdk_kms::Client::from_conf(config))
}

/// Ask KMS whether `key_id` names a key at all.
///
/// `DescribeKey` is the read the service itself performs here: a stream this operation
/// accepts is a stream whose records are wrapped with the caller's key, so the identifier is
/// RESOLVED before the configuration is stored — which is why the operation models
/// `KMSNotFoundException` at all. It also resolves every form `KeyId` takes (a key id, a key
/// or alias ARN, an alias name, and the service's own predefined alias), so this asks KMS
/// rather than deciding from the shape of the value.
///
/// A definite negative is only KMS's own "no such key" answer, with the client status it
/// came with. Anything else — a fault, a throttle, a refusal about the CALL, no KMS
/// mounted beside this service to ask, or a region this emulator will not put in an
/// endpoint's authority ([`kms_client`]) — says nothing about the key's existence and is
/// reported as [`KeyLookup::Indeterminate`], so nothing is refused that could not be
/// grounded in KMS's answer.
async fn resolve_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key_id: &str,
) -> KeyLookup {
    let Some(client) = kms_client(ctx, region) else {
        return KeyLookup::Indeterminate;
    };
    let error = match client.describe_key().key_id(key_id).send().await {
        Ok(_) => return KeyLookup::Resolved,
        Err(error) => error,
    };
    let about_the_key = error.as_service_error().is_some_and(
        ::aws_sdk_kms::operation::describe_key::DescribeKeyError::is_not_found_exception,
    );
    // A 5xx is about KMS, not about the key, whatever code travels with it.
    let client_error = error
        .raw_response()
        .is_some_and(|response| response.status().is_client_error());
    if about_the_key && client_error {
        return KeyLookup::NoSuchKey;
    }
    // Visible to an operator: an indeterminate lookup is accepted, so without this line it
    // would be indistinguishable from a key KMS resolved.
    ::tracing::warn!(
        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
        "KMS gave no answer about this stream's KeyId; accepting it"
    );
    KeyLookup::Indeterminate
}

/// The modelled refusal for a `KeyId` KMS holds no key for: "The request was rejected
/// because the specified entity or resource can't be found" (`KMSNotFoundException`). The
/// identifier is echoed from the request, since that is the value that resolved to nothing.
fn kms_not_found(key_id: &str) -> StartStreamEncryptionError {
    StartStreamEncryptionErrorModeled::KMSNotFoundException(KMSNotFoundException {
        message: Some(format!("Key '{key_id}' does not exist")),
    })
    .into()
}

impl<Eng> StartStreamEncryption<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartStreamEncryption<Eng>
{
    type Input = StartStreamEncryptionInput;
    type Output = StartStreamEncryptionOutput;
    type Error = StartStreamEncryptionError;

    const NAME: &'static str = "StartStreamEncryption";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_STREAM_ENCRYPTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = START_STREAM_ENCRYPTION_OUTPUT_SCHEMA;
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
            StartStreamEncryptionError::from(
                StartStreamEncryptionErrorModeled::InternalFailureException(
                    InternalFailureException {
                        message: Some("the stream store is unavailable".to_string()),
                    },
                ),
            )
        })?;

        let now = ::std::time::SystemTime::now();
        // The refusals that only READ the store are answered first, and the lock is
        // released before the KMS lookup below is awaited: a request the stored streams
        // already refuse never asks KMS about a key.
        if let Err(refusal) = {
            let store = state.lock();
            refusal_from_state(&store, &region, now, &input)
        } {
            audit(&akid, &region, "refused");
            return Err(refusal);
        }

        // Nothing is stored for a key that resolves to nothing: the stream keeps the
        // encryption configuration it had, so a stream AWS would have left unencrypted is
        // left unencrypted here too.
        if resolve_key(ctx, &region, &input.key_id).await == KeyLookup::NoSuchKey {
            audit(&akid, &region, "refused");
            return Err(kms_not_found(&input.key_id));
        }

        let outcome = {
            let now = ::std::time::SystemTime::now();
            let mut store = state.lock();
            start_stream_encryption(&mut store, &region, now, &input)
        };
        audit(
            &akid,
            &region,
            if outcome.is_ok() { "started" } else { "refused" },
        );
        outcome?;

        // `StartStreamEncryption` models no output members: the encryption configuration it
        // stored is what `DescribeStreamSummary` and `DescribeStream` report for the stream,
        // and the records written to it from here on carry that type.
        Ok(StartStreamEncryptionOutput)
    }
}
