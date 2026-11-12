// SPDX-License-Identifier: CC0-1.0

//! The wire form of every instant this service reports.
//!
//! RULE: a member the model binds to a timestamp carrying `@timestampFormat("date-time")`
//! travels on the restJson1 wire as an RFC 3339 / ISO-8601 date-time STRING naming the
//! instant the service holds — never as the epoch-seconds NUMBER the protocol defaults to
//! when a member schema declares no format. Every AWS SDK deserializes such a member with a
//! date-time parser, so a bare number is a value no client of the modelled API can read.
//!
//! Every timestamp shape of this model declares that format, which the first test reads off
//! the shipped Smithy model rather than off a copied list — so the rule reaches every shape
//! this service reports an instant in, not only the one a divergence was observed on, and a
//! model refresh that introduced an `epoch-seconds` or `http-date` member is reported here
//! instead of being served in the wrong form.
//!
//! The second test asserts the rule on the WIRE, over one row per implemented path that
//! reports an instant: the single-resource reads and the listings of the farm, queue, fleet,
//! storage-profile, limit, budget, worker, queue-fleet-association and job families. The
//! third asserts it for a NESTED member — a budget's schedule reaches its two instants
//! through a union and a structure — in both directions, since what a request carries for
//! such a member is read with the same format it is written with.
//!
//! The harness (`Emulator`) and the fleet helpers are the ones the other suites drive the
//! service with, so every call goes through routing, deserialization, constraint validation,
//! the handler and serialization, exactly as the router does in production.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};

use crate::farm_queue_job::{string_at, Emulator};
use crate::fleet::create_fleet;

/// This service's Smithy namespace: the prefix of every shape id of its own model.
const NAMESPACE: &str = "com.amazonaws.deadline";

/// How far an instant this service minted may sit from this test's own clock reading, in
/// seconds.
///
/// The point of the assertion is that the member names the moment the service wrote rather
/// than zero, a fixed literal or a parse of its own text, so the slack is generous enough
/// that a loaded or slow machine cannot turn it into a timing failure.
const INSTANT_SLACK_SECONDS: f64 = 600.0;

/// Members that are an instant whenever a document carries them at all. An instant a record
/// does not hold yet is an ABSENT member, not a zero, so these are asserted only where they
/// appear.
const OPTIONAL_INSTANTS: [&str; 4] = ["updatedAt", "startedAt", "endedAt", "queueStoppedAt"];

// ---------------------------------------------------------------------------
// The rule on the model: every timestamp this model declares is a `date-time`
// ---------------------------------------------------------------------------

/// This service's own Smithy model, as shipped in this crate.
fn model() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-deadline-deadline.json"
    );
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("parse {path}: {error}"))
}

/// The `@timestampFormat` a member-like node declares, on the node itself or on the shape it
/// targets.
fn declared_format<'model>(
    shapes: &'model serde_json::Map<String, Value>,
    node: &'model Value,
) -> Option<&'model str> {
    let on_node = node
        .get("traits")
        .and_then(|traits| traits.get("smithy.api#timestampFormat"))
        .and_then(Value::as_str);
    let on_target = node
        .get("target")
        .and_then(Value::as_str)
        .and_then(|target| shapes.get(target))
        .and_then(|shape| shape.get("traits"))
        .and_then(|traits| traits.get("smithy.api#timestampFormat"))
        .and_then(Value::as_str);
    on_node.or(on_target)
}

/// Whether a member-like node targets a timestamp: the Smithy prelude's own `Timestamp`, or a
/// model shape declared `"type": "timestamp"`.
fn targets_a_timestamp(shapes: &serde_json::Map<String, Value>, node: &Value) -> bool {
    match node.get("target").and_then(Value::as_str) {
        Some("smithy.api#Timestamp") => true,
        Some(target) => {
            shapes.get(target).and_then(|shape| shape.get("type")) == Some(&json!("timestamp"))
        }
        None => false,
    }
}

