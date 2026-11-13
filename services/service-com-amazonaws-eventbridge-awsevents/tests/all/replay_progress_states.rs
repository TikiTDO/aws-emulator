// SPDX-License-Identifier: CC0-1.0

//! A replay's own progress, and what a `CancelReplay` may still stop, end to end
//! over the wire (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Op>`)
//! through the real `MultiServiceRouter`.
//!
//! The RULE asserted here: a started replay stays in a state a cancel can stop —
//! through EVERY non-terminal state of its progression, "Starting" and then
//! "Running" — until it has finished its window or a cancel arrives, so a
//! `CancelReplay` issued while a client is monitoring the replay is ACCEPTED
//! (HTTP 200, the replay's own ARN, `State = CANCELLING`) and the replay then
//! settles in the terminal `CANCELLED` with a `ReplayEndTime`, never in
//! `COMPLETED`; `IllegalStatusException` is reserved for a replay whose state this
//! call cannot change — "a replay can be canceled only when the state is Running
//! or Starting" — i.e. one already terminal or already cancelling.
//!
//! The table spans the class rather than one recorded case: its rows differ in
//! WHICH non-terminal state the cancel is issued in (`STARTING`, `RUNNING`), in
//! how many archived events the replay's window selected (two, one, none at all),
//! and in the form and length of the replay's name — every character class the
//! modelled `ReplayName` pattern `^[\.\-_A-Za-z0-9]+$` admits, and its maximum
//! modelled length of 64 (`smithy_query(query="operation",operation="CancelReplay")`).
//! A control replay nobody stops is watched through its whole progression, so the
//! states a replay passes on its own are asserted as well as the ones a cancel
//! puts it in.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for, and so the region segment of
/// every ARN these tests build.
const REGION: &str = "eu-central-1";

/// How long a poll of a replay's progress waits, at the OUTSIDE, for a state the
/// clock owes it: generous, because what is asserted is the state the replay
/// converges on and never how fast the host got it there.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll of a replay's progress waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The emulator: one router carrying only this service over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    Emulator { router }
}

/// This emulator's account id — the account segment of every ARN it mints.
fn account() -> &'static str {
    engine::emulator_account_id()
}

async fn call(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/events/aws4_request, \
         SignedHeaders=host, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace; the empty string on a
/// success.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The `message` an error body carries.
fn message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Epoch seconds now, which is what an `awsJson1_1` time stamp carries.
fn epoch_now() -> i64 {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now()).secs()
}

