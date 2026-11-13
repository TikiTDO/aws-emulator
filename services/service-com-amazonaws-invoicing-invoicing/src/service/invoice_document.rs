// SPDX-License-Identifier: CC0-1.0

//! Serves an invoice's documents at the pre-signed URLs `GetInvoicePDF` hands out for
//! them.
//!
//! `GetInvoicePDF` "returns a URL to download the invoice document and supplemental
//! documents associated with an invoice", and the location this emulator manages those
//! documents at is this emulator itself: the operation signs a URL addressing the path
//! [`crate::state::InvoiceDocumentTarget::path`] gives the document, carrying the
//! authenticator the store minted for it, so the URL leads back here and verifies, and this
//! route answers a `GET` on that address with the document rendered from the stored invoice.
//!
//! The model binds no operation to those paths — a document is not an API call, it is the
//! artifact the API points at — so the route is claimed through
//! [`smithy_server_runtime::Service::override_operation`] rather than through the generated
//! routing table. It is claimed only for a PRE-SIGNED request ([`is_presigned`]), whose
//! signature and expiry window the framework verifies before the route runs; WHICH document
//! an address may reach is then decided by the store that minted it
//! (`crate::state::...::invoice_document`), against the authenticator the address carries
//! and the account whose invoices that store holds. So a document's only address is the one
//! `GetInvoicePDF` handed out for it: an address a caller assembled, or one minted for
//! another document, reaches nothing.

use std::sync::Arc;

use smithy_server_runtime::{http, BoxFuture, ErasedOperation, OperationContext, Service};

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;

/// The method a document is fetched on.
const DOWNLOAD_METHOD: &str = "GET";

/// The media type the one answer of this route that is not a document is served as.
const PLAIN_MEDIA_TYPE: &str = "text/plain; charset=utf-8";

/// What this route answers a URL that names no document this emulator holds: a pre-signed
/// download URL is fetched by a plain HTTP client rather than by an AWS SDK, so the answer
/// is the status itself with a one-line reason, and no API error envelope.
const NO_DOCUMENT: &str = "no invoice document at this location\n";

/// Serves a download of one invoice document.
///
/// Owns the shared engine, as an operation does, because the document it serves is
/// rendered from the stored invoice: nothing is held beside the registry, so a download and
/// a read of the same invoice cannot disagree about what the document says.
pub struct InvoiceDocumentRoute<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> InvoiceDocumentRoute<Eng> {
    /// Build the route from the shared engine, as the service's operations are built.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The document `req` asks for and the authenticator its address carries, or `None`
    /// when the request is not a download this route serves.
    ///
    /// Two things make it one: the path, read the way the signer wrote it — its segments
    /// are percent-decoded and its empty ones kept, so an invoice id that had to be encoded
    /// in the URL, or that holds the separator a path is cut on, names the same invoice here
    /// as it does in the registry — and the SigV4 query authentication of a PRE-SIGNED
    /// request ([`is_presigned`]). Whether the address is one this emulator MINTED is not
    /// decided here but where the registry is: the authenticator read off the query travels
    /// with the target for the store to check (see [`Self::stored_document`]), so a request
    /// carrying none is answered exactly as one naming a document that does not exist.
    pub fn claims(&self, req: &RtRequest) -> Option<(crate::state::InvoiceDocumentTarget, String)> {
        if req.method() != DOWNLOAD_METHOD || !is_presigned(req.uri()) {
            return None;
        }
        let target = crate::state::parse_invoice_document_path(&decoded_segments(req.uri()))?;
        let token =
            query_field(req.uri(), crate::state::DOCUMENT_TOKEN_FIELD).unwrap_or_default();
        Some((target, token))
    }

