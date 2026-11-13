// SPDX-License-Identifier: CC0-1.0

use super::{
    storage::{ReadSource, Storage},
    types::*,
};
use bytes::Bytes;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    sync::{Arc, Weak},
};

fn db<T>(result: rusqlite::Result<T>) -> Result<T, StoreError> {
    result.map_err(|error| match error {
        rusqlite::Error::SqliteFailure(code, _) if code.code == rusqlite::ErrorCode::DiskFull => {
            StoreError::QuotaExceeded
        }
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            StoreError::Busy
        }
        _ => StoreError::Catalog,
    })
}
fn owner_text(owner: &OwnerKey) -> Result<String, StoreError> {
    serde_json::to_string(&[&owner.service, &owner.resource, &owner.generation])
        .map_err(|_| StoreError::Catalog)
}
fn owner_from(value: &str) -> Result<OwnerKey, StoreError> {
    let fields: [String; 3] = serde_json::from_str(value).map_err(|_| StoreError::Integrity)?;
    OwnerKey::new(&fields[0], &fields[1], &fields[2])
}
fn empty_digest() -> Sha256Digest {
    Sha256Digest(Sha256::digest([]).into())
}
fn encode(value: &Value, maximum: usize) -> Result<String, StoreError> {
    fn depth(value: &Value, remaining: usize) -> bool {
        if remaining == 0 {
            return false;
        }
        match value {
            Value::Array(a) => a.iter().all(|v| depth(v, remaining - 1)),
            Value::Object(o) => o.values().all(|v| depth(v, remaining - 1)),
            _ => true,
        }
    }
    if !depth(value, 32) {
        return Err(StoreError::Invalid("metadata nesting"));
    }
    let text = serde_json::to_string(value).map_err(|_| StoreError::Invalid("metadata"))?;
    if text.len() > maximum {
        return Err(StoreError::QuotaExceeded);
    }
    Ok(text)
}
fn protocol_json(value: &ProtocolMetadata, maximum: usize) -> Result<String, StoreError> {
    token(&value.schema)?;
    if value.version == 0 || value.bytes.len() > maximum {
        return Err(StoreError::Invalid("protocol metadata"));
    }
    encode(
        &json!({"schema":value.schema,"version":value.version,"bytes":value.bytes.as_ref()}),
        maximum,
    )
}
fn protocol_from(value: Option<String>) -> Result<Option<ProtocolMetadata>, StoreError> {
    value
        .map(|text| {
            let data: Value = serde_json::from_str(&text).map_err(|_| StoreError::Integrity)?;
            let bytes: Vec<u8> =
                serde_json::from_value(data["bytes"].clone()).map_err(|_| StoreError::Integrity)?;
            ProtocolMetadata::new(
                data["schema"].as_str().ok_or(StoreError::Integrity)?,
                data["version"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or(StoreError::Integrity)?,
                Bytes::from(bytes),
            )
        })
        .transpose()
}
struct Active {
    lease: Weak<()>,
    length: u64,
    hash: Sha256,
    interrupted: bool,
}
struct StageRow {
    owner: OwnerKey,
    maximum: u64,
    expected: Option<u64>,
    revision: u64,
    length: u64,
    digest: Sha256Digest,
    protocol: Option<ProtocolMetadata>,
    sealed: Option<Sha256Digest>,
    retired: bool,
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fault {
    PartialAppend,
    AfterCheckpoint,
    AfterInstall,
    BeforeCommit,
    AfterCommit,
}

pub(crate) struct Backend {
    pub scope: Arc<Scope>,
    pub limits: StoreLimits,
    pub durability: Durability,
    connection: Connection,
    storage: Storage,
    active: HashMap<String, Active>,
    degraded: HashSet<Sha256Digest>,
    degraded_stages: HashSet<String>,
    #[cfg(test)]
    pub(crate) fault: Option<Fault>,
}
impl Backend {
    pub fn open(config: StoreConfig) -> Result<Self, StoreError> {
        let scope = Arc::new(config.scope);
        let (storage, fresh, durability) = match config.mode {
            Mode::Memory => (Storage::memory(), true, Durability::Ephemeral),
            Mode::Volume(root) => {
                let (store, fresh) = Storage::volume(&root, &scope, config.limits.max_data_files)?;
                (store, fresh, Durability::Durable)
            }
        };
        let connection = match storage.catalog_path() {
            None => db(Connection::open_in_memory())?,
            Some(path) => {
                if !fresh {
                    let metadata =
                        fs::symlink_metadata(&path).map_err(|_| StoreError::Integrity)?;
                    if !metadata.is_file() || metadata.file_type().is_symlink() {
                        return Err(StoreError::Integrity);
                    }
                } else {
                    let mut options = fs::OpenOptions::new();
                    options.create_new(true).write(true);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::OpenOptionsExt;
                        options.mode(0o600);
                    }
                    options.open(&path).map_err(|_| StoreError::Io)?;
                }
                db(Connection::open_with_flags(
                    path,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                        | rusqlite::OpenFlags::SQLITE_OPEN_FULL_MUTEX,
                ))?
            }
        };
        db(connection.busy_timeout(std::time::Duration::from_secs(2)))?;
        let page_size: u64 = db(connection.query_row("PRAGMA page_size", [], |row| row.get(0)))?;
        let max_pages = config.limits.max_catalog_bytes / page_size;
        let admitted_pages: u64 = db(connection.query_row(
            &format!("PRAGMA max_page_count={max_pages}"),
            [],
            |row| row.get(0),
        ))?;
        if admitted_pages > max_pages {
            return Err(StoreError::QuotaExceeded);
        }
        // One catalog owner. EXCLUSIVE locking survives transaction boundaries and prevents a
        // second process from acquiring a conflicting in-memory stage writer lease.
        db(connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA; PRAGMA fullfsync=ON; PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE;"))?;
        if fresh {
            db(connection.execute_batch("CREATE TABLE identity (singleton INTEGER PRIMARY KEY CHECK(singleton=1), envelope TEXT NOT NULL, revision INTEGER NOT NULL);
                CREATE TABLE payloads (digest TEXT PRIMARY KEY, length INTEGER NOT NULL);
                CREATE TABLE stages (id TEXT PRIMARY KEY, owner TEXT NOT NULL, upload_key TEXT, maximum INTEGER NOT NULL, expected INTEGER, revision INTEGER NOT NULL, length INTEGER NOT NULL, digest TEXT NOT NULL, protocol TEXT, sealed TEXT, retired INTEGER NOT NULL DEFAULT 0, UNIQUE(owner,upload_key));
                CREATE TABLE records (owner TEXT NOT NULL, key BLOB NOT NULL, revision INTEGER NOT NULL, deleted INTEGER NOT NULL, envelope TEXT NOT NULL, PRIMARY KEY(owner,key));
                CREATE TABLE commits (id TEXT PRIMARY KEY, owner TEXT NOT NULL, fingerprint TEXT NOT NULL, revision INTEGER NOT NULL, refs TEXT NOT NULL);"))?;
            let envelope = json!({"format":"aws-emulator.state","service":"payload-store","namespace":scope.namespace,"generation":scope.generation,"version":1,"kind":"transactional-catalog"}).to_string();
            db(connection.execute("INSERT INTO identity VALUES(1,?1,0)", [&envelope]))?;
        } else {
            let text: String = db(connection.query_row(
                "SELECT envelope FROM identity WHERE singleton=1",
                [],
                |row| row.get(0),
            ))?;
            let expected = json!({"format":"aws-emulator.state","service":"payload-store","namespace":scope.namespace,"generation":scope.generation,"version":1,"kind":"transactional-catalog"});
            if serde_json::from_str::<Value>(&text).ok().as_ref() != Some(&expected) {
                return Err(StoreError::NamespaceMismatch);
            }
        }
        db(connection.execute_batch("COMMIT;"))?;
        storage.sync_directory()?;
        let mut backend = Self {
            scope,
            limits: config.limits,
            durability,
            connection,
            storage,
            active: HashMap::new(),
            degraded: HashSet::new(),
            degraded_stages: HashSet::new(),
            #[cfg(test)]
            fault: None,
        };
        backend.audit()?;
        Ok(backend)
    }
    fn audit(&mut self) -> Result<(), StoreError> {
        for table in ["stages", "records", "commits"] {
            let count: u64 =
                db(self
                    .connection
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)))?;
            let limit = match table {
                "stages" => self.limits.max_stages,
                "records" => self.limits.max_records,
                _ => self.limits.max_commits,
            };
            if count > limit as u64 {
                return Err(StoreError::QuotaExceeded);
            }
        }
        let payloads: Vec<(String, u64)> = {
            let mut statement = db(self
                .connection
                .prepare("SELECT digest,length FROM payloads"))?;
            let rows = db(statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?))))?;
            rows.collect::<rusqlite::Result<_>>()
                .map_err(|_| StoreError::Catalog)?
        };
        for (digest, length) in payloads {
            let digest = Sha256Digest::parse(&digest)?;
            if self.storage.verify_blob(digest, length).is_err() {
                self.degraded.insert(digest);
            }
        }
        let ids: Vec<String> = {
            let mut statement = db(self
                .connection
                .prepare("SELECT id FROM stages WHERE retired=0"))?;
            let rows = db(statement.query_map([], |r| r.get(0)))?;
            rows.collect::<rusqlite::Result<_>>()
                .map_err(|_| StoreError::Catalog)?
        };
        for id in ids {
            let row = self.stage(&id)?;
            if self
                .storage
                .verify_checkpoint(&id, row.length, row.digest)
                .is_err()
            {
                self.degraded_stages.insert(id);
            }
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn volume_directory(&self) -> Option<std::path::PathBuf> {
        self.storage
            .catalog_path()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
    }
    pub fn inventory(&self) -> Result<BootInventory, StoreError> {
        let count = |table| -> Result<u64, StoreError> {
            db(self
                .connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)))
        };
        if self.storage.accounting_uncertain() {
            return Err(StoreError::Interrupted);
        }
        Ok(BootInventory {
            stages: count("stages")?,
            sealed_payloads: count("payloads")?,
            records: count("records")?,
            retained_files: self.storage.retained_files(),
            retained_bytes: self.storage.used(),
            degraded_payloads: self.degraded.len() as u64,
            data_quota_exceeded: self.storage.used() > self.limits.max_storage_bytes,
        })
    }
    pub fn healthy(&self) -> Result<(), StoreError> {
        // An IO failure during commit/rollback must not expose this connection's uncommitted view.
        if !self.connection.is_autocommit() {
            return Err(StoreError::Interrupted);
        }
        Ok(())
    }
    pub fn scope_check(&self, scope: &Scope) -> Result<(), StoreError> {
        if self.scope.as_ref() == scope {
            Ok(())
        } else {
            Err(StoreError::NamespaceMismatch)
        }
    }
    fn stage(&self, id: &str) -> Result<StageRow, StoreError> {
        let row = db(self.connection.query_row("SELECT owner,maximum,expected,revision,length,digest,protocol,sealed,retired FROM stages WHERE id=?1", [id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,u64>(1)?,r.get::<_,Option<u64>>(2)?,r.get::<_,u64>(3)?,r.get::<_,u64>(4)?,r.get::<_,String>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,Option<String>>(7)?,r.get::<_,bool>(8)?))).optional())?.ok_or(StoreError::NotFound)?;
        Ok(StageRow {
            owner: owner_from(&row.0)?,
            maximum: row.1,
            expected: row.2,
            revision: row.3,
            length: row.4,
            digest: Sha256Digest::parse(&row.5)?,
            protocol: protocol_from(row.6)?,
            sealed: row.7.map(|v| Sha256Digest::parse(&v)).transpose()?,
            retired: row.8,
        })
    }
    fn checkpoint_of(&self, id: &UploadId, row: &StageRow) -> Checkpoint {
        Checkpoint {
            upload: id.clone(),
            revision: row.revision,
            offset: row.length,
            digest: row.digest,
        }
    }
    fn checkpoint_matches(
        &self,
        id: &UploadId,
        row: &StageRow,
        expected: &Checkpoint,
    ) -> Result<(), StoreError> {
        self.scope_check(&id.scope)?;
        if &self.checkpoint_of(id, row) != expected {
            return Err(StoreError::CheckpointConflict);
        }
        Ok(())
    }
    pub fn begin(&mut self, spec: StageSpec, lease: &Arc<()>) -> Result<UploadId, StoreError> {
        if spec.max_bytes == 0
            || spec.max_bytes > self.limits.max_stage_bytes
            || spec.expected_length.is_some_and(|n| n > spec.max_bytes)
        {
            return Err(StoreError::Invalid("stage byte limit"));
        }
        if let Some(key) = &spec.upload_key {
            token(key)?;
        }
        let owner = owner_text(&spec.owner)?;
        let count: usize = db(self
            .connection
            .query_row("SELECT COUNT(*) FROM stages", [], |r| r.get(0)))?;
        if count >= self.limits.max_stages {
            return Err(StoreError::QuotaExceeded);
        }
        if let Some(key) = &spec.upload_key {
            if db(self
                .connection
                .query_row(
                    "SELECT 1 FROM stages WHERE owner=?1 AND upload_key=?2",
                    params![owner, key],
                    |_| Ok(()),
                )
                .optional())?
            .is_some()
            {
                return Err(StoreError::CheckpointConflict);
            }
        }
        let id = UploadId {
            scope: self.scope.clone(),
            id: uuid::Uuid::new_v4().to_string(),
        };
        self.storage
            .create_stage(&id.id, self.limits.max_data_files)?;
        db(self.connection.execute("INSERT INTO stages(id,owner,upload_key,maximum,expected,revision,length,digest) VALUES(?1,?2,?3,?4,?5,0,0,?6)", params![id.id,owner,spec.upload_key,spec.max_bytes,spec.expected_length,empty_digest().to_string()]))?;
        self.active.insert(
            id.id.clone(),
            Active {
                lease: Arc::downgrade(lease),
                length: 0,
                hash: Sha256::new(),
                interrupted: false,
            },
        );
        Ok(id)
    }
    pub fn lookup_upload(
        &self,
        owner: &OwnerKey,
        key: &str,
        by_upload_key: bool,
    ) -> Result<Option<UploadSnapshot>, StoreError> {
        token(key)?;
        let query = if by_upload_key {
            "SELECT id FROM stages WHERE owner=?1 AND upload_key=?2 AND retired=0"
        } else {
            "SELECT id FROM stages WHERE owner=?1 AND id=?2 AND retired=0"
        };
        let id: Option<String> = db(self
            .connection
            .query_row(query, params![owner_text(owner)?, key], |r| r.get(0))
            .optional())?;
        let Some(id) = id else {
            return Ok(None);
        };
        if self.degraded_stages.contains(&id) {
            return Err(StoreError::Integrity);
        }
        let row = self.stage(&id)?;
        let upload = UploadId {
            scope: self.scope.clone(),
            id,
        };
        let sealed = row.sealed.map(|digest| self.payload(digest)).transpose()?;
        Ok(Some(UploadSnapshot {
            checkpoint: self.checkpoint_of(&upload, &row),
            owner: row.owner,
            protocol: row.protocol,
            id: upload,
            sealed,
        }))
    }
    fn active(&self, id: &UploadId, lease: &Arc<()>) -> Result<&Active, StoreError> {
        self.scope_check(&id.scope)?;
        let active = self.active.get(&id.id).ok_or(StoreError::Busy)?;
        if !active
            .lease
            .upgrade()
            .is_some_and(|l| Arc::ptr_eq(&l, lease))
        {
            return Err(StoreError::Busy);
        }
        Ok(active)
    }
    pub fn resume(
        &mut self,
        id: &UploadId,
        checkpoint: &Checkpoint,
        lease: &Arc<()>,
    ) -> Result<OwnerKey, StoreError> {
        self.scope_check(&id.scope)?;
        if self
            .active
            .get(&id.id)
            .and_then(|a| a.lease.upgrade())
            .is_some()
        {
            return Err(StoreError::Busy);
        }
        let row = self.stage(&id.id)?;
        self.checkpoint_matches(id, &row, checkpoint)?;
        if row.retired || row.sealed.is_some() {
            return Err(StoreError::Invalid("retired or sealed stage"));
        }
        let hash = self.storage.resume_stage(&id.id, row.length, row.digest)?;
        self.degraded_stages.remove(&id.id);
        self.active.insert(
            id.id.clone(),
            Active {
                lease: Arc::downgrade(lease),
                length: row.length,
                hash,
                interrupted: false,
            },
        );
        Ok(row.owner)
    }
    pub fn progress(&self, id: &UploadId, lease: &Arc<()>) -> Result<StageProgress, StoreError> {
        let active = self.active(id, lease)?;
        if active.interrupted {
            return Err(StoreError::Interrupted);
        }
        Ok(StageProgress {
            length: active.length,
            sha256: Sha256Digest(active.hash.clone().finalize().into()),
        })
    }
    pub fn append(
        &mut self,
        id: &UploadId,
        lease: &Arc<()>,
        offset: u64,
        bytes: Bytes,
    ) -> Result<u64, StoreError> {
        let observed = self.progress(id, lease)?.length;
        if offset != observed {
            return Err(StoreError::OffsetMismatch {
                expected: observed,
                supplied: offset,
            });
        }
        let row = self.stage(&id.id)?;
        if row.retired || row.sealed.is_some() {
            return Err(StoreError::Invalid("retired or sealed stage"));
        }
        let next = observed
            .checked_add(bytes.len() as u64)
            .ok_or(StoreError::QuotaExceeded)?;
        if bytes.len() > self.limits.max_chunk_bytes
            || next > row.maximum
            || row.expected.is_some_and(|n| next > n)
            || self
                .storage
                .used()
                .checked_add(bytes.len() as u64)
                .is_none_or(|n| n > self.limits.max_storage_bytes)
        {
            return Err(StoreError::QuotaExceeded);
        }
        #[cfg(test)]
        if self.fault == Some(Fault::PartialAppend) {
            self.fault = None;
            self.storage
                .append(&id.id, observed, &bytes[..bytes.len() / 2])?;
            if let Some(active) = self.active.get_mut(&id.id) {
                active.interrupted = true;
            }
            return Err(StoreError::Io);
        }
        if let Err(error) = self.storage.append(&id.id, observed, &bytes) {
            if let Some(active) = self.active.get_mut(&id.id) {
                active.interrupted = true;
            }
            return Err(error);
        }
        let active = self.active.get_mut(&id.id).ok_or(StoreError::Busy)?;
        active.hash.update(&bytes);
        active.length = next;
        Ok(next)
    }
    pub fn checkpoint(
        &mut self,
        id: &UploadId,
        lease: &Arc<()>,
        metadata: ProtocolMetadata,
    ) -> Result<Checkpoint, StoreError> {
        let progress = self.progress(id, lease)?;
        let text = protocol_json(&metadata, self.limits.max_metadata_bytes)?;
        let row = self.stage(&id.id)?;
        if row.retired || row.sealed.is_some() {
            return Err(StoreError::Invalid("retired or sealed stage"));
        }
        let revision = row
            .revision
            .checked_add(1)
            .filter(|n| *n <= i64::MAX as u64)
            .ok_or(StoreError::QuotaExceeded)?;
        self.storage.flush_stage(&id.id)?;
        db(self.connection.execute("UPDATE stages SET revision=?1,length=?2,digest=?3,protocol=?4 WHERE id=?5 AND revision=?6", params![revision,progress.length,progress.sha256.to_string(),text,id.id,row.revision]))?;
        #[cfg(test)]
        if self.fault == Some(Fault::AfterCheckpoint) {
            self.fault = None;
            return Err(StoreError::Interrupted);
        }
        Ok(Checkpoint {
            upload: id.clone(),
            revision,
            offset: progress.length,
            digest: progress.sha256,
        })
    }
    pub fn rollback(
        &mut self,
        id: &UploadId,
        lease: &Arc<()>,
        previous: &Checkpoint,
    ) -> Result<(), StoreError> {
        self.active(id, lease)?;
        let row = self.stage(&id.id)?;
        self.checkpoint_matches(id, &row, previous)?;
        if row.retired || row.sealed.is_some() {
            return Err(StoreError::Invalid("retired or sealed stage"));
        }
        let hash = self.storage.resume_stage(&id.id, row.length, row.digest)?;
        let active = self.active.get_mut(&id.id).ok_or(StoreError::Busy)?;
        active.hash = hash;
        active.length = row.length;
        active.interrupted = false;
        Ok(())
    }
    pub fn seal(
        &mut self,
        id: &UploadId,
        lease: &Arc<()>,
        expected: IntegrityExpectation,
    ) -> Result<SealedPayload, StoreError> {
        let progress = self.progress(id, lease)?;
        let row = self.stage(&id.id)?;
        if row.retired || row.sealed.is_some() {
            return Err(StoreError::Invalid("retired or sealed stage"));
        }
        let actual = self.storage.hash_stage(&id.id, row.maximum)?;
        if actual != (progress.length, progress.sha256)
            || expected.length.is_some_and(|n| n != actual.0)
            || row.expected.is_some_and(|n| n != actual.0)
            || expected.sha256.is_some_and(|d| d != actual.1)
        {
            return Err(StoreError::Integrity);
        }
        self.storage.flush_stage(&id.id)?;
        self.storage.install(
            &id.id,
            actual.1,
            actual.0,
            self.limits.max_storage_bytes,
            self.limits.max_data_files,
        )?;
        #[cfg(test)]
        if self.fault == Some(Fault::AfterInstall) {
            self.fault = None;
            return Err(StoreError::Io);
        }
        let transaction = db(self.connection.transaction())?;
        db(transaction.execute(
            "INSERT INTO payloads(digest,length) VALUES(?1,?2) ON CONFLICT(digest) DO NOTHING",
            params![actual.1.to_string(), actual.0],
        ))?;
        db(transaction.execute(
            "UPDATE stages SET sealed=?1 WHERE id=?2",
            params![actual.1.to_string(), id.id],
        ))?;
        db(transaction.commit())?;
        Ok(SealedPayload {
            payload: PayloadHandle {
                scope: self.scope.clone(),
                digest: actual.1,
                length: actual.0,
            },
            upload: id.clone(),
        })
    }
    fn payload(&self, digest: Sha256Digest) -> Result<PayloadHandle, StoreError> {
        let length: u64 = db(self
            .connection
            .query_row(
                "SELECT length FROM payloads WHERE digest=?1",
                [digest.to_string()],
                |r| r.get(0),
            )
            .optional())?
        .ok_or(StoreError::Integrity)?;
        if self.degraded.contains(&digest) {
            return Err(StoreError::Integrity);
        }
        Ok(PayloadHandle {
            scope: self.scope.clone(),
            digest,
            length,
        })
    }
    pub fn resolve_payload(
        &self,
        digest: Sha256Digest,
        length: u64,
    ) -> Result<PayloadHandle, StoreError> {
        let handle = self.payload(digest)?;
        if handle.length != length {
            return Err(StoreError::Integrity);
        }
        Ok(handle)
    }
    pub fn read(&self, handle: &PayloadHandle) -> Result<ReadSource, StoreError> {
        self.scope_check(&handle.scope)?;
        if self.payload(handle.digest)? != *handle {
            return Err(StoreError::Integrity);
        }
        self.storage.open_blob(handle.digest, handle.length)
    }
    pub fn abort(&mut self, id: &UploadId, lease: &Arc<()>) -> Result<(), StoreError> {
        self.active(id, lease)?;
        let row = self.stage(&id.id)?;
        if row.revision != 0 || row.sealed.is_some() {
            return Err(StoreError::Invalid(
                "acknowledged stage requires transactional retirement",
            ));
        }
        db(self
            .connection
            .execute("UPDATE stages SET retired=1 WHERE id=?1", [&id.id]))?;
        self.storage.abort_unacknowledged(&id.id)?;
        self.active.remove(&id.id);
        Ok(())
    }
    fn envelope(&self, key: &RecordKey, value: Option<&RecordData>) -> Result<String, StoreError> {
        let refs = match value {
            Some(data) => {
                if data.schema_version == 0
                    || data.references.len() > self.limits.max_references_per_record
                {
                    return Err(StoreError::Invalid("record schema/references"));
                }
                let mut unique = HashSet::new();
                let mut refs = Vec::new();
                for handle in &data.references {
                    self.scope_check(&handle.scope)?;
                    if self.payload(handle.digest)? != *handle {
                        return Err(StoreError::Integrity);
                    }
                    // Seal/boot admission established durable immutable content. Do not rehash
                    // every layer again for each tag/shard record in the same transaction.
                    if !unique.insert(handle.digest) {
                        return Err(StoreError::Invalid("duplicate reference"));
                    }
                    refs.push(json!({"digest":handle.digest.to_string(),"size":handle.length}));
                }
                refs
            }
            None => Vec::new(),
        };
        encode(
            &json!({"format":"aws-emulator.state","service":key.owner.service,"namespace":self.scope.namespace,"generation":self.scope.generation,
            "version":value.map_or(1,|v|v.schema_version),"kind":if value.is_some(){"payload-ref"}else{"payload-ref-deleted"},
            "key":{"resource":key.owner.resource,"generation":key.owner.generation,"record":key.key.as_ref()},
            "refs":refs,"meta":value.map_or(Value::Null,|v|v.metadata.clone())}),
            self.limits.max_metadata_bytes,
        )
    }
    fn decode(
        &self,
        key: &RecordKey,
        text: &str,
        deleted: bool,
    ) -> Result<Option<RecordData>, StoreError> {
        if text.len() > self.limits.max_metadata_bytes {
            return Err(StoreError::Integrity);
        }
        let envelope: Value = serde_json::from_str(text).map_err(|_| StoreError::Integrity)?;
        let expected_key = json!({"resource":key.owner.resource,"generation":key.owner.generation,"record":key.key.as_ref()});
        if envelope["format"] != "aws-emulator.state"
            || envelope["service"] != key.owner.service
            || envelope["namespace"] != self.scope.namespace
            || envelope["generation"] != self.scope.generation
            || envelope["key"] != expected_key
            || envelope["kind"]
                != if deleted {
                    "payload-ref-deleted"
                } else {
                    "payload-ref"
                }
        {
            return Err(StoreError::Integrity);
        }
        let version = envelope["version"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v > 0)
            .ok_or(StoreError::Integrity)?;
        let values = envelope["refs"].as_array().ok_or(StoreError::Integrity)?;
        if values.len() > self.limits.max_references_per_record {
            return Err(StoreError::Integrity);
        }
        let mut references = Vec::new();
        for value in values {
            let digest =
                Sha256Digest::parse(value["digest"].as_str().ok_or(StoreError::Integrity)?)?;
            references.push(
                self.resolve_payload(digest, value["size"].as_u64().ok_or(StoreError::Integrity)?)?,
            );
        }
        if deleted {
            if !references.is_empty() {
                return Err(StoreError::Integrity);
            }
            Ok(None)
        } else {
            Ok(Some(RecordData {
                schema_version: version,
                metadata: envelope["meta"].clone(),
                references,
            }))
        }
    }
    pub fn record(&self, key: &RecordKey) -> Result<Option<RecordSnapshot>, StoreError> {
        let row = db(self
            .connection
            .query_row(
                "SELECT revision,deleted,envelope FROM records WHERE owner=?1 AND key=?2",
                params![owner_text(&key.owner)?, key.key.as_ref()],
                |r| {
                    Ok((
                        r.get::<_, u64>(0)?,
                        r.get::<_, bool>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional())?;
        row.map(|(revision, deleted, text)| {
            Ok(RecordSnapshot {
                key: key.clone(),
                revision: RecordRevision(revision),
                value: self.decode(key, &text, deleted)?,
            })
        })
        .transpose()
    }
    pub fn records(
        &self,
        owner: &OwnerKey,
        cursor: Option<RecordCursor>,
        limit: usize,
    ) -> Result<RecordPage, StoreError> {
        if limit == 0 || limit > self.limits.max_page_records {
            return Err(StoreError::Invalid("record page limit"));
        }
        let revision: u64 = db(self.connection.query_row(
            "SELECT revision FROM identity WHERE singleton=1",
            [],
            |r| r.get(0),
        ))?;
        let after = match cursor {
            Some(cursor) => {
                self.scope_check(&cursor.scope)?;
                if cursor.owner != *owner {
                    return Err(StoreError::NamespaceMismatch);
                }
                if cursor.revision != revision {
                    return Err(StoreError::EnumerationChanged);
                }
                cursor.after
            }
            None => Bytes::new(),
        };
        let mut statement = db(self.connection.prepare("SELECT key,revision,deleted,length(CAST(envelope AS BLOB)),envelope FROM records WHERE owner=?1 AND key>?2 ORDER BY key LIMIT ?3"))?;
        let mut rows = db(statement.query(params![
            owner_text(owner)?,
            after.as_ref(),
            (limit + 1) as u64
        ]))?;
        let mut records = Vec::new();
        let mut bytes = 0usize;
        let mut more = false;
        while let Some(row) = db(rows.next())? {
            // Check the encoded byte length BEFORE materializing this row's metadata.
            let size: usize = db(row.get(3))?;
            if size > self.limits.max_metadata_bytes {
                return Err(StoreError::Integrity);
            }
            if records.len() == limit
                || bytes
                    .checked_add(size)
                    .is_none_or(|n| n > self.limits.max_page_bytes)
            {
                more = true;
                break;
            }
            let key = RecordKey::new(owner.clone(), Bytes::from(db(row.get::<_, Vec<u8>>(0))?))?;
            let revision = db(row.get::<_, u64>(1))?;
            let deleted = db(row.get::<_, bool>(2))?;
            let text = db(row.get::<_, String>(4))?;
            bytes += size;
            records.push(RecordSnapshot {
                value: self.decode(&key, &text, deleted)?,
                key,
                revision: RecordRevision(revision),
            });
        }
        let next = if more {
            Some(RecordCursor {
                scope: self.scope.clone(),
                owner: owner.clone(),
                revision,
                after: records
                    .last()
                    .ok_or(StoreError::QuotaExceeded)?
                    .key
                    .key
                    .clone(),
            })
        } else {
            None
        };
        Ok(RecordPage { records, next })
    }
    fn receipt_refs(&self, text: &str) -> Result<Vec<PayloadHandle>, StoreError> {
        let refs: Vec<(String, u64)> =
            serde_json::from_str(text).map_err(|_| StoreError::Integrity)?;
        if refs.len()
            > self
                .limits
                .max_records_per_transaction
                .saturating_mul(self.limits.max_references_per_record)
        {
            return Err(StoreError::Integrity);
        }
        refs.into_iter()
            .map(|(digest, length)| {
                Ok(PayloadHandle {
                    scope: self.scope.clone(),
                    digest: Sha256Digest::parse(&digest)?,
                    length,
                })
            })
            .collect()
    }
    pub fn recover(&self, id: &CommitId) -> Result<CommitStatus, StoreError> {
        self.scope_check(&id.scope)?;
        let result: Option<(u64, String)> = db(self
            .connection
            .query_row(
                "SELECT revision,refs FROM commits WHERE id=?1",
                [&id.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional())?;
        Ok(match result {
            Some((revision, refs)) => CommitStatus::Committed(CommitReceipt {
                id: id.clone(),
                revision: RecordRevision(revision),
                durability: self.durability,
                references: self.receipt_refs(&refs)?,
            }),
            None => CommitStatus::NotCommitted,
        })
    }
    pub fn commit(&mut self, change: ReferenceTransaction) -> Result<CommitReceipt, CommitError> {
        self.scope_check(&change.id.scope)?;
        if change.changes.len() + change.retire.len() == 0
            || change.changes.len() + change.retire.len() > self.limits.max_records_per_transaction
        {
            return Err(StoreError::Invalid("transaction cardinality").into());
        }
        let owner = owner_text(&change.owner)?;
        let mut keys = HashSet::new();
        let mut prepared = Vec::new();
        let mut transaction_bytes = 0usize;
        for entry in &change.changes {
            if entry.key.owner != change.owner || !keys.insert(entry.key.key.clone()) {
                return Err(StoreError::Invalid("transaction owner/duplicate key").into());
            }
            let envelope = self.envelope(&entry.key, entry.value.as_ref())?;
            transaction_bytes = transaction_bytes
                .checked_add(envelope.len())
                .ok_or(StoreError::QuotaExceeded)?;
            if transaction_bytes > self.limits.max_transaction_bytes {
                return Err(StoreError::QuotaExceeded.into());
            }
            let condition = match entry.condition {
                RecordCondition::Any => json!({"any":true}),
                RecordCondition::Absent => json!({"absent":true}),
                RecordCondition::Exact(revision) => json!({"revision":revision.0}),
            };
            prepared.push((entry, envelope, condition));
        }
        let mut stage_ids = HashSet::new();
        for retirement in &change.retire {
            self.scope_check(&retirement.upload.scope)?;
            let row = self.stage(&retirement.upload.id)?;
            if row.owner != change.owner || !stage_ids.insert(retirement.upload.id.clone()) {
                return Err(StoreError::Invalid("stage retirement owner").into());
            }
            self.checkpoint_matches(&retirement.upload, &row, &retirement.checkpoint)?;
        }
        let canonical = json!({"owner":owner,"changes":prepared.iter().map(|(entry,text,condition)|json!({"key":entry.key.key.as_ref(),"condition":condition,"envelope":text})).collect::<Vec<_>>(),
            "retire":change.retire.iter().map(|r|json!({"id":r.upload.id,"revision":r.checkpoint.revision,"offset":r.checkpoint.offset,"digest":r.checkpoint.digest.to_string()})).collect::<Vec<_>>()});
        let canonical = serde_json::to_vec(&canonical).map_err(|_| StoreError::Catalog)?;
        if canonical.len() > self.limits.max_transaction_bytes {
            return Err(StoreError::QuotaExceeded.into());
        }
        let fingerprint = Sha256Digest(Sha256::digest(canonical).into()).to_string();
        if let Some((old, revision, refs)) = db(self
            .connection
            .query_row(
                "SELECT fingerprint,revision,refs FROM commits WHERE id=?1",
                [&change.id.id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, u64>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional())?
        {
            if old != fingerprint {
                return Err(StoreError::Invalid("CommitId reused for different changes").into());
            }
            return Ok(CommitReceipt {
                id: change.id,
                revision: RecordRevision(revision),
                durability: self.durability,
                references: self.receipt_refs(&refs)?,
            });
        }
        let commits: usize =
            db(self
                .connection
                .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0)))?;
        if commits >= self.limits.max_commits {
            return Err(StoreError::QuotaExceeded.into());
        }
        let mut references = Vec::new();
        let mut reference_digests = HashSet::new();
        for entry in &change.changes {
            if let Some(value) = &entry.value {
                for handle in &value.references {
                    if reference_digests.insert(handle.digest) {
                        references.push(handle.clone());
                    }
                }
            }
        }
        let refs = serde_json::to_string(
            &references
                .iter()
                .map(|r| (r.digest.to_string(), r.length))
                .collect::<Vec<_>>(),
        )
        .map_err(|_| StoreError::Catalog)?;
        let transaction = db(self.connection.transaction())?;
        let mut extra = 0usize;
        for (entry, _, _) in &prepared {
            let prior: Option<(u64, bool)> = db(transaction
                .query_row(
                    "SELECT revision,deleted FROM records WHERE owner=?1 AND key=?2",
                    params![owner, entry.key.key.as_ref()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional())?;
            let matches = match entry.condition {
                RecordCondition::Any => true,
                RecordCondition::Absent => prior.is_none_or(|(_, deleted)| deleted),
                RecordCondition::Exact(expected) => {
                    prior.is_some_and(|(revision, _)| revision == expected.0)
                }
            };
            if !matches {
                return Err(CommitError::Conflict);
            }
            if prior.is_none() {
                extra += 1;
            }
        }
        let count: usize =
            db(transaction.query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0)))?;
        if count
            .checked_add(extra)
            .is_none_or(|n| n > self.limits.max_records)
        {
            return Err(StoreError::QuotaExceeded.into());
        }
        let revision: u64 = db(transaction.query_row(
            "SELECT revision FROM identity WHERE singleton=1",
            [],
            |r| r.get(0),
        ))?;
        let revision = revision
            .checked_add(1)
            .filter(|n| *n <= i64::MAX as u64)
            .ok_or(StoreError::QuotaExceeded)?;
        for (entry, envelope, _) in prepared {
            db(transaction.execute("INSERT INTO records(owner,key,revision,deleted,envelope) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(owner,key) DO UPDATE SET revision=excluded.revision,deleted=excluded.deleted,envelope=excluded.envelope", params![owner,entry.key.key.as_ref(),revision,entry.value.is_none(),envelope]))?;
        }
        for retired in &change.retire {
            db(transaction.execute(
                "UPDATE stages SET retired=1 WHERE id=?1",
                [&retired.upload.id],
            ))?;
        }
        db(transaction.execute(
            "UPDATE identity SET revision=?1 WHERE singleton=1",
            [revision],
        ))?;
        db(transaction.execute(
            "INSERT INTO commits(id,owner,fingerprint,revision,refs) VALUES(?1,?2,?3,?4,?5)",
            params![change.id.id, owner, fingerprint, revision, refs],
        ))?;
        #[cfg(test)]
        if self.fault == Some(Fault::BeforeCommit) {
            self.fault = None;
            return Err(StoreError::Io.into());
        }
        if transaction.commit().is_err() {
            return Err(CommitError::OutcomeUnknown(change.id));
        }
        #[cfg(test)]
        if self.fault == Some(Fault::AfterCommit) {
            self.fault = None;
            return Err(CommitError::OutcomeUnknown(change.id));
        }
        Ok(CommitReceipt {
            id: change.id,
            revision: RecordRevision(revision),
            durability: self.durability,
            references,
        })
    }
}