/// The `State` `DescribeReplay` reports for `name`.
async fn reported_state(emulator: &Emulator, case: &str, name: &str) -> String {
    let (status, body) = call(emulator, "DescribeReplay", &json!({ "ReplayName": name })).await;
    assert_eq!(
        status, 200,
        "case {case}: DescribeReplay answered {status} {body}"
    );
    body["State"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: no State in {body}"))
        .to_string()
}

/// The replay `name`, read back once its `State` is `wanted`: the read is
/// repeated until it is, or until [`SETTLE_DEADLINE`] passes, so what the test
/// waits for is the CONDITION and not a span. A replay that settles in any OTHER
/// terminal state fails at once — no later read can move it to `wanted`.
async fn poll_until_state(emulator: &Emulator, case: &str, name: &str, wanted: &str) -> Value {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) = call(emulator, "DescribeReplay", &json!({ "ReplayName": name })).await;
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
            "case {case}: expected the replay to reach {wanted}, but it settled in {reported:?}, \
             which nothing leaves: {body}"
        );
        assert!(
            Instant::now() < deadline,
            "case {case}: expected the replay to reach {wanted}, got {reported:?} up to the \
             deadline: {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A bus and an archive on it, carrying `events` events whose own event time lies
/// in the window the replays below select; answers (bus ARN, archive ARN).
async fn given_archive(emulator: &Emulator, label: &str, events: usize) -> (String, String) {
    let bus = format!("{label}-bus");
    let (status, body) = call(emulator, "CreateEventBus", &json!({ "Name": bus })).await;
    assert_eq!(
        status, 200,
        "case {label}: CreateEventBus answered {status} {body}"
    );
    let bus_arn = body["EventBusArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: no EventBusArn in {body}"))
        .to_string();

    let archive = format!("{label}-archive");
    let (status, body) = call(
        emulator,
        "CreateArchive",
        &json!({ "ArchiveName": archive, "EventSourceArn": bus_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label}: CreateArchive answered {status} {body}"
    );
    let archive_arn = body["ArchiveArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: no ArchiveArn in {body}"))
        .to_string();

    for at in 0..events {
        let (status, body) = call(
            emulator,
            "PutEvents",
            &json!({
                "Entries": [{
                    "EventBusName": bus,
                    "Source": "com.demo.orders",
                    "DetailType": "Order Placed",
                    "Detail": format!("{{\"id\":{at}}}"),
                    "Time": epoch_now() - 600 - i64::try_from(at).unwrap_or_default(),
                }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: PutEvents answered {status} {body}"
        );
    }
    (bus_arn, archive_arn)
}

/// A `StartReplay` over `archive_arn` onto `bus_arn`, for a window that brackets
/// every archived event.
async fn start_replay(
    emulator: &Emulator,
    bus_arn: &str,
    archive_arn: &str,
    name: &str,
) -> (u16, Value) {
    let anchor = epoch_now();
    call(
        emulator,
        "StartReplay",
        &json!({
            "ReplayName": name,
            "EventSourceArn": archive_arn,
            "EventStartTime": anchor - 3_600,
            "EventEndTime": anchor,
            "Destination": { "Arn": bus_arn },
        }),
    )
    .await
}

/// A replay is cancellable in EVERY non-terminal state it passes through, and a
/// cancel accepted there makes it settle cancelled rather than completed.
#[tokio::test]
async fn a_replay_is_cancellable_in_every_state_it_passes_before_it_finishes() {
    let emulator = emulator();

    // (case, the replay's name, the non-terminal state the cancel is issued in,
    //  how many archived events its window selected)
    let rows: [(&str, &str, &str, usize); 4] = [
        (
            "cancelled straight after the start",
            "cancel-while-starting",
            state::REPLAY_STATE_STARTING,
            2,
        ),
        (
            "cancelled once the replay is running",
            "cancel.while_running-9",
            state::REPLAY_STATE_RUNNING,
            1,
        ),
        (
            "cancelled while running, window selected nothing",
            "EmptyWindow.cancelled_while-RUNNING",
            state::REPLAY_STATE_RUNNING,
            0,
        ),
        (
            "cancelled under a name of the greatest modelled length",
            // 64 characters: the maximum `ReplayName` the model admits.
            "a64.character_name-that-is-exactly-the-longest-one-the-model-adm",
            state::REPLAY_STATE_STARTING,
            3,
        ),
    ];

    for (index, (case, replay, cancel_in, events)) in rows.into_iter().enumerate() {
        assert!(
            replay.len() <= 64,
            "case {case}: the row's name is longer than the modelled maximum, got {}",
            replay.len()
        );
        // The archive and bus of this row, named for the row rather than for the
        // replay: an `ArchiveName` is modelled shorter than a `ReplayName`.
        let (bus_arn, archive_arn) = given_archive(&emulator, &format!("row-{index}"), events).await;
        let (status, body) = start_replay(&emulator, &bus_arn, &archive_arn, replay).await;
        assert_eq!(
            status, 200,
            "case {case}: StartReplay answered {status} {body}"
        );
        assert_eq!(
            body["State"],
            json!(state::REPLAY_STATE_STARTING),
            "case {case}: a started replay is answered in a non-terminal state, got {body}"
        );

        // The state the row cancels IN is reached first, so the cancel below
        // really is a cancel of a replay in THAT state.
        poll_until_state(&emulator, case, replay, cancel_in).await;

        let expected_arn = format!("arn:aws:events:{REGION}:{}:replay/{replay}", account());
        let (status, body) = call(&emulator, "CancelReplay", &json!({ "ReplayName": replay })).await;
        assert_eq!(
            status, 200,
            "case {case}: a cancel of a replay in {cancel_in} is accepted, got {status} {body}"
        );
        assert_eq!(
            body["ReplayArn"],
            json!(expected_arn),
            "case {case}: expected the replay's own ARN {expected_arn}, got {body}"
        );
        assert_eq!(
            body["State"],
            json!(state::REPLAY_STATE_CANCELLING),
            "case {case}: an accepted cancel answers the transient cancelling state, got {body}"
        );

        // Straight after the cancel the transient state is what a read reports,
        // and the replay has not ended yet.
        let (status, body) = call(&emulator, "DescribeReplay", &json!({ "ReplayName": replay })).await;
        assert_eq!(
            status, 200,
            "case {case}: DescribeReplay answered {status} {body}"
        );
        assert_eq!(
            body["State"],
            json!(state::REPLAY_STATE_CANCELLING),
            "case {case}: the cancel is still under way, got {body}"
        );
        assert!(
            body["ReplayEndTime"].is_null(),
            "case {case}: a replay that has not settled reports no ReplayEndTime, got {body}"
        );

        // A second cancel is refused while the first is still under way: this is
        // a state the call cannot change.
        let (status, body) = call(&emulator, "CancelReplay", &json!({ "ReplayName": replay })).await;
        assert_eq!(
            status, 400,
            "case {case}: a cancel of a cancelling replay is refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "IllegalStatusException",
            "case {case}: expected IllegalStatusException, got {status} {body}"
        );
        let expected = state::replay_not_cancellable(replay);
        assert_eq!(
            message(&body),
            expected,
            "case {case}: expected {expected:?}, got {body}"
        );

        // And the replay settles CANCELLED — never COMPLETED, which the poll
        // treats as a failure because nothing leaves it — with an end time.
        let settled = poll_until_state(
            &emulator,
            case,
            replay,
            state::REPLAY_STATE_CANCELLED,
        )
        .await;
        assert!(
            settled["ReplayEndTime"].as_f64().is_some(),
            "case {case}: a replay that has settled reports when it ended, got {settled}"
        );
        let again = reported_state(&emulator, case, replay).await;
        assert_eq!(
            again,
            state::REPLAY_STATE_CANCELLED,
            "case {case}: a cancelled replay stays cancelled, got {again:?}"
        );
    }
}

/// The control: a replay nobody stops passes through every state of its
/// progression on its own, in order, and only then is it past cancelling.
#[tokio::test]
async fn a_replay_nobody_stops_passes_starting_then_running_then_completed() {
    let emulator = emulator();
    let (bus_arn, archive_arn) = given_archive(&emulator, "progression", 2).await;
    let replay = "left-alone";
    let (status, body) = start_replay(&emulator, &bus_arn, &archive_arn, replay).await;
    assert_eq!(status, 200, "StartReplay answered {status} {body}");

    // Every DISTINCT state the replay is seen in, in the order it was seen, up to
    // the state it settles in.
    let deadline = Instant::now() + SETTLE_DEADLINE;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let reported = reported_state(&emulator, "the progression", replay).await;
        if seen.last() != Some(&reported) {
            seen.push(reported.clone());
        }
        if state::is_terminal_replay_state(&reported) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case the progression: the replay never settled, seen {seen:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    assert_eq!(
        seen,
        vec![
            state::REPLAY_STATE_STARTING.to_string(),
            state::REPLAY_STATE_RUNNING.to_string(),
            state::REPLAY_STATE_COMPLETED.to_string(),
        ],
        "case the progression: a replay nobody stops is seen starting, then running, then \
         completed, got {seen:?}"
    );

    // And once it has finished its window there is nothing left to cancel.
    let (status, body) = call(&emulator, "CancelReplay", &json!({ "ReplayName": replay })).await;
    assert_eq!(
        status, 400,
        "case a replay that finished its window: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "IllegalStatusException",
        "case a replay that finished its window: expected IllegalStatusException, got {body}"
    );
    let expected = state::replay_not_cancellable(replay);
    assert_eq!(
        message(&body),
        expected,
        "case a replay that finished its window: expected {expected:?}, got {body}"
    );
    // The refused cancel changed nothing: the replay is still the completed one.
    let after = reported_state(&emulator, "after the refused cancel", replay).await;
    assert_eq!(
        after,
        state::REPLAY_STATE_COMPLETED,
        "case after the refused cancel: the replay is unchanged, got {after:?}"
    );
}
