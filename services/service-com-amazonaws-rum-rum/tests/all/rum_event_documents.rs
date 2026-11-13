// SPDX-License-Identifier: CC0-1.0

//! The RUM EVENT DOCUMENT a read reports for an event the data plane accepted.
//!
//! The rule under test: an event `PutRumEvents` accepts is reported by
//! `GetAppMonitorData` as the service's own RUM event document — the fields RUM publishes
//! for a RUM event, each computed from the delivered batch and from the app monitor's
//! stored state, with the event's instant as `event_timestamp` in MILLISECONDS since the
//! epoch, which is the unit `TimeRange`'s bounds are expressed in. The client's own
//! `{id, timestamp, type, metadata, details}` structure is not what a reader gets back.
//!
//! The fields are the first-level keys the model enumerates for a RUM event
//! (`smithy_query(query="doc", shape="MetricDefinitionRequest")`: "Valid key values for
//! first-level keys in the `ValueKey`, `EventPattern`, and `DimensionKeys` fields" —
//! `account_id`, `application_Id`, `application_version`, `application_name`, `batch_id`,
//! `event_details`, `event_id`, `event_interaction`, `event_timestamp`, `event_type`,
//! `event_version`, `log_stream`, `metadata`, `sessionId`, `user_details`, `userId`), with
//! the application's id spelled `application_id` as the wire document spells it, and the
//! `metadata_keys` / `metadata_values` projections of `metadata` beside it.
//!
//! The rows span what the rule generalises over: whether the batch carries `metadata` at
//! all, whether that metadata is a JSON object, the TYPES its values have, whether the
//! client stamped a schema version or an interaction on the event (and in which of the two
//! documents), whether the batch names a user session, the shape of `details` (an object, a
//! nested one, an array, text that is no JSON at all), and the value of the instant (an
//! ordinary one, one with a subsecond part, one before the epoch).
//!
//! Each row also asserts the unit: the window that covers the event in MILLISECONDS does,
//! and the same instant read as SECONDS covers nothing.

use serde_json::{json, Map, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{call, create, emulator, ACCOUNT};

/// How many milliseconds one second is: `RumEvent$timestamp` travels as the epoch seconds
/// restJson1 carries a `Timestamp` as, while the document and the query window are in epoch
/// milliseconds.
const MILLIS_PER_SECOND: i64 = 1_000;

/// One delivered event, and the document the read then owes for it.
struct Row {
    case: &'static str,
    /// `PutRumEventsRequest$BatchId`.
    batch_id: &'static str,
    /// `UserDetails$userId` / `UserDetails$sessionId`, for the batches that name them.
    user_id: Option<&'static str>,
    session_id: Option<&'static str>,
    /// `AppMonitorDetails$version`: the client's own view of the application's version.
    version: Option<&'static str>,
    /// `RumEvent$id`.
    event_id: &'static str,
    /// `RumEvent$timestamp` as the wire carries it, and the milliseconds the document must
    /// then report.
    timestamp: Value,
    millis: i64,
    /// `RumEvent$type`.
    event_type: &'static str,
    /// `RumEvent$metadata`, an `@mediaType("application/json")` string, for the events that
    /// carry one.
    metadata: Option<&'static str>,
    /// `RumEvent$details`, the one such document every event carries.
    details: &'static str,
    /// The `event_version` the document must report: the version the client stamped its
    /// payload with, where it stamped one.
    event_version: Option<&'static str>,
    /// The `event_interaction` the document must report: the interaction the client
    /// reported, and the session's zeroth where it reported none.
    interaction: Value,
}

impl Row {
    /// The monitor this row's batch is delivered to: one per row, so the window a row asks
    /// for selects its own event alone.
    fn monitor(&self, index: usize) -> String {
        format!("documented-{index}")
    }

    /// This batch as `PutRumEvents` carries it.
    fn batch(&self, monitor: &str, monitor_id: &str) -> Value {
        let mut user_details = json!({});
        if let Some(user_id) = self.user_id {
            user_details["userId"] = json!(user_id);
        }
        if let Some(session_id) = self.session_id {
            user_details["sessionId"] = json!(session_id);
        }
        let mut app_monitor_details = json!({"name": monitor, "id": monitor_id});
        if let Some(version) = self.version {
            app_monitor_details["version"] = json!(version);
        }
        let mut event = json!({
            "id": self.event_id,
            "timestamp": self.timestamp,
            "type": self.event_type,
            "details": self.details,
        });
        if let Some(metadata) = self.metadata {
            event["metadata"] = json!(metadata);
        }
        json!({
            "BatchId": self.batch_id,
            "AppMonitorDetails": app_monitor_details,
            "UserDetails": user_details,
            "RumEvents": [event],
        })
    }

    /// `metadata` as the document reports it: the document the client serialized, and
    /// nothing at all for an event that carried none.
    fn reported_metadata(&self) -> Value {
        match self.metadata {
            Some(metadata) => as_json(metadata),
            None => json!({}),
        }
    }

    /// The whole document the read owes for this row's event, for the monitor
    /// `monitor`/`monitor_id` it was delivered to.
    fn expected_document(&self, monitor: &str, monitor_id: &str) -> Value {
        let metadata = self.reported_metadata();
        let (keys, values) = projections(&metadata);
        let mut document = json!({
            "application_id": monitor_id,
            "application_name": monitor,
            "event_id": self.event_id,
            "event_timestamp": self.millis,
            "event_type": self.event_type,
            "log_stream": self.session_id.unwrap_or(monitor_id),
            "batch_id": self.batch_id,
            "metadata": metadata,
            "metadata_keys": keys,
            "metadata_values": values,
            "event_details": as_json(self.details),
            "user_details": {},
            "account_id": ACCOUNT,
            "event_interaction": self.interaction.clone(),
        });
        if let Some(version) = self.version {
            document["application_version"] = json!(version);
        }
        if let Some(version) = self.event_version {
            document["event_version"] = json!(version);
        }
        if let Some(user_id) = self.user_id {
            document["userId"] = json!(user_id);
            document["user_details"]["userId"] = json!(user_id);
        }
        if let Some(session_id) = self.session_id {
            document["sessionId"] = json!(session_id);
            document["user_details"]["sessionId"] = json!(session_id);
        }
        document
    }
}

/// `text` as the JSON document it carries, or as the text itself when it is no JSON at all
/// — which is how a `JsonValue` member the model constrains in no way is reported.
fn as_json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| json!(text))
}

