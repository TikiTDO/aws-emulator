// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Smithy server runtime framework.
//!
//! Provides the core traits and types for building in-process service
//! emulators driven by proc-macro code generation from Smithy models.

// Lets `#[smithy_service]`-generated code refer to items in this crate via
// the absolute path `::smithy_server_runtime::*` even when the macro is
// expanded inside this crate itself.
extern crate self as smithy_server_runtime;

// Re-export third-party crates the macro expansion references, so consumer
// crates don't have to declare them as direct dependencies.
pub use axum;
pub use http;
pub use http_body_util;

pub mod endpoint;
pub mod errors;
pub mod extract;
pub mod multi_service_router;
pub mod operation;
pub mod pre_dispatch;
pub mod router;
pub mod service;
pub mod signed_url;
pub mod transport;
pub mod virtual_host;
pub mod wire;
pub mod wire_protocol;

pub use extract::{Akid, FromSmithyParts, RequestTelemetry};
// The operation contract: one modeled operation, its typed-call step, and the
// request-metadata context the handler receives.
pub use operation::{
    run_operation_http, CallbackRecursionDepth, EmbeddedEmulator, Operation, OperationContext,
    CALLBACK_RECURSION_LIMIT,
};
// The service contract, the type-erased operation the framework runs, the
// model-derived operation routing table one service holds, and the vended
// service identity. They live in this crate because they name both
// `ServerProtocol` and `OperationContext`.
pub use service::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationRoutingTable, ServerHttpProtocol,
    Service, ServiceIdentity,
};
// The multi-service router: the emulator core that mounts N services and
// dispatches each request to the one service that claims it.
pub use multi_service_router::MultiServiceRouter;
// The TLS root a host trusts this process's listeners by, and the URLs a handler hands out that
// address this emulator.
pub use emulator_server::TlsCertificateAuthority;
pub use endpoint::{EndpointUrl, UNSERVED_URL_BASE};
pub use signed_url::{SignError, SignedUrl, SignedUrlRequest, UrlSigner, MAX_SIGNED_URL_EXPIRY};
// The virtual-hosted addresses a service's endpoint rules declare, and the host split.
pub use virtual_host::{VirtualHost, VirtualHostAddressing};
// Per-request protocol selection. The `#[smithy_service]` expansion builds a
// `DeclaredProtocols` from the model's protocol traits and names the other
// types.
pub use wire_protocol::{DeclaredProtocol, DeclaredProtocols, WireProtocol, WireProtocolKind};
// Re-exported because a `RefusalRenderer` cannot be written without naming them:
// a `Refusal` carries `Violation`s, and deciding how to word one means reading
// its `kind`. Requiring a second crate just to branch on a refusal would make the
// seam unusable from the crate that owns the service.
pub use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
// Pre-dispatch protocol-error rendering, called by the `#[smithy_service]`
// expansion instead of being emitted into every service crate.
pub use pre_dispatch::{
    protocol_error_response, protocol_error_response_with, refusal_of, render_refusal,
    PreDispatchProtocol, Refusal, RefusalRenderer, Refused, UNRECOGNISED_PROTOCOL_BODY,
};

// Re-export the schema-aware traits so generated code (the
// `#[smithy_service]` expansion) can refer to them under stable paths.
// `xml_escape` is re-exported rather than reimplemented. Two hand-rolled XML
// escapers cannot be kept in lockstep: a fix applied to only one leaves either the
// restXml pre-dispatch envelope or `query_error_body` emitting documents no SDK can
// parse. The implementation lives in the XML codec module of the crate below this
// one, which is where XML escaping belongs; this alias keeps `crate::xml_escape`
// working for `pre_dispatch` and for any generated code that names it.
pub use aws_smithy_schema_server::xml_escape;

// Re-exported so the `#[smithy_service]` expansion can name it under the stable
// `::smithy_server_runtime::` path that all generated code uses. It lives in
// `emulator-server` (this crate's existing dependency); the additive
// new-surface `Service::can_handle` emitted by the macro resolves the request's
// route key through it. Re-exporting here means a consumer crate that expands
// the macro needs no direct dependency on `emulator-server`.
pub use emulator_server::resolve_route_key;
pub use aws_smithy_schema_server::{DeserializableStruct, ServerProtocol};
pub use errors::OperationError;

pub use smithy_macros::smithy_service;

/// Percent-decode a URL-encoded string (e.g., path segments, query values).
pub fn percent_decode(input: &str) -> String {
    let mut out = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_string())
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
