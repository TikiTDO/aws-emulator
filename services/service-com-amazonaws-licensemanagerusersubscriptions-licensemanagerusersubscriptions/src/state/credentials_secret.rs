// SPDX-License-Identifier: CC0-1.0

//! Resolving the Secrets Manager secret a request's `CredentialsProvider` names against
//! what the account holds.
//!
//! A `CredentialsProvider` is "information about the credential provider for user
//! administration", and its only member points at a secret of the caller's own account:
//! `SecretsManagerCredentialsProvider$SecretId` is "the ID of the Secrets Manager secret
//! that contains credentials". This service creates no secret, so every such reference
//! names a resource the account has to hold already — exactly as a registration's
//! `Settings` name subnets and a security group it does not own ([`super::ec2_fleet`]) and
//! as an `ActiveDirectoryIdentityProvider` names a directory of the account's own fleet.
//! Whether the account holds it is therefore not a question this service's records can
//! answer: it is put to the service that OWNS secrets, through the AWS SDK driven over the
//! in-process callback ([`smithy_server_runtime::OperationContext::callback`]), which
//! routes inside this emulator, so no socket is opened, nothing leaves the process and this
//! crate names no other service's crate.
//!
//! The question asked is DESCRIBE, not retrieve: a request is admitted because the account
//! holds a secret of that id whose credentials can still be retrieved, which is a fact
//! about the secret's metadata — so no credential value is ever read into this service, let
//! alone stored or logged. Nothing is cached either: a secret created a moment ago resolves
//! at once, and one that has been deleted stops resolving at once.
//!
//! Two answers are definite negatives about the reference, and only those two refuse a
//! request: the account holds no secret of that id, and the secret it holds was marked for
//! deletion, so its credentials cannot be retrieved any more. Each is stated in the
//! sentence the owner of secrets itself states for that fact, the same way a refused subnet
//! is stated in the sentence its owner uses. Every other outcome — Secrets Manager
//! answering about itself, or not answering at all because nothing routed to it beside this
//! service — is EVIDENCE OF NOTHING and refuses nothing.

use crate::ops::types::{CredentialsProvider, IdentityProvider, LicenseServerSettings};
use crate::state::StateError;

/// The sentence stated for a `SecretId` the account holds no secret of.
///
/// The wording the service that owns secrets answers for that fact with: a
/// `ResourceNotFoundException` naming no identifier, so nothing of the request is reflected
/// back. Quoting the owner is what makes a caller read the same fact however it reached the
/// lookup — the rule [`super::ec2_fleet`] states a refused subnet and security group by.
const SECRET_NOT_FOUND: &str = "Secrets Manager can't find the specified secret.";

/// The sentence stated for a secret that is marked for deletion.
///
/// The owner's wording again, for the other definite negative: the account still holds the
/// record, so it is found, but its value can no longer be retrieved — so it is no
/// credentials provider a license server or a domain can be administered through.
const SECRET_MARKED_FOR_DELETION: &str =
    "You can't perform this operation on the secret because it was marked for deletion.";

/// The error codes Secrets Manager answers that say the account has NO usable secret of the
/// id it was asked about, rather than something about Secrets Manager or about the call:
/// the resource it cannot resolve, and an identifier it cannot read as a `SecretId` at all.
///
/// Deliberately short. A throttle, a server fault and an authorization refusal describe the
/// caller or the service rather than the secret, so none of them turns into a refusal here.
const SECRET_ABSENT_ERROR_CODES: &[&str] =
    &["ResourceNotFoundException", "InvalidParameterException"];

