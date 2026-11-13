Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c3d14e0783a295ea61944c8e0b9e5efd8a41c9df

# A record whose EventTime + TtlDuration cannot be formatted keeps its enforced expiry but loses the ExpiresAt it reports, though the crate claims that case is unreachable

_Filed: 2026-11-13T09:21:36Z_
_Operation: UpdateRecord | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c3d14e0783a295ea61944c8e0b9e5efd8a41c9df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StoredRecord::recompute_expiry` — the one derivation of `ExpiresAt = EventTime + TtlDuration` in this crate, called directly by `UpdateRecord`'s handler (update_record.rs:465) and by `crate::state::prepare_record` for the two whole-record writes — stores the derived instant in TWO members and silently throws one of them away when the instant cannot be formatted: `self.expires_at = at.and_then(|at| iso8601(at).ok());` keeps `None` on a formatting error while `self.expires_at_secs = at;` keeps the instant. The record is therefore stored with an expiry the store compares against (`purge_expired`) but with no `ExpiresAt` string any read can report, so `GetRecord`/`BatchGetRecord` with `ExpirationTimeResponse=Enabled` answer no `ExpiresAt` for a record the caller wrote WITH a `TtlDuration`.

The crate asserts this cannot happen — `iso8601`'s doc says the error case is one "which no event time plus a modelled `TtlDuration` reaches" — but this same crate accepts an event time of ANY finite f64 seconds (`parse_event_time_secs`) and its offline-store half documents and TESTS exactly the opposite conclusion for such a value ("a magnitude beyond what a duration or an instant holds yields no instant at all", with the pinned case `"1e300"` => no instant). An event time like `1e300` (or an epoch value mistakenly passed in milliseconds) therefore reaches the dropped-error path, and the two members of one record disagree.

The same handler's own documentation states the rule the code then breaks: "A `TtlDuration` replaces the record's TTL duration, from which `ExpiresAt = EventTime + TtlDuration` is re-derived", and `StoredRecord::expires_at`'s doc allows the member to be absent in only two cases, neither of which is this one. The fix belongs in `crate::state` (the shared derivation), which is why this is filed against the handler that calls `recompute_expiry` directly.

## Evidence

The derivation that drops the error — services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/state.rs:290-297:
```
    pub fn recompute_expiry(&mut self) {
        let at = self
            .ttl_secs
            .zip(self.event_time_secs())
            .map(|(ttl, event_time)| event_time + ttl);
        self.expires_at = at.and_then(|at| iso8601(at).ok());
        self.expires_at_secs = at;
    }
```

The claim it relies on — src/state.rs:559-565:
```
/// `secs` as the ISO-8601 string an `ExpiresAt` carries.
///
/// `Err` only for an instant [`DateTime`] cannot format (one outside the range a
/// timestamp can state), which no event time plus a modelled `TtlDuration` reaches.
pub fn iso8601(secs: f64) -> Result<String, aws_smithy_types::date_time::DateTimeFormatError> {
    DateTime::from_secs_f64(secs).fmt(Format::DateTime)
}
```

The same crate accepting an event time far outside that range — src/state.rs:548-557:
```
pub fn parse_event_time_secs(event_time: &str) -> Option<f64> {
    if let Ok(parsed) = DateTime::from_str(event_time, Format::DateTime) {
        return Some(parsed.as_secs_f64());
    }
    event_time
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|secs| secs.is_finite())
}
```

and the crate's own offline-store half stating (and pinning) that such a value states no instant — src/state/offline_store.rs:152-156:
```
/// An event-time feature may carry ANY number of seconds since the epoch — the value is a
/// `Fractional` or an ISO-8601 string from the request — so a magnitude beyond what a
/// duration or an instant holds yields no instant at all rather than failing the ingestion
```
with the pinned case src/state/offline_store.rs:567-573:
```
            (
                "an event time no instant can state",
                vec![
                    feature("customer_id", "C1"),
                    feature("event_time", "1e300"),
                ],
                None,
            ),
```

The rules the dropped string breaks — src/state.rs:260-262:
```
    /// `EventTime + TtlDuration`, as an ISO-8601 string; absent when the record was
    /// written with no `TtlDuration` or with no parsable event time.
    pub expires_at: Option<String>,
```
and src/ops/update_record.rs:17-19:
```
//! `ConflictException` (409) leaving the record untouched, while an omitted `EventTime`
//! keeps the stored one. A `TtlDuration` replaces the record's TTL duration, from which
//! `ExpiresAt = EventTime + TtlDuration` is re-derived;
```
the call site in the handler — src/ops/update_record.rs:462-465:
```
        if let Some(secs) = ttl_secs {
            updated.ttl_secs = Some(secs);
        }
        updated.recompute_expiry();
```
and the read that then reports nothing — src/ops/get_record.rs:244-246:
```
        let expires_at = expiration_enabled
            .then(|| record.expires_at.clone())
            .flatten();
```

Test coverage: the TTL/`ExpiresAt` suites only use event times inside the representable range (tests/all/update_record_partial_update.rs:595-631 and tests/all/online_store_record_lifecycle.rs:470-511 walk every `TtlDurationUnit` with ordinary ISO-8601 event times), so no existing test pins the unformattable case — the only place the crate pins such a value is the offline-store unit test quoted above, which concludes the opposite of state.rs:562.

## Steps to Reproduce

1. Through the SageMaker control plane, CreateFeatureGroup `customers` with RecordIdentifierFeatureName=customer_id, EventTimeFeatureName=event_time, OnlineStoreConfig.EnableOnlineStore=true (no default TtlDuration).
2. PUT /FeatureGroup/customers with Record=[{FeatureName:customer_id,ValueAsString:"C1"},{FeatureName:event_time,ValueAsString:"1e300"},{FeatureName:city,ValueAsString:"Seattle"}] — accepted (parse_event_time_secs, state.rs:548-557, accepts any finite f64).
3. POST /FeatureGroup/customers/Record with {"RecordIdentifierValueAsString":"C1","Features":[{"FeatureName":"event_time","ValueAsString":"1e300"},{"FeatureName":"city","ValueAsString":"Tacoma"}],"TtlDuration":{"Unit":"Hours","Value":1}} — 200; `updated.ttl_secs` is set and `recompute_expiry()` runs (update_record.rs:462-465).
4. GET /FeatureGroup/customers?RecordIdentifierValueAsString=C1&ExpirationTimeResponse=Enabled — the response carries NO `ExpiresAt`, although the update specified a `TtlDuration` and the handler documents `ExpiresAt = EventTime + TtlDuration` as re-derived.
5. Observe that the record nevertheless holds `expires_at_secs = Some(1e300)` (state.rs:296), i.e. the instant the store enforces exists while the string the API reports was dropped by `iso8601(at).ok()` (state.rs:295) — the error path state.rs:562 claims "no event time plus a modelled TtlDuration reaches".
6. Repeat step 2-4 with PutRecord carrying the same event time and `TtlDuration` directly (the same derivation via crate::state::prepare_record, state.rs:458) for the identical result.
