// SPDX-License-Identifier: CC0-1.0

//! WHERE a packaging group's egress is ANNOUNCED: the one value every projection of a
//! group reports as its `DomainName`, and the root of every playback URL beneath it
//! ([`super::playback_url`]).
//!
//! `DomainName` is an address the caller DIALS afterwards — a player, a CDN origin
//! configuration, a health probe — so it has to name a host this emulator answers on.
//! AWS publishes a group's egress on a per-group hostname beneath its own regional
//! domain ([`packaging_group_domain_name`]), and a client on the machine this emulator
//! runs on resolves none of those names: announcing one sends that client off this
//! process entirely. The address announced is therefore the endpoint the creating
//! request reached, in three cases:
//!
//! * a request that reached the EMULATOR'S OWN endpoint comes from a client configured
//!   with that one endpoint and resolving nothing else, so the group is announced there;
//! * a request that addressed some other host on an emulator that IS served on a port is
//!   announced that port — the one address this process answers on — so a group minted
//!   by an in-process call of another service publishes an address its caller can reach
//!   too;
//! * a request that addressed one of AWS's own hosts on an emulator served on no port at
//!   all comes from a client that resolves them, and there is no own address to announce,
//!   so the hostname AWS itself would publish stands.
//!
//! The choice is made ONCE, when the group is created, and persisted as the group's
//! `DomainName`: the create answer, every `DescribePackagingGroup`, every
//! `ListPackagingGroups` element, `UpdatePackagingGroup` and `ConfigureLogs` all project
//! that stored value, so a group is announced at one stable address however it is read.

use super::*;

/// The scheme-and-separator prefix a packaging group's `DomainName` carries.
///
/// `DomainName` is documented as "The fully qualified domain name for Assets in the
/// PackagingGroup", but what this service puts on the wire for it is the egress
/// ORIGIN a player fetches from — scheme included — as the recorded create and
/// describe responses of this service show, and as every `EgressEndpoint$Url` built
/// from it needs. Playback is TLS-only, so the prefix is always this one. It carries
/// the `://` separator, so a caller building a URL beneath the origin (see
/// [`super::playback_url`]) appends a path and never re-adds a scheme.
pub const EGRESS_ORIGIN_PREFIX: &str = "https://";

/// The egress origin a packaging group created by `parts` is ANNOUNCED at, which is what
/// this service stores and reports as that group's `DomainName` — an ORIGIN, not a bare
/// hostname, so it is used as the root of a URL rather than as a host.
///
/// See the module docs for the three cases. `parts` is the request that creates the group
/// and `served_base` the base URL
/// [`::smithy_server_runtime::OperationContext::endpoint`] reports for this emulator.
pub fn announced_egress_origin(
    parts: &http::request::Parts,
    served_base: &str,
    region: &str,
    group_id: &str,
) -> String {
    match reached_emulator_endpoint(parts).or_else(|| served_endpoint_authority(served_base)) {
        Some(authority) => format!("{EGRESS_ORIGIN_PREFIX}{authority}"),
        None => packaging_group_domain_name(group_id, region),
    }
}

/// The EMULATOR'S OWN endpoint this request reached — an address or a loopback name, which
/// is what a client configured with one local endpoint URL sends — and `None` for a request
/// whose host names a service, which is a client that resolves AWS's own hosts.
///
/// The tree's one reading of that question
/// (`service_common::data_plane_addressing::reached_emulator_endpoint`), so this service
/// writes no second rule for which host is the emulator's own; it reads the host off the
/// header or off an absolute-form URI, as both carry it.
fn reached_emulator_endpoint(parts: &http::request::Parts) -> Option<String> {
    ::service_common::data_plane_addressing::reached_emulator_endpoint(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    )
}

/// The `host[:port]` this emulator is SERVED at, or `None` for one served on no port at
/// all — the tree's one reading of that question
/// (`service_common::data_plane_addressing::served_endpoint_authority`).
fn served_endpoint_authority(served_base: &str) -> Option<String> {
    ::service_common::data_plane_addressing::served_endpoint_authority(served_base)
}

/// The egress origin AWS ITSELF publishes a packaging group's assets at: the hostname a
/// client that resolves AWS's own names is told to fetch from, which is what this service
/// announces only when it has no own address to announce in its place (see the module
/// docs).
///
/// Shaped like the real one: [`EGRESS_ORIGIN_PREFIX`] followed by an opaque per-group
/// host label, the `egress` data-plane label, this service's own name, the region and
/// `amazonaws.com`. The label is DERIVED from the group's identity, never minted per
/// call, so the value is stable across reads and distinct per group.
pub fn packaging_group_domain_name(group_id: &str, region: &str) -> String {
    let label = digest_hex(
        &format!("{}/{region}/{group_id}", engine::emulator_account_id()),
        16,
    );
    format!("{EGRESS_ORIGIN_PREFIX}{label}.egress.{ARN_NAMESPACE}.{region}.amazonaws.com")
}
