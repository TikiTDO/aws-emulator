Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 14c2f807276b097453c7eebbf32725a3995b3836

# A record posted with no value for the group's event-time feature bypasses the historic-record comparison and silently overwrites a newer stored record (its values, EventTime and TTL)

_Filed: 2026-11-11T19:42:59Z_
_Operation: BatchWriteRecord | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `14c2f807276b097453c7eebbf32725a3995b3836`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The shared ingestion of this data plane (`crate::state::FeatureGroupStore::ingest`, called by BatchWriteRecord and by PutRecord) only performs the historic-record comparison when BOTH the posted record and the stored record have an event time. A posted `Record` (or batch `Entry.Record`) that carries no value for the feature group's own `EventTimeFeatureName` therefore skips the comparison entirely and is stored as a full over-write: the previously stored record — which may carry a much later `EventTime`, a `TtlDuration` and a computed `ExpiresAt` — is destroyed, and the replacement carries `event_time: None`, `ttl_secs: None`, `expires_at: None`, so the record also stops expiring and reports no `ExpiresAt`.

That contradicts the model sentence the ingestion is built from (reproduced in the crate at put_record.rs:59-62) and, more sharply, it contradicts the crate's own rule for an event time it cannot read: `prepare_record` REFUSES a value in neither accepted timestamp format precisely so the historic-record rule is not "silently disabling both" (state.rs:416-430), while the ABSENT value one line earlier is accepted and silently disables the very same rule (state.rs:408-414).

Both whole-record writers share this path, so a batched write and a single write are judged the same wrong way; the symptom is filed here because BatchWriteRecord's handler calls both `prepare_record` and `ingest` and the batch module doc claims entries are ingested "exactly as" the same record written on its own.

## Evidence

The rule the ingestion is documented to enforce — services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/state.rs:173-179 (doc of `ingest`), quoting the PutRecord documentation also reproduced at services/.../src/ops/put_record.rs:59-62:
"If a new record's EventTime is greater, the new record is written to both the OnlineStore and OfflineStore. Otherwise, the record is a historic record and it is written only to the OfflineStore": this is the ONLINE store, so a posted record whose event time is not later than the stored record's leaves that record standing ([`Ingested::Historic`])

The code that skips it — services/.../src/state.rs:180-195:
```
    pub fn ingest(&mut self, prepared: PreparedRecord) -> Ingested {
        if let (Some(posted), Some(stored)) = (
            prepared.event_time_secs,
            self.record(&prepared.record.identifier)
                .and_then(StoredRecord::event_time_secs),
        ) {
            if posted <= stored {
                return Ingested::Historic;
            }
        }
        if self.insert_record(prepared.record) {
```
`prepared.event_time_secs` is `None` whenever the posted record carries no value for the group's event-time feature — services/.../src/state.rs:408-414:
```
        // A record that carries no value for the group's event-time feature has no event
        // time, so neither the historic-record rule nor a TTL has anything to compute from.
        let event_time_feature_name = &definition.event_time_feature_name;
        let event_time = record
            .iter()
            .find(|feature| feature.feature_name == *event_time_feature_name)
            .and_then(|feature| feature.value_as_string.clone());
```
and the SAME function refuses the neighbouring case for exactly the reason that is violated here — services/.../src/state.rs:416-418:
```
    // An event time the group identified must be a timestamp: the TTL and the
    // historic-record rule are both computed from it, so a value in neither accepted
    // format is refused rather than silently disabling both.
```
(the refusal itself at state.rs:419-430).

What is lost: `prepare_record` builds the replacement from this request alone — services/.../src/state.rs:449-458 — `event_time` (None), `ttl_secs` (None) and then `stored.recompute_expiry();`, which per state.rs:290-297 sets BOTH `expires_at` and `expires_at_secs` to `None`. `insert_record` (state.rs:158-168) then replaces the stored record under the same identifier unconditionally.

BatchWriteRecord reaches both — services/.../src/ops/batch_write_record.rs:284-295 (`crate::state::prepare_record(&definition, &entry.record, ttl_duration)`) and batch_write_record.rs:328-333:
```
            match group.ingest(prepared) {
                // A historic entry is written only to the OfflineStore, which the
                // replication above delivers: nothing was stored online and nothing failed.
                crate::state::Ingested::Stored | crate::state::Ingested::Historic => {}
```
with the module doc claiming parity at batch_write_record.rs:4-8 ("through the same preparation and ingestion one `PutRecord` performs … so a batched write and a single write of the same record cannot be judged differently").

Test coverage: the only test of a record with no event-time value writes it into an EMPTY group — services/.../tests/all/batch_write_record_bulk_ingestion.rs:359-375: "// A record with no value for the group's event-time feature has no event time to order against, and is stored all the same" — so the overwrite-of-an-existing-record case is unpinned (a fix cannot change that row: `ingest` returns `Historic` only when a STORED record has an event time). The historic-record protection itself is pinned only for posted records that DO carry an event time: tests/all/online_store_record_lifecycle.rs:343-347 and tests/all/offline_store_replication.rs:824-842.

## Steps to Reproduce

1. Through the SageMaker control plane, CreateFeatureGroup `customers` with RecordIdentifierFeatureName=customer_id, EventTimeFeatureName=event_time and OnlineStoreConfig.EnableOnlineStore=true (so the shared directory holds the definition this data plane resolves against).
2. PUT /FeatureGroup/customers with Record=[{FeatureName:customer_id,ValueAsString:"C1"},{FeatureName:event_time,ValueAsString:"2026-06-01T00:00:00Z"},{FeatureName:city,ValueAsString:"Seattle"}] and TtlDuration={Unit:"Weeks",Value:52}.
3. GET /FeatureGroup/customers?RecordIdentifierValueAsString=C1&ExpirationTimeResponse=Enabled — the record and its ExpiresAt come back.
4. POST /BatchWriteRecord with Entries=[{FeatureGroupName:"customers",Record:[{FeatureName:customer_id,ValueAsString:"C1"},{FeatureName:city,ValueAsString:"Tulsa"}]}] — i.e. an entry whose record omits the group's event_time feature (equivalently PUT /FeatureGroup/customers with the same Record).
5. The call answers 200 with no Errors and no UnprocessedEntries, and the repeated GetRecord now returns only customer_id/city=Tulsa: the record with event time 2026-06-01 is gone, its event time is gone, and ExpirationTimeResponse=Enabled reports no ExpiresAt, so the 52-week TTL will never fall due (state.rs:128-135 has no expires_at_secs left to act on).
6. Contrast the refusal path the crate takes for the neighbouring input: the same entry with event_time="not-a-timestamp" is refused (ValidationError / per-entry Errors) by state.rs:419-430 rather than being allowed to disable the comparison.
