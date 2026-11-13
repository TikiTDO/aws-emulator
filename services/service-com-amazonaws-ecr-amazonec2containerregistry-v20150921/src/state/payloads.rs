// SPDX-License-Identifier: CC0-1.0

//! ECR adapters over the ONE boot-installed payload store. These helpers neither
//! authenticate callers nor create a fallback store. Protocol handlers retain
//! repository policy/generation checks and own durable reference transactions.

use bytes::Bytes;
use emulator_server::transport::{TransportBody, TransportError};
use service_common::payload_store::{PayloadReader, PayloadStore, PayloadStoreBinding, StagedPayload, StoreError};
use sha2::{Digest, Sha256};
use smithy_server_runtime::transport::{PayloadReceipt, PayloadSink};
use std::{io::{self, Read}, pin::Pin, task::{Context, Poll}};
use tokio::io::{AsyncRead, ReadBuf};

/// No store is created here. Boot installs its selected namespace before restore.
pub fn bound_store<Eng: Send + Sync + 'static>(engine: &Eng) -> Result<PayloadStore, StoreError> {
    let engine = (engine as &dyn std::any::Any)
        .downcast_ref::<engine::InMemoryEngine>()
        .ok_or(StoreError::Invalid("ECR payload engine binding"))?;
    let binding = engine.state::<PayloadStoreBinding>();
    let store = binding.lock().store.clone();
    store.ok_or(StoreError::Invalid("ECR payload store not installed"))
}

/// This request's suffix, not the acknowledged prefix. A transport receipt must
/// match these bytes before the caller checkpoints TOTAL stage progress.
pub struct UploadSuffix<'a> {
    stage: &'a mut StagedPayload,
    offset: u64,
    count: u64,
    maximum: u64,
    digest: Sha256,
}
impl<'a> UploadSuffix<'a> {
    pub fn new(stage: &'a mut StagedPayload, acknowledged_offset: u64, maximum: u64) -> Self {
        Self { stage, offset: acknowledged_offset, count: 0, maximum, digest: Sha256::new() }
    }
    pub fn validate(self, receipt: &PayloadReceipt) -> Result<u64, StoreError> {
        let digest: [u8; 32] = self.digest.finalize().into();
        if receipt.validation().decoded_bytes() != self.count
            || receipt.validation().decoded_sha256() != &digest {
            return Err(StoreError::Integrity);
        }
        Ok(self.offset)
    }
}
impl PayloadSink for UploadSuffix<'_> {
    fn append<'a>(&'a mut self, bytes: Bytes) -> smithy_server_runtime::BoxFuture<'a, Result<(), TransportError>> {
        Box::pin(async move {
            if self.count.checked_add(bytes.len() as u64).is_none_or(|n| n > self.maximum) {
                return Err(StoreError::QuotaExceeded.into());
            }
            // A B1 visible frame may be larger than B2's append quantum. These
            // borrowed spans do not collect a second entity or bypass its quota.
            for chunk in bytes.chunks(self.stage.max_append_bytes().min(64 * 1024)) {
                let accepted = self.stage.append(self.offset, Bytes::copy_from_slice(chunk)).await?;
                let expected = self.offset.checked_add(chunk.len() as u64).ok_or(StoreError::Integrity)?;
                if accepted != expected { return Err(StoreError::Integrity.into()); }
                self.digest.update(chunk);
                self.count = self.count.checked_add(chunk.len() as u64).ok_or(StoreError::Integrity)?;
                self.offset = accepted;
            }
            Ok(())
        })
    }
}

/// GET/HEAD still pass their signed entity through EOF verification. They may
/// not smuggle an ignored body past the read operation's empty-entity contract.
pub struct EmptyEntity;
impl PayloadSink for EmptyEntity {
    fn append<'a>(&'a mut self, bytes: Bytes) -> smithy_server_runtime::BoxFuture<'a, Result<(), TransportError>> {
        Box::pin(async move {
            if bytes.is_empty() { Ok(()) } else { Err(StoreError::Invalid("read request carries payload").into()) }
        })
    }
}

