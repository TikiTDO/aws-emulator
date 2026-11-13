// SPDX-License-Identifier: CC0-1.0

//! ECR's versioned records in the one boot-installed catalog. Repository config
//! is the bounded discovery directory, not an independent B5 snapshot. Image
//! manifests, ancillary metadata and tag sets are separate bounded payloads.
use super::*;
use super::payloads::{self, ManifestDocument, RepositoryScope};
use bytes::Bytes;
use service_common::payload_store::{PayloadStore, RecordChange, RecordCondition, RecordData, ReferenceTransaction, StageRetirement, StoreError, CommitError, RecordRevision, PayloadHandle};
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepositoryConfig {
    generation: String, name: String, registry_id: String, arn: String,
    created_at: SystemTime, image_tag_mutability: String,
    #[serde(with = "super::catalog_codec::model_vec")]
    filters: Vec<ImageTagMutabilityExclusionFilter>,
    #[serde(with = "super::catalog_codec::model")]
    scanning: ImageScanningConfiguration,
    #[serde(with = "super::catalog_codec::model")]
    encryption: EncryptionConfiguration,
    #[serde(with = "super::catalog_codec::model_vec")]
    tags: Vec<Tag>,
    policy: Option<String>, lifecycle: Option<StoredLifecyclePolicy>, preview: Option<LifecyclePreview>,
}
impl RepositoryConfig {
    fn capture(repo: &Repo) -> Self {
        Self { generation: repo.payload_generation.clone(), name: repo.name.clone(), registry_id: repo.registry_id.clone(), arn: repo.arn.clone(),
            created_at: repo.created_at, image_tag_mutability: repo.image_tag_mutability.clone(), filters: repo.image_tag_mutability_exclusion_filters.clone(),
            scanning: repo.image_scanning_config.clone(), encryption: repo.encryption_config.clone(), tags: repo.tags.clone(),
            policy: repo.policy_text.clone(), lifecycle: repo.lifecycle_policy.clone(), preview: repo.lifecycle_preview.clone() }
    }
    fn restore(self, revision: RecordRevision) -> Result<Repo, StoreError> {
        if self.name.is_empty() || self.name.len() > 256 || self.generation.len() != 64
            || !self.generation.bytes().all(|v| v.is_ascii_hexdigit()) || self.registry_id != registry_id() { return Err(StoreError::Integrity); }
        Ok(Repo { payload_generation: self.generation, payload_revision: Some(revision), name: self.name, registry_id: self.registry_id,
            arn: self.arn, created_at: self.created_at, image_tag_mutability: self.image_tag_mutability,
            image_tag_mutability_exclusion_filters: self.filters, image_scanning_config: self.scanning, encryption_config: self.encryption,
            tags: self.tags, policy_text: self.policy, lifecycle_policy: self.lifecycle, lifecycle_preview: self.preview,
            images: BTreeMap::new(), tag_index: BTreeMap::new(), layer_blobs: BTreeMap::new() })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageMetadata {
    digest: String, manifest_media_type: Option<String>, artifact_media_type: Option<String>,
    size_in_bytes: Option<i64>, pushed_at: SystemTime, scan: Option<ImageScan>, status: String,
    last_archived_at: Option<SystemTime>, last_activated_at: Option<SystemTime>, activate_due_at: Option<SystemTime>,
    last_recorded_pull_time: Option<SystemTime>, replications: Vec<ImageReplication>, signings: Vec<signing::ImageSigning>,
}
impl ImageMetadata {
    fn capture(image: &ImageRecord) -> Self {
        Self { digest: image.digest.clone(), manifest_media_type: image.manifest_media_type.clone(), artifact_media_type: image.artifact_media_type.clone(),
            size_in_bytes: image.size_in_bytes, pushed_at: image.pushed_at, scan: image.scan.clone(), status: image.status.clone(),
            last_archived_at: image.last_archived_at, last_activated_at: image.last_activated_at, activate_due_at: image.activate_due_at,
            last_recorded_pull_time: image.last_recorded_pull_time, replications: image.replications.clone(), signings: image.signings.clone() }
    }
    fn restore(self, document: String, handle: PayloadHandle, tags: BTreeSet<String>) -> Result<ImageRecord, StoreError> {
        if manifest_digest(&document) != self.digest || tags.len() > MAX_TAGS_PER_REPOSITORY { return Err(StoreError::Integrity); }
        let mut image = ImageRecord::pushed(self.digest, document, self.manifest_media_type, self.pushed_at);
        image.manifest = ManifestDocument::Stored(handle);
        image.tags = tags; image.artifact_media_type = self.artifact_media_type; image.size_in_bytes = self.size_in_bytes;
        image.scan = self.scan; image.status = self.status; image.last_archived_at = self.last_archived_at;
        image.last_activated_at = self.last_activated_at; image.activate_due_at = self.activate_due_at;
        image.last_recorded_pull_time = self.last_recorded_pull_time; image.replications = self.replications; image.signings = self.signings;
        Ok(image)
    }
}
fn json_bytes<T: Serialize>(value: &T) -> Result<Bytes, StoreError> {
    let bytes = serde_json::to_vec(value).map_err(|_| StoreError::Integrity)?;
    if bytes.len() > 4 * 1024 * 1024 { return Err(StoreError::QuotaExceeded); }
    Ok(Bytes::from(bytes))
}
async fn staged(store: &PayloadStore, bytes: Bytes) -> Result<(PayloadHandle, StageRetirement), StoreError> {
    let (sealed, checkpoint) = payloads::stage_document(store, payloads::registry_owner()?, bytes, 4 * 1024 * 1024).await?;
    Ok((sealed.handle().clone(), StageRetirement { upload: sealed.upload().clone(), checkpoint }))
}

pub async fn repository_change(store: &PayloadStore, repo: &Repo) -> Result<(RecordChange, StageRetirement), StoreError> {
    let scope = payloads::repository_owner(repo)?;
    let (handle, retirement) = staged(store, json_bytes(&RepositoryConfig::capture(repo))?).await?;
    Ok((RecordChange { key: scope.config_key()?, condition: scope.revision.map_or(RecordCondition::Absent, RecordCondition::Exact),
        value: Some(RecordData { schema_version: 1, metadata: serde_json::json!({"kind":"ecr-repository", "name":repo.name, "generation":repo.payload_generation}), references: vec![handle] }) }, retirement))
}

/// Publish a new repository incarnation before its API/cache-creation ACK.
/// The same group builder is reused for cache creation plus initial image.
pub async fn publish_repository(mutation: &payloads::MutationGuard, store: &PayloadStore, repo: &mut Repo) -> Result<(), CommitError> {
    let (change, retirement) = repository_change(store, repo).await?;
    let receipt = mutation.commit(store, ReferenceTransaction { id: store.new_commit_id(), owner: payloads::registry_owner()?, changes: vec![change], retire: vec![retirement] }).await?;
    repo.payload_revision = Some(receipt.revision);
    Ok(())
}

/// Stage only changed images/tag sets. Unchanged handles are reused; deleted
/// image records lose all their reference set in the same conditional group.
pub async fn image_changes(
    store: &PayloadStore, scope: &RepositoryScope, before: &Repo, after: &mut Repo,
) -> Result<(Vec<RecordChange>, Vec<StageRetirement>), StoreError> {
    image_changes_with_staged_layers(store, scope, before, after, Vec::new()).await
}

/// Cache creation must publish config + synthetic layers + images together.
/// Candidate authority is the ACTUAL same-group layer RecordChange, never an
/// unscoped handle map. Return those changes so a caller cannot validate them
/// here then accidentally omit them from the group it publishes.
pub async fn image_changes_with_staged_layers(
    store: &PayloadStore, scope: &RepositoryScope, before: &Repo, after: &mut Repo,
    layer_changes: Vec<RecordChange>,
) -> Result<(Vec<RecordChange>, Vec<StageRetirement>), StoreError> {
    if before.name != scope.name || after.name != scope.name
        || before.payload_generation != scope.generation || after.payload_generation != scope.generation {
        return Err(StoreError::Integrity);
    }
    let mut staged_layers = BTreeMap::new();
    for change in &layer_changes {
        let value = change.value.as_ref().ok_or(StoreError::Integrity)?;
        if matches!(change.condition, RecordCondition::Any)
            || value.schema_version != 1 || value.references.len() != 1
            || value.metadata.as_object().is_none_or(|v| v.len() != 4)
            || value.metadata.get("kind").and_then(serde_json::Value::as_str) != Some("ecr-layer") {
            return Err(StoreError::Integrity);
        }
        for field in ["mediaType", "operatingSystem", "packageManager"] {
            match value.metadata.get(field) {
                Some(serde_json::Value::Null) => {},
                Some(serde_json::Value::String(v)) if v.len() <= 64 * 1024 => {},
                _ => return Err(StoreError::Integrity),
            }
        }
        let handle = &value.references[0];
        if change.key != payloads::layer_key(scope, handle.sha256())?
            || staged_layers.insert(handle.sha256().to_string(), handle.clone()).is_some() {
            return Err(StoreError::Integrity);
        }
    }
    let mut changes = layer_changes; let mut retire = Vec::new();
    let digests: BTreeSet<_> = before.images.keys().chain(after.images.keys()).cloned().collect();
    for digest in digests {
        let key = scope.key("image", &digest)?;
        let old = store.record(&key).await?;
        let Some(image) = after.images.get_mut(&digest) else {
            if let Some(old) = old { changes.push(RecordChange { key, condition: RecordCondition::Exact(old.revision), value: None }); }
            continue;
        };
        if let Some(previous) = before.images.get(&digest) {
            if previous.tags == image.tags && json_bytes(&ImageMetadata::capture(previous))? == json_bytes(&ImageMetadata::capture(image))?
                && matches!((&previous.manifest, &image.manifest), (ManifestDocument::Stored(a), ManifestDocument::Stored(b)) if a == b) { continue; }
        }
        let manifest = match &image.manifest {
            ManifestDocument::Stored(handle) => handle.clone(),
            ManifestDocument::Pending(value) => { let (handle, closed) = staged(store, Bytes::copy_from_slice(value.as_bytes())).await?; retire.push(closed); handle }
        };
        let metadata_bytes = json_bytes(&ImageMetadata::capture(image))?;
        // A tag-only edit does not create a new image-metadata document. Reuse
        // the exact catalog reference, after checking its envelope and content,
        // rather than spending another stage/retirement per retained image.
        let reusable_metadata = if let (Some(previous), Some(snapshot)) = (before.images.get(&digest), old.as_ref()) {
            if json_bytes(&ImageMetadata::capture(previous))? == metadata_bytes {
                let value = snapshot.value.as_ref().ok_or(StoreError::Integrity)?;
                if value.schema_version != 1 || value.references.len() < 3
                    || value.metadata.as_object().is_none_or(|v| v.len() != 4)
                    || value.metadata.get("kind").and_then(serde_json::Value::as_str) != Some("ecr-image")
                    || value.metadata.get("repository").and_then(serde_json::Value::as_str) != Some(scope.name.as_str())
                    || value.metadata.get("generation").and_then(serde_json::Value::as_str) != Some(scope.generation.as_str())
                    || value.metadata.get("digest").and_then(serde_json::Value::as_str) != Some(digest.as_str())
                    || value.references.first() != Some(&manifest) {
                    return Err(StoreError::Integrity);
                }
                let retained: ImageMetadata = read_json(store, &value.references[1]).await?;
                if json_bytes(&retained)? != metadata_bytes { return Err(StoreError::Integrity); }
                Some(value.references[1].clone())
            } else { None }
        } else { None };
        let metadata = match reusable_metadata {
            Some(handle) => handle,
            None => { let (handle, closed) = staged(store, metadata_bytes).await?; retire.push(closed); handle }
        };
        // Metadata-only events (scan, archive/restore, pull time) do not stage
        // an unchanged tag set. Validate the actual retained reference first.
        let tag_bytes = json_bytes(&image.tags)?;
        let reusable_tags = if let (Some(previous), Some(snapshot)) = (before.images.get(&digest), old.as_ref()) {
            if previous.tags == image.tags {
                let value = snapshot.value.as_ref().ok_or(StoreError::Integrity)?;
                if value.schema_version != 1 || value.references.len() < 3
                    || value.metadata.as_object().is_none_or(|v| v.len() != 4)
                    || value.metadata.get("kind").and_then(serde_json::Value::as_str) != Some("ecr-image")
                    || value.metadata.get("repository").and_then(serde_json::Value::as_str) != Some(scope.name.as_str())
                    || value.metadata.get("generation").and_then(serde_json::Value::as_str) != Some(scope.generation.as_str())
                    || value.metadata.get("digest").and_then(serde_json::Value::as_str) != Some(digest.as_str())
                    || value.references.first() != Some(&manifest) { return Err(StoreError::Integrity); }
                let retained: Vec<String> = read_json(store,&value.references[2]).await?;
                if json_bytes(&retained)? != tag_bytes { return Err(StoreError::Integrity); }
                Some(value.references[2].clone())
            } else { None }
        } else { None };
        let tags = match reusable_tags {
            Some(handle) => handle,
            None => { let (handle,closed) = staged(store,tag_bytes).await?; retire.push(closed); handle },
        };
        let mut references = vec![manifest.clone(), metadata, tags];
        match &image.references {
            ManifestContent::Blobs(blobs) => for blob in blobs {
                let handle = match staged_layers.get(&blob.digest) {
                    Some(handle) => handle.clone(),
                    None => payloads::stored_layer(store, scope, service_common::payload_store::Sha256Digest::parse(&blob.digest)?).await?.ok_or(StoreError::Integrity)?.payload,
                };
                if blob.size.is_some_and(|size| u64::try_from(size).ok() != Some(handle.len())) { return Err(StoreError::Integrity); }
                references.push(handle);
            },
            ManifestContent::List(children) => for child in children {
                let child = before.images.get(child).ok_or(StoreError::Integrity)?;
                let ManifestDocument::Stored(handle) = &child.manifest else { return Err(StoreError::Integrity); };
                references.push(handle.clone());
            },
            ManifestContent::Opaque => {},
        }
        image.manifest = ManifestDocument::Stored(manifest);
        changes.push(RecordChange { key, condition: old.map_or(RecordCondition::Absent, |v| RecordCondition::Exact(v.revision)), value: Some(RecordData {
            schema_version: 1, metadata: serde_json::json!({"kind":"ecr-image", "repository":scope.name, "generation":scope.generation, "digest":digest}), references,
        }) });
    }
    Ok((changes, retire))
}

pub async fn publish_images(mutation: &payloads::MutationGuard, store: &PayloadStore, before: &Repo, after: &mut Repo) -> Result<(), CommitError> {
    let scope = payloads::repository_owner(before)?;
    let (mut changes, retire) = image_changes(store, &scope, before, after).await?;
    changes.push(scope.assertion(store).await?);
    let receipt = mutation.commit(store, ReferenceTransaction { id: store.new_commit_id(), owner: scope.owner, changes, retire }).await?;
    after.payload_revision = Some(receipt.revision); Ok(())
}

/// Exact catalog-owned counts, not a global-store nonempty heuristic.
#[derive(Debug, Default)]
pub struct RepositoryRestoreReport {
    pub repositories: usize,
    pub images: usize,
    pub layers: usize,
    pub uploads: usize,
    pub global_records: usize,
}

/// Complete catalog-owned control/payload projection, before listeners. Does
/// not restore transient Basic credentials or signed download grants.
pub async fn restore_catalog<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Result<RepositoryRestoreReport, StoreError> {
    restore_repository_catalog(engine).await
}

/// Boot calls this after installing B2 and before listeners. The service mutation
/// gate excludes service writers; B2's cursor also refuses a changed catalog
/// revision. Build privately and replace RAM only after BOTH passes validate.
/// The historical name remains for the pending-publication caller. Global
/// control records now assemble privately and apply with the repository maps.
/// Callers still must wire this before listeners; source is not runtime proof.
pub async fn restore_repository_catalog<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Result<RepositoryRestoreReport, StoreError> {
    let gate = payloads::mutation_guard(engine).await?;
    restore_repository_catalog_guarded(engine, &gate).await
}

pub(super) async fn restore_repository_catalog_guarded<Eng: Send + Sync + 'static>(
    engine: &Eng, _gate: &payloads::MutationGuard,
) -> Result<RepositoryRestoreReport, StoreError> {
    let store = payloads::bound_store(engine)?;
    let (globals, global_records) = super::catalog_global::load(&store, _gate).await?;
    let owner = payloads::registry_owner()?;
    let mut repositories = BTreeMap::new();
    let mut cursor = None;
    loop {
        // One record fits every admitted positive page-count profile; B2 still
        // enforces its byte cap before materializing the record's metadata.
        let page = store.list_records(&owner, cursor, 1).await?;
        for record in page.records {
            let Some(value) = record.value else { continue; };
            let key = std::str::from_utf8(record.key.key()).map_err(|_| StoreError::Integrity)?;
            let is_config = key.starts_with("repository/");
            let declared_config = value.metadata.get("kind").and_then(serde_json::Value::as_str) == Some("ecr-repository");
            if is_config != declared_config { return Err(StoreError::Integrity); }
            if !is_config { continue; }
            if value.schema_version != 1 || value.references.len() != 1
                || value.metadata.as_object().is_none_or(|v| v.len() != 3) { return Err(StoreError::Integrity); }
            let config: RepositoryConfig = read_json(&store, &value.references[0]).await?;
            let repo = config.restore(record.revision)?;
            let scope = payloads::repository_owner(&repo)?;
            if record.key != scope.config_key()?
                || value.metadata.get("name").and_then(serde_json::Value::as_str) != Some(repo.name.as_str())
                || value.metadata.get("generation").and_then(serde_json::Value::as_str) != Some(repo.payload_generation.as_str())
                || repositories.len() >= MAX_REPOSITORIES
                || repositories.insert(repo.name.clone(), repo).is_some() { return Err(StoreError::Integrity); }
        }
        cursor = page.next;
        if cursor.is_none() { break; }
    }

    let mut uploads = BTreeMap::new();
    let mut report = RepositoryRestoreReport { repositories: repositories.len(), global_records, ..Default::default() };
    let mut cursor = None;
    loop {
        let page = store.list_records(&owner, cursor, 1).await?;
        for record in page.records {
            let Some(value) = record.value else { continue; };
            let key = std::str::from_utf8(record.key.key()).map_err(|_| StoreError::Integrity)?;
            if !key.starts_with("repo/") { continue; }
            // Repository names contain slashes; the final generation/kind/id
            // components do not. Never split a repository at its first slash.
            let (name, generation, kind, id) = scoped_key(key)?;
            let Some(repo) = repositories.get_mut(name) else { continue; };
            if repo.payload_generation != generation { continue; } // retired incarnation stays inert
            let scope = payloads::repository_owner(repo)?;
            if record.key != scope.key(kind, id)? { return Err(StoreError::Integrity); }
            match kind {
                "layer" => {
                    if repo.layer_blobs.len() >= MAX_LAYERS_PER_REPOSITORY { return Err(StoreError::Integrity); }
                    let digest = service_common::payload_store::Sha256Digest::parse(id)?;
                    let layer = payloads::stored_layer(&store, &scope, digest).await?.ok_or(StoreError::Integrity)?;
                    let size = i64::try_from(layer.payload.len()).map_err(|_| StoreError::Integrity)?;
                    if repo.layer_blobs.insert(id.to_string(), LayerBlob {
                        digest: id.to_string(), size: Some(size), media_type: layer.media_type,
                        filesystem: layer.filesystem, payload: Some(layer.payload),
                    }).is_some() { return Err(StoreError::Integrity); }
                    report.layers += 1;
                }
                "image" => {
                    if value.schema_version != 1 || value.references.len() < 3
                        || value.metadata.as_object().is_none_or(|v| v.len() != 4)
                        || value.metadata.get("kind").and_then(serde_json::Value::as_str) != Some("ecr-image")
                        || value.metadata.get("repository").and_then(serde_json::Value::as_str) != Some(name)
                        || value.metadata.get("generation").and_then(serde_json::Value::as_str) != Some(generation)
                        || value.metadata.get("digest").and_then(serde_json::Value::as_str) != Some(id)
                        || repo.images.len() >= MAX_IMAGES_PER_REPOSITORY { return Err(StoreError::Integrity); }
                    let document = payloads::read_document(&store, &value.references[0]).await?;
                    let metadata: ImageMetadata = read_json(&store, &value.references[1]).await?;
                    let tag_list: Vec<String> = read_json(&store, &value.references[2]).await?;
                    let tag_count = tag_list.len();
                    let tags: BTreeSet<String> = tag_list.into_iter().collect();
                    if tags.len() != tag_count { return Err(StoreError::Integrity); }
                    let image = metadata.restore(document, value.references[0].clone(), tags)?;
                    if image.digest != id { return Err(StoreError::Integrity); }
                    restore_tags(&mut repo.tag_index, &image.tags, &image.digest)?;
                    if repo.images.insert(image.digest.clone(), image).is_some() { return Err(StoreError::Integrity); }
                    report.images += 1;
                }
                "upload" => {
                    if uploads.len() >= MAX_LAYER_UPLOADS { return Err(StoreError::Integrity); }
                    let bound = super::payload_upload::lookup(&store, &scope, id).await?.ok_or(StoreError::Integrity)?;
                    if uploads.insert(id.to_string(), bound.index_projection()).is_some() { return Err(StoreError::Integrity); }
                    report.uploads += 1;
                }
                _ => return Err(StoreError::Integrity),
            }
        }
        cursor = page.next;
        if cursor.is_none() { break; }
    }

    // All images and layers are present now. Validate references against the
    // CURRENT repository incarnation, not merely a namespace-valid CAS handle.
    for repo in repositories.values() {
        let scope = payloads::repository_owner(repo)?;
        for image in repo.images.values() {
            let record = store.record(&scope.key("image", &image.digest)?).await?.ok_or(StoreError::Integrity)?;
            let value = record.value.ok_or(StoreError::Integrity)?;
            let mut expected = Vec::new();
            match &image.references {
                ManifestContent::Blobs(blobs) => for blob in blobs {
                    expected.push(repo.layer_blobs.get(&blob.digest).and_then(|v| v.payload.clone()).ok_or(StoreError::Integrity)?);
                },
                ManifestContent::List(children) => for child in children {
                    let child = repo.images.get(child).ok_or(StoreError::Integrity)?;
                    let ManifestDocument::Stored(handle) = &child.manifest else { return Err(StoreError::Integrity); };
                    expected.push(handle.clone());
                },
                ManifestContent::Opaque => {},
            }
            if value.references.get(3..) != Some(expected.as_slice()) { return Err(StoreError::Integrity); }
        }
    }
    let state = service_state(engine).ok_or(StoreError::Invalid("ECR state binding"))?;
    let mut state = state.lock();
    state.repositories = repositories;
    state.uploads = uploads;
    super::catalog_global::apply(&mut state, globals);
    Ok(report)
}

async fn read_json<T: serde::de::DeserializeOwned>(store: &PayloadStore, handle: &PayloadHandle) -> Result<T, StoreError> {
    let text = payloads::read_document(store, handle).await?;
    serde_json::from_str(&text).map_err(|_| StoreError::Integrity)
}

fn scoped_key(key: &str) -> Result<(&str, &str, &str, &str), StoreError> {
    let rest = key.strip_prefix("repo/").ok_or(StoreError::Integrity)?;
    let mut parts = rest.rsplitn(4, '/');
    let id = parts.next().filter(|v| !v.is_empty()).ok_or(StoreError::Integrity)?;
    let kind = parts.next().filter(|v| !v.is_empty()).ok_or(StoreError::Integrity)?;
    let generation = parts.next().filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit())).ok_or(StoreError::Integrity)?;
    let name = parts.next().filter(|v| !v.is_empty() && v.len() <= 256).ok_or(StoreError::Integrity)?;
    Ok((name, generation, kind, id))
}

