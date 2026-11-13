// SPDX-License-Identifier: CC0-1.0

//! Large blob reads use the same selected exchange in HTTP and the generated
//! embedded connector. No payload is reconstructed in a synchronous RtResponse.
use super::*;
use crate::state::payloads;
use bytes::Bytes;
use emulator_server::transport::{ExchangeContext, ExchangeFuture, TransportBody};
use service_common::payload_store::Sha256Digest;
use smithy_server_runtime::transport::{PayloadHandler, PayloadOperation, PayloadReceiveError, PayloadRequest, PayloadRoute};

#[derive(Clone)]
enum Target {
    Blob { repository: String, digest: String },
    Download(crate::state::LayerDownloadTarget),
}
struct ReadExchange<Eng: Send + Sync + 'static> { engine: Arc<Eng>, target: Target }

pub(super) fn route<Eng: Send + Sync + 'static>(engine: Arc<Eng>, parts: &http::request::Parts) -> Option<PayloadRoute> {
    let target = if registry_v2::claims_registry_v2(parts) {
        match registry_v2::registry_v2_target(parts.uri.path())? {
            registry_v2::RegistryV2Target::Blob { repository_name, digest } => Target::Blob { repository: repository_name, digest },
            _ => return None,
        }
    } else {
        Target::Download(crate::state::parse_layer_download_path(parts.uri.path())?)
    };
    Some(PayloadRoute { operation: PayloadOperation::Native("EcrPayloadRead"), handler: Arc::new(ReadExchange { engine, target }) })
}

pub(super) fn header_request(parts: &http::request::Parts) -> Result<RtRequest, http::Response<Bytes>> {
    let mut request = http::Request::new(SdkBody::empty());
    *request.method_mut() = parts.method.clone();
    *request.uri_mut() = parts.uri.clone();
    *request.version_mut() = parts.version;
    *request.headers_mut() = parts.headers.clone();
    RtRequest::try_from(request).map_err(|_| error(400, "invalid request headers"))
}
fn error(code: u16, text: &'static str) -> http::Response<Bytes> {
    let body = Bytes::from(serde_json::json!({"errors":[{"code":"UNKNOWN", "message":text}]}).to_string());
    let mut response = http::Response::new(body);
    *response.status_mut() = http::StatusCode::from_u16(code).unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR);
    response.headers_mut().insert("content-type", http::HeaderValue::from_static("application/json"));
    response.headers_mut().insert("docker-distribution-api-version", http::HeaderValue::from_static("registry/2.0"));
    response
}
pub(super) fn small(response: RtResponse) -> http::Response<Bytes> {
    // Existing auth errors are bounded service-owned documents, never payloads.
    let Some(bytes) = response.body().bytes() else { return error(500, "unexpected non-control response body"); };
    let mut result = http::Response::new(Bytes::copy_from_slice(bytes));
    *result.status_mut() = http::StatusCode::from_u16(response.status().as_u16()).unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR);
    for (key, value) in response.headers().iter() {
        if let (Ok(key), Ok(value)) = (http::HeaderName::from_bytes(key.as_bytes()), http::HeaderValue::from_str(value)) {
            result.headers_mut().append(key, value);
        }
    }
    result
}

