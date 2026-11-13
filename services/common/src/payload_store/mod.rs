// SPDX-License-Identifier: CC0-1.0

//! One payload catalog for streamed S3/ECR bytes. No authentication capability lives here.
//! Persistent success follows content fsync/install and the SQLite reference transaction.
//! IO runs on bounded caller-sized chunks through spawn_blocking; no service mutex crosses await.
//! Stages/checkpoints survive handle abandonment. No boot garbage collection, reset or host-path API.
mod backend;
mod storage;
mod types;
pub use types::*;

use backend::Backend;
use bytes::Bytes;
use std::{
    collections::HashMap,
    io::{self, Seek, SeekFrom},
    ops::Range,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use storage::ReadSource;
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

/// Install once in the engine's existing typed state map BEFORE restore/listener startup.
/// This slot never opens a store, inspects environment, or manufactures an ephemeral fallback.
#[derive(Default)]
pub struct PayloadStoreBinding {
    pub store: Option<PayloadStore>,
}

#[derive(Clone)]
pub struct PayloadStore {
    inner: Arc<Mutex<Backend>>,
    scope: Arc<Scope>,
    durability: Durability,
    chunk: usize,
    append_chunk: usize,
    composition_limit: usize,
    operations: Arc<tokio::sync::Semaphore>,
    readers: Arc<tokio::sync::Semaphore>,
    pending: Arc<Mutex<HashMap<String, usize>>>,
    #[cfg(test)]
    commit_gate: Arc<Mutex<Option<Arc<CommitGate>>>>,
}
impl std::fmt::Debug for PayloadStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PayloadStore")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}
impl PayloadStore {
    pub async fn open_store(config: StoreConfig) -> Result<Self, StoreError> {
        let backend = tokio::task::spawn_blocking(move || Backend::open(config))
            .await
            .map_err(|_| StoreError::Worker)??;
        let scope = backend.scope.clone();
        let durability = backend.durability;
        let append_chunk = backend.limits.max_chunk_bytes;
        let chunk = append_chunk.min(64 * 1024);
        let composition_limit = backend.limits.max_references_per_record;
        let operations = Arc::new(tokio::sync::Semaphore::new(
            backend.limits.max_inflight_operations,
        ));
        let readers = Arc::new(tokio::sync::Semaphore::new(backend.limits.max_read_leases));
        Ok(Self {
            inner: Arc::new(Mutex::new(backend)),
            scope,
            durability,
            chunk,
            append_chunk,
            composition_limit,
            operations,
            readers,
            pending: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(test)]
            commit_gate: Arc::new(Mutex::new(None)),
        })
    }
    pub fn durability(&self) -> Durability {
        self.durability
    }
    pub fn max_append_bytes(&self) -> usize {
        self.append_chunk
    }
    async fn call<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Backend) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, StoreError> {
        let permit = self
            .operations
            .clone()
            .try_acquire_owned()
            .map_err(|_| StoreError::Busy)?;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut backend = inner.lock().map_err(|_| StoreError::Worker)?;
            backend.healthy()?;
            work(&mut backend)
        })
        .await
        .map_err(|_| StoreError::Worker)?
    }
    pub fn new_commit_id(&self) -> CommitId {
        CommitId {
            scope: self.scope.clone(),
            id: uuid::Uuid::new_v4().to_string(),
        }
    }
    /// Restore an opaque correlation id, never a grant. recover still consults the catalog.
    pub fn commit_id(&self, opaque: &str) -> Result<CommitId, StoreError> {
        let id = uuid::Uuid::parse_str(opaque).map_err(|_| StoreError::Invalid("CommitId"))?;
        if id.to_string() != opaque {
            return Err(StoreError::Invalid("CommitId"));
        }
        Ok(CommitId {
            scope: self.scope.clone(),
            id: opaque.into(),
        })
    }
    pub async fn inventory(&self) -> Result<BootInventory, StoreError> {
        self.call(|b| b.inventory()).await
    }
    pub async fn begin(&self, spec: StageSpec) -> Result<StagedPayload, StoreError> {
        let lease = Arc::new(());
        let held = lease.clone();
        let owner = spec.owner.clone();
        let id = self.call(move |b| b.begin(spec, &held)).await?;
        Ok(StagedPayload {
            store: self.clone(),
            id,
            owner,
            lease,
        })
    }
    /// Explicit resume validates the last durable prefix, then discards ONLY its unacknowledged tail.
    pub async fn resume(
        &self,
        id: &UploadId,
        checkpoint: Checkpoint,
    ) -> Result<StagedPayload, StoreError> {
        let lease = Arc::new(());
        let held = lease.clone();
        let upload = id.clone();
        let owner = self
            .call(move |b| b.resume(&upload, &checkpoint, &held))
            .await?;
        Ok(StagedPayload {
            store: self.clone(),
            id: id.clone(),
            owner,
            lease,
        })
    }
    pub async fn lookup_upload(
        &self,
        owner: &OwnerKey,
        opaque: &str,
    ) -> Result<Option<UploadSnapshot>, StoreError> {
        let owner = owner.clone();
        let key = opaque.to_owned();
        self.call(move |b| b.lookup_upload(&owner, &key, false))
            .await
    }
    /// Explicit alias lookup; aliases never shadow opaque UploadIds.
    pub async fn lookup_upload_key(
        &self,
        owner: &OwnerKey,
        upload_key: &str,
    ) -> Result<Option<UploadSnapshot>, StoreError> {
        let owner = owner.clone();
        let key = upload_key.to_owned();
        self.call(move |b| b.lookup_upload(&owner, &key, true))
            .await
    }
    /// Only reconstructs a handle already sealed in this catalog; not arbitrary digest adoption.
    pub async fn resolve_payload(
        &self,
        digest: Sha256Digest,
        length: u64,
    ) -> Result<PayloadHandle, StoreError> {
        self.call(move |b| b.resolve_payload(digest, length)).await
    }
    pub async fn record(&self, key: &RecordKey) -> Result<Option<RecordSnapshot>, StoreError> {
        let key = key.clone();
        self.call(move |b| b.record(&key)).await
    }
    pub async fn list_records(
        &self,
        owner: &OwnerKey,
        cursor: Option<RecordCursor>,
        limit: usize,
    ) -> Result<RecordPage, StoreError> {
        let owner = owner.clone();
        self.call(move |b| b.records(&owner, cursor, limit)).await
    }
    pub async fn commit(
        &self,
        transaction: ReferenceTransaction,
    ) -> Result<CommitReceipt, CommitError> {
        let permit = self
            .operations
            .clone()
            .try_acquire_owned()
            .map_err(|_| CommitError::Store(StoreError::Busy))?;
        let inner = self.inner.clone();
        let id = transaction.id.clone();
        let worker_id = id.clone();
        let flight =
            CommitFlight::admit(self.pending.clone(), id.id.clone()).map_err(CommitError::Store)?;
        #[cfg(test)]
        let gate = self
            .commit_gate
            .lock()
            .map_err(|_| CommitError::Store(StoreError::Worker))?
            .clone();
        // Cancellation/JoinError cannot prove the transaction did not commit. Caller retains id.
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _flight = flight;
            #[cfg(test)]
            let _finished = gate.clone().map(FinishGate);
            #[cfg(test)]
            if let Some(gate) = &gate {
                gate.block();
            }
            let mut backend = inner
                .lock()
                .map_err(|_| CommitError::OutcomeUnknown(worker_id.clone()))?;
            if backend.healthy().is_err() {
                return Err(CommitError::OutcomeUnknown(worker_id));
            }
            backend.commit(transaction)
        })
        .await
        .map_err(|_| CommitError::OutcomeUnknown(id))?
    }
    pub async fn recover(&self, id: &CommitId) -> Result<CommitStatus, StoreError> {
        let id = id.clone();
        let pending = self.pending.clone();
        self.call(move |b| {
            let status = b.recover(&id)?;
            if status == CommitStatus::NotCommitted
                && pending
                    .lock()
                    .map_err(|_| StoreError::Worker)?
                    .contains_key(&id.id)
            {
                Ok(CommitStatus::InProgress)
            } else {
                Ok(status)
            }
        })
        .await
    }
    pub async fn open(
        &self,
        handle: &PayloadHandle,
        range: Option<Range<u64>>,
    ) -> Result<PayloadReader, StoreError> {
        let read_permit = self
            .readers
            .clone()
            .try_acquire_owned()
            .map_err(|_| StoreError::Busy)?;
        let range = range.unwrap_or(0..handle.len());
        if range.start > range.end || range.end > handle.len() {
            return Err(StoreError::Invalid("half-open payload range"));
        }
        let payload = handle.clone();
        let start = range.start;
        let length = range.end - range.start;
        let source = self
            .call(move |b| {
                let mut source = b.read(&payload)?;
                if let ReadSource::File(file) = &mut source {
                    file.seek(SeekFrom::Start(start))
                        .map_err(|_| StoreError::Io)?;
                }
                Ok(source)
            })
            .await?;
        let source = match source {
            ReadSource::Memory(bytes) => Reader::Memory {
                bytes,
                position: range.start as usize,
            },
            ReadSource::File(file) => {
                let mut file = tokio::fs::File::from_std(file);
                file.set_max_buf_size(self.chunk);
                Reader::File(file.take(length))
            }
        };
        Ok(PayloadReader {
            source,
            remaining: length,
            length,
            chunk: self.chunk,
            _lease: self.inner.clone(),
            _read_permit: read_permit,
        })
    }
    /// Bounded-copy composition: no intermediate whole Vec, and no implicit publication.
    pub async fn compose(
        &self,
        spec: StageSpec,
        parts: &[PayloadHandle],
        expected: IntegrityExpectation,
    ) -> Result<SealedPayload, StoreError> {
        if parts.len() > self.composition_limit {
            return Err(StoreError::QuotaExceeded);
        }
        let length = parts.iter().try_fold(0u64, |total, handle| {
            total
                .checked_add(handle.len())
                .ok_or(StoreError::QuotaExceeded)
        })?;
        if length > spec.max_bytes
            || spec
                .expected_length
                .is_some_and(|expected| expected != length)
        {
            return Err(StoreError::Integrity);
        }
        let mut stage = self.begin(spec).await?;
        let result = async {
            let mut offset = 0;
            for part in parts {
                let mut reader = self.open(part, None).await?;
                while let Some(bytes) = reader.next_chunk().await? {
                    offset = stage.append(offset, bytes).await?;
                }
            }
            Ok::<_, StoreError>(())
        }
        .await;
        if let Err(error) = result {
            let _ = stage.abort().await;
            return Err(error);
        }
        stage.seal(expected).await
    }
}