fn restore_tags(index: &mut BTreeMap<String, String>, tags: &BTreeSet<String>, digest: &str) -> Result<(), StoreError> {
    for tag in tags {
        if index.len() >= MAX_TAGS_PER_REPOSITORY || index.contains_key(tag) {
            return Err(StoreError::Integrity);
        }
        index.insert(tag.clone(), digest.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod restore_tests {
    use super::*;
    use service_common::payload_store::{StoreConfig, StoreLimits};

    #[tokio::test]
    async fn full_builder_reuses_tag_only_metadata_and_refuses_wrong_retained_reference() {
        let limits = StoreLimits {
            max_storage_bytes:64*1024*1024, max_catalog_bytes:16*1024*1024,
            max_stage_bytes:4*1024*1024, max_chunk_bytes:64*1024, max_metadata_bytes:64*1024,
            max_records_per_transaction:256, max_references_per_record:16_384,
            max_page_records:32, max_page_bytes:128*1024, max_stages:2048,
            max_records:2048, max_commits:512, max_transaction_bytes:16*1024*1024,
            max_inflight_operations:16, max_read_leases:8, max_data_files:4096,
        };
        let store = PayloadStore::open_store(StoreConfig::memory("ecr-builder", "fixture", limits).unwrap()).await.unwrap();
        let mut repo = Repo {
            payload_generation:"a".repeat(64), payload_revision:None,
            name:"fixture/team".into(), registry_id:registry_id().into(), arn:repository_arn("us-east-1","fixture/team"),
            created_at:std::time::UNIX_EPOCH, image_tag_mutability:"MUTABLE".into(), image_tag_mutability_exclusion_filters:Vec::new(),
            image_scanning_config:ImageScanningConfiguration::default(), encryption_config:EncryptionConfiguration { encryption_type:"AES256".into(), kms_key:None },
            tags:Vec::new(), policy_text:None, lifecycle_policy:None, lifecycle_preview:None,
            images:BTreeMap::new(), tag_index:BTreeMap::new(), layer_blobs:BTreeMap::new(),
        };
        let (config, retired) = repository_change(&store,&repo).await.unwrap();
        let receipt = store.commit(ReferenceTransaction { id:store.new_commit_id(), owner:payloads::registry_owner().unwrap(), changes:vec![config], retire:vec![retired] }).await.unwrap();
        repo.payload_revision = Some(receipt.revision);
        // Seed with actual builder groups under the SAME 256-entry profile.
        for n in 0..100 {
            let mut next = repo.clone();
            let document = format!("{{\"fixture\":{n}}}"); // supported opaque manifest, no blob prerequisites
            let digest = manifest_digest(&document);
            let mut image = ImageRecord::pushed(digest.clone(),document,None,std::time::UNIX_EPOCH);
            image.tags.insert(format!("tag-{n}"));
            next.images.insert(digest,image);
            let scope = payloads::repository_owner(&repo).unwrap();
            let (mut changes,retire) = image_changes(&store,&scope,&repo,&mut next).await.unwrap();
            changes.push(scope.assertion(&store).await.unwrap());
            let receipt = store.commit(ReferenceTransaction { id:store.new_commit_id(), owner:scope.owner, changes, retire }).await.unwrap();
            next.payload_revision = Some(receipt.revision); repo = next;
        }
        let before = repo.clone();
        let scope = payloads::repository_owner(&before).unwrap();
        let mut original_metadata = BTreeMap::new();
        for digest in before.images.keys() {
            let value = store.record(&scope.key("image",digest).unwrap()).await.unwrap().unwrap().value.unwrap();
            original_metadata.insert(digest.clone(),value.references[1].clone());
        }
        let mut after = before.clone();
        for image in after.images.values_mut() { image.tags.clear(); }
        let (mut changes,retire) = image_changes(&store,&scope,&before,&mut after).await.unwrap();
        assert_eq!(changes.len(),100);
        assert_eq!(retire.len(),100, "only changed tag documents are staged");
        for change in &changes {
            let value = change.value.as_ref().unwrap();
            let digest = value.metadata.get("digest").unwrap().as_str().unwrap();
            assert_eq!(&value.references[1],original_metadata.get(digest).unwrap());
        }
        changes.push(scope.assertion(&store).await.unwrap());
        assert_eq!(changes.len()+retire.len(),201);
        let receipt = store.commit(ReferenceTransaction { id:store.new_commit_id(), owner:scope.owner.clone(), changes, retire }).await.unwrap();
        after.payload_revision = Some(receipt.revision);

        // Mirror event case: changing metadata must not re-stage unchanged tags.
        let before_event = after.clone();
        let scope = payloads::repository_owner(&before_event).unwrap();
        let mut original_tags = BTreeMap::new();
        for digest in before_event.images.keys() {
            let value = store.record(&scope.key("image",digest).unwrap()).await.unwrap().unwrap().value.unwrap();
            original_tags.insert(digest.clone(),value.references[2].clone());
        }
        let event_time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(100);
        for image in after.images.values_mut() { image.last_recorded_pull_time = Some(event_time); }
        let (mut changes,retire) = image_changes(&store,&scope,&before_event,&mut after).await.unwrap();
        assert_eq!(changes.len(),100);
        assert_eq!(retire.len(),100,"only changed metadata documents are staged");
        for change in &changes {
            let value = change.value.as_ref().unwrap();
            let digest = value.metadata.get("digest").unwrap().as_str().unwrap();
            assert_eq!(&value.references[2],original_tags.get(digest).unwrap());
        }
        changes.push(scope.assertion(&store).await.unwrap());
        assert_eq!(changes.len()+retire.len(),201);
        let receipt = store.commit(ReferenceTransaction { id:store.new_commit_id(),owner:scope.owner.clone(),changes,retire }).await.unwrap();
        after.payload_revision = Some(receipt.revision);
        for digest in after.images.keys() {
            let value = store.record(&scope.key("image",digest).unwrap()).await.unwrap().unwrap().value.unwrap();
            let restored: ImageMetadata = read_json(&store,&value.references[1]).await.unwrap();
            assert_eq!(restored.last_recorded_pull_time,Some(event_time));
        }

        // A namespace-valid but WRONG image's metadata is not reuse authority.
        let digests: Vec<_> = after.images.keys().cloned().collect();
        let key = scope.key("image",&digests[0]).unwrap();
        let old = store.record(&key).await.unwrap().unwrap();
        let mut corrupt = old.value.unwrap();
        corrupt.references[1] = original_metadata.get(&digests[1]).unwrap().clone();
        store.commit(ReferenceTransaction { id:store.new_commit_id(), owner:scope.owner,
            changes:vec![RecordChange { key, condition:RecordCondition::Exact(old.revision), value:Some(corrupt) }], retire:Vec::new() }).await.unwrap();
        let mut changed = after.clone();
        changed.images.get_mut(&digests[0]).unwrap().tags.insert("retry".into());
        let scope = payloads::repository_owner(&after).unwrap();
        assert!(matches!(image_changes(&store,&scope,&after,&mut changed).await,Err(StoreError::Integrity)));
    }
    #[test]
    fn nested_repository_key_keeps_its_complete_name() {
        let generation = "a".repeat(64);
        let key = format!("repo/team/component/{generation}/upload/id-1");
        assert_eq!(scoped_key(&key).unwrap(), ("team/component", generation.as_str(), "upload", "id-1"));
        assert!(scoped_key("repo/team/component/bad/upload/id-1").is_err());
        assert!(scoped_key(&format!("repo/team/{generation}/image/")).is_err());
    }
    #[test]
    fn duplicate_tag_ownership_is_integrity_not_last_wins() {
        let mut index = BTreeMap::new();
        let tags = BTreeSet::from(["latest".to_string()]);
        restore_tags(&mut index, &tags, "sha256:first").unwrap();
        assert!(restore_tags(&mut index, &tags, "sha256:second").is_err());
    }
}
