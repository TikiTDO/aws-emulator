// SPDX-License-Identifier: CC0-1.0

use bytes::Bytes;
use serde_json::Value;
use std::{fmt, path::PathBuf, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    Invalid(&'static str),
    UnsupportedChecksum,
    NamespaceMismatch,
    QuotaExceeded,
    NotFound,
    Busy,
    CheckpointConflict,
    OffsetMismatch { expected: u64, supplied: u64 },
    Integrity,
    Interrupted,
    EnumerationChanged,
    Io,
    Catalog,
    Worker,
}
impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "payload store: {self:?}")
    }
}
impl std::error::Error for StoreError {}
impl From<std::io::Error> for StoreError {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}

#[derive(Clone, Debug)]
pub struct StoreLimits {
    /// Payload/stage/temporary file bytes (including retained orphan files).
    pub max_storage_bytes: u64,
    /// SQLite main catalog byte ceiling; rollback journal can temporarily add at most a catalog-sized copy.
    pub max_catalog_bytes: u64,
    pub max_stage_bytes: u64,
    pub max_chunk_bytes: usize,
    pub max_metadata_bytes: usize,
    pub max_records_per_transaction: usize,
    pub max_references_per_record: usize,
    pub max_page_records: usize,
    pub max_page_bytes: usize,
    pub max_stages: usize,
    pub max_records: usize,
    pub max_commits: usize,
    pub max_transaction_bytes: usize,
    pub max_inflight_operations: usize,
    pub max_read_leases: usize,
    pub max_data_files: usize,
}
impl StoreLimits {
    pub(crate) fn validate(&self) -> Result<(), StoreError> {
        if self.max_catalog_bytes < 64 * 1024
            || self.max_catalog_bytes > i64::MAX as u64
            || self.max_storage_bytes == 0
            || self.max_storage_bytes > i64::MAX as u64
            || self.max_stage_bytes == 0
            || self.max_stage_bytes > self.max_storage_bytes
            || self.max_chunk_bytes == 0
            || self.max_chunk_bytes as u64 > self.max_stage_bytes
            || self.max_metadata_bytes == 0
            || self.max_metadata_bytes > 1024 * 1024
            || self.max_records_per_transaction == 0
            || self.max_records_per_transaction > 4096
            || self.max_references_per_record == 0
            || self.max_references_per_record > 16384
            || self.max_page_records == 0
            || self.max_page_records > 4096
            || self.max_page_bytes < self.max_metadata_bytes
            || self.max_page_bytes > 16 * 1024 * 1024
            || self.max_stages == 0
            || self.max_stages > 1_000_000
            || self.max_records == 0
            || self.max_records > 1_000_000
            || self.max_commits == 0
            || self.max_commits > 1_000_000
            || self.max_transaction_bytes == 0
            || self.max_transaction_bytes > 16 * 1024 * 1024
            || self.max_inflight_operations == 0
            || self.max_inflight_operations > 1024
            || self.max_read_leases == 0
            || self.max_read_leases > 4096
            || self.max_data_files < 3
            || self.max_data_files > 1_000_000
        {
            return Err(StoreError::Invalid("finite store limits"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Scope {
    pub namespace: String,
    pub generation: String,
}
#[derive(Clone, Debug)]
pub(crate) enum Mode {
    Memory,
    Volume(PathBuf),
}
/// Boot-selected active LOCAL volume, never a client path or an ambient environment setting.
/// The boot owner must admit filesystem placement; this API does not infer SSD/NAS from a pathname.
#[derive(Clone, Debug)]
pub struct StoreConfig {
    pub(crate) scope: Scope,
    pub(crate) mode: Mode,
    pub(crate) limits: StoreLimits,
}
impl StoreConfig {
    pub fn memory(
        namespace: &str,
        generation: &str,
        limits: StoreLimits,
    ) -> Result<Self, StoreError> {
        Self::new(namespace, generation, Mode::Memory, limits)
    }
    pub fn volume(
        root: PathBuf,
        namespace: &str,
        generation: &str,
        limits: StoreLimits,
    ) -> Result<Self, StoreError> {
        if !root.is_absolute() {
            return Err(StoreError::Invalid("absolute selected volume root"));
        }
        Self::new(namespace, generation, Mode::Volume(root), limits)
    }
    fn new(
        namespace: &str,
        generation: &str,
        mode: Mode,
        limits: StoreLimits,
    ) -> Result<Self, StoreError> {
        token(namespace)?;
        token(generation)?;
        limits.validate()?;
        Ok(Self {
            scope: Scope {
                namespace: namespace.into(),
                generation: generation.into(),
            },
            mode,
            limits,
        })
    }
}
pub(crate) fn token(value: &str) -> Result<(), StoreError> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        Err(StoreError::Invalid("identity"))
    } else {
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Durability {
    Ephemeral,
    Durable,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OwnerKey {
    pub(crate) service: String,
    pub(crate) resource: String,
    pub(crate) generation: String,
}
impl OwnerKey {
    pub fn new(service: &str, resource: &str, generation: &str) -> Result<Self, StoreError> {
        token(service)?;
        token(resource)?;
        token(generation)?;
        Ok(Self {
            service: service.into(),
            resource: resource.into(),
            generation: generation.into(),
        })
    }
    pub fn service(&self) -> &str {
        &self.service
    }
    pub fn resource(&self) -> &str {
        &self.resource
    }
    pub fn generation(&self) -> &str {
        &self.generation
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha256Digest(pub(crate) [u8; 32]);
impl Sha256Digest {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    pub fn parse(value: &str) -> Result<Self, StoreError> {
        let text = value
            .strip_prefix("sha256:")
            .ok_or(StoreError::Invalid("SHA256 digest"))?;
        if text.len() != 64 {
            return Err(StoreError::Invalid("SHA256 digest"));
        }
        let mut bytes = [0u8; 32];
        for (i, pair) in text.as_bytes().chunks_exact(2).enumerate() {
            let digit = |c: u8| match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                _ => None,
            };
            bytes[i] = digit(pair[0])
                .zip(digit(pair[1]))
                .map(|(a, b)| a * 16 + b)
                .ok_or(StoreError::Invalid("SHA256 digest"))?;
        }
        Ok(Self(bytes))
    }
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
    pub fn as_hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
impl fmt::Debug for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", self.as_hex())
    }
}
impl fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrityExpectation {
    pub length: Option<u64>,
    pub sha256: Option<Sha256Digest>,
}
impl IntegrityExpectation {
    pub fn none() -> Self {
        Self {
            length: None,
            sha256: None,
        }
    }
    pub fn checksum(algorithm: &str, value: &str) -> Result<Self, StoreError> {
        if algorithm != "sha256" {
            return Err(StoreError::UnsupportedChecksum);
        }
        Ok(Self {
            length: None,
            sha256: Some(Sha256Digest::parse(value)?),
        })
    }
}
#[derive(Clone, Debug)]
pub struct ProtocolMetadata {
    pub schema: String,
    pub version: u32,
    pub bytes: Bytes,
}
impl ProtocolMetadata {
    pub fn new(schema: &str, version: u32, bytes: Bytes) -> Result<Self, StoreError> {
        token(schema)?;
        if version == 0 {
            return Err(StoreError::Invalid("protocol schema version"));
        }
        Ok(Self {
            schema: schema.into(),
            version,
            bytes,
        })
    }
}
#[derive(Clone, Debug)]
pub struct StageSpec {
    pub owner: OwnerKey,
    pub upload_key: Option<String>,
    pub expected_length: Option<u64>,
    pub max_bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UploadId {
    pub(crate) scope: Arc<Scope>,
    pub(crate) id: String,
}
impl UploadId {
    pub fn as_str(&self) -> &str {
        &self.id
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pub(crate) upload: UploadId,
    pub(crate) revision: u64,
    pub(crate) offset: u64,
    pub(crate) digest: Sha256Digest,
}
impl Checkpoint {
    pub fn upload(&self) -> &UploadId {
        &self.upload
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn offset(&self) -> u64 {
        self.offset
    }
    pub fn sha256(&self) -> Sha256Digest {
        self.digest
    }
}
#[derive(Clone, Debug)]
pub struct StageProgress {
    pub length: u64,
    pub sha256: Sha256Digest,
}
#[derive(Clone, Debug)]
pub struct UploadSnapshot {
    pub id: UploadId,
    pub owner: OwnerKey,
    pub checkpoint: Checkpoint,
    pub protocol: Option<ProtocolMetadata>,
    pub sealed: Option<PayloadHandle>,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PayloadHandle {
    pub(crate) scope: Arc<Scope>,
    pub(crate) digest: Sha256Digest,
    pub(crate) length: u64,
}
impl PayloadHandle {
    pub fn sha256(&self) -> Sha256Digest {
        self.digest
    }
    pub fn len(&self) -> u64 {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
}
/// Content integrity only: never a grant of wire authentication or service authorization.
#[derive(Clone, Debug)]
pub struct SealedPayload {
    pub(crate) payload: PayloadHandle,
    pub(crate) upload: UploadId,
}
impl SealedPayload {
    pub fn handle(&self) -> &PayloadHandle {
        &self.payload
    }
    pub fn upload(&self) -> &UploadId {
        &self.upload
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordRevision(pub(crate) u64);
impl RecordRevision {
    pub fn get(self) -> u64 {
        self.0
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RecordKey {
    pub(crate) owner: OwnerKey,
    pub(crate) key: Bytes,
}
impl RecordKey {
    pub fn new(owner: OwnerKey, key: Bytes) -> Result<Self, StoreError> {
        if key.is_empty() || key.len() > 4096 {
            return Err(StoreError::Invalid("record key"));
        }
        Ok(Self { owner, key })
    }
    pub fn owner(&self) -> &OwnerKey {
        &self.owner
    }
    pub fn key(&self) -> &Bytes {
        &self.key
    }
}
#[derive(Clone, Copy, Debug)]
pub enum RecordCondition {
    Any,
    Absent,
    Exact(RecordRevision),
}
#[derive(Clone, Debug)]
pub struct RecordData {
    pub schema_version: u32,
    pub metadata: Value,
    pub references: Vec<PayloadHandle>,
}
#[derive(Clone, Debug)]
pub struct RecordChange {
    pub key: RecordKey,
    pub condition: RecordCondition,
    pub value: Option<RecordData>,
}
#[derive(Clone, Debug)]
pub struct StageRetirement {
    pub upload: UploadId,
    pub checkpoint: Checkpoint,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CommitId {
    pub(crate) scope: Arc<Scope>,
    pub(crate) id: String,
}
impl CommitId {
    pub fn as_str(&self) -> &str {
        &self.id
    }
}
#[derive(Clone, Debug)]
pub struct ReferenceTransaction {
    pub id: CommitId,
    pub owner: OwnerKey,
    pub changes: Vec<RecordChange>,
    pub retire: Vec<StageRetirement>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitReceipt {
    pub id: CommitId,
    pub revision: RecordRevision,
    pub durability: Durability,
    pub references: Vec<PayloadHandle>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitStatus {
    Committed(CommitReceipt),
    InProgress,
    NotCommitted,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitError {
    Conflict,
    OutcomeUnknown(CommitId),
    Store(StoreError),
}
impl From<StoreError> for CommitError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}
impl fmt::Display for CommitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "payload commit: {self:?}")
    }
}
impl std::error::Error for CommitError {}
#[derive(Clone, Debug)]
pub struct RecordSnapshot {
    pub key: RecordKey,
    pub revision: RecordRevision,
    pub value: Option<RecordData>,
}
#[derive(Clone, Debug)]
pub struct RecordCursor {
    pub(crate) scope: Arc<Scope>,
    pub(crate) owner: OwnerKey,
    pub(crate) revision: u64,
    pub(crate) after: Bytes,
}
#[derive(Clone, Debug)]
pub struct RecordPage {
    pub records: Vec<RecordSnapshot>,
    pub next: Option<RecordCursor>,
}
#[derive(Clone, Debug, Default)]
pub struct BootInventory {
    pub stages: u64,
    pub sealed_payloads: u64,
    pub records: u64,
    pub retained_files: u64,
    pub retained_bytes: u64,
    pub degraded_payloads: u64,
    pub data_quota_exceeded: bool,
}