    /// The document `target` names, as `(media type, bytes)`, or `None` when this store
    /// serves no such document to this address — which is where the whole decision is made:
    /// [`crate::state::ComAmazonawsInvoicingInvoicingState::invoice_document`] checks
    /// `token`, the authenticator the address carries, against the one it minted for exactly
    /// this document, and the account the address names against the account whose invoices
    /// it holds, before it reads the registry at all.
    fn stored_document(
        &self,
        target: &crate::state::InvoiceDocumentTarget,
        token: &str,
    ) -> Option<(&'static str, Vec<u8>)> {
        let state = crate::state::service_state(&*self.engine)?;
        let store = state.lock();
        let document = store.invoice_document(target, token)?;
        Some((crate::state::INVOICE_DOCUMENT_MEDIA_TYPE, document))
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for InvoiceDocumentRoute<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        // The context is not consulted, because a PRE-SIGNED fetch carries no caller
        // identity for it to hold: what the framework verified before this route ran is a
        // signature over this method, this path and this query, not who sent it. WHICH
        // document an address may reach is therefore decided by the store that minted the
        // address, against the authenticator the address carries
        // (`crate::state::...::invoice_document`) — and nothing this route does waits on
        // anything, so the document is rendered the moment it is asked for.
        let answer = match self.claims(req) {
            Some((target, token)) => match self.stored_document(&target, &token) {
                Some((media_type, body)) => document_response(200, media_type, body),
                // The address is not one this store minted for a document it holds: it
                // carries no authenticator of this store's, or one for another document, or
                // it names another account's invoices, an invoice the registry does not
                // hold, or a supplemental document that invoice does not carry. Each is
                // answered the same way: there is nothing at this location.
                None => document_response(404, PLAIN_MEDIA_TYPE, NO_DOCUMENT.as_bytes().to_vec()),
            },
            // Unreachable through an `override_operation` that hands this route only the
            // requests it claims; answered as the framework answers a request that named
            // no operation rather than guessed at.
            None => svc.definition().routing_error_response(req),
        };
        Box::pin(async move { answer })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` keeps the framework's own decision for this service: the URL this route
        // serves is SigV4 query-signed, and nothing here may weaken that.
        None
    }
}

/// One download response: the bytes, their media type and their length.
fn document_response(status: u16, media_type: &str, body: Vec<u8>) -> RtResponse {
    let length = body.len();
    let mut response = RtResponse::new(download_status(status), SdkBody::from(body));
    response
        .headers_mut()
        .insert("content-type", media_type.to_string());
    response
        .headers_mut()
        .insert("content-length", length.to_string());
    response
}

/// A status this route answers with. Every call site passes a constant, so the conversion
/// cannot fail.
fn download_status(code: u16) -> StatusCode {
    StatusCode::try_from(code).expect("an invoice document download answers constant statuses")
}

/// Whether `uri` carries the SigV4 query authentication of a pre-signed request: the
/// credential scope and the signature [`smithy_server_runtime::UrlSigner`] puts on a URL it
/// signs.
///
/// Presence is all this reads, and all it has to: the framework's own request
/// authentication is what VERIFIES such a request before any route runs — the signature
/// over the method, the path and the query, and the `X-Amz-Expires` window the signer
/// wrote, a URL past which is refused. So neither the signature nor the lifetime
/// `GetInvoicePDF` signs for is re-checked here. What this test adds is that nothing which
/// is not a pre-signed request is served from these paths at all, so a document's only
/// address is a URL this service signed for it.
fn is_presigned(uri: &str) -> bool {
    const PRESIGNED_FIELDS: [&str; 2] = ["X-Amz-Credential", "X-Amz-Signature"];
    let query = uri.split_once('?').map(|(_, query)| query).unwrap_or("");
    PRESIGNED_FIELDS.iter().all(|field| {
        query.split('&').any(|pair| {
            pair.split_once('=')
                .is_some_and(|(name, _)| name.eq_ignore_ascii_case(field))
        })
    })
}

/// The path segments of `uri`, percent-decoded, with the EMPTY ones kept — the same view
/// of a path the signer built the URL from.
///
/// An empty segment is kept because an `InvoiceId` may itself hold the separator a path is
/// cut on, and dropping empties would make two different ids one address: an id ending in a
/// separator, or holding two in a row, would resolve to the invoice whose id has neither,
/// and a URL minted for one invoice would serve the other's document.
fn decoded_segments(uri: &str) -> Vec<String> {
    uri.parse::<http::Uri>()
        .map(|parsed| {
            parsed
                .path()
                .strip_prefix('/')
                .unwrap_or_default()
                .split('/')
                .map(smithy_server_runtime::percent_decode)
                .collect()
        })
        .unwrap_or_default()
}

/// The value of one query field of `uri`, percent-decoded, or `None` when the request
/// carries no such field.
fn query_field(uri: &str, field: &str) -> Option<String> {
    let query = uri.split_once('?').map(|(_, query)| query)?;
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == field).then(|| smithy_server_runtime::percent_decode(value))
    })
}
