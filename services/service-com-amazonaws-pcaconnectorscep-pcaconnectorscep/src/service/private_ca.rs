// SPDX-License-Identifier: CC0-1.0

//! Amazon Web Services Private CA, as this service reaches it.
//!
//! A connector is a link between a SCEP endpoint and a private certificate authority that
//! Private CA — not this service — owns. So every question about that CA is answered by that
//! service's own API, over the emulator's in-process callback: whether the CA a request
//! names exists at all and whether it is one a connector could be linked to
//! ([`resolve`], which `CreateConnector` resolves its `CertificateAuthorityArn` through
//! before admitting a connector), and the certificate it holds
//! (`GetCertificateAuthorityCertificate`, which the SCEP data plane answers `GetCACert`
//! from).
//!
//! NOTHING here opens a socket or leaves the host: the transport is
//! `ctx.callback().http_client()`, the emulator's in-process channel into its own services,
//! and the endpoint is only the address that channel resolves the private CA by. The
//! credentials are the emulator's own and are used to sign these calls; they are never
//! returned or logged.

use ::aws_sdk_acmpca::types::CertificateAuthorityStatus;
use ::smithy_server_runtime::OperationContext;

/// The endpoint prefix the in-process channel resolves Amazon Web Services Private CA by,
/// which is also the service segment of every `CertificateAuthorityArn` this service accepts
/// (`^arn:aws(-[a-z]+)*:acm-pca:…`).
const ENDPOINT_PREFIX: &str = "acm-pca";

/// What Amazon Web Services Private CA answered about the certificate authority one request
/// named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {
    /// That service answered its own not-found error, which is the one answer that
    /// establishes the CA is absent.
    Absent,
    /// That service holds the CA, in a status it cannot issue certificates from and does not
    /// reach `ACTIVE` from on its own ([`ISSUING_IS_OVER_STATUSES`]).
    Unusable,
    /// That service holds a CA a connector could be linked to — and equally, the answer when
    /// the question could not be put at all, because absence and unusability are established
    /// by the owning service's own read and never inferred from a dependency that did not
    /// answer.
    Usable,
}

impl Resolved {
    /// Whether Private CA reports it holds no certificate authority under the ARN asked
    /// about, which is the one outcome that refuses a `CreateConnector` call outright.
    pub fn is_absent(self) -> bool {
        self == Self::Absent
    }

    /// The `ConnectorStatusReason` a connector linking this CA fails creation with, or `None`
    /// when the CA is one it could be linked to.
    pub fn failure_reason(self) -> Option<&'static str> {
        match self {
            // A connector naming an absent CA is refused before any record exists, so there
            // is no connector left for a status reason to be reported on.
            Self::Absent | Self::Usable => None,
            Self::Unusable => Some(crate::state::REASON_PRIVATECA_INVALID_STATE),
        }
    }
}

/// The statuses a private CA will not issue a certificate in again without being replaced or
/// deliberately put back in service, which is what makes one a CA a connector cannot link.
///
/// `DescribeCertificateAuthority` documents what each status means: `DISABLED` — "your
/// private CA has been disabled", `EXPIRED` — "your private CA certificate has expired",
/// `FAILED` — "a failed CA can never return to the pending state. You must create a new CA",
/// and `DELETED` — "your private CA is within the restoration period, after which it is
/// permanently deleted". `CREATING` and `PENDING_CERTIFICATE` are deliberately absent: both
/// are steps on the way to `ACTIVE` in that same list, so a connector created beside a CA
/// that is still being set up is not one that failed.
const ISSUING_IS_OVER_STATUSES: [CertificateAuthorityStatus; 4] = [
    CertificateAuthorityStatus::Disabled,
    CertificateAuthorityStatus::Expired,
    CertificateAuthorityStatus::Failed,
    CertificateAuthorityStatus::Deleted,
];

/// Resolve `certificate_authority_arn` against Amazon Web Services Private CA: ONE read of
/// that service, answering both whether it holds the CA and whether the status it reports is
/// one a connector could be linked to.
pub async fn resolve(
    ctx: &OperationContext<'_>,
    region: &str,
    certificate_authority_arn: &str,
) -> Resolved {
    let answer = client(ctx, region)
        .describe_certificate_authority()
        .certificate_authority_arn(certificate_authority_arn)
        .send()
        .await;
    let error = match answer {
        // The status is the owning service's own report of the CA, so a CA described without
        // one says nothing about whether it could be used.
        Ok(output) => {
            let status = output
                .certificate_authority()
                .and_then(|authority| authority.status());
            return match status {
                Some(status) if ISSUING_IS_OVER_STATUSES.contains(status) => Resolved::Unusable,
                _ => Resolved::Usable,
            };
        }
        Err(error) => error,
    };
    match error.as_service_error() {
        Some(service_error) if service_error.is_resource_not_found_exception() => Resolved::Absent,
        Some(_) => Resolved::Usable,
        None => {
            ::tracing::warn!(
                certificate_authority = %certificate_authority_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the private CA service did not answer whether the certificate authority exists"
            );
            Resolved::Usable
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