impl<Eng: Send + Sync + 'static> ReadExchange<Eng> {
    /// Synchronous transport preflight cannot consult repository projections:
    /// a pending committed create/delete is reconciled only by the async gate.
    fn preflight(&self, parts: &http::request::Parts) -> Result<(), http::Response<Bytes>> {
        if !matches!(parts.method, http::Method::GET | http::Method::HEAD) { return Err(error(405, "read method required")); }
        let request = header_request(parts)?;
        let now = std::time::SystemTime::now();
        match &self.target {
            Target::Blob { .. } => {
                let password = registry_v2::presented_password(&request).ok_or_else(|| small(registry_v2::unauthorized(&request)))?;
                let state = crate::state::service_state(&*self.engine).ok_or_else(|| error(500, "repository store unavailable"))?;
                if state.lock().honoured_registry_credential(&password, now).is_none() {
                    return Err(small(registry_v2::unauthorized(&request)));
                }
            }
            Target::Download(_) => match crate::state::layer_download_signature(request_authority(&request), request_target(request.uri()), now) {
                crate::state::LayerDownloadSignature::Honoured => {},
                crate::state::LayerDownloadSignature::Absent => return Err(small(download_error(403, "AccessDenied", DOWNLOAD_UNSIGNED_MESSAGE))),
                crate::state::LayerDownloadSignature::Expired => return Err(small(download_error(403, "AccessDenied", DOWNLOAD_EXPIRED_MESSAGE))),
                crate::state::LayerDownloadSignature::Invalid => return Err(small(download_error(403, "SignatureDoesNotMatch", SIGNATURE_MISMATCH_MESSAGE))),
            },
        }
        Ok(())
    }

    fn resolve(&self, parts: &http::request::Parts) -> Result<(payloads::RepositoryScope, Sha256Digest), http::Response<Bytes>> {
        if !matches!(parts.method, http::Method::GET | http::Method::HEAD) { return Err(error(405, "read method required")); }
        let request = header_request(parts)?;
        let now = std::time::SystemTime::now();
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| error(500, "repository store unavailable"))?;
        let (repository, digest) = match &self.target {
            Target::Blob { repository, digest } => {
                let password = registry_v2::presented_password(&request)
                    .ok_or_else(|| small(registry_v2::unauthorized(&request)))?;
                let mut state = state.lock();
                state.reclaim_expired_registry_credentials(now);
                if state.honoured_registry_credential(&password, now).is_none() {
                    return Err(small(registry_v2::unauthorized(&request)));
                }
                (repository.clone(), digest.clone())
            }
            Target::Download(target) => {
                match crate::state::layer_download_signature(request_authority(&request), request_target(request.uri()), now) {
                    crate::state::LayerDownloadSignature::Honoured => {},
                    crate::state::LayerDownloadSignature::Absent => return Err(small(download_error(403, "AccessDenied", DOWNLOAD_UNSIGNED_MESSAGE))),
                    crate::state::LayerDownloadSignature::Expired => return Err(small(download_error(403, "AccessDenied", DOWNLOAD_EXPIRED_MESSAGE))),
                    crate::state::LayerDownloadSignature::Invalid => return Err(small(download_error(403, "SignatureDoesNotMatch", SIGNATURE_MISMATCH_MESSAGE))),
                }
                let state = state.lock();
                let grant = state.layer_download_grant(&target.token)
                    .ok_or_else(|| small(download_error(404, "NoSuchKey", DOWNLOAD_NOT_FOUND_MESSAGE)))?;
                if grant.has_expired(now) { return Err(small(download_error(403, "AccessDenied", DOWNLOAD_EXPIRED_MESSAGE))); }
                if grant.layer_digest != target.layer_digest { return Err(small(download_error(404, "NoSuchKey", DOWNLOAD_NOT_FOUND_MESSAGE))); }
                (grant.repository_name.clone(), grant.layer_digest.clone())
            }
        };
        let owner = {
            let state = state.lock();
            let repository = state.repository(&repository).map_err(|_| error(404, "repository not found"))?;
            payloads::repository_owner(repository).map_err(|_| error(500, "invalid repository incarnation"))?
        };
        Ok((owner, Sha256Digest::parse(&digest).map_err(|_| error(404, "blob not found"))?))
    }
}
impl<Eng: Send + Sync + 'static> PayloadHandler for ReadExchange<Eng> {
    fn authorize(&self, parts: &http::request::Parts) -> Result<(), http::Response<Bytes>> { self.preflight(parts) }
    fn refuse(&self, _: PayloadReceiveError) -> http::Response<Bytes> { error(400, "invalid read request entity") }
    fn run(self: Arc<Self>, request: PayloadRequest, context: ExchangeContext) -> ExchangeFuture {
        Box::pin(async move {
            let head = request.parts().method == http::Method::HEAD;
            let mut observed = http::Request::new(());
            *observed.method_mut() = request.parts().method.clone();
            *observed.uri_mut() = request.parts().uri.clone();
            *observed.headers_mut() = request.parts().headers.clone();
            let (parts, _) = observed.into_parts();
            let mut empty = payloads::EmptyEntity;
            if let Err(e) = request.receive_into(&mut empty, &context).await { return self.refuse(e).map(TransportBody::from); }
            let mutation = match payloads::mutation_guard(&*self.engine).await {
                Ok(v) => v, Err(_) => return error(500, "repository admission unavailable").map(TransportBody::from),
            };
            // Re-read current grant/expiry/incarnation after verified EOF, while
            // repository deletion cannot pass the service mutation boundary.
            let (owner, digest) = match self.resolve(&parts) { Ok(v) => v, Err(e) => return e.map(TransportBody::from) };
            let store = match payloads::bound_store(&*self.engine) { Ok(v) => v, Err(_) => return error(500, "payload store unavailable").map(TransportBody::from) };
            let layer = match payloads::stored_layer(&store, &owner, digest).await {
                Ok(Some(v)) => v,
                Ok(None) => return error(404, "blob not found").map(TransportBody::from),
                Err(_) => return error(500, "committed blob integrity failure").map(TransportBody::from),
            };
            let total = layer.payload.len();
            let range = if head { None } else {
                let supplied = parts.headers.get_all("range");
                let mut values = supplied.iter();
                let first = values.next();
                let parsed = if values.next().is_some() { Err(()) } else {
                    match first {
                        None => Ok(None),
                        Some(value) => value.to_str().map_err(|_| ()).and_then(|v| parse_range(v, total)).map(Some),
                    }
                };
                match parsed {
                    Ok(range) => range,
                    Err(()) => {
                        let mut refused = error(416, "unsatisfiable blob range");
                        refused.headers_mut().insert("content-range", http::HeaderValue::from_str(&format!("bytes */{total}")).expect("numeric range"));
                        return refused.map(TransportBody::from);
                    }
                }
            };
            // Open even for HEAD, so it cannot claim healthy committed bytes that
            // the selected store refuses to resolve. The lease is never collected.
            let reader = match store.open(&layer.payload, range.clone()).await {
                Ok(v) => v,
                Err(_) => return error(500, "committed blob unavailable").map(TransportBody::from),
            };
            let length = reader.len();
            drop(mutation); // immutable lease survives later logical deletion
            let body = if head { drop(reader); TransportBody::from(Bytes::new()) } else { payloads::response_body(reader) };
            let mut response = http::Response::new(body);
            response.headers_mut().insert("accept-ranges", http::HeaderValue::from_static("bytes"));
            if let Some(range) = range {
                *response.status_mut() = http::StatusCode::PARTIAL_CONTENT;
                response.headers_mut().insert("content-range", http::HeaderValue::from_str(&format!("bytes {}-{}/{total}", range.start, range.end - 1)).expect("numeric range"));
            }
            response.headers_mut().insert("content-type", http::HeaderValue::from_static(crate::state::LAYER_DOWNLOAD_CONTENT_TYPE));
            response.headers_mut().insert("content-length", http::HeaderValue::from_str(&length.to_string()).expect("numeric length"));
            response.headers_mut().insert("docker-content-digest", http::HeaderValue::from_str(&digest.to_string()).expect("canonical digest"));
            response.headers_mut().insert("docker-distribution-api-version", http::HeaderValue::from_static("registry/2.0"));
            response
        })
    }
}

