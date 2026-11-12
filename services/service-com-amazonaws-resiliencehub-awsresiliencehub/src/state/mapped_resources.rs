// SPDX-License-Identifier: CC0-1.0

//! Whether the physical resource an app-version resource mapping names EXISTS —
//! asked of the service that owns it.
//!
//! `CreateAppVersionResource` maps a resource of the caller's account into the
//! application's template, and the mapping is only worth anything if that resource
//! is really there: everything built on the template afterwards — a resolution, an
//! assessment, a resiliency score — is about resources that exist. So the physical
//! identifier is resolved against the OWNING service's own read, over the
//! emulator's in-process callback
//! ([`smithy_server_runtime::EmbeddedEmulator::http_client`]): the endpoint url
//! names the owning service's public host so the request is signed and routed as
//! that service's, but no socket is opened and no name is resolved, so a resource is
//! reached only if this emulator serves it itself.
//!
//! A resource is reported ABSENT only where the component that owns it says so —
//! that service's own modelled "no such resource" error. Everything else leaves the
//! mapping unproven and therefore accepted: a resource whose type no owner here is
//! asked, a resource of another account, a service that answered something else, and
//! a service that did not answer at all. The emulator never refuses a mapping on the
//! strength of a question nobody answered.
//!
//! One probe per resource class the emulator can read: the bucket, the queue, the
//! topic, the table and the function. A type with no probe is unproven, which is why
//! [`physical_resource_is_absent`] is a single entry point rather than a check each
//! handler repeats.

use ::smithy_server_runtime::EmbeddedEmulator;

/// Build the client one probe reads the owning service through, against that
/// service's endpoint in `region`.
///
/// The endpoint url names the called service's public host so the request is signed
/// and routed as that service's, but the TRANSPORT is the emulator's own in-process
/// connector: no socket is opened and no name is resolved. No part of the request
/// chooses that destination — the host is this probe's own service name and the
/// Region is one that satisfies the model's `AwsRegion` grammar — and the
/// credentials are the emulator's own, so the caller's `Authorization` header is
/// never forwarded.
///
/// The transport is in-process, so each client disables what a network client needs
/// and this one cannot have — retries, timeouts, stalled-stream protection and
/// identity caching each need a sleep implementation the callback transport is not
/// given.
///
/// Amazon S3 is reached through the ONE shared constructor for it
/// (`service_common::s3_client`) instead, which also sets the path-style addressing
/// the emulated object store routes on.
macro_rules! client {
    ($sdk:ident, $callback:expr, $endpoint:literal, $region:expr) => {{
        let region: &str = $region;
        let config = ::$sdk::Config::builder()
            .behavior_version(::$sdk::config::BehaviorVersion::latest())
            .http_client($callback.http_client())
            .credentials_provider($callback.aws_credentials_provider())
            .region(::$sdk::config::Region::new(region.to_string()))
            .endpoint_url(format!("https://{}.{region}.amazonaws.com", $endpoint))
            .retry_config(::$sdk::config::retry::RetryConfig::disabled())
            .timeout_config(::$sdk::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::$sdk::config::IdentityCache::no_cache())
            .stalled_stream_protection(::$sdk::config::StalledStreamProtectionConfig::disabled())
            .build();
        ::$sdk::Client::from_conf(config)
    }};
}

/// The `resourceType` of each resource class a probe below can read, as
/// [`super::SUPPORTED_RESOURCE_TYPES`] names it.
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`S3_BUCKET`].
const SQS_QUEUE: &str = "AWS::SQS::Queue";
/// See [`S3_BUCKET`].
const SNS_TOPIC: &str = "AWS::SNS::Topic";
/// See [`S3_BUCKET`].
const DYNAMODB_TABLE: &str = "AWS::DynamoDB::Table";
/// See [`S3_BUCKET`].
const LAMBDA_FUNCTION: &str = "AWS::Lambda::Function";

/// Whether the service owning a resource of `resource_type` answers that it holds no
/// resource under `identifier`.
///
/// `caller_region` is the Region the mapping was made in, which is where a resource
/// named by a bare identifier lives; a resource named by ARN is looked for in the
/// Region its own ARN names. Either way the Region is one that satisfies the
/// model's own `AwsRegion` grammar ([`super::arn_region_and_account`] and
/// [`super::request_region`] are what answer it), so no part of the request reaches
/// the endpoint a probe addresses.
pub async fn physical_resource_is_absent(
    callback: &EmbeddedEmulator,
    caller_region: &str,
    resource_type: &str,
    identifier: &str,
) -> bool {
    let is_arn = ::service_common::arn::parse(identifier).is_some();
    let (arn_region, arn_account) = super::arn_region_and_account(identifier);
    // A resource of another account is not this emulator's to hold, so its absence
    // here shows nothing about it.
    if arn_account.is_some_and(|account| account != ::engine::emulator_account_id()) {
        return false;
    }
    let region = arn_region.unwrap_or(caller_region);

    match resource_type {
        S3_BUCKET => bucket_is_absent(callback, region, &native_id(identifier, "")).await,
        SQS_QUEUE => queue_is_absent(callback, region, &queue_name(identifier)).await,
        SNS_TOPIC => {
            // Amazon SNS addresses a topic by ARN alone, so a mapping that named
            // the topic by name is asked for under the ARN a topic of that name
            // answers to in this account and Region.
            let topic_arn = if is_arn {
                identifier.to_string()
            } else {
                ::engine::emulator_arn("sns", region, identifier)
            };
            topic_is_absent(callback, region, &topic_arn).await
        }
        DYNAMODB_TABLE => table_is_absent(callback, region, &native_id(identifier, "table")).await,
        LAMBDA_FUNCTION => {
            // Lambda addresses a function by name, by partial ARN or by full ARN,
            // so the identifier goes as it came.
            function_is_absent(callback, region, identifier).await
        }
        // A resource class no owner here is asked about.
        _ => false,
    }
}