#[cfg(test)]
#[derive(Default)]
struct CommitGate {
    state: Mutex<(bool, bool, bool)>,
    changed: std::sync::Condvar,
}
#[cfg(test)]
impl CommitGate {
    fn block(&self) {
        let mut state = self.state.lock().unwrap();
        state.0 = true;
        self.changed.notify_all();
        while !state.1 {
            state = self.changed.wait(state).unwrap();
        }
    }
    fn entered(&self) {
        let mut state = self.state.lock().unwrap();
        while !state.0 {
            state = self.changed.wait(state).unwrap();
        }
    }
    fn release(&self) {
        let mut state = self.state.lock().unwrap();
        state.1 = true;
        self.changed.notify_all();
    }
    fn finished(&self) {
        let mut state = self.state.lock().unwrap();
        state.2 = true;
        self.changed.notify_all();
    }
    fn wait_finished(&self) {
        let mut state = self.state.lock().unwrap();
        while !state.2 {
            state = self.changed.wait(state).unwrap();
        }
    }
}

#[cfg(test)]
struct FinishGate(Arc<CommitGate>);
#[cfg(test)]
impl Drop for FinishGate {
    fn drop(&mut self) {
        self.0.finished();
    }
}
#[cfg(test)]
struct ReleaseGate(Arc<CommitGate>);
#[cfg(test)]
impl Drop for ReleaseGate {
    fn drop(&mut self) {
        self.0.release();
    }
}

