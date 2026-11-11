// SPDX-License-Identifier: CC0-1.0

//! Amazon Web Services Private CA, as this service reaches it.
//!
//! A connector is a link between a SCEP endpoint and a private certificate authority that
//! Private CA — not this service — owns. So every question about that CA is answered by that
//! service's own API, over the emulator's in-process callback: whether the CA a request
//! names exists at all ([`names_no_certificate_authority`], which `CreateConnector` resolves
//! its `CertificateAuthorityArn` through before admitting a connector), and the certificate
//! it holds (`GetCertificateAuthorityCertificate`, which the SCEP data plane answers
//! `GetCACert` from).
//!
//! NOTHING here opens a socket or leaves the host: the transport is
//! `ctx.callback().http_client()`, the emulator's in-process channel into its own services,
//! and the endpoint is only the address that channel resolves the private CA by. The
//! credentials are the emulator's own and are used to sign these calls; they are never
//! returned or logged.

use ::smithy_server_runtime::OperationContext;

/// The endpoint prefix the in-process channel resolves Amazon Web Services Private CA by,
/// which is also the service segment of every `CertificateAuthorityArn` this service accepts
/// (`^arn:aws(-[a-z]+)*:acm-pca:…`).
const ENDPOINT_PREFIX: &str = "acm-pca";

/// Whether Amazon Web Services Private CA reports that it holds NO certificate authority
/// under `certificate_authority_arn`.
///
/// `true` only when that service answered its own not-found error, which is the one answer
/// that establishes the CA is absent. Every other outcome — the CA described, any other
/// error, or no answer at all because the channel could not reach that service — leaves the
/// question unanswered, so this reports `false`: absence is established by the owning
/// service's own read, never inferred from a dependency that did not answer.
pub async fn names_no_certificate_authority(
    ctx: &OperationContext<'_>,
    region: &str,
    certificate_authority_arn: &str,
) -> bool {
    let answer = client(ctx, region)
        .describe_certificate_authority()
        .certificate_authority_arn(certificate_authority_arn)
        .send()
        .await;
    let Err(error) = answer else {
        return false;
    };
    match error.as_service_error() {
        Some(service_error) => service_error.is_resource_not_found_exception(),
        None => {
            ::tracing::warn!(
                certificate_authority = %certificate_authority_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the private CA service did not answer whether the certificate authority exists"
            );
            false
        }
    }
}

/// The Amazon Web Services Private CA client this service reads that service through: the AWS
/// SDK driven over the emulator's own in-process transport, signing in the emulator's name —
/// Connector for SCEP reads the CA it was given with its own service principal.
pub fn client(ctx: &OperationContext<'_>, region: &str) -> ::aws_sdk_acmpca::Client {
    let config = ::aws_sdk_acmpca::Config::builder()
        .behavior_version(::aws_sdk_acmpca::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_acmpca::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://{ENDPOINT_PREFIX}.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials are handed over
        // directly.
        .retry_config(::aws_sdk_acmpca::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_acmpca::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_acmpca::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_acmpca::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_acmpca::Client::from_conf(config)
}
