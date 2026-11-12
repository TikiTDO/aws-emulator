Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7011ad447146056dadde7fecb62590edf9d79855

# PutRecord answers a ValidationError whose Message exceeds the modelled 2048-character maximum when it quotes the request's event-time value, while BatchWriteRecord cuts the same text to that bound

_Filed: 2026-11-12T01:40:29Z_
_Operation: PutRecord | Class: error-response_

> _Note: this issue was identified against the code base at commit `7011ad447146056dadde7fecb62590edf9d79855`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The shared record preparation of this data plane refuses an unreadable event time by quoting the posted value verbatim into the refusal message: `"EventTime: the event time feature '{event_time_feature_name}' must carry an ISO-8601 timestamp or seconds since the epoch, not '{at}'"`. `at` is a `FeatureValue.ValueAsString`, which the model bounds at `len 0..358400`, so a 5 000-character junk value passes the boundary validator and reaches the handler. PutRecord drops that whole string straight into the modelled `ValidationError.Message` (`validation(message)`), whose target shape `Message` the model bounds at `length 0..2048` — so the 400 response carries a member two orders of magnitude outside its own modelled maximum.

The crate already states and solves exactly this rule, one module over: BatchWriteRecord cuts the SAME text from the SAME helper to 2048 characters before reporting it (`bounded_message`, whose doc says "A refusal may quote a value the request supplied, and a `ValueAsString` is modelled `len 0..358400`, so the message is cut at the last character boundary within the bound rather than answered outside it"), against the same `Message` bound (`MAX_ERROR_MESSAGE = 2048`). PutRecord does not call it, so one and the same refusal of one and the same record is answered inside the bound through the batch API and outside it through the single-record API.

Two sibling handlers quote a request-supplied `ValueAsString` into the same `Message` shape without the bound either, and the fix must cover them: DeleteRecord's own `EventTime` refusal (`input.event_time`, `len 0..358400`) and UpdateRecord's `EventTime`/record-identifier refusals (`at` and `identifier`, both `len 0..358400`).

## Evidence

Model bound on the member the handler fills — `smithy_query(query="shape", shape="ValidationError,Message")`:
`"ValidationError": {"errorKind": "client", "httpError": 400, "members": {"Message": {"target": "Message", "length": {"min": 0, "max": 2048}}}}` and `"Message": {"type": "string", "length": {"min": 0, "max": 2048}}`.

The unbounded message — services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/state.rs:419-427 (`prepare_record`):
```
        Some(at) => match parse_event_time_secs(at) {
            Some(secs) => Some(secs),
            None => {
                return Err(format!(
                    "EventTime: the event time feature '{event_time_feature_name}' must carry \
                     an ISO-8601 timestamp or seconds since the epoch, not '{at}'"
                ))
            }
```
and PutRecord putting it into the modelled error unchanged — services/.../src/ops/put_record.rs:244-246:
```
        let prepared =
            crate::state::prepare_record(&definition, &input.record, input.ttl_duration.as_ref())
                .map_err(validation)?;
```
with services/.../src/ops/put_record.rs:309-315:
```
/// The modelled `ValidationError` (HTTP 400) for a request this operation refuses.
fn validation(message: impl Into<String>) -> PutRecordError {
    PutRecordErrorModeled::ValidationError(ValidationError {
        message: Some(message.into()),
    })
    .into()
}
```

The other side of the inconsistency — the same crate, same text, cut to the bound: services/.../src/ops/batch_write_record.rs:384-398:
```
/// `message`, cut to the modelled maximum length of an `ErrorMessage`.
///
/// A refusal may quote a value the request supplied, and a `ValueAsString` is modelled
/// `len 0..358400`, so the message is cut at the last character boundary within the bound
/// rather than answered outside it.
fn bounded_message(mut message: String) -> String {
    if message.len() > MAX_ERROR_MESSAGE {
```
services/.../src/ops/batch_write_record.rs:400-403:
```
/// The modelled maximum length of a `BatchWriteRecordError`'s `ErrorMessage`: its target
/// `Message` is `len 0..2048`
/// (`smithy_query(query="shape", shape="BatchWriteRecordError")`).
const MAX_ERROR_MESSAGE: usize = 2048;
```
applied to the identical `prepare_record` failure at services/.../src/ops/batch_write_record.rs:284-295 + :372-382 (`error_message: bounded_message(message)`).

The value that reaches the handler is request-supplied and modelled large — OPERATIONS.md:65 (`Record` / `FeatureValue`) and `smithy_query(query="shape", shape="FeatureValue")`: `"ValueAsString": {"length": {"min": 0, "max": 358400}, "pattern": ".*"}`.

Sibling sites with the same defect: services/.../src/ops/delete_record.rs:273-279 (`"EventTime: expected an ISO-8601 timestamp or seconds since the epoch, not '{}'", input.event_time` — the member is `@httpQuery` `ValueAsString`, `len 0..358400`, per OPERATIONS.md:43) and services/.../src/ops/update_record.rs:339-343 and :361-366 (quoting `at` and `identifier`).

Test coverage: none pins any refusal message length. tests/all/size_bound_statements.rs:275-276 only builds oversized NextTokens (`"t".repeat(8192)` / `(8193)`), and no test in tests/ asserts the text of the "must carry an ISO-8601 timestamp" refusal, so bounding it breaks nothing.

## Steps to Reproduce

1. Through the SageMaker control plane, CreateFeatureGroup `customers` with RecordIdentifierFeatureName=customer_id, EventTimeFeatureName=event_time and OnlineStoreConfig.EnableOnlineStore=true (so the shared directory holds the definition this data plane resolves against).
2. PUT /FeatureGroup/customers with Record=[{FeatureName:customer_id,ValueAsString:"C1"},{FeatureName:event_time,ValueAsString:"<5000 'x' characters>"}] — the value is inside the modelled `ValueAsString` bound (0..358400), so the boundary validator accepts it and the handler runs.
3. Observe the 400 response: its `Message` is the whole ~5080-character refusal (state.rs:423-426 via put_record.rs:244-246/309-315), i.e. the modelled `ValidationError.Message` maximum of 2048 is exceeded by ~2.5x.
4. POST /BatchWriteRecord with Entries=[{FeatureGroupName:"customers",Record:<the same Record>}] — the per-entry `ErrorMessage` for the identical failure is cut to 2048 characters by batch_write_record.rs:389-398, so the two APIs answer the same refusal in and out of the modelled bound.
5. Repeat with DELETE /FeatureGroup/customers?RecordIdentifierValueAsString=C1&EventTime=<5000 'x' characters> (delete_record.rs:273-279) and POST /FeatureGroup/customers/Record with event_time=<5000 'x' characters> (update_record.rs:339-343) — both also answer a Message past 2048.
