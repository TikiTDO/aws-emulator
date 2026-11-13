// SPDX-License-Identifier: CC0-1.0

//! Archive REPLAY end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter`,
//! with SQS mounted on the same engine so a redelivery is observed where a client
//! observes it: on the queue a rule's target names.
//!
//! The RULES asserted here:
//!
//! * `StartReplay` re-publishes the events ONE archive holds whose own event time
//!   lies in `[EventStartTime, EventEndTime]` onto `Destination.Arn`, and nothing
//!   else: "only events that occurred between the EventStartTime and EventEndTime
//!   are replayed", with both bounds included;
//! * a re-published event is routed by the DESTINATION BUS's rules — including a
//!   rule created after the events were archived — and `Destination.FilterArns`,
//!   "a list of ARNs for rules to replay events to", confines that routing to the
//!   rules it names;
//! * a redelivered event is the archived event, carrying the `replay-name` of the
//!   replay that redelivered it — a field a live delivery does not carry — and it
//!   is NOT archived again ("replayed events are not sent to an archive");
//! * the replay is a PERSISTED resource: `StartReplay` answers an ARN of the
//!   modelled `ReplayArn` form (`…:replay/<name>`) in a non-terminal state, and
//!   `DescribeReplay` reads it back under its name, reaching `COMPLETED` a moment
//!   later with `ReplayEndTime` and with `EventLastReplayedTime` at the event time
//!   of the last event replayed;
//! * a request no replay can be started for is refused with the modelled error for
//!   its cause — a missing destination bus, a name already held — and with a
//!   `ValidationException` for an event source, destination, filter or window the
//!   model cannot express.
//!
//! Each table spans the class rather than one recorded case: the rows differ along
//! where the window falls (including ON each bound and one second past it), how
//! many events it selects, which rules the filters name, and which member of the
//! request is wrong.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The source and detail type every rule below matches on.
const SOURCE: &str = "com.demo.orders";
const DETAIL_TYPE: &str = "Order Placed";

/// How long a poll of a replay's progress waits, at the OUTSIDE, for the state
/// the clock owes it: generous, because what is asserted is the state the replay
/// converges on and never how fast the host got it there.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll of a replay's progress waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The emulator: EventBridge and SQS on one engine, because a rule's target is an
/// SQS queue and a redelivery is asserted on the queue.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    Emulator { router }
}

/// One `awsJson` RPC call against the service whose signing name is
/// `signing_name` — the credential scope is what routes it.
async fn rpc(
    emulator: &Emulator,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/aws4_request, SignedHeaders=host, \
         Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", format!("application/x-amz-json-{json_version}"))
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

async fn events(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(emulator, "events", "AWSEvents", "1.1", operation, body).await
}

async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(emulator, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// The replay `name`, read back once its `State` is `wanted`: the read is
/// repeated until it is, or until [`SETTLE_DEADLINE`] passes, so what the test
/// waits for is the CONDITION and not a span. A replay that settles in any OTHER
/// terminal state fails at once — no further read can move it to `wanted`.
async fn poll_until_state(emulator: &Emulator, case: &str, name: &str, wanted: &str) -> Value {
    let deadline = ::std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) = events(emulator, "DescribeReplay", &json!({ "ReplayName": name })).await;
        assert_eq!(
            status, 200,
            "case {case}: DescribeReplay answered {status} {body}"
        );
        let reported = body["State"].as_str().unwrap_or_default().to_string();
        if reported == wanted {
            return body;
        }
        assert!(
            !state::is_terminal_replay_state(&reported),
            "case {case}: expected the replay to reach {wanted}, but it settled in \
             {reported:?}, which nothing leaves: {body}"
        );
        assert!(
            ::std::time::Instant::now() < deadline,
            "case {case}: expected the replay to reach {wanted}, got {reported:?} up to the \
             deadline: {body}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The wire error code of a failure body (`__type` on awsJson), without its
/// namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
        .unwrap_or_default()
}

/// A required string member of a response body.
fn member(case: &str, body: &Value, name: &str) -> String {
    body[name]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: no {name} in {body}"))
        .to_string()
}

/// Epoch seconds now, which is what an `awsJson1_1` time stamp carries.
fn epoch_now() -> i64 {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now()).secs()
}

/// A queue called `name`, carrying the policy a rule's target needs: EventBridge
/// "relies on resource-based policies" for a queue (`PutTargets`), so one that has
/// granted the service nothing receives nothing — the rule
/// `tests/all/queue_target_policy_authorization.rs` is the suite for. Answers its url
/// and the ARN a rule names it by.
async fn given_queue(emulator: &Emulator, name: &str) -> (String, String) {
    let (status, body) = sqs(emulator, "CreateQueue", &json!({ "QueueName": name })).await;
    assert_eq!(
        status, 200,
        "case {name}: CreateQueue answered {status} {body}"
    );
    let url = member(name, &body, "QueueUrl");
    let (status, body) = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: GetQueueAttributes answered {status} {body}"
    );
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueArn in {body}"))
        .to_string();
    let (status, body) = sqs(
        emulator,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": { "Policy": crate::target_dispatch::granting_events(&arn) },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: SetQueueAttributes answered {status} {body}"
    );
    (url, arn)
}

