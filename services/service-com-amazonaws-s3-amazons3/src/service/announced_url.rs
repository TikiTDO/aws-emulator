// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of the absolute URL this service ANNOUNCES for a resource — the address a
//! caller is handed to address that resource with afterwards.
//!
//! Three answers of this service carry such a URL: the `Location` header of a created bucket
//! (`CreateBucketOutput$Location`, "the global endpoint URL" of the new bucket), the
//! `Location` element of an assembled object (`CompleteMultipartUploadOutput$Location`, "the
//! URI that identifies the newly created object") and the `Location` of a browser-form
//! upload's `PostResponse`. Each is an address the client DIALS next — SDK redirect handling,
//! CLI tooling, or any code that hands the completed object's URI on.
//!
//! Such a URL has to lead back to the endpoint this emulator is serving the request on.
//! Amazon's own value names a host under `amazonaws.com`, which resolves away from this
//! process: a client that follows it leaves the emulator and talks to public AWS, where the
//! bucket and the object do not exist, or — in a sandbox with no egress — fails to connect at
//! all. So the host, and with it the scheme and the port, is the emulator's own
//! ([`announced_base`]), and only the PATH beneath it is what the request named.
//!
//! Beneath that host the bucket is addressed PATH-STYLE, whichever style the request itself
//! used: "both the virtual-hosted-style requests and the path-style requests are supported"
//! (`GetObject`'s own documentation), and a bucket's name is a label of a published AWS
//! hostname rather than of the one endpoint this emulator answers on — so the path-style
//! address is the form that is served wherever the endpoint is reached.

use ::smithy_server_runtime::{http, OperationContext};

/// The base URL — `scheme://host[:port]`, no path — every URL this service announces is
/// rooted on: the endpoint serving THIS request.
///
/// `ctx.endpoint()` is "the base URL a later request addresses… Callers send normally signed
/// requests to it", so it is the authoritative answer and the one this falls back on. Ahead
/// of it comes the emulator's own endpoint the request REALLY reached, read through the
/// tree's one reading of that question
/// (`service_common::data_plane_addressing::reached_emulator_endpoint`): a client configured
/// with one local endpoint URL resolves nothing else, so it is handed back the very address
/// it is already talking to, port included. That reading answers `None` for a host that names
/// a service — which every published S3 hostname does, the virtual-hosted address of a bucket
/// included — and such a request is answered the address this process is served at.
pub fn announced_base(ctx: &OperationContext<'_>) -> String {
    let endpoint = ctx.endpoint();
    // Without the trailing slash, which would double the separator in front of the path a
    // caller appends below.
    let base = endpoint.as_str().trim_end_matches('/');
    // The base's own authority, read through the same shared reading rather than a second
    // split of a URL in this crate; the scheme is whatever precedes it. `EndpointUrl` admits
    // only an absolute `https` URL, so the default stands in for nothing the core reports.
    let served = ::service_common::data_plane_addressing::url_authority(base);
    let scheme = match base.strip_suffix(served) {
        Some(scheme) if !scheme.is_empty() => scheme,
        _ => "https://",
    };
    let authority = reached_emulator_endpoint(ctx.parts())
        .or_else(|| served_endpoint_authority(base))
        .unwrap_or_else(|| served.to_string());
    format!("{scheme}{authority}")
}

/// The absolute URL this emulator announces for the bucket named `bucket`, or — when
/// `key_path` names one — for the object at that key in it.
///
/// `key_path` is the key as a URI PATH, percent-encoded per segment by the caller
/// (`crate::state::url_encoded_key`), so a key prefix's `/` separators stay path separators;
/// the empty string addresses the bucket itself, which is the URL a create announces.
pub fn announced_bucket_url(ctx: &OperationContext<'_>, bucket: &str, key_path: &str) -> String {
    let base = announced_base(ctx);
    match key_path.is_empty() {
        // `https://<endpoint>/<bucket>/` addresses the BUCKET itself, which is what a create
        // announces: the trailing slash is what makes it that address rather than a key.
        true => format!("{base}/{bucket}/"),
        false => format!("{base}/{bucket}/{key_path}"),
    }
}

/// The EMULATOR'S OWN endpoint this request reached — an address or a loopback name, which is
/// what a client configured with one local endpoint URL sends — and `None` for a request
/// whose host names a service, which is a client that resolves AWS's own hosts.
///
/// The tree's one reading of that question
/// (`service_common::data_plane_addressing::reached_emulator_endpoint`), so this service
/// writes no second rule for which host is the emulator's own.
fn reached_emulator_endpoint(parts: &http::request::Parts) -> Option<String> {
    ::service_common::data_plane_addressing::reached_emulator_endpoint(
        &parts.uri.to_string(),
        super::request_host(parts),
    )
}

/// The `host[:port]` this emulator is SERVED at, or `None` for one served on no port at all —
/// the tree's one reading of that question
/// (`service_common::data_plane_addressing::served_endpoint_authority`).
fn served_endpoint_authority(served_base: &str) -> Option<String> {
    ::service_common::data_plane_addressing::served_endpoint_authority(served_base)
}