// Admission is transient request lifetime, not a second durable transaction index.
struct CommitFlight {
    pending: Arc<Mutex<HashMap<String, usize>>>,
    id: String,
}
impl CommitFlight {
    fn admit(pending: Arc<Mutex<HashMap<String, usize>>>, id: String) -> Result<Self, StoreError> {
        {
            let mut map = pending.lock().map_err(|_| StoreError::Worker)?;
            let count = map.entry(id.clone()).or_default();
            *count = count.checked_add(1).ok_or(StoreError::QuotaExceeded)?;
        }
        Ok(Self { pending, id })
    }
}
impl Drop for CommitFlight {
    fn drop(&mut self) {
        if let Ok(mut map) = self.pending.lock() {
            if let Some(count) = map.get_mut(&self.id) {
                *count -= 1;
                if *count == 0 {
                    map.remove(&self.id);
                }
            }
        }
    }
}

/// A single writer lease. In-flight blocking work holds its token even if its future is dropped.
/// Drop never erases a durable prefix or reclaims a sealed payload.
pub struct StagedPayload {
    store: PayloadStore,
    id: UploadId,
    owner: OwnerKey,
    lease: Arc<()>,
}
impl StagedPayload {
    pub fn id(&self) -> &UploadId {
        &self.id
    }
    pub fn owner(&self) -> &OwnerKey {
        &self.owner
    }
    pub fn max_append_bytes(&self) -> usize {
        self.store.max_append_bytes()
    }
    pub async fn progress(&self) -> Result<StageProgress, StoreError> {
        let id = self.id.clone();
        let lease = self.lease.clone();
        self.store.call(move |b| b.progress(&id, &lease)).await
    }
    pub async fn append(&mut self, offset: u64, bytes: Bytes) -> Result<u64, StoreError> {
        let id = self.id.clone();
        let lease = self.lease.clone();
        self.store
            .call(move |b| b.append(&id, &lease, offset, bytes))
            .await
    }
    /// Invoke AFTER the request's raw-wire/service validation. Store does not accept an auth boolean.
    pub async fn checkpoint(
        &mut self,
        metadata: ProtocolMetadata,
    ) -> Result<Checkpoint, StoreError> {
        let id = self.id.clone();
        let lease = self.lease.clone();
        self.store
            .call(move |b| b.checkpoint(&id, &lease, metadata))
            .await
    }
    pub async fn rollback_to(&mut self, previous: Checkpoint) -> Result<(), StoreError> {
        let id = self.id.clone();
        let lease = self.lease.clone();
        self.store
            .call(move |b| b.rollback(&id, &lease, &previous))
            .await
    }
    /// Error consumes only this lease; lookup_upload/resume retains the prior durable checkpoint.
    pub async fn seal(self, expected: IntegrityExpectation) -> Result<SealedPayload, StoreError> {
        let id = self.id.clone();
        let lease = self.lease.clone();
        self.store
            .call(move |b| b.seal(&id, &lease, expected))
            .await
    }
    /// Refuses acknowledged/sealed stages: retire those in the grouped reference transaction.
    pub async fn abort(self) -> Result<(), StoreError> {
        let id = self.id.clone();
        let lease = self.lease.clone();
        self.store.call(move |b| b.abort(&id, &lease)).await
    }
}

