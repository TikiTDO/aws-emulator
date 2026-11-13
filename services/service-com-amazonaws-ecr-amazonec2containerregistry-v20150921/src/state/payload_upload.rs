// SPDX-License-Identifier: CC0-1.0

//! Durable upload identity and protocol metadata. The stage checkpoint owns the
//! acknowledged offset; this index never caches a second offset or byte vector.
//! No helper authenticates the caller or selects a repository incarnation.

use bytes::Bytes;
use serde_json::Value;
use service_common::payload_store::{
    Checkpoint, CommitError, PayloadStore, ProtocolMetadata, RecordChange,
    RecordCondition, RecordData, RecordKey, RecordRevision, ReferenceTransaction,
    StageSpec, StoreError, UploadSnapshot,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct UploadProtocol {
    pub principal: Option<String>,
    pub repository: String,
    pub generation: String,
    pub part_size: i64,
    pub started_at: SystemTime,
}
impl UploadProtocol {
    pub fn encode(&self) -> Result<ProtocolMetadata, StoreError> {
        let started = self.started_at.duration_since(UNIX_EPOCH)
            .map_err(|_| StoreError::Invalid("upload start time"))?;
        if self.part_size <= 0 || self.principal.as_ref().is_some_and(|v| v.len() > 4096) {
            return Err(StoreError::Invalid("upload protocol"));
        }
        let bytes = serde_json::to_vec(&serde_json::json!({
            "principal": self.principal, "repository": self.repository, "generation": self.generation, "partSize": self.part_size,
            "startedSeconds": started.as_secs(), "startedNanos": started.subsec_nanos(),
        })).map_err(|_| StoreError::Integrity)?;
        ProtocolMetadata::new("ecr-layer-upload", 1, Bytes::from(bytes))
    }
    pub fn decode(value: &ProtocolMetadata) -> Result<Self, StoreError> {
        if value.schema != "ecr-layer-upload" || value.version != 1 || value.bytes.len() > 8192 {
            return Err(StoreError::Integrity);
        }
        let value: serde_json::Value = serde_json::from_slice(&value.bytes).map_err(|_| StoreError::Integrity)?;
        let object = value.as_object().ok_or(StoreError::Integrity)?;
        if object.len() != 6 { return Err(StoreError::Integrity); }
        let principal = match object.get("principal") {
            Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(v)) if v.len() <= 4096 => Some(v.clone()),
            _ => return Err(StoreError::Integrity),
        };
        let part_size = object.get("partSize").and_then(serde_json::Value::as_i64)
            .filter(|v| *v > 0).ok_or(StoreError::Integrity)?;
        let seconds = object.get("startedSeconds").and_then(serde_json::Value::as_u64).ok_or(StoreError::Integrity)?;
        let nanos = object.get("startedNanos").and_then(serde_json::Value::as_u64)
            .filter(|v| *v < 1_000_000_000).ok_or(StoreError::Integrity)?;
        let started_at = UNIX_EPOCH.checked_add(Duration::new(seconds, nanos as u32)).ok_or(StoreError::Integrity)?;
        let repository = object.get("repository").and_then(Value::as_str).filter(|v| !v.is_empty() && v.len() <= 256).ok_or(StoreError::Integrity)?.to_string();
        let generation = object.get("generation").and_then(Value::as_str).filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit())).ok_or(StoreError::Integrity)?.to_string();
        Ok(Self { principal, repository, generation, part_size, started_at })
    }
}

pub struct BoundUpload {
    pub scope: super::payloads::RepositoryScope,
    pub snapshot: UploadSnapshot,
    pub protocol: UploadProtocol,
    pub index: RecordKey,
    pub revision: RecordRevision,
}

pub fn index_key(owner: &super::payloads::RepositoryScope, client_id: &str) -> Result<RecordKey, StoreError> {
    if client_id.is_empty() || client_id.len() > 128 || !client_id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
        return Err(StoreError::Invalid("upload id"));
    }
    owner.key("upload", client_id)
}

