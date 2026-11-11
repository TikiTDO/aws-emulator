Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: b4110983ca6b6466b7658f165e005d75330506d1

# DeleteRecord writes no deleted-record marker to the OfflineStore, though the crate already has the replication helper its siblings use

_Filed: 2026-11-11T17:31:25Z_
_Operation: DeleteRecord | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `b4110983ca6b6466b7658f165e005d75330506d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation's own modelled documentation says a deletion marks the OfflineStore in both deletion modes — and even when the deletion does NOT occur online (SoftDelete with an out-of-order EventTime). The handler never touches the offline store: it resolves the group (and so holds `definition.offline_store`, the delivery the crate's `offline_store::replicate` needs), then mutates only the in-memory OnlineStore and answers 200. `crate::state::offline_store::replicate` is called from exactly two places in this crate — PutRecord and BatchWriteRecord — so a feature group with an `OfflineStoreConfig` accumulates ingested rows in S3 that are never followed by the delete marker that makes a record's deletion visible to any offline reader. The crate's own Parquet writer already carries the `is_deleted` column and hardcodes it `false` with the comment that an INGESTED record is not a delete marker, i.e. the column exists solely for the row this operation never writes.

The handler's module doc admits the gap in so many words.

## Evidence

Smithy, DeleteRecord documentation (reproduced from the model at services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/ops/delete_record.rs:58-71): "In both cases, Feature Store appends the deleted record marker to the OfflineStore. The deleted record marker is a record with the same RecordIdentifer as the original, but with is_deleted value set to True, EventTime set to the delete input EventTime, and other feature values set to null. … For SoftDelete, the existing (not deleted) record remains in the OnlineStore, though the delete record marker is still written to the OfflineStore. HardDelete returns EventTime: 400 ValidationException … No delete record marker is written to the OfflineStore."

The handler's own admission — services/.../src/ops/delete_record.rs:14-18:
"The deleted-record marker the model says Feature Store appends to the OfflineStore is a DELETE's own rule — `HardDelete` and `SoftDelete` leave different marks there — and is left to the issue that owns this operation, so only the OnlineStore side is served here; an INGESTION's replication is [`crate::state::offline_store`]."

The whole write path of the handler (services/.../src/ops/delete_record.rs:312-323) touches only the in-memory store and never calls the replication helper:
```
        if hard_delete {
            group.remove_record(&identifier);
        } else {
            group.soft_delete_record(
```

The sibling paths that DO replicate, in this same crate:
- services/.../src/ops/put_record.rs:291-302: `if crate::state::targets_offline_store(...) { if let Some(delivery) = definition.offline_store.as_ref() { crate::state::offline_store::replicate(ctx.callback(), …).await; } }`
- services/.../src/ops/batch_write_record.rs:342-352: the same `crate::state::offline_store::replicate(...)` loop.

The column that only a delete marker can set — services/.../src/state/offline_store.rs:49-50 (`const IS_DELETED_COLUMN: &str = "is_deleted";`) and services/.../src/state/offline_store.rs:280-285:
```
        // `is_deleted`: an INGESTED record is not a delete marker.
        let mut column = next_column(&mut row_group, group_name)?;
        column
            .typed::<BoolType>()
            .write_batch(&[false], Some(&present), None)
```

Test coverage: tests/all/offline_store_replication.rs covers the ingestion replication only (PutRecord/BatchWriteRecord); no test asserts any object written for a DeleteRecord, so nothing pins the current no-op.

## Steps to Reproduce

1. Create a feature group with both an OnlineStore and an OfflineStoreConfig through the SageMaker control plane (as tests/all/offline_store_replication.rs does).
2. PUT /FeatureGroup/<name> with one record — an object appears under the configured S3 prefix (put_record.rs:291-302).
3. DELETE /FeatureGroup/<name>?RecordIdentifierValueAsString=<id>&EventTime=<later>&DeletionMode=HardDelete — the handler answers 200 after delete_record.rs:313 and writes NOTHING to S3; the offline store still shows only the ingested row with is_deleted=false.
4. Repeat with DeletionMode=SoftDelete, including with an EventTime strictly older than the stored record's (delete_record.rs:302-310, which returns 200 without deleting): again no marker object is written, although the model says the marker is still written in that case.
