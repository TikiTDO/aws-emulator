// SPDX-License-Identifier: CC0-1.0

//! The RUM event document: the ONE form a delivered `RumEvent` is held in, reported as and
//! copied as.
//!
//! `PutRumEvents` accepts the CLIENT's structure — a `RumEvent` plus the batch's
//! `BatchId`, `AppMonitorDetails` and `UserDetails` — while what a reader gets back is the
//! SERVICE's own document: that event ENRICHED, at ingestion, with the batch's context and
//! with what only the service knows (the app monitor the batch was delivered to, the
//! account that owns it, the log stream its copies accumulate in), under the field names
//! RUM publishes for a RUM event.
//!
//! Those names are the model's own. `MetricDefinitionRequest` enumerates the first-level
//! keys a custom metric's `ValueKey`, `EventPattern` and `DimensionKeys` may name in a RUM
//! event ([`super::RUM_EVENT_FIELDS`]) — the account, the application, the batch, the
//! event's own id, instant, type, version, interaction and details, the log stream, the
//! metadata and the user session — and an `EventPattern` is matched against the very
//! document a read reports, so the client's `{id, timestamp, type, metadata, details}` is
//! NOT the form anything downstream sees. The enrichment therefore happens ONCE, here, and
//! the monitor's buffer, the query that reads it back
//! ([`crate::ops::get_app_monitor_data`]) and the copy stored in CloudWatch Logs
//! ([`super::cw_log_delivery`]) all carry the same bytes. The one field whose spelling the
//! wire and that list disagree on is the application's id: the document carries it as
//! `application_id`, which is what the recorded wire document spells, while the list keeps
//! the `application_Id` the documentation of those metric members spells — one is the
//! event's field name, the other is what a metric definition is judged against.
//!
//! Recorded behaviour pins three things the model leaves open:
//!
//! * `event_timestamp` is an epoch instant in MILLISECONDS, which is the unit
//!   `TimeRange`'s bounds are expressed in as well, so a query and an event are placed on
//!   ONE scale ([`event_timestamp_millis`], [`event_instant_millis`]);
//! * `metadata` is reported as the document the client serialized it as, with the
//!   `metadata_keys` / `metadata_values` projections of it beside it;
//! * the document's `userId` and `sessionId` are reported BOTH at the top level and inside
//!   `user_details`, which is why the model's field list carries all three.

use ::serde_json::{Map, Value};

use crate::ops::types::RumEvent;

/// The field a RUM event document carries the instant the event occurred under, as an
/// epoch instant in milliseconds — one spelling for the data-plane write, the query that
/// selects on it and the copy stored in CloudWatch Logs.
pub const EVENT_TIMESTAMP_FIELD: &str = "event_timestamp";

/// The field carrying the client's own metadata document, which is where a RUM web client
/// reports the identity of the user session (`RumEvent$metadata`: "a JSON serialization of
/// the identity of the user for this session").
pub const METADATA_FIELD: &str = "metadata";

/// The field carrying the first-level KEYS of [`METADATA_FIELD`], in the order the
/// document holds them.
pub const METADATA_KEYS_FIELD: &str = "metadata_keys";

/// The field carrying the first-level VALUES of [`METADATA_FIELD`], positionally aligned
/// with [`METADATA_KEYS_FIELD`].
pub const METADATA_VALUES_FIELD: &str = "metadata_values";

/// How many milliseconds one second is, for the instant codec below.
const MILLIS_PER_SECOND: i64 = 1_000;

/// How many nanoseconds one millisecond is: what the subsecond part of an
/// [`::aws_smithy_types::DateTime`] has to be divided by to reach the same unit.
const NANOS_PER_MILLI: u32 = 1_000_000;

/// The member a client's metadata (or event details) reports the schema version of the
/// event under, which the document promotes to its own `event_version`: the RUM web client
/// stamps every event's payload with the version of the schema it serialized it under.
const CLIENT_VERSION_MEMBER: &str = "version";

/// The member a client's metadata (or event details) reports the interaction the event
/// belongs to under, which the document promotes to its own `event_interaction`.
const CLIENT_INTERACTION_MEMBER: &str = "interaction";

