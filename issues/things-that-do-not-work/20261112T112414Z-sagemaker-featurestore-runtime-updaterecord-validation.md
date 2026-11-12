Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4d88b772f7d66c5a6078a51075b00faa51fddaa1

# UpdateRecord refuses an update that specifies no TtlDuration whenever the feature group has a default TTL, though the model says an omitted EventTime keeps the record's own and applies the update

_Filed: 2026-11-12T11:24:14Z_
_Operation: UpdateRecord | Class: validation_

> _Note: this issue was identified against the code base at commit `4d88b772f7d66c5a6078a51075b00faa51fddaa1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler extends the model's one TTL/EventTime rule with a second, unmodelled one: it refuses the update with a 400 `ValidationError` whenever the FEATURE GROUP carries a default TTL (`definition.default_ttl_seconds`), even when the request specifies no `TtlDuration` at all and merely omits the event-time feature.

The model states the opposite for exactly that request: the `ValidationError` is conditioned on the REQUEST specifying a `TtlDuration` ("If you specify a TtlDuration, you must also provide an EventTime in the request. Otherwise, the operation returns a ValidationError."), while a request that omits `EventTime` is documented to SUCCEED ("If you omit EventTime, the service keeps the record's existing EventTime and applies the update."). Nothing in the model makes a group-level default TTL a reason to refuse an update.

The handler's stated justification is also contradicted by this crate's own code. The comment says the refusal is needed because "an update of a record under a default TTL re-derives that record's expiry from its event time, so this request has to state one" — but the re-derivation the handler performs (`updated.recompute_expiry()`) reads the RECORD's own event time, which the update preserves when the request omits one (`if let Some(at) = posted_event_time { updated.event_time = Some(at); }`), so the expiry is derivable without the request restating anything. On top of that, the group-level default TTL is never turned into a stored expiry anywhere in `src/`: `crate::state::prepare_record` derives `ttl_secs` from the request alone and reads `definition` only for the two feature names, so `definition.default_ttl_seconds` is read in exactly one place in the crate — this refusal — and the computation the refusal claims to protect never runs.

Consequence: every record of a feature group created with `OnlineStoreConfig.TtlDuration` is un-updatable through the documented path (post only the features you want changed); the caller is forced to restate an `EventTime` for a merge the model says applies with the record's existing one, and the posted features are silently not applied (400, record untouched).

## Evidence

Model text (`smithy_query(query="doc", shape="UpdateRecord")`, reproduced verbatim in the crate at services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/ops/update_record.rs:83-88):
```
//! record does not exist or has been soft-deleted, the operation returns a ResourceNotFound
//! error. To create a record, use PutRecord. If you provide an EventTime that is older than
//! the record's current EventTime, the service rejects the update with a ConflictException.
//! If the EventTime is equal to or newer than the current value, the service applies the
//! update. If you omit EventTime, the service keeps the record's existing EventTime and
//! applies the update. If you specify a TtlDuration, you must also provide an EventTime in
//! the request. Otherwise, the operation returns a ValidationError.
```

The handler doing the opposite — services/.../src/ops/update_record.rs:306-316:
```
        // "If you specify a TtlDuration, you must also provide an EventTime in the
        // request. Otherwise, the operation returns a ValidationError." (`ExpiresAt` is
        // `EventTime + TtlDuration`, so a TTL with no event time states no instant.) The
        // same holds for the TTL the feature group's own `OnlineStoreConfig` configures by
        // default: an update of a record under a default TTL re-derives that record's
        // expiry from its event time, so this request has to state one.
        let ttl_in_force =
            input.ttl_duration.is_some() || definition.default_ttl_seconds.is_some();
        if ttl_in_force && posted_event_time.is_none() {
            return Err(validation(EVENT_TIME_REQUIRED_FOR_TTL));
        }