/// A Secrets Manager client over the in-process callback into this same emulator.
///
/// Built exactly as the EC2 client this crate resolves a registration's network through
/// ([`super::ec2_fleet`]): the callback's own HTTP client, so the endpoint URL below is
/// only the address that in-process transport routes on — no socket is opened, no name is
/// resolved and nothing leaves this process — and the callback's credentials, which sign
/// this in-process hop and never appear in a response. `region` is the request's own
/// signing region, so the secret is looked for where the caller addressed the request:
/// Secrets Manager resolves a `SecretId` only within one Region, which is the rule the
/// owner of secrets applies and this service does not restate.
fn secrets_manager(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_secretsmanager::Client {
    let config = ::aws_sdk_secretsmanager::Config::builder()
        .behavior_version(::aws_sdk_secretsmanager::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_secretsmanager::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!("https://secretsmanager.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials the
        // callback hands over are used directly.
        .retry_config(::aws_sdk_secretsmanager::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_secretsmanager::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_secretsmanager::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_secretsmanager::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_secretsmanager::Client::from_conf(config)
}

/// What the owner of secrets answered about ONE `SecretId`.
enum SecretLookup {
    /// The account holds a secret of that id whose credentials can still be retrieved.
    Resolved,
    /// A definite negative about the reference, carrying the sentence it is stated in.
    Unusable(&'static str),
    /// Nothing was established — Secrets Manager answered about itself (a fault, a
    /// throttle, a caller it would not answer) or did not answer at all because nothing
    /// routed to it beside this service. Not a negative, so not a refusal.
    Indeterminate,
}

/// Ask the owner of secrets about `secret_id`, in `region`.
///
/// ONE secret is asked about, by id, so the work is independent of how many secrets the
/// account holds — and `DescribeSecret` answers it from the secret's metadata, without the
/// credential value ever being read.
async fn lookup(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    secret_id: &str,
) -> SecretLookup {
    match secrets_manager(ctx, region)
        .describe_secret()
        .secret_id(secret_id)
        .send()
        .await
    {
        // "The date the secret is scheduled for deletion. If it is not scheduled for
        // deletion, this field is omitted" (`DescribeSecretResponse$DeletedDate`): a secret
        // on its way out still has a record, and no retrievable value.
        Ok(described) if described.deleted_date().is_some() => {
            SecretLookup::Unusable(SECRET_MARKED_FOR_DELETION)
        }
        Ok(_) => SecretLookup::Resolved,
        Err(error) => classify(region, &error),
    }
}

/// Classify a failed `DescribeSecret`: a definite negative about the secret only when
/// Secrets Manager answered a client error naming one of [`SECRET_ABSENT_ERROR_CODES`].
fn classify(
    region: &str,
    error: &::aws_sdk_secretsmanager::error::SdkError<
        ::aws_sdk_secretsmanager::operation::describe_secret::DescribeSecretError,
    >,
) -> SecretLookup {
    // The status Secrets Manager answered with, when it answered at all: a 5xx is about
    // Secrets Manager, not about the secret, whatever code travels with it.
    let client_error = error
        .raw_response()
        .is_some_and(|response| response.status().is_client_error());
    let code =
        ::aws_sdk_secretsmanager::error::ProvideErrorMetadata::code(error).unwrap_or_default();
    if client_error
        && SECRET_ABSENT_ERROR_CODES
            .iter()
            .any(|known| known.eq_ignore_ascii_case(code))
    {
        return SecretLookup::Unusable(SECRET_NOT_FOUND);
    }
    // At `debug`, for the reason the network hops in `ec2_fleet` log there: a reference the
    // account holds no secret for is the ordinary answer to this question, so a hop that
    // established nothing is not a service fault — but it IS why a request was admitted
    // without the secret being resolved, so it stays traceable. The id the request named is
    // not recorded: it addresses a credentials secret of the caller's.
    ::tracing::debug!(
        %region,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
        "Secrets Manager established nothing about the credentials secret a request named"
    );
    SecretLookup::Indeterminate
}

/// The secret a `CredentialsProvider` names, when it names one.
///
/// `SecretsManagerCredentialsProvider$SecretId` is not `@required` (`@length(min: 1)` is
/// all it carries), so a provider that names no secret — and one that spells the empty
/// string, which that length rules out on the wire — names nothing to resolve.
fn named_secret(provider: &CredentialsProvider) -> Option<&str> {
    let CredentialsProvider::SecretsManagerCredentialsProvider(secret) = provider;
    secret
        .secret_id
        .as_deref()
        .filter(|secret_id| !secret_id.is_empty())
}

/// THE RULE, in one place: a `CredentialsProvider` a request supplies is resolved against
/// the secrets the account holds, and one naming a secret the account has none of — or one
/// it marked for deletion — is refused with [`StateError::Validation`], which each caller
/// answers on its own modelled error channel.
///
/// `Ok(())` for a request that names no credentials provider and for one whose provider
/// names no secret: there is nothing to resolve, so the rest of the request's own rules
/// decide it.
///
/// BOUNDED: at most ONE hop, for the one secret a request can name, with no lock of this
/// service held across it.
pub async fn resolve_credentials_provider(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    provider: Option<&CredentialsProvider>,
) -> Result<(), StateError> {
    let Some(secret_id) = provider.and_then(named_secret) else {
        return Ok(());
    };
    let region = crate::state::request_region(&ctx.parts().headers);
    match lookup(ctx, &region, secret_id).await {
        SecretLookup::Resolved | SecretLookup::Indeterminate => Ok(()),
        SecretLookup::Unusable(message) => Err(StateError::Validation(message.to_string())),
    }
}

/// Resolve the credentials secret a license server endpoint's settings name.
///
/// `LicenseServerSettings` carries "the type of license server and the Secrets Manager
/// secret that enables administrators to add or remove users associated with the license
/// server", and `RdsSalSettings$RdsSalCredentialsProvider` (`@required`) is "a reference to
/// the credentials provider that's used for RDS license server user administration" — so an
/// endpoint cannot be provisioned against a secret the account does not hold, and the same
/// rule [`resolve_credentials_provider`] states decides it.
pub async fn resolve_license_server_credentials(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    settings: &LicenseServerSettings,
) -> Result<(), StateError> {
    let crate::ops::types::ServerSettings::RdsSalSettings(rds_sal) = &settings.server_settings;
    resolve_credentials_provider(ctx, Some(&rds_sal.rds_sal_credentials_provider)).await
}

/// Resolve the credentials secret a self-managed Active Directory's settings name.
///
/// `ActiveDirectorySettings$DomainCredentialsProvider` "points to the `CredentialsProvider`
/// resource that contains information about the credential provider for user
/// administration", which is the same kind of reference the license server settings carry
/// and is held to the same rule — the way `DomainNetworkSettings$Subnets` is held to the
/// same existence rule as `Settings$Subnets` ([`super::ec2_fleet::resolve_domain_subnets`]).
/// The member is optional, so a provider that supplies none resolves nothing.
pub async fn resolve_domain_credentials(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    identity_provider: &IdentityProvider,
) -> Result<(), StateError> {
    let IdentityProvider::ActiveDirectoryIdentityProvider(ad) = identity_provider;
    let provider = ad
        .active_directory_settings
        .as_ref()
        .and_then(|settings| settings.domain_credentials_provider.as_ref());
    resolve_credentials_provider(ctx, provider).await
}