/// The `event_interaction` of an event whose client reported no interaction at all: the
/// field is part of every RUM event document, and an event that follows no interaction of
/// the session is at the session's zeroth.
const NO_INTERACTION: i64 = 0;

/// `RumEvent$timestamp` as a RUM event document carries it: the instant in MILLISECONDS
/// since the epoch, which is the unit `TimeRange$After`/`$Before` bound an event at.
///
/// An instant whose millisecond count no `i64` can hold saturates at that type's bound
/// rather than wrapping onto an instant on the other side of the epoch, so the ordering of
/// two delivered instants is never inverted by the codec.
pub fn event_timestamp_millis(instant: &::aws_smithy_types::DateTime) -> i64 {
    let within_the_second = i64::from(instant.subsec_nanos() / NANOS_PER_MILLI);
    instant
        .secs()
        .saturating_mul(MILLIS_PER_SECOND)
        .saturating_add(within_the_second)
}

/// The instant a RUM event `document` carries, in milliseconds since the epoch, or `None`
/// for a document whose [`EVENT_TIMESTAMP_FIELD`] is missing or is no instant at all.
///
/// The one reading of what [`event_timestamp_millis`] writes, so a query cannot place an
/// event at an instant other than the one it was delivered at. A fractional count of
/// milliseconds reads as the whole millisecond it falls in — the second BELOW the value,
/// for an instant before the epoch as well as after it — because that is the instant such a
/// document names.
pub fn event_instant_millis(document: &Value) -> Option<i64> {
    let carried = document.get(EVENT_TIMESTAMP_FIELD)?;
    carried
        .as_i64()
        .or_else(|| carried.as_f64().map(whole_millis))
}

/// A fractional count of milliseconds since the epoch as the whole millisecond it falls
/// in. A float-to-integer cast saturates at the integer's own bounds, so a value no `i64`
/// can hold lands on the nearest instant one can.
fn whole_millis(millis: f64) -> i64 {
    millis.floor() as i64
}

/// What ONE `PutRumEvents` call delivered, beside the events themselves: the context every
/// event of that batch is enriched with.
///
/// The app monitor is named from the STORED record the batch's `Id` resolved to — the
/// monitor that actually accepted the batch — rather than from the client's
/// `AppMonitorDetails`, which is the sending web client's own view and names whatever it
/// was configured with. The one member of that structure no server-side value can stand
/// for is the application's own version, which only the client knows.
#[derive(Debug, Clone, Copy)]
pub struct Delivery<'a> {
    /// The `Id` of the app monitor the batch was delivered to, as the stored record holds
    /// it: the document's `application_id`.
    pub app_monitor_id: &'a str,
    /// The `Name` of that same monitor: the document's `application_name`.
    pub app_monitor_name: &'a str,
    /// `AppMonitorDetails$version`, when the client reported one: the document's
    /// `application_version`.
    pub app_monitor_version: Option<&'a str>,
    /// `PutRumEventsRequest$BatchId` of the call that delivered the event.
    pub batch_id: &'a str,
    /// `UserDetails$userId` of the session the batch was collected from.
    pub user_id: Option<&'a str>,
    /// `UserDetails$sessionId` of that session.
    pub session_id: Option<&'a str>,
    /// The CloudWatch Logs stream this batch's copies accumulate in
    /// ([`super::cw_log_delivery::delivery_stream_name`]): the document's `log_stream`.
    pub log_stream: &'a str,
}