/// The id a resource of its own service's API answers to: the ARN's resource id when
/// the mapping named the resource by ARN — the part after `<resource_type>/` for a
/// service that qualifies its resource ids, the whole resource field for one that
/// does not — and the identifier itself when it is not an ARN.
fn native_id(identifier: &str, arn_resource_type: &str) -> String {
    let Some(arn) = ::service_common::arn::parse(identifier) else {
        return identifier.to_string();
    };
    if arn_resource_type.is_empty() {
        return arn.resource.to_string();
    }
    ::service_common::arn::resource_id_of_type(identifier, arn_resource_type)
        .unwrap_or(arn.resource)
        .to_string()
}

/// The queue NAME `identifier` names, whichever of the three forms a client holds a
/// queue by it is: a queue url (`https://sqs.<region>.amazonaws.com/<account>/<name>`),
/// an ARN (`arn:aws:sqs:<region>:<account>:<name>`), or the name itself.
fn queue_name(identifier: &str) -> String {
    identifier
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(identifier)
        .to_string()
}

/// Whether Amazon S3 answers that it holds no bucket named `name`.
///
/// The bucket is looked for in the account's own listing, the one read whose SUCCESS
/// says the object store answered at all: a listing that does not hold the bucket
/// shows it is not there, while an error — including the one a request earns when
/// nothing in this emulator serves Amazon S3 — shows nothing.
async fn bucket_is_absent(callback: &EmbeddedEmulator, region: &str, name: &str) -> bool {
    let Ok(listed) = ::service_common::s3_client::s3_client(callback, region)
        .list_buckets()
        .send()
        .await
    else {
        return false;
    };
    !listed
        .buckets()
        .iter()
        .any(|bucket| bucket.name() == Some(name))
}

/// Whether Amazon SQS answers that it holds no queue named `name` — its own
/// `QueueDoesNotExist`, which is the queue's owner saying the queue is not there.
async fn queue_is_absent(callback: &EmbeddedEmulator, region: &str, name: &str) -> bool {
    let answered = client!(aws_sdk_sqs, callback, "sqs", region)
        .get_queue_url()
        .queue_name(name)
        .send()
        .await;
    answered.err().is_some_and(|error| {
        error.as_service_error().is_some_and(
            ::aws_sdk_sqs::operation::get_queue_url::GetQueueUrlError::is_queue_does_not_exist,
        )
    })
}

/// Whether Amazon SNS answers that it holds no topic under `topic_arn` — its own
/// `NotFoundException`.
async fn topic_is_absent(callback: &EmbeddedEmulator, region: &str, topic_arn: &str) -> bool {
    let answered = client!(aws_sdk_sns, callback, "sns", region)
        .get_topic_attributes()
        .topic_arn(topic_arn)
        .send()
        .await;
    answered.err().is_some_and(|error| {
        error.as_service_error().is_some_and(
            ::aws_sdk_sns::operation::get_topic_attributes::GetTopicAttributesError::is_not_found_exception,
        )
    })
}

/// Whether Amazon DynamoDB answers that it holds no table named `name` — its own
/// `ResourceNotFoundException`.
async fn table_is_absent(callback: &EmbeddedEmulator, region: &str, name: &str) -> bool {
    let answered = client!(aws_sdk_dynamodb, callback, "dynamodb", region)
        .describe_table()
        .table_name(name)
        .send()
        .await;
    answered.err().is_some_and(|error| {
        error.as_service_error().is_some_and(
            ::aws_sdk_dynamodb::operation::describe_table::DescribeTableError::is_resource_not_found_exception,
        )
    })
}

/// Whether AWS Lambda answers that it holds no function under `name` — its own
/// `ResourceNotFoundException`.
async fn function_is_absent(callback: &EmbeddedEmulator, region: &str, name: &str) -> bool {
    let answered = client!(aws_sdk_lambda, callback, "lambda", region)
        .get_function()
        .function_name(name)
        .send()
        .await;
    answered.err().is_some_and(|error| {
        error.as_service_error().is_some_and(
            ::aws_sdk_lambda::operation::get_function::GetFunctionError::is_resource_not_found_exception,
        )
    })
}
