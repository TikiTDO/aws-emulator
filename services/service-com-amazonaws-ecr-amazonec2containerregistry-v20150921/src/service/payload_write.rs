// SPDX-License-Identifier: CC0-1.0

//! Native uploads append privately, verify terminal wire receipt, then checkpoint
//! or publish. A failed suffix never replaces a previously acknowledged prefix.
use super::*;
use crate::state::{self, payloads, payload_upload};
use bytes::Bytes;
use emulator_server::transport::{ExchangeContext, ExchangeFuture, TransportBody, TransportError};
use service_common::payload_store::{IntegrityExpectation, ReferenceTransaction, StoreError};
use smithy_server_runtime::transport::{PayloadHandler, PayloadOperation, PayloadReceiveError, PayloadRequest, PayloadRoute, PayloadSink};
use registry_v2::{RegistryV2Target, registry_error as error};
use registry_v2_push::{blob_stored, empty_response, query, range_matches, upload_response};

struct UploadExchange<Eng: Send + Sync + 'static> {
    engine: Arc<Eng>, repository: String, id: Option<String>,
}
pub(super) fn route<Eng: Send + Sync + 'static>(engine: Arc<Eng>, parts: &http::request::Parts) -> Option<PayloadRoute> {
    if !registry_v2::claims_registry_v2(parts) { return None; }
    let (repository, id) = match registry_v2::registry_v2_target(parts.uri.path())? {
        RegistryV2Target::Uploads { repository_name } => (repository_name, None),
        RegistryV2Target::Upload { repository_name, upload_id } => (repository_name, Some(upload_id)),
        _ => return None,
    };
    Some(PayloadRoute { operation: PayloadOperation::Native("EcrPayloadUpload"), handler: Arc::new(UploadExchange { engine, repository, id }) })
}
fn unavailable() -> RtResponse { error(500, "UNKNOWN", "durable registry upload unavailable") }
fn unknown() -> RtResponse { error(404, "BLOB_UPLOAD_UNKNOWN", "blob upload unknown") }