/// ACK only after both the empty checkpoint and its durable discovery record.
/// A failed/unknown commit does not abort the checkpoint or erase recovery data.
pub async fn initiate(
    mutation: &super::payloads::MutationGuard,
    store: &PayloadStore,
    mut owner: super::payloads::RepositoryScope,
    client_id: &str,
    protocol: UploadProtocol,
    max_bytes: u64,
) -> Result<BoundUpload, CommitError> {
    if protocol.repository != owner.name || protocol.generation != owner.generation { return Err(StoreError::Integrity.into()); }
    let index = index_key(&owner, client_id)?;
    let mut stage = store.begin(StageSpec {
        owner: owner.owner.clone(), upload_key: None,
        expected_length: None, max_bytes,
    }).await?;
    let checkpoint = stage.checkpoint(protocol.encode()?).await?;
    let id = stage.id().clone();
    drop(stage);
    let receipt = mutation.commit(store, ReferenceTransaction {
        id: store.new_commit_id(), owner: owner.owner.clone(),
        changes: vec![owner.assertion(store).await?, RecordChange {
            key: index, condition: RecordCondition::Absent,
            value: Some(RecordData {
                schema_version: 1,
                metadata: serde_json::json!({"kind":"ecr-upload", "id":id.as_str(), "repository":owner.name, "generation":owner.generation}),
                references: Vec::new(),
            }),
        }], retire: Vec::new(),
    }).await?;
    owner.revision = Some(receipt.revision);
    let bound = lookup(store, &owner, client_id).await?.ok_or(StoreError::Integrity)?;
    if bound.snapshot.checkpoint != checkpoint { return Err(StoreError::Integrity.into()); }
    Ok(bound)
}

/// Resolve from the catalog, never solely from an in-memory upload map. This is
/// also the restart path for a client continuing an acknowledged upload id.
pub async fn lookup(
    store: &PayloadStore, owner: &super::payloads::RepositoryScope, client_id: &str,
) -> Result<Option<BoundUpload>, StoreError> {
    owner.assertion(store).await?;
    let index = index_key(owner, client_id)?;
    let Some(record) = store.record(&index).await? else { return Ok(None); };
    let Some(value) = record.value else { return Ok(None); };
    let object = value.metadata.as_object().ok_or(StoreError::Integrity)?;
    if value.schema_version != 1 || object.len() != 4 || !value.references.is_empty()
        || object.get("kind").and_then(serde_json::Value::as_str) != Some("ecr-upload") {
        return Err(StoreError::Integrity);
    }
    let id = object.get("id").and_then(serde_json::Value::as_str).ok_or(StoreError::Integrity)?;
    let snapshot = store.lookup_upload(&owner.owner, id).await?.ok_or(StoreError::Integrity)?;
    let protocol = UploadProtocol::decode(snapshot.protocol.as_ref().ok_or(StoreError::Integrity)?)?;
    if protocol.repository != owner.name || protocol.generation != owner.generation
        || object.get("repository").and_then(Value::as_str) != Some(owner.name.as_str())
        || object.get("generation").and_then(Value::as_str) != Some(owner.generation.as_str()) { return Err(StoreError::Integrity); }
    Ok(Some(BoundUpload { scope: owner.clone(), snapshot, protocol, index, revision: record.revision }))
}

/// Used in the SAME transaction as layer publication or explicit cancellation.
/// The exact index revision and authoritative stage checkpoint prevent closing a
/// different upload. Retirement is logical; no physical reclamation is implied.
pub fn retirement(bound: &BoundUpload) -> (RecordChange, service_common::payload_store::StageRetirement) {
    (RecordChange {
        key: bound.index.clone(), condition: RecordCondition::Exact(bound.revision), value: None,
    }, service_common::payload_store::StageRetirement {
        upload: bound.snapshot.id.clone(), checkpoint: bound.snapshot.checkpoint.clone(),
    })
}

/// The range oracle is independent of storage and mutates nothing on refusal.
pub fn validate_part(
    checkpoint: &Checkpoint, first: i64, last: i64, length: usize, maximum: u64,
) -> Result<(), super::LayerPartRejection> {
    use super::LayerPartRejection as R;
    let declared = last.checked_sub(first).and_then(|v| v.checked_add(1)).filter(|v| *v > 0);
    if declared != Some(i64::try_from(length).unwrap_or(i64::MAX)) {
        return Err(R::PartSizeMismatch);
    }
    if u64::try_from(first).ok() != Some(checkpoint.offset()) { return Err(R::NonConsecutivePart); }
    if checkpoint.offset().checked_add(length as u64).is_none_or(|v| v > maximum) {
        return Err(R::UploadTooLarge);
    }
    Ok(())
}

