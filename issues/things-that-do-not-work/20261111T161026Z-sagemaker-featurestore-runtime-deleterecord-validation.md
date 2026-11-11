Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f0a035afe3a6d4bc3a732ff8fc28ae95e972cd65

# DeleteRecord skips its EventTime timestamp check when TargetStores omits the OnlineStore (or the group has no OnlineStore), while PutRecord and UpdateRecord validate the request first

_Filed: 2026-11-11T16:10:26Z_
_Operation: DeleteRecord | Class: validation_

> _Note: this issue was identified against the code base at commit `f0a035afe3a6d4bc3a732ff8fc28ae95e972cd65`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteRecord returns the `TargetStores`/no-online-store shortcut BEFORE it parses the request's `EventTime`, so a request whose required `EventTime` is not a timestamp at all is answered 200 whenever `TargetStores` does not name the OnlineStore, or whenever the resolved feature group has no online store. The same unreadable `EventTime` is refused with the modelled `ValidationError` (400) when the request targets the OnlineStore, so one and the same malformed request gets two different verdicts depending on a member that only selects WHICH store is written.

The crate states the opposite rule twice, in the two sibling writers: every check that is on the REQUEST is made regardless of which stores the caller named, and only the store WRITE is skipped. PutRecord runs `prepare_record` (which refuses an unparsable event time) before its `TargetStores` branch; UpdateRecord parses the posted `EventTime` at the top and answers its offline-only shortcut only after, with a comment saying so explicitly. DeleteRecord is the one writer that lets an OfflineStore-only target excuse a request defect, and it does so for a member the model marks `@required`.

## Evidence

services/.../src/ops/delete_record.rs:260-264 — the shortcut, taken before any EventTime reading:
```
        if !crate::state::targets_online_store(input.target_stores.as_ref())
            || !definition.online_store_enabled
        {
            return Ok(DeleteRecordOutput);
        }
```
services/.../src/ops/delete_record.rs:271-279 — the check that is therefore skipped:
```
        // The deletion is ordered against the stored record's event time, so the
        // request's own `EventTime` must be a timestamp.
        let Some(requested_secs) = crate::state::parse_event_time_secs(&input.event_time) else {
            return Err(validation(format!(
                "EventTime: expected an ISO-8601 timestamp or seconds since the epoch, \
                 not '{}'",
```
The sibling rule, stated in this same crate:
services/.../src/ops/put_record.rs:239-246 — "Every check it makes is on the REQUEST, so it answers the same whatever stores the caller named and whichever stores the feature group has: a record this operation cannot read is a request defect, not something an OfflineStore-only write excuses." followed by `let prepared = crate::state::prepare_record(...)` — which is the call that refuses an unparsable event time (services/.../src/state.rs:419-427: "EventTime: the event time feature '{event_time_feature_name}' must carry an ISO-8601 timestamp or seconds since the epoch, not '{at}'").
services/.../src/ops/update_record.rs:264-268 — "Every check from here to the store lock is on the REQUEST, so it answers the same whatever stores the caller named: a request this operation cannot act on is a request defect, not something an OfflineStore-only target excuses."
services/.../src/ops/update_record.rs:416-424 — "It is answered HERE, and not earlier: the request's own rules, the record's existence and the ordering against its event time hold whichever store an update is aimed at, so an offline-only request is validated and resolved exactly as any other and only the WRITE is skipped." followed by `if !crate::state::targets_online_store(...) { return Ok(UpdateRecordOutput); }`.
Model: `EventTime` on DeleteRecord is `required: true` (smithy_query operation=DeleteRecord: "EventTime": {"target": "ValueAsString", "httpQuery": "EventTime", "required": true}).
Test coverage: the DeleteRecord event-time ordering cases in tests/all/rejection_statements.rs:236-320 all name no TargetStores (so they take the online path), and no test under tests/all/ pairs a DeleteRecord TargetStores=OfflineStore with a malformed EventTime (grep for `TargetStores|OfflineStore` across tests/all shows DeleteRecord TargetStores only in size_bound_statements.rs:442-447 and online_store_record_lifecycle.rs:1288-1289, both with well-formed event times), so nothing pins the current answer.

## Steps to Reproduce

1. Create a feature group through the SageMaker control plane with both an OnlineStore and an OfflineStore (as tests/all/offline_store_replication.rs does).
2. Call `DELETE /FeatureGroup/<name>?RecordIdentifierValueAsString=C1&EventTime=banana&TargetStores=OnlineStore` — the handler reaches delete_record.rs:273 and answers 400 ValidationError "EventTime: expected an ISO-8601 timestamp or seconds since the epoch, not 'banana'".
3. Call the same request with `&TargetStores=OfflineStore` instead — the handler returns at delete_record.rs:263 and answers 200, accepting the same unreadable required member.
4. Compare with `POST /FeatureGroup/<name>/Record` (UpdateRecord) carrying `TargetStores=["OfflineStore"]` and an event-time feature value of `banana`: update_record.rs:334-345 refuses it 400, before the offline-only shortcut at line 422.
