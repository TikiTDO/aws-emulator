// SPDX-License-Identifier: CC0-1.0

//! The endpoint a registry address this service ANNOUNCES names, and the repository URI
//! built on it.
//!
//! `Repository$repositoryUri` ("The URI for the repository. You can use this URI for
//! container image `push` and `pull` operations") and
//! `AuthorizationData$proxyEndpoint` ("The registry URL to use for this authorization
//! token in a `docker login` command") are not labels a caller reads back: each is an
//! address a container client then DIALS, so the one requirement on it is that THIS
//! emulator answers the registry API there. AWS's own
//! `<accountId>.dkr.ecr.<region>.amazonaws.com` host does not resolve to this emulator on
//! the machine it runs on, so an address naming it sends every push and pull out of the
//! emulator entirely.
//!
//! Two endpoints can satisfy that, and [`announced_registry_endpoint`] picks between them
//! with the tree's one reading of the question
//! (`service_common::data_plane_addressing::reached_emulator_endpoint`):
//!
//! - the endpoint the CALLER reached this emulator on, when that endpoint names no
//!   service — the emulator's own socket, a loopback name, and the registry host itself,
//!   whose first label is an account id. `crate::service`'s registry data plane claims a
//!   `/v2` request exactly when its endpoint names no service, so such a caller is
//!   handed the address it is already talking to and reaches the registry there;
//! - otherwise the address this emulator is served at, because a caller that came in on
//!   some service's endpoint host (an SDK pointed at `api.ecr.<region>.amazonaws.com`) is
//!   handed an address a registry client cannot use: that host routes to a service, so no
//!   registry request addressed to it reaches this plane.
//!
//! The Region a call is signed for names the repository's ARN, which is a label rather
//! than an address; one endpoint serves every Region here.

/// The endpoint every registry address this service announces is built on: the endpoint
/// the caller reached this emulator on when a registry client can use it, and the address
/// this emulator is served at otherwise (see the module docs).
pub fn announced_registry_endpoint(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> String {
    let parts = ctx.parts();
    ::service_common::data_plane_addressing::reached_emulator_endpoint(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    )
    .unwrap_or_else(|| {
        ::service_common::data_plane_addressing::emulator_endpoint_authority(ctx)
    })
}

/// `<registry endpoint>/<name>` — the URI a client pushes to and pulls from, announced at
/// `registry_endpoint` ([`announced_registry_endpoint`]).
///
/// The repository's own name is appended exactly as AWS appends it to its registry host,
/// so a client composes the same tag, digest or registry v2 path on what it was handed.
pub fn repository_uri(registry_endpoint: &str, name: &str) -> String {
    format!("{registry_endpoint}/{name}")
}

/// `https://<registry endpoint>` — the registry URL a `docker login` is pointed at,
/// announced at `registry_endpoint` ([`announced_registry_endpoint`]).
///
/// `AuthorizationData$proxyEndpoint` states the shape — "The Amazon ECR registry URL
/// format is `https://aws_account_id.dkr.ecr.region.amazonaws.com`" — a scheme followed by
/// the registry's endpoint, which here is the endpoint this emulator serves that registry
/// at. The scheme is the one this emulator's endpoint carries.
pub fn registry_login_url(registry_endpoint: &str) -> String {
    format!("https://{registry_endpoint}")
}
