// SPDX-License-Identifier: CC0-1.0

//! How a client ADDRESSES this service's data plane, and which address a published URL
//! announces.
//!
//! MediaPackage hands a client two kinds of URL to DIAL later: `IngestEndpoints[].Url` on
//! a channel, which "the source stream should be sent" to, and the `Url` of every manifest
//! an origin endpoint publishes, which a player fetches. AWS serves both on per-group
//! hostnames under its own regional domain ([`super::egress_domain`]), whose first label is
//! opaque — and a client on the machine this emulator runs on resolves none of those names.
//! A URL this emulator announces therefore has to lead back to the endpoint the caller is
//! already talking to, with everything the hostname carried moved into the PATH:
//!
//! * a request that reached the EMULATOR'S OWN endpoint comes from a client that resolves
//!   nothing else, so the plane is announced on that same endpoint
//!   ([`local_data_plane_url`]);
//! * a resource created through any other request on an emulator that IS served on a port
//!   is announced on that port, the one address this process answers on, so a group minted
//!   by an in-process call of another service publishes an address its caller can reach too;
//! * a request that addressed one of AWS's own hosts on an emulator served on no port at
//!   all comes from a client that resolves them, so the plane is announced on the hostname
//!   [`super::egress_domain`] builds — which this plane also claims, host-style.
//!
//! The choice is made ONCE, when the channel group is created, and stored as the group's
//! `EgressDomain`: every URL beneath a group — its channels' ingest endpoints and its
//! endpoints' manifests — is rooted on that one value, so a group's whole data plane is
//! announced at one address and a later read reports what the create did.
//!
//! [`addressed_data_plane`] is the inverse, and the only reader of either addressing: it
//! answers which plane a request addresses, in which Region, at which of AWS's own
//! data-plane paths — from the published HOSTNAME or from the local PATH, whichever the
//! request carried.

use super::*;

/// The segment that names THIS service's data plane behind the reserved path namespace
/// every data plane this emulator serves on its own endpoint sits under
/// (`service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`): the service's own
/// signing name, so a path under that namespace naming another plane is never read as this
/// one's.
const DATA_PLANE_SEGMENT: &str = SIGNING_NAME;

/// The first path segment of AWS's own EGRESS path (`/out/v1/<group>/<channel>/…`), which
/// is what names that half of the plane once the hostname no longer does.
const EGRESS_PATH_SEGMENT: &str = "out";

/// The first path segment of AWS's own INGEST path (`/in/v1/<group>/<index>/<channel>/…`).
const INGEST_PATH_SEGMENT: &str = "in";

/// The domain every URL beneath one channel group is announced on, resolved when the group
/// is created and stored as its `EgressDomain`.
///
/// See the module docs for the three cases. `parts` is the request that created the group
/// and `served_base` the base URL [`::smithy_server_runtime::OperationContext::endpoint`]
/// reports for this emulator.
pub fn announced_data_plane_domain(
    parts: &::http::request::Parts,
    served_base: &str,
    region: &str,
    group: &str,
) -> String {
    reached_emulator_endpoint(parts)
        .or_else(|| served_endpoint_authority(served_base))
        .unwrap_or_else(|| egress_domain(region, group))
}

/// The EMULATOR'S OWN endpoint this request reached — an address or a loopback name, which
/// is what a client configured with one local endpoint URL sends — and `None` for a request
/// whose host names a service, which is a client that resolves AWS's own hosts.
///
/// The tree's one reading of that question
/// (`service_common::data_plane_addressing::reached_emulator_endpoint`), so this service
/// writes no second rule for which host is the emulator's own; it reads the host off the
/// header or off an absolute-form URI, as both carry it.
fn reached_emulator_endpoint(parts: &::http::request::Parts) -> Option<String> {
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

/// One URL this service publishes: `domain` plus `path`, which is AWS's own data-plane
/// path (`/out/v1/…` or `/in/v1/…`).
///
/// A `domain` that is one of the hostnames this service publishes
/// ([`data_plane_of_host`]) carries the Region and the plane itself, so AWS's path follows
/// it unchanged. Any other domain is one of the emulator's own addresses, which carries
/// neither, so the path is the local one ([`local_data_plane_url`]).
pub fn data_plane_url(domain: &str, region: &str, path: &str) -> String {
    match data_plane_of_host(domain) {
        Some(_) => format!("https://{domain}{path}"),
        None => local_data_plane_url(domain, region, path),
    }
}

/// One URL on the EMULATOR'S OWN endpoint: `https://<authority>/_aws/<service>/<region>`
/// then AWS's own data-plane path.
///
/// It carries what the published hostname carries, with the Region moved out of the host
/// and into the path, behind the reserved namespace segment and the segment that names this
/// service's plane. The plane itself (`out` or `in`) is still named by AWS's own first path
/// segment, so one path shape serves both addressings.
pub fn local_data_plane_url(authority: &str, region: &str, path: &str) -> String {
    format!(
        "https://{authority}{}{path}",
        local_data_plane_prefix(region)
    )
}

/// The path prefix a local data-plane URL carries in front of AWS's own path.
fn local_data_plane_prefix(region: &str) -> String {
    format!(
        "/{}/{DATA_PLANE_SEGMENT}/{region}",
        ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE
    )
}

/// What a data-plane request addresses: which half of the plane, the Region whose
/// resources it reaches, and AWS's own data-plane path beneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressedDataPlane {
    pub plane: DataPlane,
    pub region: String,
    /// The path as AWS's own addressing writes it (`/out/v1/…`, `/in/v1/…`), whichever
    /// addressing the request used — so the path readers ([`parse_egress_path`],
    /// [`parse_ingest_path`]) have one form to read.
    pub path: String,
}

/// What a request addresses on this service's data plane, or `None` for a request that
/// addresses none of it.
///
/// The inverse of [`data_plane_url`], and the one reader of both addressings: the local
/// PATH first, because it names the plane and the Region explicitly, then the published
/// HOSTNAME, which names both in its labels. A request carrying neither is not one this
/// service published a URL for.
pub fn addressed_data_plane(host: Option<&str>, path: &str) -> Option<AddressedDataPlane> {
    if let Some(addressed) = local_addressed_data_plane(path) {
        return Some(addressed);
    }
    let host = host?;
    Some(AddressedDataPlane {
        plane: data_plane_of_host(host)?,
        region: data_plane_region(host)?,
        path: path.to_string(),
    })
}

/// What a path on the EMULATOR'S OWN endpoint addresses, or `None` when it is not one
/// [`local_data_plane_url`] mints there.
fn local_addressed_data_plane(path: &str) -> Option<AddressedDataPlane> {
    use ::service_common::data_plane_addressing::{first_segment, EMULATOR_PATH_NAMESPACE};
    let (namespace, rest) = first_segment(path)?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (segment, rest) = first_segment(rest)?;
    if segment != DATA_PLANE_SEGMENT {
        return None;
    }
    let (region, rest) = first_segment(rest)?;
    if region.is_empty() {
        return None;
    }
    Some(AddressedDataPlane {
        plane: plane_of_path(rest)?,
        region: region.to_string(),
        path: rest.to_string(),
    })
}

/// Which half of the plane one of AWS's own data-plane paths names, or `None` for a path
/// that names neither.
fn plane_of_path(path: &str) -> Option<DataPlane> {
    match ::service_common::data_plane_addressing::first_segment(path)?.0 {
        EGRESS_PATH_SEGMENT => Some(DataPlane::Egress),
        INGEST_PATH_SEGMENT => Some(DataPlane::Ingest),
        _ => None,
    }
}