/// Modeled UploadLayerPart reaches this only after its normal request decoder
/// and service range/cap checks. Native bodies use UploadSuffix + terminal EOF
/// receipt instead; this function is not a native authentication shortcut.
pub async fn append_modeled_part(
    mutation: &super::payloads::MutationGuard,
    store: &PayloadStore, bound: &BoundUpload, bytes: &[u8],
) -> Result<Checkpoint, StoreError> {
    let old = bound.snapshot.checkpoint.clone();
    let mut stage = store.resume(&bound.snapshot.id, old.clone()).await?;
    let mut offset = old.offset();
    for chunk in bytes.chunks(stage.max_append_bytes().min(64 * 1024)) {
        match stage.append(offset, Bytes::copy_from_slice(chunk)).await {
            Ok(next) if next == offset + chunk.len() as u64 => offset = next,
            Ok(_) => {
                stage.rollback_to(old).await?;
                return Err(StoreError::Integrity);
            }
            Err(error) => {
                // A failed append never replaces the previous ACK. Rollback is
                // explicit and checkpoint-bound; if it too fails, resume still
                // starts from the durable checkpoint, not a guessed progress.
                stage.rollback_to(old).await?;
                return Err(error);
            }
        }
    }
    mutation.checkpoint(stage, bound.protocol.encode()?).await
}

impl BoundUpload {
    pub fn index_projection(&self) -> super::LayerUpload {
        super::LayerUpload {
            registry_principal: self.protocol.principal.clone(),
            repository_name: self.protocol.repository.clone(),
            part_size: self.protocol.part_size,
            started_at: self.protocol.started_at,
            snapshot: self.snapshot.clone(),
        }
    }
}

/// Pressure-triggered logical retirement, never physical reclamation. At most
/// 80 index changes + 80 stage retirements + 80 distinct config assertions = 240
/// entries under the installed 256 budget. No duplicate config key in a group.
pub async fn reclaim_abandoned<Eng: Send + Sync + 'static>(
    engine: &Eng, mutation: &super::payloads::MutationGuard, store: &PayloadStore,
    now: SystemTime,
) -> Result<(), CommitError> {
    let state = super::service_state(engine).ok_or(StoreError::Invalid("ECR state binding"))?;
    let candidates = {
        let state = state.lock();
        if state.uploads.len() < super::MAX_LAYER_UPLOADS { return Ok(()); }
        state.abandoned_upload_candidates(now, super::MAX_LAYER_UPLOADS)
    };
    for batch in candidates.chunks(80) {
        let mut configs = std::collections::BTreeMap::new();
        let mut changes = Vec::new(); let mut retire = Vec::new();
        for (id, repository) in batch {
            let (scope, original_id) = {
                let state = state.lock();
                let Some(upload) = state.uploads.get(id) else { continue; };
                if upload.repository_name != *repository { return Err(StoreError::Integrity.into()); }
                let repo = state.repositories.get(repository).ok_or(StoreError::Integrity)?;
                (super::payloads::repository_owner(repo)?, upload.snapshot.id.clone())
            };
            let bound = lookup(store, &scope, id).await?.ok_or(StoreError::Integrity)?;
            if bound.snapshot.id != original_id { return Err(StoreError::Integrity.into()); }
            // The authoritative protocol, not an old RAM candidate, owns age.
            if !now.duration_since(bound.protocol.started_at).is_ok_and(|age| age >= super::ABANDONED_UPLOAD_AGE) { continue; }
            if !configs.contains_key(&scope.name) {
                configs.insert(scope.name.clone(), scope.assertion(store).await?);
            }
            let (close, retired) = retirement(&bound);
            changes.push(close); retire.push(retired);
        }
        if retire.is_empty() { continue; }
        changes.extend(configs.into_values());
        mutation.commit(store, ReferenceTransaction {
            id: store.new_commit_id(), owner: super::payloads::registry_owner()?, changes, retire,
        }).await?;
        // Resolve original outcome and reload authority before any slot/byte credit
        // or next group. Cancellation/Unknown preserves the pending correlation.
        mutation.reconcile(engine).await?;
    }
    Ok(())
}