/// A read lease becomes frames without materializing its payload. Headers and
/// range selection are service-owned and must use the same selected lease length.
pub fn response_body(reader: PayloadReader) -> TransportBody {
    let remaining = reader.len();
    TransportBody::new(ReaderBody { reader, remaining })
}
struct ReaderBody { reader: PayloadReader, remaining: u64 }
impl http_body::Body for ReaderBody {
    type Data = Bytes;
    type Error = io::Error;
    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<http_body::Frame<Bytes>, io::Error>>> {
        if self.remaining == 0 { return Poll::Ready(None); }
        let mut storage = vec![0u8; self.remaining.min(64 * 1024) as usize];
        let mut buffer = ReadBuf::new(&mut storage);
        match Pin::new(&mut self.reader).poll_read(cx, &mut buffer) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => { self.remaining = 0; Poll::Ready(Some(Err(error))) }
            Poll::Ready(Ok(())) => {
                let read = buffer.filled().len();
                if read == 0 || read as u64 > self.remaining {
                    self.remaining = 0;
                    return Poll::Ready(Some(Err(io::Error::new(io::ErrorKind::UnexpectedEof, "ECR payload integrity"))));
                }
                self.remaining -= read as u64;
                storage.truncate(read);
                Poll::Ready(Some(Ok(http_body::Frame::data(Bytes::from(storage)))))
            }
        }
    }
    fn is_end_stream(&self) -> bool { self.remaining == 0 }
    fn size_hint(&self) -> http_body::SizeHint { http_body::SizeHint::with_exact(self.remaining) }
}

/// Retain the existing tar/gzip scanner behind a two-chunk bounded bridge. It
/// runs outside the async worker and sees no path; dropping this future drops
/// the sender so a blocked parser cannot retain an open producer forever.
pub async fn scan_payload(mut reader: PayloadReader) -> Result<super::LayerFilesystem, StoreError> {
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    let parsing = tokio::task::spawn_blocking(move || super::scan_layer_reader(BlockingChunks {
        receiver, current: Bytes::new(), offset: 0,
    }));
    let mut failure = None;
    loop {
        match reader.next_chunk().await {
            Ok(Some(bytes)) => if sender.send(bytes).await.is_err() { break; },
            Ok(None) => break,
            Err(error) => { failure = Some(error); break; }
        }
    }
    drop(sender);
    let result = parsing.await.map_err(|_| StoreError::Worker)?;
    if let Some(error) = failure { return Err(error); }
    Ok(result)
}
struct BlockingChunks {
    receiver: tokio::sync::mpsc::Receiver<Bytes>, current: Bytes, offset: usize,
}
impl Read for BlockingChunks {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() { return Ok(0); }
        while self.offset == self.current.len() {
            let Some(bytes) = self.receiver.blocking_recv() else { return Ok(0); };
            self.current = bytes; self.offset = 0;
        }
        let count = output.len().min(self.current.len() - self.offset);
        output[..count].copy_from_slice(&self.current[self.offset..self.offset + count]);
        self.offset += count;
        Ok(count)
    }
}

/// Closed layer record stored beside (not inside) repository configuration.
/// A descriptor has no such record and cannot fabricate a PayloadHandle.
#[derive(Clone, Debug)]
pub struct StoredLayer {
    pub payload: service_common::payload_store::PayloadHandle,
    pub media_type: Option<String>,
    pub filesystem: super::LayerFilesystem,
    pub revision: service_common::payload_store::RecordRevision,
}

pub fn layer_key(
    owner: &RepositoryScope,
    digest: service_common::payload_store::Sha256Digest,
) -> Result<service_common::payload_store::RecordKey, StoreError> {
    owner.key("layer", &digest.to_string())
}

