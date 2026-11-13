// SPDX-License-Identifier: CC0-1.0

//! The ONE construction of the in-process AWS Organizations client an emulated service
//! drives, so no two services configure that hop differently — the sibling of
//! [`crate::iam_client`] for the organization's own records.
//!
//! A service that acts on an organization a client named — attaching the service control
//! policy a budget action's `Definition` names to each of its targets, resolving the
//! organization a quota or a configuration applies across — reaches AWS Organizations the way
//! any emulated service reaches another: the AWS SDK driven over the emulator's own
//! in-process transport, through the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]). The transport is what makes the
//! call in-process: no socket is opened, no name is resolved, and nothing outside this process
//! is reachable from it. The endpoint URL below is addressing for that transport, not a host
//! this process connects to, and it is built from the Region alone — no part of a caller's
//! request is a destination.
//!
//! The copy that predates this module — `organizations_client` in
//! `service-com-amazonaws-servicequotas-servicequotasv20190624/src/state/organization.rs` — is
//! to be MIGRATED onto it in the change that owns that service. Nothing new should build the
//! client by hand.

use smithy_server_runtime::EmbeddedEmulator;

/// The AWS Organizations client a service asks about, or acts on, an organization through,
/// addressed to `region`.
///
/// AWS Organizations is partition-global: it holds one organization per management account
/// whichever Region it is asked in, so `region` is the asking service's own constant rather
/// than anything read off a request. The transport is in-process, so there is nothing to
/// retry, time out or watch for a stalled stream — each of those would need a sleep
/// implementation this client is not given — and the credentials the callback hands over are
/// used directly rather than cached. Those credentials are the emulator's own: the caller's
/// `Authorization` header is never forwarded.
#[must_use]
pub fn organizations_client(
    callback: &EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_organizations::Client {
    let config = ::aws_sdk_organizations::Config::builder()
        .behavior_version(::aws_sdk_organizations::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_organizations::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!("https://organizations.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_organizations::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_organizations::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_organizations::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_organizations::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_organizations::Client::from_conf(config)
}