/// Every message body waiting on the queue at `url`, as raw text.
async fn drain(emulator: &Emulator, url: &str) -> Vec<String> {
    let (status, body) = sqs(
        emulator,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case receive {url}: ReceiveMessage answered {status} {body}"
    );
    body["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One rule of the scenario's bus: its name, its ARN (what a filter names it by)
/// and the url of the queue its one target is.
struct Rule {
    name: String,
    arn: String,
    queue: String,
}

/// Everything a row below is run against: a custom bus, two rules on it each
/// delivering to its own queue, and an `ENABLED` archive of the whole bus.
struct Scenario {
    bus: String,
    bus_arn: String,
    archive: String,
    archive_arn: String,
    rules: Vec<Rule>,
}

/// The pattern every rule below carries, which the published events match.
fn matching_pattern() -> String {
    json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string()
}

/// A rule called `name` on `bus`, with one SQS queue target; answers it.
async fn given_rule(emulator: &Emulator, bus: &str, name: &str) -> Rule {
    let (queue, queue_arn) = given_queue(emulator, name).await;
    let (status, body) = events(
        emulator,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": bus,
            "EventPattern": matching_pattern(),
            "State": "ENABLED",
        }),
    )
    .await;
    assert_eq!(status, 200, "case {name}: PutRule answered {status} {body}");
    let arn = member(name, &body, "RuleArn");
    let (status, body) = events(
        emulator,
        "PutTargets",
        &json!({
            "Rule": name,
            "EventBusName": bus,
            "Targets": [{ "Id": "t1", "Arn": queue_arn }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: PutTargets answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {name}: PutTargets reported a failed entry: {body}"
    );
    Rule {
        name: name.to_string(),
        arn,
        queue,
    }
}

/// The bus, rules and `ENABLED` archive every row starts from. `label` keeps one
/// row's resources apart from another's.
async fn given_scenario(emulator: &Emulator, label: &str) -> Scenario {
    let bus = format!("{label}-bus");
    let (status, body) = events(emulator, "CreateEventBus", &json!({ "Name": bus })).await;
    assert_eq!(
        status, 200,
        "case {label}: CreateEventBus answered {status} {body}"
    );
    let bus_arn = member(label, &body, "EventBusArn");

    let mut rules = Vec::new();
    for rule in ["alpha", "beta"] {
        rules.push(given_rule(emulator, &bus, &format!("{label}-{rule}")).await);
    }

    let archive = format!("{label}-archive");
    // No `EventPattern`: "if you do not specify a pattern to filter events sent
    // to the archive, all events are sent to the archive".
    let (status, body) = events(
        emulator,
        "CreateArchive",
        &json!({ "ArchiveName": archive, "EventSourceArn": bus_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label}: CreateArchive answered {status} {body}"
    );
    let archive_arn = member(label, &body, "ArchiveArn");
    // The archive is capturing from the create on; the read confirms the state
    // the create answered.
    let (status, body) = events(
        emulator,
        "DescribeArchive",
        &json!({ "ArchiveName": archive }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label}: DescribeArchive answered {status} {body}"
    );
    assert_eq!(
        body["State"],
        json!(state::ARCHIVE_STATE_ENABLED),
        "case {label}: the archive has to be capturing before any event is published: {body}"
    );

    Scenario {
        bus,
        bus_arn,
        archive,
        archive_arn,
        rules,
    }
}

/// Publish one event per entry of `offsets` (seconds from `anchor`, which is the
/// event's OWN time) onto the scenario's bus and answer their `EventId`s, once
/// the archive reports it has captured them all.
async fn given_archived(
    emulator: &Emulator,
    scenario: &Scenario,
    anchor: i64,
    offsets: &[i64],
) -> Vec<String> {
    let entries: Vec<Value> = offsets
        .iter()
        .enumerate()
        .map(|(index, offset)| {
            json!({
                "EventBusName": scenario.bus,
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": json!({ "order": index }).to_string(),
                "Time": anchor + offset,
            })
        })
        .collect();
    let (status, body) = events(emulator, "PutEvents", &json!({ "Entries": entries })).await;
    assert_eq!(status, 200, "PutEvents answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "PutEvents reported a failed entry: {body}"
    );
    let ids: Vec<String> = body["Entries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry["EventId"]
                .as_str()
                .unwrap_or_else(|| panic!("no EventId in {body}"))
                .to_string()
        })
        .collect();
    assert_eq!(
        ids.len(),
        offsets.len(),
        "every published event is ingested, got {ids:?}"
    );
    let count = archived_count(emulator, &scenario.archive).await;
    assert_eq!(
        count,
        i64::try_from(offsets.len()).expect("the row publishes a handful of events"),
        "the archive has to hold the published events before they can be replayed"
    );
    ids
}

/// What `DescribeArchive` counts in the archive called `name`.
async fn archived_count(emulator: &Emulator, name: &str) -> i64 {
    let (status, body) = events(emulator, "DescribeArchive", &json!({ "ArchiveName": name })).await;
    assert_eq!(
        status, 200,
        "case {name}: DescribeArchive answered {status} {body}"
    );
    body["EventCount"].as_i64().unwrap_or_default()
}

/// Take the LIVE deliveries off every rule's queue, so what a row reads
/// afterwards is what the REPLAY delivered. Each live body is asserted to carry
/// no `replay-name`: that field is what tells a redelivered event apart.
async fn drain_live_deliveries(emulator: &Emulator, case: &str, scenario: &Scenario) {
    for rule in &scenario.rules {
        for body in drain(emulator, &rule.queue).await {
            let parsed: Value = serde_json::from_str(&body).unwrap_or_else(|error| {
                panic!("case {case}: a delivered body is not JSON ({error}): {body:?}")
            });
            assert!(
                parsed.get(state::REPLAY_NAME_FIELD).is_none(),
                "case {case}: a LIVE delivery carries no replay name, got {parsed}"
            );
        }
    }
}

/// `StartReplay` over the scenario's archive for the window `window` (a pair of
/// epoch seconds), optionally confined to `filters`.
async fn start_replay(
    emulator: &Emulator,
    scenario: &Scenario,
    name: &str,
    window: (i64, i64),
    filters: Option<Vec<String>>,
) -> (u16, Value) {
    let mut destination = json!({ "Arn": scenario.bus_arn });
    if let Some(filters) = filters {
        destination["FilterArns"] = json!(filters);
    }
    events(
        emulator,
        "StartReplay",
        &json!({
            "ReplayName": name,
            "EventSourceArn": scenario.archive_arn,
            "EventStartTime": window.0,
            "EventEndTime": window.1,
            "Destination": destination,
        }),
    )
    .await
}

/// Which rules a row's `Destination.FilterArns` names.
#[derive(Debug, Clone, Copy)]
enum Filters {
    /// No `FilterArns` at all: every rule of the destination bus routes.
    None,
    /// Only the first rule.
    First,
    /// Both rules, named explicitly.
    Both,
}

/// The window a replay selects from the archive, and what it selects.
///
/// The two archived events sit at `OLDER` and `NEWER` seconds before now, so a
/// row can place a window around them, on either bound, or past it.
const OLDER: i64 = -1_200;
const NEWER: i64 = -300;

/// One row of the table below: the case, the window it replays (as offsets from
/// the row's anchor), which rules its `FilterArns` name, how many redeliveries
/// each rule's queue holds afterwards, and the event time the replay reports as
/// the last one it replayed.
type WindowRow = (
    &'static str,
    (i64, i64),
    Filters,
    [usize; 2],
    Option<i64>,
);

/// Which archived events reach which rule's queue, and what the replay reports
/// as the event time of the last event it replayed.
#[tokio::test]
async fn a_replay_redelivers_the_archived_events_its_window_selects() {
    // (case, window, filters, replayed per rule [alpha, beta], EventLastReplayedTime)
    let rows: [WindowRow; 8] = [
        (
            "a window around every archived event",
            (OLDER - 600, NEWER + 240),
            Filters::None,
            [2, 2],
            Some(NEWER),
        ),
        (
            "a window around the older event only",
            (OLDER - 600, OLDER + 600),
            Filters::None,
            [1, 1],
            Some(OLDER),
        ),
        (
            // The window is closed at its start: an event at that instant
            // "occurred between the EventStartTime and EventEndTime".
            "a window starting ON the older event's own time",
            (OLDER, OLDER + 600),
            Filters::None,
            [1, 1],
            Some(OLDER),
        ),
        (
            // And closed at its end, for the same reason.
            "a window ending ON the older event's own time",
            (OLDER - 600, OLDER),
            Filters::None,
            [1, 1],
            Some(OLDER),
        ),
        (
            // One second past that bound selects nothing.
            "a window ending one second before the older event",
            (OLDER - 600, OLDER - 1),
            Filters::None,
            [0, 0],
            None,
        ),
        (
            "a window between the two archived events",
            (OLDER + 60, NEWER - 60),
            Filters::None,
            [0, 0],
            None,
        ),
        (
            // "A list of ARNs for rules to replay events to": the rule the
            // filters do not name receives nothing, even though its pattern
            // matches.
            "FilterArns naming the first rule only",
            (OLDER - 600, NEWER + 240),
            Filters::First,
            [2, 0],
            Some(NEWER),
        ),
        (
            "FilterArns naming both rules",
            (OLDER - 600, NEWER + 240),
            Filters::Both,
            [2, 2],
            Some(NEWER),
        ),
    ];

    for (case, window, filters, expected, last_replayed) in rows {
        let emulator = emulator();
        let scenario = given_scenario(&emulator, "replay").await;
        // ONE anchor for the row: the events are published at `anchor + offset`
        // and the window is built from the same anchor, so a bound that is meant
        // to fall ON an event's own time falls exactly on it.
        let anchor = epoch_now();
        let archived = given_archived(&emulator, &scenario, anchor, &[OLDER, NEWER]).await;
        drain_live_deliveries(&emulator, case, &scenario).await;
        let captured = archived_count(&emulator, &scenario.archive).await;

        let named = match filters {
            Filters::None => None,
            Filters::First => Some(vec![scenario.rules[0].arn.clone()]),
            Filters::Both => Some(scenario.rules.iter().map(|rule| rule.arn.clone()).collect()),
        };
        let replay = "window-probe";
        let (status, body) = start_replay(
            &emulator,
            &scenario,
            replay,
            (anchor + window.0, anchor + window.1),
            named,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: StartReplay answered {status} {body}"
        );
        assert_eq!(
            member(case, &body, "ReplayArn"),
            format!(
                "arn:aws:events:{REGION}:{}:replay/{replay}",
                engine::emulator_account_id()
            ),
            "case {case}: the replay answers an ARN of the modelled form, got {body}"
        );
        assert_eq!(
            member(case, &body, "State"),
            state::REPLAY_STATE_STARTING,
            "case {case}: a started replay is answered in a non-terminal state, got {body}"
        );

        for (rule, expected) in scenario.rules.iter().zip(expected) {
            let bodies = drain(&emulator, &rule.queue).await;
            assert_eq!(
                bodies.len(),
                expected,
                "case {case}: expected {expected} redelivery/redeliveries to rule {}, got \
                 {bodies:?}",
                rule.name
            );
            for body in &bodies {
                let parsed: Value = serde_json::from_str(body).unwrap_or_else(|error| {
                    panic!("case {case}: a redelivered body is not JSON ({error}): {body:?}")
                });
                assert_eq!(
                    parsed[state::REPLAY_NAME_FIELD],
                    json!(replay),
                    "case {case}: a redelivered event carries the replay's name, got {parsed}"
                );
                let id = parsed["id"].as_str().unwrap_or_default().to_string();
                assert!(
                    archived.contains(&id),
                    "case {case}: a redelivered event is one of the ARCHIVED events \
                     ({archived:?}), got {parsed}"
                );
                assert_eq!(
                    parsed["source"],
                    json!(SOURCE),
                    "case {case}: the archived envelope is redelivered unchanged, got {parsed}"
                );
            }
        }

        // "Replayed events are not sent to an archive", which is what the
        // archive's own managed rule reads the replay name for.
        assert_eq!(
            archived_count(&emulator, &scenario.archive).await,
            captured,
            "case {case}: the replayed events must not be archived again"
        );

        // What the replay reports it replayed, read back under its name.
        let (status, body) = events(
            &emulator,
            "DescribeReplay",
            &json!({ "ReplayName": replay }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: DescribeReplay answered {status} {body}"
        );
        let reported = body["EventLastReplayedTime"].as_f64();
        match last_replayed {
            Some(offset) => assert!(
                reported.is_some_and(|at| at == seconds(anchor + offset)),
                "case {case}: expected EventLastReplayedTime at the last replayed event's own \
                 time ({}), got {body}",
                seconds(anchor + offset)
            ),
            None => assert!(
                reported.is_none(),
                "case {case}: a replay that selected no event reports no \
                 EventLastReplayedTime, got {body}"
            ),
        }
    }
}

/// An epoch second as an `awsJson1_1` time stamp carries it.
fn seconds(at: i64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let at = at as f64;
    at
}

/// A rule created AFTER the events were archived still receives them: routing
/// happens when the replay re-publishes, against the rules the destination bus
/// holds then — not against the rules it held when the event was captured.
#[tokio::test]
async fn a_replay_reaches_a_rule_created_after_the_events_were_archived() {
    let emulator = emulator();
    let scenario = given_scenario(&emulator, "late").await;
    let anchor = epoch_now();
    let archived = given_archived(&emulator, &scenario, anchor, &[OLDER]).await;
    drain_live_deliveries(&emulator, "a later rule", &scenario).await;

    let later = given_rule(&emulator, &scenario.bus, "late-gamma").await;
    let (status, body) = start_replay(
        &emulator,
        &scenario,
        "late-replay",
        (anchor + OLDER - 600, anchor + NEWER),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case a later rule: StartReplay answered {status} {body}"
    );

    let bodies = drain(&emulator, &later.queue).await;
    assert_eq!(
        bodies.len(),
        1,
        "case a later rule: the rule created after the archive still receives the replayed \
         event, got {bodies:?}"
    );
    let parsed: Value = serde_json::from_str(&bodies[0]).unwrap_or_else(|error| {
        panic!("case a later rule: the redelivered body is not JSON ({error})")
    });
    assert_eq!(
        parsed["id"],
        json!(archived[0]),
        "case a later rule: the redelivered event is the archived one, got {parsed}"
    );
    assert_eq!(
        parsed[state::REPLAY_NAME_FIELD],
        json!("late-replay"),
        "case a later rule: the redelivered event carries the replay's name, got {parsed}"
    );
}

/// A replay's own progress: answered in a non-terminal state, reaching
/// `COMPLETED` a moment later with a `ReplayEndTime`, and reported the same way
/// by every later read.
#[tokio::test]
async fn a_replay_reaches_completed_and_stays_there() {
    let emulator = emulator();
    let scenario = given_scenario(&emulator, "progress").await;
    let anchor = epoch_now();
    let _ = given_archived(&emulator, &scenario, anchor, &[OLDER]).await;
    let replay = "progress-replay";
    let (status, body) = start_replay(
        &emulator,
        &scenario,
        replay,
        (anchor + OLDER - 600, anchor + NEWER),
        None,
    )
    .await;
    assert_eq!(status, 200, "StartReplay answered {status} {body}");
    let started = member("progress", &body, "ReplayArn");

    // Straight after the call the transient state is still observable.
    let (status, body) = events(
        &emulator,
        "DescribeReplay",
        &json!({ "ReplayName": replay }),
    )
    .await;
    assert_eq!(status, 200, "DescribeReplay answered {status} {body}");
    assert_eq!(
        body["State"],
        json!(state::REPLAY_STATE_STARTING),
        "case straight after the start: the replay is still in its transient state, got {body}"
    );
    assert!(
        body["ReplayEndTime"].is_null(),
        "case straight after the start: a replay that has not finished reports no \
         ReplayEndTime, got {body}"
    );
    let rows = [
        ("ReplayName", json!(replay)),
        ("ReplayArn", json!(started)),
        ("EventSourceArn", json!(scenario.archive_arn)),
    ];
    for (name, expected) in rows {
        assert_eq!(
            body[name], expected,
            "case straight after the start: expected {name} to be {expected}, got {body}"
        );
    }
    assert_eq!(
        body["Destination"]["Arn"],
        json!(scenario.bus_arn),
        "case straight after the start: the replay reports the destination it was started \
         with, got {body}"
    );

    // The replay reaches its terminal state on its own: looked for by polling the
    // replay's own progress, not by waiting a fixed time. Then every read reports
    // that state and the same end time.
    poll_until_state(&emulator, "the replay's progress", replay, state::REPLAY_STATE_COMPLETED)
        .await;
    let mut seen: Vec<Value> = Vec::new();
    for read in 0..2 {
        let (status, body) = events(
            &emulator,
            "DescribeReplay",
            &json!({ "ReplayName": replay }),
        )
        .await;
        assert_eq!(status, 200, "read {read}: DescribeReplay answered {status} {body}");
        assert_eq!(
            body["State"],
            json!(state::REPLAY_STATE_COMPLETED),
            "read {read}: the replay reaches its terminal state on its own, got {body}"
        );
        assert!(
            body["ReplayEndTime"].as_f64().is_some(),
            "read {read}: a completed replay reports when it ended, got {body}"
        );
        seen.push(body["ReplayEndTime"].clone());
    }
    assert_eq!(
        seen[0], seen[1],
        "two reads of one replay report the same end time, got {seen:?}"
    );
}

/// Every request no replay can be started for, each refused with the error for
/// ITS cause.
#[tokio::test]
async fn start_replay_refuses_a_request_no_replay_can_be_started_for() {
    let emulator = emulator();
    let scenario = given_scenario(&emulator, "refusal").await;
    let other = given_scenario(&emulator, "other").await;
    let anchor = epoch_now();
    let _ = given_archived(&emulator, &scenario, anchor, &[OLDER]).await;
    let account = engine::emulator_account_id();
    let window = json!({
        "EventStartTime": anchor + OLDER - 600,
        "EventEndTime": anchor + NEWER,
    });
    // The replay that already exists, so the duplicate-name row has one to collide
    // with.
    let (status, body) = start_replay(
        &emulator,
        &scenario,
        "taken",
        (anchor + OLDER - 600, anchor + NEWER),
        None,
    )
    .await;
    assert_eq!(status, 200, "the first StartReplay answered {status} {body}");

    // (case, the member that is wrong, the error code it is refused with)
    let rows: [(&str, Value, &str); 7] = [
        (
            // An `EventSourceArn` that resolves to no archive is refused as the
            // PARAMETER it is, not as a resource that went missing.
            "an EventSourceArn naming no archive",
            json!({
                "ReplayName": "missing-archive",
                "EventSourceArn": format!("arn:aws:events:{REGION}:{account}:archive/nope"),
                "Destination": { "Arn": scenario.bus_arn },
            }),
            "ValidationException",
        ),
        (
            "an EventSourceArn that names no archive resource at all",
            json!({
                "ReplayName": "not-an-archive",
                "EventSourceArn": scenario.bus_arn,
                "Destination": { "Arn": scenario.bus_arn },
            }),
            "ValidationException",
        ),
        (
            "a ReplayName already held",
            json!({
                "ReplayName": "taken",
                "EventSourceArn": scenario.archive_arn,
                "Destination": { "Arn": scenario.bus_arn },
            }),
            "ResourceAlreadyExistsException",
        ),
        (
            "a Destination.Arn that is not an event bus ARN",
            json!({
                "ReplayName": "not-a-bus",
                "EventSourceArn": scenario.archive_arn,
                "Destination": { "Arn": format!("arn:aws:events:{REGION}:{account}:rule/x") },
            }),
            "ValidationException",
        ),
        (
            // "You can replay events only to the event bus specified to create
            // the archive".
            "a Destination.Arn naming a bus the archive was not created from",
            json!({
                "ReplayName": "other-bus",
                "EventSourceArn": scenario.archive_arn,
                "Destination": { "Arn": other.bus_arn },
            }),
            "ValidationException",
        ),
        (
            "a FilterArns naming a rule on another bus",
            json!({
                "ReplayName": "other-rule",
                "EventSourceArn": scenario.archive_arn,
                "Destination": {
                    "Arn": scenario.bus_arn,
                    "FilterArns": [other.rules[0].arn.clone()],
                },
            }),
            "ValidationException",
        ),
        (
            "a FilterArns naming no rule at all",
            json!({
                "ReplayName": "no-rule",
                "EventSourceArn": scenario.archive_arn,
                "Destination": {
                    "Arn": scenario.bus_arn,
                    "FilterArns": [format!("arn:aws:events:{REGION}:{account}:rule/nope")],
                },
            }),
            "ValidationException",
        ),
    ];

    for (case, mut request, expected) in rows {
        for (name, value) in window.as_object().cloned().unwrap_or_default() {
            request[name] = value;
        }
        let (status, body) = events(&emulator, "StartReplay", &request).await;
        assert_eq!(
            error_code(&body),
            expected,
            "case {case}: expected {expected}, got {status} {body}"
        );
    }

    // A window whose end does not come after its start is refused on its own,
    // over both forms of it.
    for (case, start, end) in [
        ("an EventEndTime before the EventStartTime", NEWER, OLDER),
        ("an EventEndTime equal to the EventStartTime", OLDER, OLDER),
    ] {
        let (status, body) = start_replay(
            &emulator,
            &scenario,
            "bad-window",
            (anchor + start, anchor + end),
            None,
        )
        .await;
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {status} {body}"
        );
    }

    // And a read of a replay nobody started names the missing replay.
    let (status, body) = events(
        &emulator,
        "DescribeReplay",
        &json!({ "ReplayName": "nobody-started-this" }),
    )
    .await;
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case a replay nobody started: expected ResourceNotFoundException, got {status} {body}"
    );
}
