Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: b4110983ca6b6466b7658f165e005d75330506d1

# UpdateRecord never replicates the updated record to the OfflineStore, and an UpdateRecord naming only the OfflineStore writes nowhere at all

_Filed: 2026-11-11T17:31:25Z_
_Operation: UpdateRecord | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `b4110983ca6b6466b7658f165e005d75330506d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TargetStores` on UpdateRecord is documented as "The target stores for the record update. By default, Amazon SageMaker Feature Store updates the record in all stores associated with the FeatureGroup." The handler only ever writes this service's in-memory OnlineStore: it resolves the feature group (so it holds `definition.offline_store`, the delivery `crate::state::offline_store::replicate` takes) but never calls the replication its two sibling writers call, and when `TargetStores` names only the OfflineStore it returns 200 having written nothing anywhere. So a default (no `TargetStores`) update of a record in a feature group with an `OfflineStoreConfig` silently stops propagating to S3 after the initial ingestion, and an explicitly offline-only update is a hollow 200.

Both the handler body and the module doc admit the gap.

## Evidence

Model (UpdateRecord$TargetStores, reproduced from the model at services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/ops/update_record.rs:101-103): "`TargetStores` (TargetStores, len 1..2) — The target stores for the record update. By default, Amazon SageMaker Feature Store updates the record in all stores associated with the FeatureGroup."

The handler's own admissions:
- services/.../src/ops/update_record.rs:369-374: "A feature group that HAS no `OnlineStore` holds no online record to update, and the update AWS would replicate to the OfflineStore is not replicated here (see the module docs), so there is nothing for this handler to do once the request itself has been found well-formed"
- services/.../src/ops/update_record.rs:36-38: "The OfflineStore side of an UPDATE — the partially updated record AWS replicates there — is left to the issue that owns this operation: the ingestion of a WHOLE record is what [`crate::state::offline_store`] replicates today."

The offline-only path that writes nothing — services/.../src/ops/update_record.rs:422-424:
```
        if !crate::state::targets_online_store(input.target_stores.as_ref()) {
            return Ok(UpdateRecordOutput);
        }
```

The sibling writers in this same crate DO call the helper for the store this one skips:
- services/.../src/ops/put_record.rs:291-302: `if crate::state::targets_offline_store(input.target_stores.as_ref()) { if let Some(delivery) = definition.offline_store.as_ref() { crate::state::offline_store::replicate(...).await; } }`
- services/.../src/ops/batch_write_record.rs:301-309 and :342-352: the same two steps, per entry.
Grep for `offline_store::replicate` in this crate's `src/` returns only put_record.rs:293 and batch_write_record.rs:343.

Test coverage: tests/all/offline_store_replication.rs exercises PutRecord/BatchWriteRecord replication only; no test asserts an S3 object for an UpdateRecord, so the current no-op is unpinned.

## Steps to Reproduce

1. Create a feature group with an OnlineStore and an OfflineStoreConfig through the SageMaker control plane (as tests/all/offline_store_replication.rs does).
2. PUT /FeatureGroup/<name> with a record — an object lands under the configured S3 prefix (put_record.rs:291-302).
3. POST /FeatureGroup/<name>/Record (UpdateRecord) with `Features` changing one feature value and a newer event time, and no `TargetStores` — 200 is returned, the in-memory record is updated (update_record.rs:470), and no new object appears in the offline store.
4. Repeat the same UpdateRecord with `TargetStores: ["OfflineStore"]` — the handler returns 200 at update_record.rs:423 having written nothing to either store.