pub async fn stored_layer(
    store: &PayloadStore,
    owner: &RepositoryScope,
    digest: service_common::payload_store::Sha256Digest,
) -> Result<Option<StoredLayer>, StoreError> {
    owner.assertion(store).await?;
    let Some(record) = store.record(&layer_key(owner, digest)?).await? else { return Ok(None); };
    let Some(value) = record.value else { return Ok(None); };
    let fields = value.metadata.as_object().ok_or(StoreError::Integrity)?;
    if value.schema_version != 1 || fields.len() != 4
        || fields.get("kind").and_then(serde_json::Value::as_str) != Some("ecr-layer")
        || value.references.len() != 1 || value.references[0].sha256() != digest {
        return Err(StoreError::Integrity);
    }
    let optional = |name| -> Result<Option<String>, StoreError> {
        match fields.get(name) {
            Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(text)) if text.len() <= 64 * 1024 => Ok(Some(text.clone())),
            _ => Err(StoreError::Integrity),
        }
    };
    Ok(Some(StoredLayer {
        payload: value.references[0].clone(),
        media_type: optional("mediaType")?,
        filesystem: super::LayerFilesystem {
            operating_system: optional("operatingSystem")?,
            package_manager: optional("packageManager")?,
        },
        revision: record.revision,
    }))
}

/// One atomic layer publication and upload retirement. Auth/range/digest checks
/// precede this call. The caller retains commit_id across uncertain response loss.
/// Conflict never authorizes aborting a prior checkpoint or deleting sealed bytes.
pub async fn publish_layer(
    mutation: &MutationGuard,
    store: &PayloadStore,
    owner: RepositoryScope,
    commit_id: service_common::payload_store::CommitId,
    payload: service_common::payload_store::PayloadHandle,
    upload: &super::payload_upload::BoundUpload,
    media_type: Option<String>,
    filesystem: super::LayerFilesystem,
) -> Result<(bool, service_common::payload_store::RecordRevision), service_common::payload_store::CommitError> {
    use service_common::payload_store::{RecordChange, RecordCondition, RecordData, ReferenceTransaction};
    let key = layer_key(&owner, payload.sha256())?;
    let old = store.record(&key).await?;
    let existing = stored_layer(store, &owner, payload.sha256()).await?;
    if existing.as_ref().is_some_and(|v| v.payload != payload) { return Err(StoreError::Integrity.into()); }
    let fresh = existing.is_none();
    let (close, retirement) = super::payload_upload::retirement(upload);
    let mut changes = vec![close, owner.assertion(store).await?];
    if fresh {
        changes.push(RecordChange {
            key,
            condition: old.map_or(RecordCondition::Absent, |v| RecordCondition::Exact(v.revision)),
            value: Some(RecordData {
                schema_version: 1,
                metadata: serde_json::json!({
                    "kind": "ecr-layer", "mediaType": media_type,
                    "operatingSystem": filesystem.operating_system,
                    "packageManager": filesystem.package_manager,
                }),
                references: vec![payload],
            }),
        });
    } else if let Some(old) = old {
        // Duplicate completion still depends on that exact existing layer, not
        // merely on a previous existence observation.
        changes.push(RecordChange { key, condition: RecordCondition::Exact(old.revision), value: old.value });
    }
    let receipt = mutation.commit(store, ReferenceTransaction {
        id: commit_id, owner: owner.owner.clone(), changes, retire: vec![retirement],
    }).await?;
    Ok((fresh, receipt.revision))
}

/// Repository names are scoped by the explicit persisted incarnation, not their
/// creation timestamp. The repository envelope must restore this field first.
#[derive(Clone, Debug)]
pub struct RepositoryScope {
    pub owner: service_common::payload_store::OwnerKey,
    pub name: String,
    pub generation: String,
    pub revision: Option<service_common::payload_store::RecordRevision>,
}
pub fn registry_owner() -> Result<service_common::payload_store::OwnerKey, StoreError> {
    service_common::payload_store::OwnerKey::new("ecr", super::registry_id(), "catalog-v1")
}
pub fn repository_owner(repo: &super::Repo) -> Result<RepositoryScope, StoreError> {
    Ok(RepositoryScope { owner: registry_owner()?, name: repo.name.clone(), generation: repo.payload_generation.clone(), revision: repo.payload_revision })
}
impl RepositoryScope {
    pub fn key(&self, kind: &str, id: &str) -> Result<service_common::payload_store::RecordKey, StoreError> {
        service_common::payload_store::RecordKey::new(self.owner.clone(), Bytes::from(format!("repo/{}/{}/{kind}/{id}", self.name, self.generation)))
    }
    pub fn config_key(&self) -> Result<service_common::payload_store::RecordKey, StoreError> {
        service_common::payload_store::RecordKey::new(self.owner.clone(), Bytes::from(format!("repository/{}", self.name)))
    }
    /// Exact no-op rewrite is B2's existing compare-and-set assertion. Stable
    /// owner scope is never an auth or repository-incarnation proof.
    pub async fn assertion(&self, store: &PayloadStore) -> Result<service_common::payload_store::RecordChange, StoreError> {
        use service_common::payload_store::{RecordChange, RecordCondition};
        let record = store.record(&self.config_key()?).await?.ok_or(StoreError::NotFound)?;
        if Some(record.revision) != self.revision { return Err(StoreError::CheckpointConflict); }
        let value = record.value.ok_or(StoreError::NotFound)?;
        if value.schema_version != 1 || value.metadata.get("kind").and_then(serde_json::Value::as_str) != Some("ecr-repository")
            || value.metadata.get("name").and_then(serde_json::Value::as_str) != Some(self.name.as_str())
            || value.metadata.get("generation").and_then(serde_json::Value::as_str) != Some(self.generation.as_str()) {
            return Err(StoreError::Integrity);
        }
        Ok(RecordChange { key: record.key, condition: RecordCondition::Exact(record.revision), value: Some(value) })
    }
}