/// Every member-like node of one shape, as `(member path, node)`: a structure's or union's
/// members, a list's element, and a map's key and value — the four places a model can bind a
/// timestamp.
fn member_nodes(shape_id: &str, shape: &Value) -> Vec<(String, Value)> {
    let mut nodes = Vec::new();
    if let Some(members) = shape.get("members").and_then(Value::as_object) {
        for (name, node) in members {
            nodes.push((format!("{shape_id}${name}"), node.clone()));
        }
    }
    for single in ["member", "key", "value"] {
        if let Some(node) = shape.get(single) {
            nodes.push((format!("{shape_id}${single}"), node.clone()));
        }
    }
    nodes
}

/// The rule on the model: every member of this model that targets a timestamp declares
/// `@timestampFormat("date-time")`.
///
/// This is what makes the service-wide date-time codec (`src/service.rs`'s
/// `override_protocol`) the model's own rule rather than a guess.
#[test]
fn every_timestamp_member_of_the_model_declares_the_date_time_format() {
    let model = model();
    let shapes = model
        .get("shapes")
        .and_then(Value::as_object)
        .expect("the model must declare shapes");

    let mut timestamp_members = 0_usize;
    let mut wrong: Vec<String> = Vec::new();
    for (shape_id, shape) in shapes {
        if !shape_id.starts_with(NAMESPACE) {
            continue;
        }
        for (path, node) in member_nodes(shape_id, shape) {
            if !targets_a_timestamp(shapes, &node) {
                continue;
            }
            timestamp_members += 1;
            let format = declared_format(shapes, &node);
            if format != Some("date-time") {
                wrong.push(format!("{path}: {format:?}"));
            }
        }
    }

    assert!(
        wrong.is_empty(),
        "every timestamp member must declare the date-time format, got {} that do not: \
         {wrong:?}",
        wrong.len()
    );
    // A floor, so a walk that found nothing cannot pass as agreement.
    assert!(
        timestamp_members >= 50,
        "the walk must have reached this model's timestamp members, got {timestamp_members}"
    );
}

// ---------------------------------------------------------------------------
// The rule on the wire
// ---------------------------------------------------------------------------

/// Assert `member` of `document` is the RFC 3339 date-time string the rule requires, and
/// answer the instant it names in epoch seconds.
///
/// The failure messages name the two ways the rule is broken: a non-string (which is what the
/// protocol default's epoch NUMBER is) and a string no date-time parser accepts.
fn instant_seconds(case: &str, document: &Value, member: &str) -> f64 {
    let value = document
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {document}"));
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {member} is modelled @timestampFormat(\"date-time\"), so it must be \
             an RFC 3339 string and never the protocol-default epoch number, got {value}"
        )
    });
    // Smithy's `date-time` is RFC 3339 at the `Z` offset, so both designators are part of the
    // form a generated SDK's parser is handed.
    assert!(
        text.contains('T') && text.ends_with('Z'),
        "case {case:?}: {member} must carry the T separator and the Z offset, got {text:?}"
    );
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| {
            panic!(
                "case {case:?}: {member} = {text:?} must parse as an RFC 3339 date-time: {error}"
            )
        })
        .as_secs_f64()
}

/// One document a path answered, under the case that names the path.
struct Reported {
    case: String,
    document: Value,
}

/// This test's clock reading, in epoch seconds.
fn now_seconds() -> f64 {
    DateTime::from(std::time::SystemTime::now()).as_secs_f64()
}

/// The items one listing reported, under the member that holds them.
fn items(case: &str, status: u16, body: &Value, member: &str) -> Vec<Value> {
    assert_eq!(status, 200, "{case}: expected 200, got {status} with {body}");
    body.get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{case}: expected a {member} list, got {body}"))
        .clone()
}

