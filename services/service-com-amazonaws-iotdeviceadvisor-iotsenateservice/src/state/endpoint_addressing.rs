// SPDX-License-Identifier: CC0-1.0

//! WHERE a Device Advisor endpoint is ANNOUNCED: the one value `GetEndpoint` vends and a
//! suite run reports and persists as the host its primary device connects to.
//!
//! `Endpoint` is not a label a client reads — it is the address a device under test then
//! DIALS, so it has to name a host this emulator answers on. AWS vends that address
//! beneath its own regional domain ([`super::endpoint_for`]), and a client on the machine
//! this emulator runs on resolves none of those names: announcing one sends the device off
//! this process entirely, to an address nothing here serves. The address announced is
//! therefore the endpoint the asking request reached, in three cases:
//!
//! * a request that reached the EMULATOR'S OWN endpoint comes from a client configured
//!   with that one endpoint and resolving nothing else, so that is where the device is
//!   sent;
//! * a request that addressed some other host on an emulator that IS served on a port is
//!   announced that port — the one address this process answers on — so a caller reached
//!   through another service is handed an address it can reach too;
//! * a request that addressed one of AWS's own hosts on an emulator served on no port at
//!   all comes from a client that resolves them, and there is no own address to announce
//!   in its place, so the host AWS itself vends stands — the account-level one for a
//!   request that names no device, that device's own one for a request that names one
//!   ([`super::endpoint_for`]).
//!
//! The reading of which host a request addressed, and of which host is the emulator's own,
//! is the tree's single one (`service_common::data_plane_addressing`), so this service
//! writes no second rule for it.
//!
//! The member's modelled length (`Endpoint`, `len 45..75`) is the shape of the hostname AWS
//! vends, which [`super::endpoint_for`] keeps. An address a client has to DIAL cannot be
//! padded to that length, and a host inside the bounds that nothing here answers on is no
//! endpoint at all, so the address the request reached is announced exactly as the client
//! reached it.

use super::{DeviceIdentity, endpoint_for};

/// The Device Advisor endpoint a request is ANNOUNCED, which is what `GetEndpoint` vends
/// and what a suite run stores and reports for its primary device.
///
/// See the module docs for the three cases. `parts` is the request asking for the address,
/// `served_base` the base URL [`::smithy_server_runtime::OperationContext::endpoint`]
/// reports for this emulator, `region` the region the host is asked for and `device` the
/// device the request names, if it names one.
pub fn announced_endpoint(
    parts: &::http::request::Parts,
    served_base: &str,
    region: &str,
    device: &DeviceIdentity<'_>,
) -> String {
    reached_emulator_endpoint(parts)
        .or_else(|| served_endpoint_authority(served_base))
        .unwrap_or_else(|| endpoint_for(region, device))
}

/// The EMULATOR'S OWN endpoint this request reached — an address or a loopback name, which
/// is what a client configured with one local endpoint URL sends — and `None` for a request
/// whose host names a service, which is a client that resolves AWS's own hosts.
///
/// The tree's one reading of that question
/// (`service_common::data_plane_addressing::reached_emulator_endpoint`): it reads the host
/// off the header or off an absolute-form URI, as both carry it.
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