/// Stage a bounded control document or synthetic cache config, not an arbitrary
/// layer. The caller keeps the returned checkpoint with its commit identity until
/// reference publication is known. This does not publish or acknowledge anything.
pub async fn stage_document(
    store: &PayloadStore,
    owner: service_common::payload_store::OwnerKey,
    bytes: Bytes,
    max_bytes: u64,
) -> Result<(service_common::payload_store::SealedPayload, service_common::payload_store::Checkpoint), StoreError> {
    use service_common::payload_store::{IntegrityExpectation, ProtocolMetadata, StageSpec};
    if max_bytes > 4 * 1024 * 1024 || bytes.len() as u64 > max_bytes {
        return Err(StoreError::QuotaExceeded);
    }
    let mut stage = store.begin(StageSpec {
        owner, upload_key: None, expected_length: Some(bytes.len() as u64), max_bytes,
    }).await?;
    let mut offset = 0;
    for chunk in bytes.chunks(stage.max_append_bytes().min(64 * 1024)) {
        offset = stage.append(offset, Bytes::copy_from_slice(chunk)).await?;
    }
    let checkpoint = stage.checkpoint(ProtocolMetadata::new(
        "ecr-control-document", 1, Bytes::from_static(b"{}"),
    )?).await?;
    let sealed = stage.seal(IntegrityExpectation {
        length: Some(bytes.len() as u64), sha256: Some(checkpoint.sha256()),
    }).await?;
    Ok((sealed, checkpoint))
}

/// Serializes service-owned metadata publication across awaits. This is not a
/// payload store and holds no bytes. Repository/image/upload mutations must use
/// the same gate; parking_lot state guards remain confined to snapshot/apply blocks.
#[derive(Default)]
struct MutationGate(std::sync::Arc<tokio::sync::Mutex<()>>);
#[derive(Default)]
struct PendingPublication(std::sync::Arc<parking_lot::Mutex<Option<PendingMutation>>>);

type SealResult = Result<service_common::payload_store::PayloadHandle, StoreError>;
type CheckpointResult = Result<service_common::payload_store::Checkpoint, StoreError>;
#[derive(Clone)]
enum PendingMutation {
    Reference(service_common::payload_store::CommitId),
    Seal(tokio::sync::watch::Receiver<Option<SealResult>>),
    Checkpoint(tokio::sync::watch::Receiver<Option<CheckpointResult>>),
}
impl PendingMutation {
    /// None is genuinely running. Closed-without-outcome is UNKNOWN, not done.
    fn checkpoint_outcome(&self) -> Result<Option<CheckpointResult>, StoreError> {
        let Self::Checkpoint(receiver) = self else { return Err(StoreError::Integrity); };
        let outcome = receiver.borrow().clone();
        if outcome.is_none() && receiver.has_changed().is_err() {
            // Sender can publish and close between these two observations.
            let terminal = receiver.borrow().clone();
            return terminal.map(Some).ok_or(StoreError::Worker);
        }
        Ok(outcome)
    }
}

