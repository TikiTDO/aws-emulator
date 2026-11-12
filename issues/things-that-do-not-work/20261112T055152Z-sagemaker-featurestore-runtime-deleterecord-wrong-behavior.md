Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 848ff7798cc0e070898f50ccf51befabeb3eafe0

# A SoftDelete throws away the record's TtlDuration and ExpiresAt, so a soft-deleted record is never hard-deleted when its TTL falls due

_Filed: 2026-11-12T05:51:52Z_
_Operation: DeleteRecord | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `848ff7798cc0e070898f50ccf51befabeb3eafe0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteRecord in `SoftDelete` mode moves the stored record's `EventTime` to the delete input's `EventTime` and, in the same breath, clears the record's TTL state: `expires_at`, `expires_at_secs` and `ttl_secs` are all set to `None`. Nothing in the crate states a reason for discarding them, and two things in the crate say they must not be discarded.

(1) The `StoredRecord::ttl_secs` field exists precisely so that a call which MOVES a record's event time can re-derive the expiry from the duration the record was written with — its own doc says so — and `soft_delete_record` is exactly such a call: it writes a new `event_time` on the record (state.rs:220) and then drops the duration instead of calling the crate's `StoredRecord::recompute_expiry`, which is what the sibling write path (UpdateRecord) calls for the very same situation.

(2) `FeatureGroupStore::purge_expired` only removes records whose `expires_at_secs` is set, so once the TTL state is cleared the record can never be hard-deleted by the clock. Two observable consequences: `ListRecords` with `IncludeSoftDeletedRecords=true` keeps reporting the identifier for the lifetime of the process, long past the expiry the record was ingested with (and the identifier permanently occupies one of the group's record slots); and `FeatureGroupStore::ingest`'s historic-record comparison keeps ordering every later `PutRecord` against the tombstone's event time forever, instead of the record disappearing when its expiry fell due and leaving the identifier free.

The tombstone rule the handler is implementing says nothing about the TTL: the model describes the marker as "a record with the same RecordIdentifer as the original, but with is_deleted value set to True, EventTime set to the delete input EventTime, and other feature values set to null" — the record's time-to-live is not one of the "other feature values", and the model's `TtlDuration` is documented as the duration after which "the record is hard deleted".

## Evidence

Model, `TtlDuration` shape doc (`smithy_query(query="shape", shape="TtlDuration")`): "Time to live duration, where the record is hard deleted after the expiration time is reached; ExpiresAt = EventTime + TtlDuration."

The crate's own stated reason for keeping the duration — services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/state.rs:266-271:
```
    /// The record-level `TtlDuration` in SECONDS, kept so a later call that moves the
    /// record's `EventTime` can re-derive `ExpiresAt = EventTime + TtlDuration` from the
    /// duration the record was written with. `UpdateRecord` documents exactly that: "If
    /// you omit this parameter, the record's existing TTL setting remains unchanged",
    /// and the setting is the DURATION, not the instant it happened to fall due at.
    pub ttl_secs: Option<f64>,
```

The code that breaks it — state.rs:218-223 (`soft_delete_record`, whose doc quotes only the tombstone rule, state.rs:202-205):
```
        if let Some(record) = self.records.get_mut(identifier) {
            record.soft_deleted = true;
            record.event_time = Some(event_time.to_owned());
            record.expires_at = None;
            record.expires_at_secs = None;
            record.ttl_secs = None;
```

The sibling path that does the opposite for the same "the event time moved" case — src/ops/update_record.rs:453-465:
```
        // "If you omit EventTime, the service keeps the record's existing EventTime."
        if let Some(at) = posted_event_time {
            updated.event_time = Some(at);
        }
        // "If you omit this parameter, the record's existing TTL setting remains
        // unchanged" (`TtlDuration`), so a request that names one replaces the record's
        // duration and one that does not keeps it — and `ExpiresAt` is re-derived from
        // the duration and the record's event time either way, because an update may
        // have moved that event time.
        if let Some(secs) = ttl_secs {
            updated.ttl_secs = Some(secs);
        }
        updated.recompute_expiry();
```

Why the record then never goes away — state.rs:128-135:
```
    pub fn purge_expired(&mut self, now_secs: f64) {
        if self.next_expiry_secs.is_none_or(|due| due > now_secs) {
            return;
        }
        self.records
            .retain(|_, record| record.expires_at_secs.is_none_or(|at| at > now_secs));
```
and why a later write stays ordered against it — state.rs:180-189 (`ingest`, comparing the posted event time with `self.record(...).and_then(StoredRecord::event_time_secs)`).

The call site — src/ops/delete_record.rs:312-321:
```
        if hard_delete {
            group.remove_record(&identifier);
        } else {
            group.soft_delete_record(
                &identifier,
                &input.event_time,
                &definition.record_identifier_feature_name,
                &definition.event_time_feature_name,
            );
        }
```

Test coverage: no test pins the TTL of a soft-deleted record, so re-deriving it breaks nothing. tests/all/online_store_record_lifecycle.rs:707-745 (`delete_record_removes_the_record_from_the_online_store`) soft-deletes records written with NO `TtlDuration` at all and only asserts that `ListRecords` with `IncludeSoftDeletedRecords=true` still shows the tombstone ("`SoftDelete` keeps a tombstone `ListRecords` can still show", :710, assertions at :769-784). The TTL purge is pinned only for records that were never deleted — tests/all/online_store_record_lifecycle.rs:573-622 (`a_record_whose_ttl_has_passed_is_gone_from_the_online_store`).

## Steps to Reproduce

1. Through the SageMaker control plane, CreateFeatureGroup `customers` with RecordIdentifierFeatureName=customer_id, EventTimeFeatureName=event_time and OnlineStoreConfig.EnableOnlineStore=true, so this data plane resolves the name.
2. PUT /FeatureGroup/customers with Record=[{FeatureName:customer_id,ValueAsString:"C1"},{FeatureName:event_time,ValueAsString:"2000-01-01T00:00:00Z"}] and TtlDuration={Unit:"Seconds",Value:1} — the expiry is already due, and (per tests/all/online_store_record_lifecycle.rs:573-622) the record is purged on the next read: ListRecords returns no identifiers.
3. Repeat step 2 on a fresh emulator, then immediately DELETE /FeatureGroup/customers?RecordIdentifierValueAsString=C1&EventTime=2000-01-02T00:00:00Z&DeletionMode=SoftDelete (200). state.rs:221-223 now clears expires_at/expires_at_secs/ttl_secs.
4. POST /FeatureGroup/customers/ListRecords with {"IncludeSoftDeletedRecords": true} — "C1" is still listed, and stays listed for the life of the process, although the record's TTL expired before the delete was even submitted.
5. PUT /FeatureGroup/customers again with the same identifier and an event_time of "1999-01-01T00:00:00Z": the write is judged historic against the tombstone's event time (state.rs:180-189) and the online store keeps the tombstone — forever, because nothing can ever expire it.
