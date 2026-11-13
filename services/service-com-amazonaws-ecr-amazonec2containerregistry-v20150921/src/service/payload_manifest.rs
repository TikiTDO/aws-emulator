// SPDX-License-Identifier: CC0-1.0

//! Manifest control documents are individually bounded; published images retain
//! immutable handles. Their payload exchange is shared with the embedded SDK.
use super::*;
use bytes::Bytes;
use crate::state::{self, payloads};
use emulator_server::transport::{ExchangeContext, ExchangeFuture, TransportBody, TransportError};
use smithy_server_runtime::transport::{PayloadHandler, PayloadOperation, PayloadReceiveError, PayloadRequest, PayloadRoute, PayloadSink};
use registry_v2::{RegistryV2Target, registry_error as error};

struct ManifestExchange<Eng: Send + Sync + 'static> { engine: Arc<Eng>, repository: String, reference: String }
pub(super) fn route<Eng: Send + Sync + 'static>(engine: Arc<Eng>, parts: &http::request::Parts) -> Option<PayloadRoute> {
    if !registry_v2::claims_registry_v2(parts) { return None; }
    let RegistryV2Target::Manifest { repository_name, reference } = registry_v2::registry_v2_target(parts.uri.path())? else { return None; };
    Some(PayloadRoute { operation: PayloadOperation::Native("EcrManifest"), handler: Arc::new(ManifestExchange { engine, repository: repository_name, reference }) })
}
struct DocumentSink(Vec<u8>);
impl PayloadSink for DocumentSink {
    fn append<'a>(&'a mut self, bytes: Bytes) -> smithy_server_runtime::BoxFuture<'a, Result<(), TransportError>> {
        Box::pin(async move {
            if self.0.len().checked_add(bytes.len()).is_none_or(|n| n > 4 * 1024 * 1024) { return Err(service_common::payload_store::StoreError::QuotaExceeded.into()); }
            self.0.extend_from_slice(&bytes); Ok(())
        })
    }
}
impl<Eng: Send + Sync + 'static> ManifestExchange<Eng> {
    fn credential(&self, request: &RtRequest) -> Result<String, RtResponse> {
        if !matches!(request.method(), "GET" | "HEAD" | "PUT") { return Err(error(405, "UNSUPPORTED", "unsupported manifest method")); }
        let state = state::service_state(&*self.engine).ok_or_else(|| error(500, "UNKNOWN", "repository store unavailable"))?;
        let password = registry_v2::presented_password(request).ok_or_else(|| registry_v2::unauthorized(request))?;
        let principal = {
            let mut state = state.lock(); let now = std::time::SystemTime::now();
            state.reclaim_expired_registry_credentials(now);
            state.honoured_registry_credential(&password, now).ok_or_else(|| registry_v2::unauthorized(request))?.principal_arn.clone()
        };
        Ok(principal)
    }
    fn principal(&self, request: &RtRequest) -> Result<String, RtResponse> {
        let principal = self.credential(request)?;
        if request.method() == "PUT" && !state::authorization::registry_write_allowed(&*self.engine, &principal, &self.repository, "PutImage") {
            return Err(error(403, "DENIED", "requested access to the resource is denied"));
        }
        Ok(principal)
    }
    async fn execute(&self, request: PayloadRequest, context: ExchangeContext) -> Result<RtResponse, RtResponse> {
        let mut headers = payload_read::header_request(request.parts()).map_err(|_| error(400, "MANIFEST_INVALID", "invalid request headers"))?;
        if headers.method() == "PUT" {
            let mut sink = DocumentSink(Vec::new());
            request.receive_into(&mut sink, &context).await.map_err(|cause| match cause {
                PayloadReceiveError::Sink(cause) if cause.downcast_ref::<service_common::payload_store::StoreError>() == Some(&service_common::payload_store::StoreError::QuotaExceeded) => error(413, "MANIFEST_INVALID", "manifest exceeds registry capacity"),
                _ => error(400, "MANIFEST_INVALID", "manifest entity could not be verified"),
            })?;
            let mutation = context.until_cancelled(payloads::mutation_guard(&*self.engine)).await
                .map_err(|_| error(500, "UNKNOWN", "manifest admission cancelled"))?
                .map_err(|_| error(500, "UNKNOWN", "manifest admission unavailable"))?;
            self.principal(&headers)?;
            *headers.body_mut() = SdkBody::from(sink.0);
            let state = state::service_state(&*self.engine).ok_or_else(|| error(500, "UNKNOWN", "repository store unavailable"))?;
            return Ok(EmulatedService::<Eng>::push_manifest(&*self.engine, &mutation, &state, &headers, &self.repository, &self.reference, std::time::SystemTime::now()).await);
        }
        let mut empty = payloads::EmptyEntity;
        request.receive_into(&mut empty, &context).await.map_err(|_| error(400, "MANIFEST_INVALID", "invalid read entity"))?;
        let mutation = context.until_cancelled(payloads::mutation_guard(&*self.engine)).await.map_err(|_| error(500, "UNKNOWN", "read admission cancelled"))?
            .map_err(|_| error(500, "UNKNOWN", "read admission unavailable"))?;
        let principal = self.principal(&headers)?;
        let state = state::service_state(&*self.engine).ok_or_else(|| error(500, "UNKNOWN", "repository store unavailable"))?;
        let (before,image) = {
            let state = state.lock();
            let repo = state.repository(&self.repository).map_err(|_| error(404, "NAME_UNKNOWN", "repository name not known to registry"))?;
            let image = repo.lookup_image(&registry_v2::addressed_image(&self.reference)).map_err(|_| error(404, "MANIFEST_UNKNOWN", "manifest unknown"))?.clone();
            (repo.clone(),image)
        };
        let store = payloads::bound_store(&*self.engine).map_err(|_| error(500, "UNKNOWN", "payload store unavailable"))?;
        let document = image.manifest.read(&store).await.map_err(|_| error(500, "UNKNOWN", "committed manifest unavailable"))?;
        // Expiry/revocation may change while immutable bytes are read.
        if self.principal(&headers)? != principal { return Err(registry_v2::unauthorized(&headers)); }
        let (served, media_type) = state::negotiated_manifest(&image, &document, registry_v2::accepted_media_types(&headers).as_deref());
        let digest = if served == document { image.digest.clone() } else { state::manifest_digest(&served) };
        if headers.method() == "GET" {
            let excluded = state.lock().pull_time_is_excluded(&principal);
            let mut after = before.clone();
            if mark_pull(&mut after,&image.digest,excluded,std::time::SystemTime::now())
                .map_err(|_| error(500,"UNKNOWN","pull projection unavailable"))? {
                state::catalog::publish_images(&mutation,&store,&before,&mut after).await
                    .map_err(|_| error(500,"UNKNOWN","pull metadata publication unavailable"))?;
                // No await after known outcome and before the matching RAM apply.
                state.lock().repositories.insert(after.name.clone(),after);
            }
        }
        drop(mutation);
        if headers.method() == "GET" { state::pull_metrics::count_repository_pull(&*self.engine, &self.repository); }
        let content_type = media_type.as_deref().map_or("application/json", registry_v2::honoured_media_type);
        Ok(registry_v2::content_response(served.as_bytes(), content_type, &digest, headers.method() == "GET"))
    }
}

