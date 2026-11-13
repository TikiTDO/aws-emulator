// SPDX-License-Identifier: CC0-1.0

//! Resolving the resource a delivery source represents, against the service that owns it.
//!
//! `PutDeliverySource` creates "a logical object that represents the resource that is actually
//! sending the logs", and its `resourceArn` is "The ARN of the Amazon Web Services resource
//! that is generating and sending logs" — a resource of ANOTHER service. A source over a
//! resource that does not exist represents nothing and can never send a log event, so the
//! reference is RESOLVED where the owning service holds it, exactly as this service resolves
//! an export task's destination bucket ([`super::export_destination`]) and an import task's
//! source ([`super::import_source`]), rather than being stored on the caller's word.
//!
//! Each owning service is reached the way the framework documents a handler reaching another
//! emulated service ([`smithy_server_runtime::OperationContext::callback`]): an AWS SDK client
//! over the in-process callback's HTTP client and credentials, so nothing here opens a socket
//! or leaves the host. The question is put with the operation that service documents for
//! reading one of its resources — `HeadBucket` for an Amazon S3 bucket ("You can use this
//! operation to determine if a bucket exists"), `GetDistribution` for a CloudFront
//! distribution.
//!
//! A refusal has to be GROUNDED in what the owning service answered, so everything else
//! resolves to nothing and refuses nothing: a sender whose service this emulator holds no
//! client for, a wildcard ARN (which names a kind of resource rather than one resource), an
//! emulator assembled without the owning service to ask, and any other failure to ask it.
//!
//! The one caller-supplied value that reaches a client's AUTHORITY is the signing Region, and
//! it is narrowed before it gets there ([`service_common::arn::region_label`]), so no request
//! can re-point a client at a host of its own choosing: it reaches this emulator's own
//! services and nothing else, whatever resource a caller names.

use super::callback_client::callback_client;

/// The ARN vendor of each sender this emulator can RESOLVE a resource of: an Amazon S3 bucket
/// and a CloudFront distribution. A sender whose service this emulator holds no client for is
/// absent from here, and nothing is established — or refused — about one of its resources.
///
/// An S3 bucket ARN carries the bucket name as its whole resource segment, so the vendor alone
/// places one; a distribution's ARN names its resource type — the one the vended-log catalogue
/// spells for that sender too ([`super::CONFIGURATION_TEMPLATE_SOURCES`]) — and its id follows.
const S3_VENDOR: &str = "s3";
const CLOUDFRONT_DISTRIBUTION: (&str, &str) = ("cloudfront", "distribution");

/// Amazon S3's own sentence for a bucket it does not hold, as its `NoSuchBucket` states it:
/// "The specified bucket does not exist".
///
/// Stated here rather than read off the answer because `HeadBucket` — "the response is empty"
/// — carries no message to read: the operation answers a bare `404`, so this is the only place
/// that sentence can come from.
const NO_SUCH_BUCKET_MESSAGE: &str = "The specified bucket does not exist";

/// CloudFront's own sentence for a distribution it does not hold (`NoSuchDistribution`), for
/// the one case its answer carries no message of its own to quote.
const NO_SUCH_DISTRIBUTION_MESSAGE: &str = "The specified distribution does not exist.";

/// The refusal for a `resourceArn` whose owning service reports it holds no such resource, or
/// `None` where nothing of the kind was established — the resource is there, the owning service
/// gave no answer, or it is not one this emulator can ask about at all (see the module docs).
///
/// The sentence is the OWNING SERVICE's for its own missing resource: CloudFront's is read off
/// the answer it gave, and Amazon S3's is the one `NoSuchBucket` states, because the operation
/// that settles a bucket answers with no body at all ([`NO_SUCH_BUCKET_MESSAGE`]). Nothing of
/// the request is interpolated into either.
pub async fn absent_resource_message(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource_arn: &str,
) -> Option<String> {
    let (vendor, resource) = super::arn_service_and_resource(resource_arn)?;
    // A WILDCARD ARN names a kind of resource rather than one resource — the `logType` member
    // documents one for the log types that are sent account-wide ("use a wildcard ARN for the
    // hub resource", "use a wildcard log group ARN") — so there is no single resource to ask
    // the owning service about.
    if resource.contains('*') {
        return None;
    }
    if vendor == S3_VENDOR {
        // An Amazon S3 bucket ARN carries the bucket name as its whole resource segment
        // (`arn:aws:s3:::<bucket>`), and the question is the one an export task's destination
        // is already resolved with.
        super::export_destination::bucket_is_absent(ctx, region, resource)
            .await
            .then(|| NO_SUCH_BUCKET_MESSAGE.to_string())
    } else if vendor == CLOUDFRONT_DISTRIBUTION.0 {
        absent_distribution_message(ctx, region, resource_arn).await
    } else {
        None
    }
}

/// The sentence CloudFront answered with when it reports that it holds no distribution under
/// the one `resource_arn` names, as IT worded it.
///
/// `None` — refuse nothing — for every answer that is not CloudFront reporting the distribution
/// absent, for the reason the module docs give: an ARN that names no distribution id, an
/// emulator assembled with no CloudFront to ask, or any other failure to ask it.
async fn absent_distribution_message(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource_arn: &str,
) -> Option<String> {
    // An ARN that names no distribution id names no distribution to ask about.
    let id = ::service_common::arn::resource_id_of_type(resource_arn, CLOUDFRONT_DISTRIBUTION.1)
        .filter(|id| !id.is_empty())?;
    // Which Region the distribution is looked up in decides nothing here — the emulator serves
    // one CloudFront over the in-process callback — so a scope that carries no Region label
    // falls back to the default rather than putting caller text in the authority.
    let region =
        ::service_common::arn::region_label(region).unwrap_or(crate::state::DEFAULT_REGION);
    let client = callback_client!(aws_sdk_cloudfront, ctx.callback(), "cloudfront", region);
    match client.get_distribution().id(id).send().await {
        // CloudFront holds the distribution.
        Ok(_) => None,
        // CloudFront says it holds no such distribution: the refusal is grounded in its answer,
        // and so is the sentence it carries — the one CloudFront worded for its own missing
        // resource, quoted as it stands. An answer of that error carrying no message at all
        // leaves the sentence that error states ([`NO_SUCH_DISTRIBUTION_MESSAGE`]).
        Err(error) if reported_absent(&error) => Some(
            error
                .as_service_error()
                .and_then(::aws_smithy_types::error::metadata::ProvideErrorMetadata::message)
                .unwrap_or(NO_SUCH_DISTRIBUTION_MESSAGE)
                .to_string(),
        ),
        // Any other answer is this emulator failing to ask, not CloudFront reporting a missing
        // distribution, so it refuses nothing — and it is worth a warning, because a deployment
        // with no CloudFront to ask is one where this resolution does not run. The caller's own
        // value is rendered in its escaped `Debug` form, so an ARN carrying a newline cannot
        // forge a log line.
        Err(error) => {
            ::tracing::warn!(
                resource_arn = ?resource_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a delivery source's resource could not be resolved against CloudFront"
            );
            None
        }
    }
}

/// Whether a CloudFront `GetDistribution` failure is CloudFront reporting that it holds no
/// distribution under the id, as opposed to the call not reaching CloudFront at all.
fn reported_absent(
    error: &::aws_sdk_cloudfront::error::SdkError<
        ::aws_sdk_cloudfront::operation::get_distribution::GetDistributionError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error.as_service_error().is_some_and(
        ::aws_sdk_cloudfront::operation::get_distribution::GetDistributionError::is_no_such_distribution,
    )
}