impl<Eng: Send + Sync + 'static> UploadExchange<Eng> {
    fn credential(&self, request: &RtRequest) -> Result<String, RtResponse> {
        if !registry_v2_push::valid_repository(&self.repository) { return Err(error(400, "NAME_INVALID", "invalid repository name")); }
        if !(self.id.is_none() && request.method() == "POST"
            || self.id.is_some() && matches!(request.method(), "GET" | "PATCH" | "PUT" | "DELETE")) {
            return Err(error(405, "UNSUPPORTED", "unsupported upload method"));
        }
        let state = state::service_state(&*self.engine).ok_or_else(unavailable)?;
        let password = registry_v2::presented_password(request).ok_or_else(|| registry_v2::unauthorized(request))?;
        let principal = {
            let mut state = state.lock();
            let now = std::time::SystemTime::now();
            state.reclaim_expired_registry_credentials(now);
            state.honoured_registry_credential(&password, now)
                .ok_or_else(|| registry_v2::unauthorized(request))?.principal_arn.clone()
        };
        Ok(principal)
    }
    fn principal(&self, request: &RtRequest) -> Result<String, RtResponse> {
        let principal = self.credential(request)?;
        let action = if self.id.is_none() { "InitiateLayerUpload" } else if request.method() == "PUT" { "CompleteLayerUpload" } else { "UploadLayerPart" };
        if !state::authorization::registry_write_allowed(&*self.engine, &principal, &self.repository, action) {
            return Err(error(403, "DENIED", "requested access to the resource is denied"));
        }
        Ok(principal)
    }
    async fn execute(&self, request: PayloadRequest, context: ExchangeContext) -> Result<RtResponse, RtResponse> {
        let headers = super::payload_read::header_request(request.parts()).map_err(|_| error(400, "BLOB_UPLOAD_INVALID", "invalid request headers"))?;
        let _mutation = context.until_cancelled(payloads::mutation_guard(&*self.engine)).await
            .map_err(|_| unavailable())?.map_err(|_| unavailable())?;
        let principal = self.principal(&headers)?;
        let state = state::service_state(&*self.engine).ok_or_else(unavailable)?;
        let payload_store = payloads::bound_store(&*self.engine).map_err(|_| unavailable())?;
        let capacity = emulator_server::ecr_capacity::configured().map_err(|_| unavailable())?;
        let (mut owner, held_bytes) = {
            let state = state.lock();
            let repo = state.repository(&self.repository).map_err(|_| error(404, "NAME_UNKNOWN", "repository name not known to registry"))?;
            (payloads::repository_owner(repo).map_err(|_| unavailable())?, state.caller_bytes_held())
        };
        let id = self.id.clone().unwrap_or_else(crate::ops::initiate_layer_upload::new_upload_id);
        let mut bound = if self.id.is_none() {
            if state.lock().uploads.len() >= state::MAX_LAYER_UPLOADS { return Err(error(413, "BLOB_UPLOAD_INVALID", "registry upload capacity exceeded")); }
            let bound = payload_upload::initiate(&_mutation, &payload_store, owner.clone(), &id, payload_upload::UploadProtocol {
                principal: Some(principal.clone()), repository: owner.name.clone(), generation: owner.generation.clone(), part_size: state::REPORTED_LAYER_PART_SIZE,
                started_at: std::time::SystemTime::now(),
            }, capacity.layer_bytes as u64).await.map_err(|_| unavailable())?;
            {
                let mut state = state.lock();
                state.repository_mut(&self.repository).map_err(|_| unavailable())?.payload_revision = Some(bound.revision);
                state.uploads.insert(id.clone(), bound.index_projection());
            }
            // POST ?digest has two reference groups under this same gate.
            // Resolve/reload the first before any second group; cancellation here
            // retains its ID. Rebind the scope and upload from catalog authority.
            _mutation.reconcile(&*self.engine).await.map_err(|_| unavailable())?;
            owner = {
                let state = state.lock();
                payloads::repository_owner(state.repository(&self.repository).map_err(|_| unavailable())?)
                    .map_err(|_| unavailable())?
            };
            payload_upload::lookup(&payload_store, &owner, &id).await.map_err(|_| unavailable())?.ok_or_else(unknown)?
        } else {
            payload_upload::lookup(&payload_store, &owner, &id).await.map_err(|_| unavailable())?.ok_or_else(unknown)?
        };
        if bound.protocol.principal.as_deref() != Some(&principal) { return Err(unknown()); }
        let prefix = bound.snapshot.checkpoint.offset();
        if matches!(headers.method(), "GET" | "DELETE") {
            let mut empty = payloads::EmptyEntity;
            request.receive_into(&mut empty, &context).await.map_err(|_| error(400, "BLOB_UPLOAD_INVALID", "invalid upload request entity"))?;
            if headers.method() == "DELETE" {
                let (close, retirement) = payload_upload::retirement(&bound);
                let receipt = _mutation.commit(&payload_store, ReferenceTransaction { id: payload_store.new_commit_id(), owner: owner.owner.clone(),
                    changes: vec![close, owner.assertion(&payload_store).await.map_err(|_| unavailable())?], retire: vec![retirement],
                }).await.map_err(|_| unavailable())?;
                {
                    let mut state = state.lock();
                    state.repository_mut(&self.repository).map_err(|_| unavailable())?.payload_revision = Some(receipt.revision);
                    state.uploads.remove(&id);
                }
                return Ok(empty_response(204));
            }
            return Ok(upload_response(204, &self.repository, &id, prefix as usize));
        }
        let completing = headers.method() == "PUT" || self.id.is_none() && query(&headers, "digest")?.is_some();
        let requested_digest = if completing {
            let value = query(&headers, "digest")?.filter(|v| state::is_layer_digest(v))
                .ok_or_else(|| error(400, "DIGEST_INVALID", "completion requires one sha256 digest"))?;
            if !state::authorization::registry_write_allowed(&*self.engine, &principal, &self.repository, "CompleteLayerUpload") {
                return Err(error(403, "DENIED", "requested access to the resource is denied"));
            }
            Some(value)
        } else { None };
        let can_append = state::authorization::registry_write_allowed(&*self.engine, &principal, &self.repository, "UploadLayerPart");
        // A successful seal retained after a publication conflict is retriable only
        // with an empty suffix; no append/resume of sealed content is possible.
        let payload = if let Some(payload) = &bound.snapshot.sealed {
            let mut empty = payloads::EmptyEntity;
            request.receive_into(&mut empty, &context).await.map_err(|_| error(400, "BLOB_UPLOAD_INVALID", "sealed upload accepts no new suffix"))?;
            if !completing { return Err(error(400, "BLOB_UPLOAD_INVALID", "upload is already sealed")); }
            payload.clone()
        } else {
            let mut stage = payload_store.resume(&bound.snapshot.id, bound.snapshot.checkpoint.clone()).await.map_err(|_| unavailable())?;
            let allowance = (capacity.layer_bytes as u64).saturating_sub(prefix)
                .min((capacity.store_bytes as u64).saturating_sub(held_bytes as u64));
            let sink = payloads::UploadSuffix::new(&mut stage, prefix, allowance);
            let mut sink = GrantedSuffix { sink, allowed: can_append };
            let receipt = match request.receive_into(&mut sink, &context).await {
                Ok(receipt) => receipt,
                Err(cause) => {
                    let refusal = match &cause {
                        PayloadReceiveError::Sink(cause) if cause.downcast_ref::<StoreError>() == Some(&StoreError::QuotaExceeded) => error(413, "BLOB_UPLOAD_INVALID", "registry upload capacity exceeded"),
                        PayloadReceiveError::Sink(cause) if cause.downcast_ref::<StoreError>() == Some(&StoreError::Invalid("native upload grant")) => error(403, "DENIED", "requested access to the resource is denied"),
                        _ => error(400, "BLOB_UPLOAD_INVALID", "upload entity was not accepted"),
                    };
                    drop(sink);
                    stage.rollback_to(bound.snapshot.checkpoint.clone()).await.map_err(|_| unavailable())?;
                    return Err(refusal);
                }
            };
            let total = sink.sink.validate(&receipt).map_err(|_| unavailable())?;
            if !range_matches(&headers, prefix as usize, (total - prefix) as usize) {
                stage.rollback_to(bound.snapshot.checkpoint.clone()).await.map_err(|_| unavailable())?;
                let mut response = error(416, "BLOB_UPLOAD_INVALID", "chunk does not continue this upload");
                response.headers_mut().insert("range", format!("0-{}", prefix.saturating_sub(1)));
                return Err(response);
            }
            if !completing {
                bound.snapshot.checkpoint = _mutation.checkpoint(stage, bound.protocol.encode().map_err(|_| unavailable())?).await.map_err(|_| unavailable())?;
                state.lock().uploads.insert(id.clone(), bound.index_projection());
                return Ok(upload_response(202, &self.repository, &id, total as usize));
            }
            let progress = stage.progress().await.map_err(|_| unavailable())?;
            if requested_digest.as_deref() != Some(progress.sha256.to_string().as_str()) {
                stage.rollback_to(bound.snapshot.checkpoint.clone()).await.map_err(|_| unavailable())?;
                return Err(error(400, "DIGEST_INVALID", "digest does not match uploaded content"));
            }
            _mutation.seal(stage, IntegrityExpectation { length: Some(total), sha256: Some(progress.sha256) }).await.map_err(|_| unavailable())?
        };
        // Resolve the owned seal before scanning/publication. This reload accounts
        // any sealed suffix beyond the prior checkpoint even if later work fails.
        _mutation.reconcile(&*self.engine).await.map_err(|_| unavailable())?;
        owner = {
            let state = state.lock();
            payloads::repository_owner(state.repository(&self.repository).map_err(|_| unavailable())?).map_err(|_| unavailable())?
        };
        bound = payload_upload::lookup(&payload_store, &owner, &id).await.map_err(|_| unavailable())?.ok_or_else(unknown)?;
        let digest = payload.sha256().to_string();
        if requested_digest.as_deref() != Some(digest.as_str()) { return Err(error(400, "DIGEST_INVALID", "digest does not match uploaded content")); }
        {
            let state = state.lock();
            let repo = state.repository(&self.repository).map_err(|_| unavailable())?;
            if !repo.layer_blobs.contains_key(&digest) && repo.layer_blobs.len() >= state::MAX_LAYERS_PER_REPOSITORY {
                return Err(error(413, "BLOB_UPLOAD_INVALID", "repository blob capacity exceeded"));
            }
        }
        let filesystem = payloads::scan_payload(payload_store.open(&payload, None).await.map_err(|_| unavailable())?).await.map_err(|_| unavailable())?;
        let (_, revision) = payloads::publish_layer(&_mutation, &payload_store, owner, payload_store.new_commit_id(), payload.clone(), &bound, None, filesystem.clone())
            .await.map_err(|_| unavailable())?;
        {
            let mut state = state.lock();
            let repo = state.repository_mut(&self.repository).map_err(|_| unavailable())?;
            repo.payload_revision = Some(revision);
            repo.layer_blobs.entry(digest.clone()).or_insert_with(|| state::LayerBlob {
                digest: digest.clone(), size: Some(payload.len() as i64), media_type: None,
                filesystem, payload: Some(payload),
            });
            state.uploads.remove(&id);
        }
        Ok(blob_stored(&self.repository, &digest))
    }
}
struct GrantedSuffix<'a> { sink: payloads::UploadSuffix<'a>, allowed: bool }
impl PayloadSink for GrantedSuffix<'_> {
    fn append<'a>(&'a mut self, bytes: Bytes) -> smithy_server_runtime::BoxFuture<'a, Result<(), TransportError>> {
        Box::pin(async move {
            if !self.allowed && !bytes.is_empty() { return Err(StoreError::Invalid("native upload grant").into()); }
            self.sink.append(bytes).await
        })
    }
}
impl<Eng: Send + Sync + 'static> PayloadHandler for UploadExchange<Eng> {
    fn authorize(&self, parts: &http::request::Parts) -> Result<(), http::Response<Bytes>> {
        let request = super::payload_read::header_request(parts)?;
        self.credential(&request).map(|_| ()).map_err(super::payload_read::small)
    }
    fn refuse(&self, _: PayloadReceiveError) -> http::Response<Bytes> {
        super::payload_read::small(error(400, "BLOB_UPLOAD_INVALID", "upload request could not be verified"))
    }
    fn run(self: Arc<Self>, request: PayloadRequest, context: ExchangeContext) -> ExchangeFuture {
        Box::pin(async move {
            let response = match self.execute(request, context).await { Ok(v) | Err(v) => v };
            super::payload_read::small(response).map(TransportBody::from)
        })
    }
}