/// One explicit checked GET range; HEAD ignores Range. No multipart allocation.
fn parse_range(value: &str, length: u64) -> Result<std::ops::Range<u64>, ()> {
    let value = value.strip_prefix("bytes=").ok_or(())?;
    if length == 0 || value.len() > 64 || value.contains(',') { return Err(()); }
    let (first, last) = value.split_once('-').ok_or(())?;
    let number = |text: &str| -> Result<u64, ()> {
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) { return Err(()); }
        text.parse().map_err(|_| ())
    };
    if first.is_empty() {
        let suffix = number(last)?;
        if suffix == 0 { return Err(()); }
        return Ok(length.saturating_sub(suffix)..length);
    }
    let start = number(first)?;
    if start >= length { return Err(()); }
    let end = if last.is_empty() { length } else {
        let last = number(last)?;
        if last < start { return Err(()); }
        last.saturating_add(1).min(length)
    };
    Ok(start..end)
}

#[cfg(test)]
mod range_tests {
    use super::parse_range;
    #[test]
    fn single_ranges_are_checked_and_half_open() {
        assert_eq!(parse_range("bytes=2-4", 8), Ok(2..5));
        assert_eq!(parse_range("bytes=2-", 8), Ok(2..8));
        assert_eq!(parse_range("bytes=-3", 8), Ok(5..8));
        assert_eq!(parse_range("bytes=-99", 8), Ok(0..8));
        for bad in ["bytes=8-", "bytes=4-2", "bytes=-0", "bytes=1-2,4-5", "items=1-2", "bytes=+1-2"] {
            assert!(parse_range(bad, 8).is_err());
        }
        assert!(parse_range("bytes=0-0", 0).is_err());
    }
}