/// One delivered `RumEvent` as the RUM event document the service holds, reports and
/// copies it as.
///
/// Every field the service can fill from the delivery and its own state is always carried;
/// a field that can only come from an OPTIONAL member of the batch
/// (`application_version`, `userId`, `sessionId`) is carried when the batch carried it,
/// since nothing is reported that the client did not send. `metadata` and `details` are
/// `@mediaType("application/json")` strings, so each is re-parsed into the document rather
/// than nested as escaped text; a value that is not valid JSON is kept verbatim as a
/// string, since the model constrains neither.
pub fn event_document(delivery: &Delivery<'_>, event: &RumEvent) -> String {
    let metadata = event
        .metadata
        .as_deref()
        .map(json_member)
        .unwrap_or_else(|| Value::Object(Map::new()));
    let details = json_member(&event.details);

    // The fields go in in the order RUM reports them in, which is the order a recorded
    // document carries them in: the session, then the application, then the event, then
    // the documents the client sent, then the account and the interaction.
    let mut document = Map::new();
    let mut user = Map::new();
    for (field, carried) in [("userId", delivery.user_id), ("sessionId", delivery.session_id)] {
        if let Some(carried) = carried {
            document.insert(field.to_string(), Value::String(carried.to_string()));
            user.insert(field.to_string(), Value::String(carried.to_string()));
        }
    }
    document.insert(
        "application_id".to_string(),
        Value::String(delivery.app_monitor_id.to_string()),
    );
    if let Some(version) = delivery.app_monitor_version {
        document.insert(
            "application_version".to_string(),
            Value::String(version.to_string()),
        );
    }
    document.insert(
        "application_name".to_string(),
        Value::String(delivery.app_monitor_name.to_string()),
    );
    document.insert("event_id".to_string(), Value::String(event.id.clone()));
    document.insert(
        EVENT_TIMESTAMP_FIELD.to_string(),
        Value::from(event_timestamp_millis(&event.timestamp)),
    );
    document.insert(
        super::EVENT_TYPE_FIELD.to_string(),
        Value::String(event.r#type.clone()),
    );
    // The schema version and the interaction are the event's own, as the client stamped
    // its payload with them: the details first, since they describe THIS event, and the
    // session's metadata after it.
    if let Some(version) = client_member(&details, &metadata, CLIENT_VERSION_MEMBER) {
        document.insert("event_version".to_string(), version);
    }
    document.insert(
        "log_stream".to_string(),
        Value::String(delivery.log_stream.to_string()),
    );
    document.insert(
        "batch_id".to_string(),
        Value::String(delivery.batch_id.to_string()),
    );
    let (keys, values) = metadata_projections(&metadata);
    document.insert(METADATA_FIELD.to_string(), metadata.clone());
    document.insert(METADATA_KEYS_FIELD.to_string(), Value::Array(keys));
    document.insert(METADATA_VALUES_FIELD.to_string(), Value::Array(values));
    document.insert(super::EVENT_DETAILS_FIELD.to_string(), details.clone());
    document.insert("user_details".to_string(), Value::Object(user));
    document.insert(
        "account_id".to_string(),
        Value::String(::engine::emulator_account_id().to_string()),
    );
    document.insert(
        "event_interaction".to_string(),
        client_member(&details, &metadata, CLIENT_INTERACTION_MEMBER)
            .unwrap_or_else(|| Value::from(NO_INTERACTION)),
    );
    Value::Object(document).to_string()
}

/// The value the client reported for `member` in this event's `details`, or failing that in
/// its session `metadata` — the two documents a RUM web client stamps its payload in.
fn client_member(details: &Value, metadata: &Value, member: &str) -> Option<Value> {
    [details, metadata]
        .into_iter()
        .filter_map(|document| document.get(member))
        .find(|value| !value.is_null())
        .cloned()
}

/// The `metadata_keys` / `metadata_values` projections of a metadata document: its
/// first-level keys, and its values at the same positions.
///
/// They are a FLAT pair of lists over one document, so each value is projected as the text
/// it is written as; metadata the client did not serialize as a JSON object projects
/// nothing, since it has no first-level keys to flatten.
fn metadata_projections(metadata: &Value) -> (Vec<Value>, Vec<Value>) {
    let Some(members) = metadata.as_object() else {
        return (Vec::new(), Vec::new());
    };
    members
        .iter()
        .map(|(key, value)| {
            (
                Value::String(key.clone()),
                Value::String(projected_text(value)),
            )
        })
        .unzip()
}

/// One metadata value as the text the projection carries it as: a string as itself, and
/// every other JSON value as the text it is written as.
fn projected_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// A `JsonValue` member as the document it carries, or as a plain string when it does not
/// parse as JSON.
fn json_member(text: &str) -> Value {
    ::serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
}