/// A `CreateBudget` body carrying every member the model requires, with a fixed schedule
/// whose two `date-time` members are written the way an SDK serializes them.
fn budget_body(display_name: &str, queue_id: &str, start: &str, end: &str) -> Value {
    json!({
        "displayName": display_name,
        "description": "the instants of a budget",
        "usageTrackingResource": {"queueId": queue_id},
        "approximateDollarLimit": 10.0,
        "actions": [{"type": "STOP_SCHEDULING_AND_COMPLETE_TASKS", "thresholdPercentage": 80.0}],
        "schedule": {"fixed": {"startTime": start, "endTime": end}},
    })
}

/// Every document an implemented path of this service reports an instant in: one row per
/// resource family per path, which is what the rule generalises over.
async fn reported_documents(emulator: &Emulator) -> Vec<Reported> {
    let mut rows: Vec<Reported> = Vec::new();
    let farm_id = emulator.farm("Instant farm").await;
    let queue_id = emulator.queue(&farm_id, "Instant queue").await;

    let mut read = |case: &str, document: Value| {
        rows.push(Reported {
            case: case.to_string(),
            document,
        });
    };

    // The farm itself, read back and listed.
    let (status, farm) = emulator.get(&format!("/2023-10-12/farms/{farm_id}")).await;
    assert_eq!(status, 200, "GetFarm: expected 200, got {status} with {farm}");
    read("GetFarm", farm);
    let (status, listed) = emulator.get("/2023-10-12/farms").await;
    for (index, item) in items("ListFarms", status, &listed, "farms")
        .into_iter()
        .enumerate()
    {
        read(&format!("ListFarms item {index}"), item);
    }

    // The queue.
    let (status, queue) = emulator
        .get(&format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}"))
        .await;
    assert_eq!(status, 200, "GetQueue: expected 200, got {status} with {queue}");
    read("GetQueue", queue);

    // A fleet, read back and listed, and the worker registered with it.
    let fleet_id = create_fleet(emulator, &farm_id, "Instant fleet").await;
    let (status, fleet) = emulator
        .get(&format!("/2023-10-12/farms/{farm_id}/fleets/{fleet_id}"))
        .await;
    assert_eq!(status, 200, "GetFleet: expected 200, got {status} with {fleet}");
    read("GetFleet", fleet);
    let (status, listed) = emulator
        .get(&format!("/2023-10-12/farms/{farm_id}/fleets"))
        .await;
    for (index, item) in items("ListFleets", status, &listed, "fleets")
        .into_iter()
        .enumerate()
    {
        read(&format!("ListFleets item {index}"), item);
    }

    let (status, worker) = emulator
        .post(
            &format!("/2023-10-12/farms/{farm_id}/fleets/{fleet_id}/workers"),
            json!({"hostProperties": {"hostName": "render-01"}}),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateWorker: expected 200, got {status} with {worker}"
    );
    let worker_id = string_at(&worker, "workerId");
    let (status, worker) = emulator
        .get(&format!(
            "/2023-10-12/farms/{farm_id}/fleets/{fleet_id}/workers/{worker_id}"
        ))
        .await;
    assert_eq!(status, 200, "GetWorker: expected 200, got {status} with {worker}");
    read("GetWorker", worker);

    // The queue-fleet association of that queue and fleet.
    let (status, body) = emulator
        .call(
            "PUT",
            &format!("/2023-10-12/farms/{farm_id}/queue-fleet-associations"),
            Some(json!({"queueId": queue_id, "fleetId": fleet_id})),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateQueueFleetAssociation: expected 200, got {status} with {body}"
    );
    let (status, association) = emulator
        .get(&format!(
            "/2023-10-12/farms/{farm_id}/queue-fleet-associations/{queue_id}/{fleet_id}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetQueueFleetAssociation: expected 200, got {status} with {association}"
    );
    read("GetQueueFleetAssociation", association);

    // A storage profile.
    let (status, body) = emulator
        .post(
            &format!("/2023-10-12/farms/{farm_id}/storage-profiles"),
            json!({"displayName": "Instant profile", "osFamily": "LINUX"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateStorageProfile: expected 200, got {status} with {body}"
    );
    let storage_profile_id = string_at(&body, "storageProfileId");
    let (status, profile) = emulator
        .get(&format!(
            "/2023-10-12/farms/{farm_id}/storage-profiles/{storage_profile_id}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetStorageProfile: expected 200, got {status} with {profile}"
    );
    read("GetStorageProfile", profile);

    // A limit.
    let (status, body) = emulator
        .post(
            &format!("/2023-10-12/farms/{farm_id}/limits"),
            json!({"displayName": "Instant limit", "amountRequirementName": "maya", "maxCount": 4}),
        )
        .await;
    assert_eq!(status, 200, "CreateLimit: expected 200, got {status} with {body}");
    let limit_id = string_at(&body, "limitId");
    let (status, limit) = emulator
        .get(&format!("/2023-10-12/farms/{farm_id}/limits/{limit_id}"))
        .await;
    assert_eq!(status, 200, "GetLimit: expected 200, got {status} with {limit}");
    read("GetLimit", limit);

    // A budget, read back and listed.
    let (status, body) = emulator
        .post(
            &format!("/2023-10-12/farms/{farm_id}/budgets"),
            budget_body(
                "Instant budget",
                &queue_id,
                "2026-01-01T00:00:00Z",
                "2026-01-02T00:00:00Z",
            ),
        )
        .await;
    assert_eq!(status, 200, "CreateBudget: expected 200, got {status} with {body}");
    let budget_id = string_at(&body, "budgetId");
    let (status, budget) = emulator
        .get(&format!("/2023-10-12/farms/{farm_id}/budgets/{budget_id}"))
        .await;
    assert_eq!(status, 200, "GetBudget: expected 200, got {status} with {budget}");
    read("GetBudget", budget);
    let (status, listed) = emulator
        .get(&format!("/2023-10-12/farms/{farm_id}/budgets"))
        .await;
    for (index, item) in items("ListBudgets", status, &listed, "budgets")
        .into_iter()
        .enumerate()
    {
        read(&format!("ListBudgets item {index}"), item);
    }

    // A job, read back and listed.
    let job_id = emulator.job(&farm_id, &queue_id, "Instant job", 50).await;
    let (status, job) = emulator
        .get(&format!(
            "/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs/{job_id}"
        ))
        .await;
    assert_eq!(status, 200, "GetJob: expected 200, got {status} with {job}");
    read("GetJob", job);
    let (status, listed) = emulator
        .get(&format!(
            "/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs"
        ))
        .await;
    for (index, item) in items("ListJobs", status, &listed, "jobs")
        .into_iter()
        .enumerate()
    {
        read(&format!("ListJobs item {index}"), item);
    }

    rows
}

/// The rule on the wire: every instant every implemented path of this service reports is an
/// RFC 3339 date-time string naming the moment the service wrote.
#[tokio::test]
async fn every_reported_instant_is_a_date_time_string() {
    let emulator = Emulator::new();
    let before = now_seconds();
    let rows = reported_documents(&emulator).await;
    let after = now_seconds();

    assert!(
        rows.len() >= 12,
        "the table must span every implemented path that reports an instant, got {} rows",
        rows.len()
    );

    for row in &rows {
        let seconds = instant_seconds(&row.case, &row.document, "createdAt");
        assert!(
            seconds >= before - INSTANT_SLACK_SECONDS && seconds <= after + INSTANT_SLACK_SECONDS,
            "case {:?}: expected createdAt within {INSTANT_SLACK_SECONDS}s of this run \
             ({before}..{after}), got {seconds}",
            row.case
        );
        for member in OPTIONAL_INSTANTS {
            if row.document.get(member).is_some() {
                instant_seconds(&row.case, &row.document, member);
            }
        }
    }
}

/// The rule for a NESTED member, in both directions: the two instants of a budget's fixed
/// schedule are reached through a union and a structure, and each is carried as the RFC 3339
/// string the request sent — so the instant the store holds is exactly the one the caller
/// named.
///
/// The rows differ along what the rule generalises over: sub-second precision, the era of the
/// instant (the epoch itself, a leap day, past the turn of the century) and a window of a
/// single second.
#[tokio::test]
async fn a_nested_date_time_round_trips_as_the_string_the_request_sent() {
    // case, startTime, endTime
    let rows: [(&str, &str, &str); 5] = [
        ("utc zulu", "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z"),
        (
            "sub-second precision",
            "2026-03-04T05:06:07.123Z",
            "2026-03-05T05:06:07.125Z",
        ),
        ("a leap day", "2028-02-29T12:00:00Z", "2028-03-01T12:00:00Z"),
        (
            "past the turn of the century",
            "2099-12-31T23:59:59Z",
            "2100-01-01T00:00:00Z",
        ),
        (
            "the unix epoch and one second of it",
            "1970-01-01T00:00:00Z",
            "1970-01-01T00:00:01Z",
        ),
    ];

    let emulator = Emulator::new();
    let farm_id = emulator.farm("Nested instant farm").await;
    let queue_id = emulator.queue(&farm_id, "Nested instant queue").await;

    for (case, start, end) in rows {
        let (status, body) = emulator
            .post(
                &format!("/2023-10-12/farms/{farm_id}/budgets"),
                budget_body(case, &queue_id, start, end),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudget to answer 200, got {status} with {body}"
        );
        let budget_id = string_at(&body, "budgetId");

        let (status, budget) = emulator
            .get(&format!("/2023-10-12/farms/{farm_id}/budgets/{budget_id}"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetBudget to answer 200, got {status} with {budget}"
        );
        let schedule = budget
            .get("schedule")
            .and_then(|schedule| schedule.get("fixed"))
            .unwrap_or_else(|| panic!("case {case:?}: expected schedule.fixed in {budget}"));
        for (member, sent) in [("startTime", start), ("endTime", end)] {
            let reported = instant_seconds(case, schedule, member);
            let expected = DateTime::from_str(sent, Format::DateTime)
                .expect("the case is an RFC 3339 date-time")
                .as_secs_f64();
            assert!(
                (reported - expected).abs() < f64::EPSILON,
                "case {case:?}: expected {member} to name the instant {sent} ({expected}s), \
                 got {reported}s from {schedule}"
            );
        }
    }
}

/// The rule holds on a read that mints nothing: the same date-time string answers every
/// later read, so the form is the serializer's and not a per-call rendering.
#[tokio::test]
async fn a_second_read_reports_the_same_date_time_string() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Stable instant farm").await;

    // One row per resource family whose single-resource read is implemented and which can be
    // created from a farm alone.
    let (status, body) = emulator
        .post(
            &format!("/2023-10-12/farms/{farm_id}/storage-profiles"),
            json!({"displayName": "Stable profile", "osFamily": "MACOS"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateStorageProfile: expected 200, got {status} with {body}"
    );
    let storage_profile_id = string_at(&body, "storageProfileId");
    let fleet_id = create_fleet(&emulator, &farm_id, "Stable fleet").await;
    let paths: [(&str, String); 3] = [
        ("GetFarm", format!("/2023-10-12/farms/{farm_id}")),
        (
            "GetStorageProfile",
            format!("/2023-10-12/farms/{farm_id}/storage-profiles/{storage_profile_id}"),
        ),
        (
            "GetFleet",
            format!("/2023-10-12/farms/{farm_id}/fleets/{fleet_id}"),
        ),
    ];

    for (case, path) in &paths {
        let (status, first) = emulator.get(path).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {first}"
        );
        let text = string_at(&first, "createdAt");
        let (_, again) = emulator.get(path).await;
        assert_eq!(
            string_at(&again, "createdAt"),
            text,
            "case {case:?}: a second read must report the same createdAt {text:?}, got {again}"
        );
    }
}
