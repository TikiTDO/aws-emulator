// SPDX-License-Identifier: CC0-1.0

//! Durable synthetic pull-through import. One group publishes repository identity,
//! configuration layer and image/tag references; no upstream image bytes are claimed.
use super::{catalog, payloads, registry_pull::{CacheImportPlan, PullRefusal}, LayerBlob, LayerFilesystem, Repo};
use service_common::payload_store::{PayloadStore, RecordChange, RecordCondition, RecordData, ReferenceTransaction, StageRetirement};

pub(super) async fn publish<Eng: Send + Sync + 'static>(
    engine: &Eng,
    mutation: &payloads::MutationGuard,
    store: &PayloadStore,
    mut plan: CacheImportPlan,
    authorize: impl Fn() -> Result<(), PullRefusal>,
) -> Result<Repo, PullRefusal> {
    let unavailable = |_| PullRefusal::StorageUnavailable;
    let capacity = emulator_server::ecr_capacity::configured().map_err(|_| PullRefusal::StorageUnavailable)?;
    let state = super::service_state(engine).ok_or(PullRefusal::StorageUnavailable)?;
    let held = { state.lock().caller_bytes_held() };
    if plan.config.len() > capacity.layer_bytes || held.checked_add(plan.config.len()).is_none_or(|v| v > capacity.store_bytes) {
        return Err(PullRefusal::StorageUnavailable);
    }
    let scope = payloads::repository_owner(&plan.before).map_err(unavailable)?;
    let (sealed, checkpoint) = payloads::stage_document(store, scope.owner.clone(),
        bytes::Bytes::copy_from_slice(plan.config.as_bytes()), capacity.layer_bytes as u64).await.map_err(unavailable)?;
    let handle = sealed.handle().clone();
    if handle.sha256().to_string() != plan.config_digest { return Err(PullRefusal::StorageUnavailable); }
    let key = payloads::layer_key(&scope, handle.sha256()).map_err(unavailable)?;
    let previous = store.record(&key).await.map_err(unavailable)?;
    let layer = RecordChange {
        key, condition: previous.map_or(RecordCondition::Absent, |record| RecordCondition::Exact(record.revision)),
        value: Some(RecordData { schema_version: 1,
            metadata: serde_json::json!({"kind":"ecr-layer", "mediaType":"application/vnd.oci.image.config.v1+json", "operatingSystem":null, "packageManager":null}),
            references: vec![handle.clone()],
        }),
    };
    plan.after.layer_blobs.insert(plan.config_digest.clone(), LayerBlob {
        digest: plan.config_digest, size: Some(plan.config.len() as i64),
        media_type: Some("application/vnd.oci.image.config.v1+json".into()),
        filesystem: LayerFilesystem::default(), payload: Some(handle),
    });
    plan.after.refresh_image_sizes();
    // Canonical builder owns the image schema and checks same-group layer input.
    let (mut changes, mut retire) = catalog::image_changes_with_staged_layers(
        store, &scope, &plan.before, &mut plan.after, vec![layer],
    ).await.map_err(unavailable)?;
    retire.push(StageRetirement { upload: sealed.upload().clone(), checkpoint });
    if scope.revision.is_some() {
        changes.push(scope.assertion(store).await.map_err(unavailable)?);
    } else {
        let (config, closed) = catalog::repository_change(store, &plan.after).await.map_err(unavailable)?;
        changes.push(config); retire.push(closed);
    }
    // The ECR mutation token excludes rule/config mutation; IAM can change while
    // staging, so take the actual role/repository policy decision again here.
    authorize()?;
    {
        let state = state.lock();
        match (state.repositories.get(&scope.name), scope.revision) {
            (None, None) => {},
            (Some(repo), Some(revision)) if repo.payload_generation == scope.generation && repo.payload_revision == Some(revision) => {},
            _ => return Err(PullRefusal::StorageUnavailable),
        }
    }
    let receipt = mutation.commit(store, ReferenceTransaction {
        id: store.new_commit_id(), owner: scope.owner, changes, retire,
    }).await.map_err(|_| PullRefusal::StorageUnavailable)?;
    plan.after.payload_revision = Some(receipt.revision);
    Ok(plan.after)
}