/// The `metadata_keys` / `metadata_values` projections of a metadata document: its
/// first-level keys in the order it holds them, and each of its values as the text it is
/// written as. Metadata that is no JSON object has no first-level keys, so it projects
/// nothing.
fn projections(metadata: &Value) -> (Vec<Value>, Vec<Value>) {
    metadata
        .as_object()
        .map(|members| {
            members
                .iter()
                .map(|(key, value)| {
                    (
                        json!(key),
                        match value {
                            Value::String(text) => json!(text),
                            other => json!(other.to_string()),
                        },
                    )
                })
                .unzip()
        })
        .unwrap_or_default()
}

/// The rows every case below delivers.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a page view whose metadata and details are both stamped",
            batch_id: "11111111-2222-4333-8444-555555555555",
            user_id: Some("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("1.0.0"),
            event_id: "11111111-1234-4234-8234-123456789012",
            timestamp: json!(1_704_067_200_i64),
            millis: 1_704_067_200_000,
            event_type: "com.amazon.rum.page_view_event",
            metadata: Some(
                "{\"version\":\"1.0.0\",\"pageId\":\"/home\",\"browserName\":\"Chrome\",\
                 \"interaction\":3}",
            ),
            details: "{\"version\":\"1.0.0\",\"pageId\":\"/home\"}",
            event_version: Some("1.0.0"),
            interaction: json!(3),
        },
        Row {
            case: "an event carrying no metadata at all",
            batch_id: "22222222-2222-4333-8444-555555555555",
            user_id: Some("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("2.3.1"),
            event_id: "22111111-1234-4234-8234-123456789012",
            timestamp: json!(1_704_067_260_i64),
            millis: 1_704_067_260_000,
            event_type: "com.amazon.rum.js_error_event",
            metadata: None,
            details: "{\"version\":\"0.9.0\",\"message\":\"boom\"}",
            // The details are the event's own, so a version stamped there is the event's
            // version even when the session's metadata is absent.
            event_version: Some("0.9.0"),
            interaction: json!(0),
        },
        Row {
            case: "metadata whose values are not all strings",
            batch_id: "33333333-2222-4333-8444-555555555555",
            user_id: Some("bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("eeeeeeee-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("1.0.0"),
            event_id: "33111111-1234-4234-8234-123456789012",
            timestamp: json!(1_704_070_800_i64),
            millis: 1_704_070_800_000,
            event_type: "com.amazon.rum.http_event",
            metadata: Some(
                "{\"pageId\":42,\"cached\":true,\"sampleRate\":0.25,\"interaction\":7}",
            ),
            details: "{\"request\":{\"method\":\"GET\",\"status\":200}}",
            event_version: None,
            interaction: json!(7),
        },
        Row {
            case: "a batch that names no session, no user and no version",
            batch_id: "44444444-2222-4333-8444-555555555555",
            user_id: None,
            session_id: None,
            version: None,
            event_id: "44111111-1234-4234-8234-123456789012",
            timestamp: json!(1_704_074_400_i64),
            millis: 1_704_074_400_000,
            event_type: "com.amazon.rum.session_start_event",
            metadata: Some("{\"browserName\":\"Edge\"}"),
            details: "{}",
            event_version: None,
            interaction: json!(0),
        },
        Row {
            case: "details that are a JSON array",
            batch_id: "55555555-2222-4333-8444-555555555555",
            user_id: Some("cccccccc-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("dddddddd-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("1.0.0"),
            event_id: "55111111-1234-4234-8234-123456789012",
            timestamp: json!(1_704_078_000_i64),
            millis: 1_704_078_000_000,
            event_type: "com.amazon.rum.performance_resource_event",
            metadata: Some("{\"version\":\"1.2.3\"}"),
            details: "[1,2,3]",
            event_version: Some("1.2.3"),
            interaction: json!(0),
        },
        Row {
            case: "metadata and details that are no JSON at all",
            batch_id: "66666666-2222-4333-8444-555555555555",
            user_id: Some("cccccccc-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("dddddddd-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("1.0.0"),
            event_id: "66111111-1234-4234-8234-123456789012",
            timestamp: json!(1_704_081_600_i64),
            millis: 1_704_081_600_000,
            event_type: "com.amazon.rum.custom_event",
            metadata: Some("not a document"),
            details: "neither is this",
            event_version: None,
            interaction: json!(0),
        },
        Row {
            case: "an instant carrying a subsecond part",
            batch_id: "77777777-2222-4333-8444-555555555555",
            user_id: Some("cccccccc-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("dddddddd-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("1.0.0"),
            event_id: "77111111-1234-4234-8234-123456789012",
            timestamp: json!(1_704_085_200.75_f64),
            millis: 1_704_085_200_750,
            event_type: "com.amazon.rum.page_view_event",
            metadata: Some("{\"pageId\":\"/pricing\"}"),
            details: "{\"pageTitle\":\"Pricing\"}",
            event_version: None,
            interaction: json!(0),
        },
        Row {
            case: "an instant before the epoch",
            batch_id: "88888888-2222-4333-8444-555555555555",
            user_id: Some("cccccccc-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("dddddddd-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("1.0.0"),
            event_id: "88111111-1234-4234-8234-123456789012",
            timestamp: json!(-1.5_f64),
            millis: -1_500,
            event_type: "com.amazon.rum.page_view_event",
            metadata: Some("{\"pageId\":\"/past\"}"),
            details: "{\"pageTitle\":\"Past\"}",
            event_version: None,
            interaction: json!(0),
        },
    ]
}

/// Deliver one batch, asserting the data plane accepted it.
async fn deliver(router: &MultiServiceRouter, case: &str, monitor_id: &str, body: Value) {
    let reply = call(
        router,
        "POST",
        &format!("/appmonitors/{monitor_id}/"),
        Some(body),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected PutRumEvents to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// The documents `GetAppMonitorData` reports for `monitor` over the window `[after,
/// before]`, both in milliseconds since the epoch.
async fn reported(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    after: i64,
    before: i64,
) -> Vec<Value> {
    let reply = call(
        router,
        "POST",
        &format!("/appmonitor/{monitor}/data"),
        Some(json!({"TimeRange": {"After": after, "Before": before}})),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected GetAppMonitorData to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json(case)["Events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            let text = entry.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: a reported event must be a JSON string, got {entry}")
            });
            serde_json::from_str(text).unwrap_or_else(|error| {
                panic!("case {case:?}: a reported event must be a document, got {text:?} ({error})")
            })
        })
        .collect()
}

/// The field names a document carries, sorted — what a missing or an extra field shows up
/// in.
fn fields(document: &Value) -> Vec<String> {
    let mut names: Vec<String> = document
        .as_object()
        .unwrap_or(&Map::new())
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn every_accepted_event_is_reported_as_the_rum_event_document() {
    for (index, row) in rows().iter().enumerate() {
        let (router, _engine) = emulator();
        let monitor = row.monitor(index);
        let monitor_id = create(
            &router,
            row.case,
            json!({"Name": &monitor, "Domain": "documents.example.com"}),
        )
        .await;
        deliver(
            &router,
            row.case,
            &monitor_id,
            row.batch(&monitor, &monitor_id),
        )
        .await;

        // The window that covers the event is the one in MILLISECONDS: the single
        // millisecond the instant falls in, since both bounds are inclusive.
        let documents = reported(&router, row.case, &monitor, row.millis, row.millis).await;
        assert_eq!(
            documents.len(),
            1,
            "case {:?}: the window covering the delivered instant must report the one \
             accepted event, got {documents:?}",
            row.case
        );
        let expected = row.expected_document(&monitor, &monitor_id);
        assert_eq!(
            fields(&documents[0]),
            fields(&expected),
            "case {:?}: expected exactly the fields RUM publishes for a RUM event, got {}",
            row.case,
            documents[0]
        );
        assert_eq!(
            documents[0], expected,
            "case {:?}: expected the enriched document {expected}, got {}",
            row.case, documents[0]
        );

        // And the unit is the millisecond: the same instant read as SECONDS names a window
        // a thousandfold away from the event, which covers nothing. Only the rows at
        // positive instants can ask that, since a `Before` of zero is the member's modelled
        // default and stands for now.
        if row.millis > 0 {
            let seconds = row.millis / MILLIS_PER_SECOND;
            let outside = reported(
                &router,
                row.case,
                &monitor,
                seconds - MILLIS_PER_SECOND,
                seconds + MILLIS_PER_SECOND,
            )
            .await;
            assert!(
                outside.is_empty(),
                "case {:?}: a window naming the instant in seconds covers no event, since \
                 the bounds are milliseconds, got {outside:?}",
                row.case
            );
        }

        // Reading reports the same document again: the read changed nothing.
        let again = reported(&router, row.case, &monitor, row.millis, row.millis).await;
        assert_eq!(
            again, documents,
            "case {:?}: expected a second read to report the same document, got {again:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn one_document_is_reported_for_every_event_of_a_batch() {
    // The projection is per EVENT, not per batch: a batch carrying several events is
    // reported as several documents, each carrying that event's own id, instant, type and
    // details beside the batch's shared context.
    let (router, _engine) = emulator();
    let monitor = "documented-batch";
    let monitor_id = create(
        &router,
        "a batch of three",
        json!({"Name": monitor, "Domain": "documents.example.com"}),
    )
    .await;
    let session = "ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb";
    let batch_id = "99999999-2222-4333-8444-555555555555";
    let instants: [i64; 3] = [1_704_067_200_000, 1_704_067_260_000, 1_704_067_320_000];
    let events: Vec<Value> = instants
        .iter()
        .enumerate()
        .map(|(step, millis)| {
            json!({
                "id": format!("9911111{step}-1234-4234-8234-123456789012"),
                "timestamp": millis / MILLIS_PER_SECOND,
                "type": "com.amazon.rum.page_view_event",
                "metadata": format!("{{\"pageId\":\"/page-{step}\"}}"),
                "details": format!("{{\"pageTitle\":\"Page {step}\"}}"),
            })
        })
        .collect();
    deliver(
        &router,
        "a batch of three",
        &monitor_id,
        json!({
            "BatchId": batch_id,
            "AppMonitorDetails": {"name": monitor, "id": monitor_id, "version": "1.0.0"},
            "UserDetails": {"userId": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee", "sessionId": session},
            "RumEvents": events,
        }),
    )
    .await;

    let documents = reported(
        &router,
        "a batch of three",
        monitor,
        instants[0],
        instants[2],
    )
    .await;
    assert_eq!(
        documents.len(),
        instants.len(),
        "case \"a batch of three\": expected one document per delivered event, got \
         {documents:?}"
    );
    for (step, document) in documents.iter().enumerate() {
        let case = format!("event {step} of the batch");
        assert_eq!(
            document["event_id"].as_str(),
            Some(format!("9911111{step}-1234-4234-8234-123456789012").as_str()),
            "case {case:?}: expected the events in delivery order, got {document}"
        );
        assert_eq!(
            document["event_timestamp"].as_i64(),
            Some(instants[step]),
            "case {case:?}: expected each event's own instant in milliseconds, got {document}"
        );
        assert_eq!(
            document["metadata"]["pageId"].as_str(),
            Some(format!("/page-{step}").as_str()),
            "case {case:?}: expected each event's own metadata, got {document}"
        );
        assert_eq!(
            document["batch_id"].as_str(),
            Some(batch_id),
            "case {case:?}: expected every document to carry the batch's id, got {document}"
        );
        assert_eq!(
            document["log_stream"].as_str(),
            Some(session),
            "case {case:?}: expected every document to name the session's stream, got \
             {document}"
        );
        assert_eq!(
            document["application_id"].as_str(),
            Some(monitor_id.as_str()),
            "case {case:?}: expected every document to name the monitor the batch reached, \
             got {document}"
        );
    }
}