fn mark_pull(repo: &mut state::Repo, digest: &str, excluded: bool, now: std::time::SystemTime)
    -> Result<bool,service_common::payload_store::StoreError> {
    if excluded { return Ok(false); }
    let image = repo.images.get_mut(digest).ok_or(service_common::payload_store::StoreError::Integrity)?;
    image.last_recorded_pull_time = Some(now);
    Ok(true)
}
impl<Eng: Send + Sync + 'static> PayloadHandler for ManifestExchange<Eng> {
    fn authorize(&self, parts: &http::request::Parts) -> Result<(), http::Response<Bytes>> {
        self.credential(&payload_read::header_request(parts)?).map(|_| ()).map_err(payload_read::small)
    }
    fn refuse(&self, _: PayloadReceiveError) -> http::Response<Bytes> { payload_read::small(error(400, "MANIFEST_INVALID", "manifest request could not be verified")) }
    fn run(self: Arc<Self>, request: PayloadRequest, context: ExchangeContext) -> ExchangeFuture {
        Box::pin(async move { let response = match self.execute(request, context).await { Ok(v) | Err(v) => v }; payload_read::small(response).map(TransportBody::from) })
    }
}

#[cfg(test)]
mod pull_projection_tests {
    use super::*;
    #[test]
    fn excluded_pull_and_failed_plan_leave_the_original_projection_intact() {
        let now = std::time::UNIX_EPOCH;
        let mut before = state::Repo {
            payload_generation:"a".repeat(64),payload_revision:None,name:"fixture".into(),registry_id:state::registry_id().into(),
            arn:state::repository_arn("us-east-1","fixture"),created_at:now,image_tag_mutability:"MUTABLE".into(),
            image_tag_mutability_exclusion_filters:Vec::new(),image_scanning_config:crate::ops::types::ImageScanningConfiguration::default(),
            encryption_config:crate::ops::types::EncryptionConfiguration { encryption_type:"AES256".into(),kms_key:None },
            tags:Vec::new(),policy_text:None,lifecycle_policy:None,lifecycle_preview:None,
            images:std::collections::BTreeMap::new(),tag_index:std::collections::BTreeMap::new(),layer_blobs:std::collections::BTreeMap::new(),
        };
        before.images.insert("image".into(),state::ImageRecord::pushed("image".into(),"{}".into(),None,now));
        let mut excluded = before.clone();
        assert!(!mark_pull(&mut excluded,"image",true,now).unwrap());
        assert!(excluded.images["image"].last_recorded_pull_time.is_none());
        let mut planned = before.clone();
        assert!(mark_pull(&mut planned,"image",false,now).unwrap());
        assert_eq!(planned.images["image"].last_recorded_pull_time,Some(now));
        assert!(before.images["image"].last_recorded_pull_time.is_none());
        assert!(mark_pull(&mut planned,"missing",false,now).is_err());
    }
}
