// SPDX-License-Identifier: CC0-1.0

//! The ONE construction of the in-process AWS Systems Manager client an emulated service
//! drives, so no two services configure that hop differently — the sibling of
//! [`crate::iam_client`] for the account's documents, nodes and parameters.
//!
//! A service that runs a document, resolves a parameter or reaches a managed node on a
//! client's behalf — the runbook a budget action's `Definition` names, the remediation a
//! configuration rule names — reaches Systems Manager the way any emulated service reaches
//! another: the AWS SDK driven over the emulator's own in-process transport, through the
//! operation callback ([`smithy_server_runtime::OperationContext::callback`]). The transport
//! is what makes the call in-process: no socket is opened, no name is resolved, and nothing
//! outside this process is reachable from it. The endpoint URL below is addressing for that
//! transport, not a host this process connects to, and it is built from the Region alone — so
//! a Region a request carries selects an endpoint of THIS emulator and can never be an
//! address. A caller-supplied Region reaches it only through [`crate::arn::region_label`],
//! which admits nothing but a Region label.
//!
//! The copy that predates this module — `ssm_client` in
//! `service-com-amazonaws-configservice-starlingdoveservice/src/state/remediation_target.rs` —
//! is to be MIGRATED onto it in the change that owns that service. Nothing new should build
//! the client by hand.

use smithy_server_runtime::EmbeddedEmulator;

/// The Systems Manager client a service drives, addressed to `region`.
///
/// Systems Manager holds its documents, nodes and parameters per Region, so `region` is the
/// Region of the work being asked for. The transport is in-process, so there is nothing to
/// retry, time out or watch for a stalled stream — each of those would need a sleep
/// implementation this client is not given — and the credentials the callback hands over are
/// used directly rather than cached. Those credentials are the emulator's own: the caller's
/// `Authorization` header is never forwarded.
#[must_use]
pub fn ssm_client(callback: &EmbeddedEmulator, region: &str) -> ::aws_sdk_ssm::Client {
    let config = ::aws_sdk_ssm::Config::builder()
        .behavior_version(::aws_sdk_ssm::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_ssm::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ssm.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_ssm::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ssm::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ssm::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ssm::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_ssm::Client::from_conf(config)
}