enum Reader {
    Memory { bytes: Bytes, position: usize },
    File(tokio::io::Take<tokio::fs::File>),
}
/// Read lease survives record removal. No public path, Vec-returning API or GC-on-drop.
pub struct PayloadReader {
    source: Reader,
    remaining: u64,
    length: u64,
    chunk: usize,
    _lease: Arc<Mutex<Backend>>,
    _read_permit: tokio::sync::OwnedSemaphorePermit,
}
impl PayloadReader {
    pub fn len(&self) -> u64 {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
    pub async fn next_chunk(&mut self) -> Result<Option<Bytes>, StoreError> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let size = self
            .chunk
            .min(usize::try_from(self.remaining).unwrap_or(self.chunk));
        let mut bytes = vec![0; size];
        let read = self
            .read(&mut bytes)
            .await
            .map_err(|_| StoreError::Integrity)?;
        if read == 0 {
            return Err(StoreError::Integrity);
        }
        bytes.truncate(read);
        Ok(Some(Bytes::from(bytes)))
    }
}
impl AsyncRead for PayloadReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.remaining == 0 || output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let before = output.filled().len();
        let result = match &mut this.source {
            Reader::Memory { bytes, position } => {
                let count = this
                    .chunk
                    .min(output.remaining())
                    .min(this.remaining as usize);
                output.put_slice(&bytes[*position..*position + count]);
                *position += count;
                Poll::Ready(Ok(()))
            }
            Reader::File(file) => Pin::new(file).poll_read(cx, output),
        };
        if let Poll::Ready(Ok(())) = &result {
            let read = output.filled().len() - before;
            if read == 0 {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "payload integrity",
                )));
            }
            this.remaining -= read as u64;
        }
        result
    }
}

#[cfg(test)]
mod tests;