/// Proof of exclusive ECR catalog publication admission. The pending slot belongs
/// to the engine, not this cancellable request/guard. Never clear it on Drop.
pub struct MutationGuard {
    _lock: tokio::sync::OwnedMutexGuard<()>,
    pending: std::sync::Arc<parking_lot::Mutex<Option<PendingMutation>>>,
    #[cfg(test)]
    checkpoint_pause: Option<std::sync::Arc<CheckpointPause>>,
}
impl MutationGuard {
    /// Resolve the previous group while retaining exclusive mutation ownership.
    /// Used by multi-group requests (monolithic POST initiation then completion).
    pub async fn reconcile<Eng: Send + Sync + 'static>(&self, engine: &Eng) -> Result<(), StoreError> {
        let original = { self.pending.lock().clone() };
        if let Some(original) = original {
            match &original {
                PendingMutation::Reference(id) => {
                    let store = bound_store(engine)?;
                    match store.recover(id).await? {
                        service_common::payload_store::CommitStatus::InProgress => return Err(StoreError::Busy),
                        service_common::payload_store::CommitStatus::Committed(_) | service_common::payload_store::CommitStatus::NotCommitted => {},
                    }
                }
                PendingMutation::Seal(receiver) => {
                    let outcome = receiver.borrow().clone();
                    if outcome.is_none() {
                        if receiver.has_changed().is_err() {
                            if receiver.borrow().is_none() { return Err(StoreError::Worker); }
                        } else { return Err(StoreError::Busy); }
                    }
                }
                PendingMutation::Checkpoint(_) => {
                    // A terminal Result (success OR failure) is published only
                    // AFTER the owned stage and actual B2 call have completed.
                    if original.checkpoint_outcome()?.is_none() { return Err(StoreError::Busy); }
                }
            }
            super::catalog::restore_repository_catalog_guarded(engine, self).await?;
            // No await after atomic composed reload. Cancellation/errors above
            // retain the outcome, including while the loader itself is queued.
            *self.pending.lock() = None;
        }
        Ok(())
    }

    /// Transfer the stage to one service-owned task BEFORE yielding. The engine
    /// retains its result receiver; request Drop cannot cancel the checkpoint or
    /// discard the only completion witness. One unresolved job per mutation gate.
    pub async fn checkpoint(
        &self, stage: StagedPayload, metadata: service_common::payload_store::ProtocolMetadata,
    ) -> CheckpointResult {
        #[cfg(test)]
        let pause = self.checkpoint_pause.clone();
        let mut receiver = self.start_checkpoint(async move {
            let mut stage = stage;
            #[cfg(test)]
            if let Some(pause) = pause {
                pause.entered.add_permits(1);
                let permit = pause.release.acquire().await.map_err(|_| StoreError::Worker)?;
                permit.forget();
            }
            let result = stage.checkpoint(metadata).await;
            drop(stage); // release the actual writer lease BEFORE terminal outcome
            result
        })?;
        loop {
            let result = { receiver.borrow().clone() };
            if let Some(result) = result { return result; }
            receiver.changed().await.map_err(|_| StoreError::Worker)?;
        }
    }

    /// Seal has the same cancellation ownership as a byte-ACK checkpoint.
    /// This task consumes the actual stage; only the awaited B2 terminal outcome
    /// (after its worker lease drops) may permit catalog reload/capacity credit.
    pub async fn seal(&self, stage: StagedPayload, expected: service_common::payload_store::IntegrityExpectation) -> SealResult {
        let (sender, mut receiver) = tokio::sync::watch::channel(None);
        {
            let mut pending = self.pending.lock();
            if pending.is_some() { return Err(StoreError::Busy); }
            *pending = Some(PendingMutation::Seal(receiver.clone()));
        }
        #[cfg(test)]
        let pause = self.checkpoint_pause.clone();
        tokio::spawn(async move {
            #[cfg(test)]
            if let Some(pause) = pause {
                pause.entered.add_permits(1);
                let Ok(permit) = pause.release.acquire().await else { return; };
                permit.forget();
            }
            let result = stage.seal(expected).await.map(|sealed| sealed.handle().clone());
            sender.send_replace(Some(result));
        });
        loop {
            let result = { receiver.borrow().clone() };
            if let Some(result) = result { return result; }
            receiver.changed().await.map_err(|_| StoreError::Worker)?;
        }
    }

    fn start_checkpoint<F>(&self, work: F) -> Result<tokio::sync::watch::Receiver<Option<CheckpointResult>>, StoreError>
    where F: std::future::Future<Output = CheckpointResult> + Send + 'static {
        let (sender, receiver) = tokio::sync::watch::channel(None);
        {
            let mut pending = self.pending.lock();
            if pending.is_some() { return Err(StoreError::Busy); }
            // Reserved synchronously before spawn or any cancellation-capable await.
            *pending = Some(PendingMutation::Checkpoint(receiver.clone()));
        }
        tokio::spawn(async move {
            let result = work.await;
            sender.send_replace(Some(result));
        });
        Ok(receiver)
    }

    pub async fn commit(&self, store: &PayloadStore, transaction: service_common::payload_store::ReferenceTransaction)
        -> Result<service_common::payload_store::CommitReceipt, service_common::payload_store::CommitError> {
        {
            let mut pending = self.pending.lock();
            if pending.is_some() { return Err(StoreError::Busy.into()); }
            // Reserve before the first commit await. Even a definitive error is
            // reconciled on next admission; no cancellation window loses the ID.
            *pending = Some(PendingMutation::Reference(transaction.id.clone()));
        }
        store.commit(transaction).await
    }
}

