Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 140723b65076e66e2cb90e34db509407cbbeb018

# PutRecord accepts an event-time feature posted as ValueAsStringList and silently treats the record as having no event time, while the same function refuses the identifier feature and UpdateRecord refuses the event-time feature

_Filed: 2026-11-13T08:53:43Z_
_Operation: PutRecord | Class: validation_

> _Note: this issue was identified against the code base at commit `140723b65076e66e2cb90e34db509407cbbeb018`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The shared record preparation of this data plane (`crate::state::prepare_record`, called by PutRecord and BatchWriteRecord) reads the group's event-time feature with `.and_then(|feature| feature.value_as_string.clone())`. A posted record that DOES carry the group's `EventTimeFeatureName` but carries its value in `ValueAsStringList` (or in neither value member) therefore yields `event_time = None` and is accepted as a record with no event time at all: the TTL is dropped (`recompute_expiry` sets both `expires_at` and `expires_at_secs` to `None` when there is no parsable event time) and the historic-record comparison in `FeatureGroupStore::ingest` is bypassed, so the write over-writes a stored record whose `EventTime` is newer.

The crate refuses exactly this input shape in two other places. (1) The SAME function refuses the group's record-identifier feature when it carries a list instead of `ValueAsString`, with a dedicated message. (2) UpdateRecord refuses the group's EVENT-TIME feature when it carries a list, with the message `"EventTime: the event time feature '{event_time_feature_name}' must carry ValueAsString, not ValueAsStringList"`. And the comment two lines below the silent read states the rule that is broken: an event time the group identified "must be a timestamp" and a value this plane cannot read is "refused rather than silently disabling both".

This is a different input from the already-filed BatchWriteRecord wrong-behavior issue (20261111T194259Z, a record that OMITS the event-time feature entirely): here the feature is present and the group's own event-time column is being set, in the one form this crate elsewhere declares unreadable, and the fix is the refusal UpdateRecord already performs.

## Evidence

The silent read — services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/state.rs:408-414 (`prepare_record`):
```
        // A record that carries no value for the group's event-time feature has no event
        // time, so neither the historic-record rule nor a TTL has anything to compute from.
        let event_time_feature_name = &definition.event_time_feature_name;
        let event_time = record
            .iter()
            .find(|feature| feature.feature_name == *event_time_feature_name)
            .and_then(|feature| feature.value_as_string.clone());
```
The rule the same function states immediately after — src/state.rs:416-418:
```
    // An event time the group identified must be a timestamp: the TTL and the
    // historic-record rule are both computed from it, so a value in neither accepted
    // format is refused rather than silently disabling both.
```
The refusal the same function DOES make for the neighbouring member — src/state.rs:393-398:
```
        Some(_) => {
            return Err(format!(
                "the record identifier feature '{identifier_feature_name}' must carry \
                 ValueAsString, not ValueAsStringList"
            ))
        }
```
The sibling handler that refuses the EVENT-TIME feature for the same reason — src/ops/update_record.rs:288-304:
```
        let posted_event_time = match input
            .features
            .iter()
            .find(|feature| feature.feature_name == event_time_feature_name)
        {
            None => None,
            Some(FeatureValue { value_as_string: Some(value), .. }) => Some(value.clone()),
            Some(_) => {
                return Err(validation(format!(
                    "EventTime: the event time feature '{event_time_feature_name}' must carry \
                     ValueAsString, not ValueAsStringList"
                )))
            }
```
What the acceptance costs — src/state.rs:449-458 stores `event_time` (None) and then `stored.recompute_expiry()`, which per src/state.rs:290-297 sets BOTH `expires_at` and `expires_at_secs` to `None`; and src/state.rs:180-195 (`ingest`) compares event times only `if let (Some(posted), Some(stored))`, so the posted record is inserted over a newer stored one. PutRecord reaches both at src/ops/put_record.rs:244-246 and :274-282.

The input is modelled — `smithy_query(query="shape", shape="FeatureValue")`: `"ValueAsString": {"length": {"min": 0, "max": 358400}, "pattern": ".*"}`, `"ValueAsStringList": {"target": "ValueAsStringList", "length": {"min": 0, "max": 358400}, "type": "list"}` — both optional, so a `FeatureValue` carrying only `ValueAsStringList` passes the boundary validator.

Test coverage: the list-valued form is pinned only for the IDENTIFIER feature — tests/all/batch_write_record_bulk_ingestion.rs:484-493: `"an entry whose record-identifier feature carries a list"` with `{ "FeatureName": IDENTIFIER_FEATURE, "ValueAsStringList": ["X1"] }` expecting `"ValidationError"` / a message containing `"ValueAsString"`. `grep -n 'ValueAsStringList' tests/all` otherwise matches only non-identifier, non-event-time features (online_store_record_lifecycle.rs:227 `tags`, update_record_partial_update.rs:220/274, update_record_semantics.rs:245), so no test posts the event-time feature as a list and refusing it breaks nothing.

## Steps to Reproduce

1. Through the SageMaker control plane, CreateFeatureGroup `customers` with RecordIdentifierFeatureName=customer_id, EventTimeFeatureName=event_time and OnlineStoreConfig.EnableOnlineStore=true (so the shared directory holds the definition this data plane resolves against).
2. PUT /FeatureGroup/customers with Record=[{FeatureName:customer_id,ValueAsString:"C1"},{FeatureName:event_time,ValueAsString:"2026-06-01T00:00:00Z"},{FeatureName:city,ValueAsString:"Seattle"}] and TtlDuration={Unit:"Weeks",Value:52}; GET /FeatureGroup/customers?RecordIdentifierValueAsString=C1&ExpirationTimeResponse=Enabled returns the record and its ExpiresAt.
3. PUT /FeatureGroup/customers with Record=[{FeatureName:customer_id,ValueAsString:"C1"},{FeatureName:event_time,ValueAsStringList:["2020-01-01T00:00:00Z"]},{FeatureName:city,ValueAsString:"Tulsa"}] and TtlDuration={Unit:"Weeks",Value:52} — the event-time feature IS posted, in the list member.
4. Observe 200: state.rs:411-414 reads no event time, so the historic-record comparison at state.rs:181-189 is skipped and the 2026-06-01 record is replaced; the repeated GetRecord with ExpirationTimeResponse=Enabled now returns city=Tulsa with NO ExpiresAt, so the 52-week TTL never falls due.
5. POST /FeatureGroup/customers/Record (UpdateRecord) with Features=[{FeatureName:event_time,ValueAsStringList:["2020-01-01T00:00:00Z"]}] — update_record.rs:298-304 refuses the identical value shape with 400 ValidationError, and the same PUT body with the identifier feature in the list member is refused by state.rs:393-398: the three readings of one rule disagree.
