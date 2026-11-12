Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7011ad447146056dadde7fecb62590edf9d79855

# GetRecord withholds ExpiresAt for a record that exists when FeatureNames names none of its stored features

_Filed: 2026-11-12T01:40:29Z_
_Operation: GetRecord | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7011ad447146056dadde7fecb62590edf9d79855`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When the caller passes `FeatureNames` and the projection matches no feature the stored record carries, the handler returns `GetRecordOutput::default()` — which clears BOTH output members — so a record that exists, is live, and holds a non-null `ExpiresAt` is answered with no `ExpiresAt` even though the caller asked `ExpirationTimeResponse=Enabled`.

The model documents that member as: "Parameter to request ExpiresAt in response. If Enabled, GetRecord will return the value of ExpiresAt, if it is not null." The record's `ExpiresAt` is not null here, so the response contradicts the member's own documentation.

The handler's stated reason for the empty result covers only the `Record` member — "`Record` is modelled `len 1..`, so 'no feature matched' is the same empty result an absent identifier gives, never an empty list" — and `GetRecordResponse` models `ExpiresAt` as an INDEPENDENT optional member (`{"ExpiresAt": {"target": "ExpiresAt"}}`), with no `@required` and no coupling to `Record`. Nothing in the model or in the crate forces the expiry out of the response along with the omitted record, and the handler itself computes it one branch later for every other path.

## Evidence

Model, `GetRecordRequest$ExpirationTimeResponse` (`smithy_query(query="doc", shape="GetRecordRequest", member="ExpirationTimeResponse")`): "Parameter to request <code>ExpiresAt</code> in response. If <code>Enabled</code>, <code>GetRecord</code> will return the value of <code>ExpiresAt</code>, if it is not null. If <code>Disabled</code> and null, <code>GetRecord</code> will return null." — reproduced in the crate at services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/ops/get_record.rs:63-65.

The two output members are independent — `smithy_query(query="shape", shape="GetRecordResponse")`: `{"Record": {"target": "Record", "length": {"min": 1}, "type": "list"}, "ExpiresAt": {"target": "ExpiresAt"}}` (neither required).

The code that drops it — services/.../src/ops/get_record.rs:235-246:
```
        let projected = record.project(input.feature_names.as_deref());
        if projected.is_empty() {
            // `Record` is modelled `len 1..`, so "no feature matched" is the same
            // empty result an absent identifier gives, never an empty list.
            return Ok(GetRecordOutput::default());
        }

        // "Parameter to request ExpiresAt in response. If Enabled, GetRecord will
        // return the value of ExpiresAt, if it is not null."
        let expires_at = expiration_enabled
            .then(|| record.expires_at.clone())
            .flatten();
```
The stated reason on line 237-238 is about `Record` alone; `expires_at` is computed only on the line AFTER the early return, so the projection decides the fate of a member it says nothing about. The record on that path is a live, unexpired one (`group.live_record(&identifier)`, get_record.rs:231) whose `expires_at` is the stored ISO string (src/state.rs:260-265).

Reachable with modelled input: `FeatureNames` is `@httpQuery("FeatureName")`, `len 1..`, each name `len 1..64` pattern `^[a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63}$` — and a name the record does not carry is deliberately not an error (state.rs:299-308).

Test coverage: the projection-empty case is pinned only for the feature list, not for `ExpiresAt` — tests/all/online_store_record_lifecycle.rs:444: `("a feature the record has not", "&FeatureName=nosuch", vec![]),`, asserted at :455-462 over `record_pairs(&body)` only, with no `ExpirationTimeResponse` in the query (:449-452). The TTL test that does assert `ExpiresAt` (tests/all/online_store_record_lifecycle.rs:467-521) never passes `FeatureName`, so returning `ExpiresAt` on the projection-empty path changes no existing assertion.

## Steps to Reproduce

1. Through the SageMaker control plane, CreateFeatureGroup `customers` with RecordIdentifierFeatureName=customer_id, EventTimeFeatureName=event_time and OnlineStoreConfig.EnableOnlineStore=true.
2. PUT /FeatureGroup/customers with Record=[{FeatureName:customer_id,ValueAsString:"C1"},{FeatureName:event_time,ValueAsString:"<a day in the future, ISO-8601>"},{FeatureName:city,ValueAsString:"Seattle"}] and TtlDuration={Unit:"Weeks",Value:1} — the record is stored with a non-null ExpiresAt (= EventTime + 1 week).
3. GET /FeatureGroup/customers?RecordIdentifierValueAsString=C1&ExpirationTimeResponse=Enabled — 200 with Record and ExpiresAt, confirming the stored expiry is not null.
4. GET /FeatureGroup/customers?RecordIdentifierValueAsString=C1&ExpirationTimeResponse=Enabled&FeatureName=nosuch — 200 with an EMPTY body: no Record (the crate's stated rule) and also no ExpiresAt, although the record exists and its ExpiresAt is not null, which is what the ExpirationTimeResponse=Enabled sentence says will be returned.