pub async fn mutation_guard<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Result<MutationGuard, StoreError> {
    let concrete = (engine as &dyn std::any::Any)
        .downcast_ref::<engine::InMemoryEngine>()
        .ok_or(StoreError::Invalid("ECR mutation engine binding"))?;
    let gate = { concrete.state::<MutationGate>().lock().0.clone() };
    let pending = { concrete.state::<PendingPublication>().lock().0.clone() };
    let guard = MutationGuard { _lock: gate.lock_owned().await, pending, #[cfg(test)] checkpoint_pause: None };
    guard.reconcile(engine).await?;
    Ok(guard)
}

/// A bounded document is inline only while a push is being planned. Published
/// records retain a handle, not every repository's manifest bytes in RAM.
#[derive(Clone, Debug)]
pub enum ManifestDocument {
    Pending(String),
    Stored(service_common::payload_store::PayloadHandle),
}
impl ManifestDocument {
    pub fn len(&self) -> usize {
        match self { Self::Pending(v) => v.len(), Self::Stored(v) => v.len() as usize }
    }
    pub async fn read(&self, store: &PayloadStore) -> Result<String, StoreError> {
        match self {
            Self::Pending(value) => {
                if value.len() > 4 * 1024 * 1024 { return Err(StoreError::Integrity); }
                Ok(value.clone())
            }
            Self::Stored(handle) => read_document(store, handle).await,
        }
    }
}
pub async fn read_document(store: &PayloadStore, handle: &service_common::payload_store::PayloadHandle) -> Result<String, StoreError> {
    if handle.len() > 4 * 1024 * 1024 { return Err(StoreError::Integrity); }
    let mut reader = store.open(handle, None).await?;
    let mut value = Vec::with_capacity(handle.len() as usize);
    while let Some(chunk) = reader.next_chunk().await? {
        if value.len().checked_add(chunk.len()).is_none_or(|v| v > handle.len() as usize) { return Err(StoreError::Integrity); }
        value.extend_from_slice(&chunk);
    }
    if value.len() as u64 != handle.len() { return Err(StoreError::Integrity); }
    String::from_utf8(value).map_err(|_| StoreError::Integrity)
}


#[cfg(test)]
struct CheckpointPause { entered: tokio::sync::Semaphore, release: tokio::sync::Semaphore }

#[cfg(test)]
mod checkpoint_cancellation_tests {
    use super::*;
    use service_common::payload_store::{ProtocolMetadata, StageSpec, StoreConfig, StoreLimits};

    #[tokio::test]
    async fn cancelled_real_checkpoint_waiter_retains_running_and_terminal_outcome() {
        let limits = StoreLimits {
            max_storage_bytes:1024*1024, max_catalog_bytes:1024*1024, max_stage_bytes:1024,
            max_chunk_bytes:64, max_metadata_bytes:1024, max_records_per_transaction:16,
            max_references_per_record:16, max_page_records:16, max_page_bytes:4096,
            max_stages:16, max_records:16, max_commits:16, max_transaction_bytes:16*1024,
            max_inflight_operations:8, max_read_leases:4, max_data_files:32,
        };
        let store = PayloadStore::open_store(StoreConfig::memory("checkpoint-cancel","fixture",limits).unwrap()).await.unwrap();
        let owner = registry_owner().unwrap();
        let mut stage = store.begin(StageSpec { owner:owner.clone(), upload_key:None, expected_length:None, max_bytes:1024 }).await.unwrap();
        let metadata = ProtocolMetadata::new("fixture",1,Bytes::from_static(b"{}")).unwrap();
        stage.append(0,Bytes::from_static(b"old")).await.unwrap();
        let prefix = stage.checkpoint(metadata.clone()).await.unwrap();
        let id = stage.id().clone();
        stage.append(prefix.offset(),Bytes::from_static(b"new")).await.unwrap();
        let pending = std::sync::Arc::new(parking_lot::Mutex::new(None));
        let lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        let pause = std::sync::Arc::new(CheckpointPause { entered:tokio::sync::Semaphore::new(0),release:tokio::sync::Semaphore::new(0) });
        let guard = MutationGuard { _lock:lock.clone().lock_owned().await, pending:pending.clone(), checkpoint_pause:Some(pause.clone()) };
        let request = tokio::spawn(async move { guard.checkpoint(stage,metadata).await });
        let entered = pause.entered.acquire().await.unwrap(); entered.forget();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        let original = pending.lock().clone().unwrap();
        assert!(original.checkpoint_outcome().unwrap().is_none());
        assert_eq!(store.lookup_upload(&owner,id.as_str()).await.unwrap().unwrap().checkpoint.offset(),prefix.offset(), "early lookup is NOT completion proof");
        let next = MutationGuard { _lock:lock.lock_owned().await,pending:pending.clone(),checkpoint_pause:None };
        assert_eq!(next.reconcile(&engine::InMemoryEngine::new()).await.unwrap_err(),StoreError::Busy);
        // Cancelling reconciliation cannot take/drop the engine's only receiver.
        assert!(pending.lock().is_some());
        let PendingMutation::Checkpoint(mut receiver) = original else { panic!("checkpoint outcome missing"); };
        pause.release.add_permits(1);
        loop {
            if receiver.borrow().is_some() { break; }
            receiver.changed().await.unwrap();
        }
        let result = receiver.borrow().clone().unwrap().unwrap();
        assert_eq!(result.offset(),6);
        assert_eq!(store.lookup_upload(&owner,id.as_str()).await.unwrap().unwrap().checkpoint.offset(),6);
        assert_eq!(pending.lock().as_ref().unwrap().checkpoint_outcome().unwrap().unwrap().unwrap().offset(),6);
        let engine = engine::InMemoryEngine::new();
        engine.state::<PayloadStoreBinding>().lock().store = Some(store.clone());
        next.reconcile(&engine).await.unwrap();
        assert!(pending.lock().is_none(), "only completed composed reload retires the outcome");
        // Terminal outcome survives the original waiter, and the writer lease
        // has actually been dropped before another owner can resume it.
        let resumed = store.resume(&id,result).await.unwrap();
        drop(resumed);
    }

    #[tokio::test]
    async fn cancelled_real_seal_waiter_retains_suffix_and_terminal_outcome() {
        let limits = StoreLimits {
            max_storage_bytes:1024*1024, max_catalog_bytes:1024*1024, max_stage_bytes:1024,
            max_chunk_bytes:64, max_metadata_bytes:1024, max_records_per_transaction:16,
            max_references_per_record:16, max_page_records:16, max_page_bytes:4096,
            max_stages:16, max_records:16, max_commits:16, max_transaction_bytes:16*1024,
            max_inflight_operations:8, max_read_leases:4, max_data_files:32,
        };
        let store = PayloadStore::open_store(StoreConfig::memory("checkpoint-cancel","fixture",limits).unwrap()).await.unwrap();
        let owner = registry_owner().unwrap();
        let mut stage = store.begin(StageSpec { owner:owner.clone(), upload_key:None, expected_length:None, max_bytes:1024 }).await.unwrap();
        let metadata = ProtocolMetadata::new("fixture",1,Bytes::from_static(b"{}")).unwrap();
        stage.append(0,Bytes::from_static(b"old")).await.unwrap();
        let prefix = stage.checkpoint(metadata.clone()).await.unwrap();
        let id = stage.id().clone();
        stage.append(prefix.offset(),Bytes::from_static(b"new")).await.unwrap();
        let pending = std::sync::Arc::new(parking_lot::Mutex::new(None));
        let lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        let pause = std::sync::Arc::new(CheckpointPause { entered:tokio::sync::Semaphore::new(0),release:tokio::sync::Semaphore::new(0) });
        let guard = MutationGuard { _lock:lock.clone().lock_owned().await, pending:pending.clone(), checkpoint_pause:Some(pause.clone()) };
        let request = tokio::spawn(async move { guard.seal(stage, service_common::payload_store::IntegrityExpectation { length:Some(6), sha256:None }).await });
        let entered = pause.entered.acquire().await.unwrap(); entered.forget();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        let original = pending.lock().clone().unwrap();
        assert!(matches!(&original, PendingMutation::Seal(receiver) if receiver.borrow().is_none()));
        assert_eq!(store.lookup_upload(&owner,id.as_str()).await.unwrap().unwrap().checkpoint.offset(),prefix.offset(), "early lookup is NOT completion proof");
        let next = MutationGuard { _lock:lock.lock_owned().await,pending:pending.clone(),checkpoint_pause:None };
        assert_eq!(next.reconcile(&engine::InMemoryEngine::new()).await.unwrap_err(),StoreError::Busy);
        // Cancelling reconciliation cannot take/drop the engine's only receiver.
        assert!(pending.lock().is_some());
        let PendingMutation::Seal(mut receiver) = original else { panic!("checkpoint outcome missing"); };
        pause.release.add_permits(1);
        loop {
            if receiver.borrow().is_some() { break; }
            receiver.changed().await.unwrap();
        }
        let result = receiver.borrow().clone().unwrap().unwrap();
        assert_eq!(result.len(),6);
        let snapshot = store.lookup_upload(&owner,id.as_str()).await.unwrap().unwrap();
        assert_eq!(snapshot.checkpoint.offset(),3);
        assert_eq!(snapshot.sealed.as_ref().unwrap().len(),6);
        assert!(pending.lock().is_some());
        let engine = engine::InMemoryEngine::new();
        engine.state::<PayloadStoreBinding>().lock().store = Some(store.clone());
        next.reconcile(&engine).await.unwrap();
        assert!(pending.lock().is_none(), "only completed composed reload retires the outcome");
        // Terminal outcome survives the original waiter, and the writer lease
        // has actually been dropped before another owner can resume it.
        assert!(store.resume(&id,prefix).await.is_err(), "sealed stage cannot be resumed");
        let mut reader = store.open(&result,None).await.unwrap();
        assert_eq!(reader.next_chunk().await.unwrap().unwrap().as_ref(),b"oldnew");
    }

    #[tokio::test]
    async fn closed_seal_without_terminal_keeps_unknown_slot() {
        let (sender,receiver) = tokio::sync::watch::channel(None);
        drop(sender);
        let pending = std::sync::Arc::new(parking_lot::Mutex::new(Some(PendingMutation::Seal(receiver))));
        let lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        let guard = MutationGuard { _lock:lock.lock_owned().await, pending:pending.clone(), checkpoint_pause:None };
        assert_eq!(guard.reconcile(&engine::InMemoryEngine::new()).await.unwrap_err(),StoreError::Worker);
        assert!(pending.lock().is_some());
    }

    #[tokio::test]
    async fn closed_without_terminal_result_is_unknown_not_quiescent() {
        let (sender,receiver) = tokio::sync::watch::channel(None);
        drop(sender);
        let pending = PendingMutation::Checkpoint(receiver);
        assert_eq!(pending.checkpoint_outcome().unwrap_err(),StoreError::Worker);
    }
}