```
(the message at services/.../src/ops/update_record.rs:504-506: `"Validation Error: EventTime is required when TtlDuration is specified or when the feature group has a default TTL configured. Include the EventTime feature in the Features list."`). Note the refusal runs BEFORE the record is looked up (the store lock is taken at update_record.rs:384), so it fires even for a record whose stored event time is perfectly readable.

The crate's own code contradicting the stated reason — services/.../src/ops/update_record.rs:453-465:
```
        // "If you omit EventTime, the service keeps the record's existing EventTime."
        if let Some(at) = posted_event_time {
            updated.event_time = Some(at);
        }
        ...
        if let Some(secs) = ttl_secs {
            updated.ttl_secs = Some(secs);
        }
        updated.recompute_expiry();
```
and what `recompute_expiry` derives the expiry from — services/.../src/state.rs:290-297:
```
    pub fn recompute_expiry(&mut self) {
        let at = self
            .ttl_secs
            .zip(self.event_time_secs())
            .map(|(ttl, event_time)| event_time + ttl);
```
i.e. the RECORD's own `event_time`, which the omitted-EventTime path leaves in place — so no request-supplied event time is needed to re-derive it.

The default TTL the refusal is built on is never applied to a record anywhere in the crate — services/.../src/state.rs:435-447 (`prepare_record`, the one whole-record preparation PutRecord and BatchWriteRecord share) derives the TTL from the request only:
```
    let ttl_secs = match ttl_duration {
        None => None,
```
and `definition` is read there only at state.rs:384 (`record_identifier_feature_name`) and state.rs:410 (`event_time_feature_name`); `grep -n default_ttl_seconds src/` in this crate matches exactly one line: src/ops/update_record.rs:313.

Test coverage: the behaviour is pinned only by the row "a default TTL on the feature group, no EventTime" => 400 at services/.../tests/all/refusal_message_wording.rs:641-647, whose stated reason (tests/all/refusal_message_wording.rs:22-25) is the same derivation the code above contradicts: "An update made under a TTL (the request's own `TtlDuration`, or the DEFAULT one the feature group's `OnlineStoreConfig` configures) must carry an `EventTime`, since the record's expiry is derived from it". The neighbouring rows that match the model — "a request TtlDuration in hours, no EventTime" => 400 (:620-626) and "no TTL at all, no EventTime" => 200 (:606-612) — stay unchanged by a fix; so does the ConflictException ordering suite (tests/all/update_record_partial_update.rs:417) and tests/all/update_record_partial_update.rs:366 ("an_omitted_event_time_keeps_the_records_event_time", whose group has `default_ttl_seconds: None`, tests/all/update_record_partial_update.rs:77).

## Steps to Reproduce

1. Through the SageMaker control plane, CreateFeatureGroup `customers` with RecordIdentifierFeatureName=record_id, EventTimeFeatureName=event_time, OnlineStoreConfig.EnableOnlineStore=true and OnlineStoreConfig.TtlDuration={Unit:"Hours",Value:1} — the shared directory then holds `default_ttl_seconds = Some(3600.0)` (services/common/src/feature_store_groups.rs:74-81, written by services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_feature_group.rs:317-320).
2. PUT /FeatureGroup/customers with Record=[{FeatureName:record_id,ValueAsString:"rec-1"},{FeatureName:event_time,ValueAsString:"2026-10-23T18:20:13Z"},{FeatureName:feature_a,ValueAsString:"seeded"}] — 200.
3. POST /FeatureGroup/customers/Record with {"RecordIdentifierValueAsString":"rec-1","Features":[{"FeatureName":"feature_a","ValueAsString":"applied"}]} — no TtlDuration, no event_time feature.
4. Observe 400 `ValidationError` "Validation Error: EventTime is required when TtlDuration is specified or when the feature group has a default TTL configured. Include the EventTime feature in the Features list." (update_record.rs:312-316), although the model says "If you omit EventTime, the service keeps the record's existing EventTime and applies the update" and conditions the ValidationError on the request specifying a TtlDuration.
5. GET /FeatureGroup/customers?RecordIdentifierValueAsString=rec-1 — feature_a is still "seeded": the documented merge was dropped.
6. Repeat steps 1-5 with the group created WITHOUT OnlineStoreConfig.TtlDuration: the identical request answers 200 and the merge is applied (pinned row "no TTL at all, no EventTime" at tests/all/refusal_message_wording.rs:606-612), so the only thing deciding the refusal is a group-level default this crate never turns into a stored expiry (state.rs:435-447).
