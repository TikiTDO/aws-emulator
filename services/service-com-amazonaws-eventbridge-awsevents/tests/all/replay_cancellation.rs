// SPDX-License-Identifier: CC0-1.0

//! Stopping a replay, and the two ways a `StartReplay` can be refused for its
//! own parameters, end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter`.
//!
//! The RULES asserted here:
//!
//! * `CancelReplay` STOPS a replay that has not settled: it answers the replay's
//!   own ARN in `CANCELLING`, and the replay then settles in `CANCELLED` — not
//!   `COMPLETED` — with the `ReplayEndTime` every terminal replay carries, which
//!   every later read reports identically;
//! * a replay can be canceled "only when the state is Running or Starting"
//!   (`IllegalStatusException`), so a replay that is already cancelling or has
//!   already settled is refused with that modelled error, while a name no replay
//!   was started under is the modelled `ResourceNotFoundException`; a cancelled
//!   replay keeps its name, so `StartReplay` reusing it is
//!   `ResourceAlreadyExistsException`;
//! * a `StartReplay` whose window is not open — its `EventEndTime` not later than
//!   its `EventStartTime` — is refused with a `ValidationException` whose message
//!   is fixed text naming the member and the rule, with no variable part;
//! * a `StartReplay` whose `EventSourceArn` resolves to no archive this account
//!   holds is refused the same way, with the member and the archive the ARN named,
//!   and starts no replay at all — neither a read nor a listing finds one;
//! * `ListReplays` reports the replays from the SAME records the reads do, under
//!   at most ONE filter ("filter parameters are exclusive"), one page at a time.
//!
//! Each table spans the class rather than one recorded case: the window rows
//! differ in HOW the window is closed (end before start, by an hour and by a
//! second, and end equal to start) and carry the open window as the control; the
//! event-source rows differ in the form of the name the ARN carries and in the
//! resource type of the ARN itself; the refusal rows differ in the state the
//! replay was found in.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for, and so the region segment of
/// every ARN these tests build.
const REGION: &str = "ap-southeast-2";

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

/// How long a poll of a replay's progress waits, at the OUTSIDE, for the state
/// the clock owes it: generous, because what is asserted is the state the replay
/// converges on and never how fast the host got it there.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll of a replay's progress waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

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

/// A bus and an `ENABLED` archive on it, with one event of the archive's own in
/// it; answers (bus ARN, archive name, archive ARN).
async fn given_archive(emulator: &Emulator, label: &str) -> (String, String, String) {
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

    let (status, body) = call(
        emulator,
        "PutEvents",
        &json!({
            "Entries": [{
                "EventBusName": bus,
                "Source": "com.demo.orders",
                "DetailType": "Order Placed",
                "Detail": "{\"id\":1}",
                "Time": epoch_now() - 600,
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label}: PutEvents answered {status} {body}"
    );
    (bus_arn, archive, archive_arn)
}

/// The replay `name`, read back once its `State` is `wanted`: the read is
/// repeated until it is, or until [`SETTLE_DEADLINE`] passes, so what the test
/// waits for is the CONDITION and not a span. A replay that settles in any OTHER
/// terminal state fails at once — no further read can move it to `wanted`.
async fn poll_until_state(emulator: &Emulator, case: &str, name: &str, wanted: &str) -> Value {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
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
            "case {case}: expected the replay to reach {wanted}, but it settled in \
             {reported:?}, which nothing leaves: {body}"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "case {case}: expected the replay to reach {wanted}, got {reported:?} up to the \
             deadline: {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every replay `ListReplays` reports under `filter`.
async fn listed(emulator: &Emulator, case: &str, filter: &Value) -> Vec<Value> {
    let (status, body) = call(emulator, "ListReplays", filter).await;
    assert_eq!(
        status, 200,
        "case {case}: ListReplays answered {status} {body}"
    );
    body["Replays"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: no Replays in {body}"))
        .clone()
}

/// A `StartReplay` over `archive_arn` onto `bus_arn`, for a window that brackets
/// the archived event.
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

/// A replay in flight reaches `CANCELLED` and stays there, and the name it held
/// stays held.
#[tokio::test]
async fn a_cancelled_replay_settles_cancelled_and_stays_terminal() {
    let emulator = emulator();
    let (bus_arn, _, archive_arn) = given_archive(&emulator, "cancel").await;
    let replay = "cancel-me";

    let (status, body) = start_replay(&emulator, &bus_arn, &archive_arn, replay).await;
    assert_eq!(
        status, 200,
        "case the start: StartReplay answered {status} {body}"
    );
    let expected_arn = format!("arn:aws:events:{REGION}:{}:replay/{replay}", account());
    assert_eq!(
        body["ReplayArn"],
        json!(expected_arn),
        "case the start: expected the replay's ARN {expected_arn}, got {body}"
    );

    let (status, body) = call(&emulator, "CancelReplay", &json!({ "ReplayName": replay })).await;
    assert_eq!(
        status, 200,
        "case the cancel: CancelReplay answered {status} {body}"
    );
    assert_eq!(
        body["State"],
        json!(state::REPLAY_STATE_CANCELLING),
        "case the cancel: a cancel accepted on a replay in flight answers the transient \
         cancelling state, got {body}"
    );
    assert_eq!(
        body["ReplayArn"],
        json!(expected_arn),
        "case the cancel: expected the replay's own ARN {expected_arn}, got {body}"
    );

    // Straight after the cancel the replay is still in its transient state, and
    // has no end time yet.
    let (status, body) = call(&emulator, "DescribeReplay", &json!({ "ReplayName": replay })).await;
    assert_eq!(
        status, 200,
        "case straight after the cancel: DescribeReplay answered {status} {body}"
    );
    assert_eq!(
        body["State"],
        json!(state::REPLAY_STATE_CANCELLING),
        "case straight after the cancel: the replay is still cancelling, got {body}"
    );
    assert!(
        body["ReplayEndTime"].is_null(),
        "case straight after the cancel: a replay that has not settled reports no \
         ReplayEndTime, got {body}"
    );

    // And then it is CANCELLED — never COMPLETED — with an end time every later
    // read agrees on. The wait is for that state, not for a span.
    poll_until_state(
        &emulator,
        "after the cancel",
        replay,
        state::REPLAY_STATE_CANCELLED,
    )
    .await;
    let mut seen: Vec<Value> = Vec::new();
    for read in ["first read", "second read"] {
        let (status, body) =
            call(&emulator, "DescribeReplay", &json!({ "ReplayName": replay })).await;
        assert_eq!(
            status, 200,
            "{read}: DescribeReplay answered {status} {body}"
        );
        assert_eq!(
            body["State"],
            json!(state::REPLAY_STATE_CANCELLED),
            "{read}: a cancelled replay settles cancelled, not completed, got {body}"
        );
        assert!(
            body["ReplayEndTime"].as_f64().is_some(),
            "{read}: a replay that has settled reports when it ended, got {body}"
        );
        seen.push(body["ReplayEndTime"].clone());
    }
    assert_eq!(
        seen[0], seen[1],
        "the ReplayEndTime of a settled replay does not move between reads, got {seen:?}"
    );

    // A second cancel is refused: the replay is terminal.
    let (status, body) = call(&emulator, "CancelReplay", &json!({ "ReplayName": replay })).await;
    assert_eq!(
        error_code(&body),
        "IllegalStatusException",
        "case cancelling a terminal replay: expected IllegalStatusException, got {status} {body}"
    );
    let expected = state::replay_not_cancellable(replay);
    assert_eq!(
        message(&body),
        expected,
        "case cancelling a terminal replay: expected {expected:?}, got {body}"
    );

    // The listing reports the replay the same way the read does.
    let (status, listed) = call(&emulator, "ListReplays", &json!({ "NamePrefix": replay })).await;
    assert_eq!(status, 200, "ListReplays answered {status} {listed}");
    let replays = listed["Replays"]
        .as_array()
        .unwrap_or_else(|| panic!("no Replays in {listed}"));
    assert_eq!(
        replays.len(),
        1,
        "the prefix of one replay's name lists exactly that replay, got {listed}"
    );
    assert_eq!(
        replays[0]["State"],
        json!(state::REPLAY_STATE_CANCELLED),
        "ListReplays and DescribeReplay agree on the state, got {listed}"
    );
    assert_eq!(
        replays[0]["ReplayEndTime"], seen[0],
        "ListReplays and DescribeReplay agree on the end time, got {listed}"
    );

    // And the name is still held, so it cannot be started again.
    let (status, body) = start_replay(&emulator, &bus_arn, &archive_arn, replay).await;
    assert_eq!(
        error_code(&body),
        "ResourceAlreadyExistsException",
        "case reusing a cancelled replay's name: expected ResourceAlreadyExistsException, \
         got {status} {body}"
    );
}

/// Which state a `CancelReplay` is refused in, and with which modelled error.
#[tokio::test]
async fn cancel_replay_refuses_a_replay_it_cannot_stop() {
    let emulator = emulator();
    let (bus_arn, _, archive_arn) = given_archive(&emulator, "refuse").await;

    // A replay left alone until the clock completed it.
    let (status, body) = start_replay(&emulator, &bus_arn, &archive_arn, "completed").await;
    assert_eq!(status, 200, "the completed replay's start: {status} {body}");
    // One cancelled and left to settle.
    let (status, body) = start_replay(&emulator, &bus_arn, &archive_arn, "settled").await;
    assert_eq!(status, 200, "the settled replay's start: {status} {body}");
    let (status, body) = call(
        &emulator,
        "CancelReplay",
        &json!({ "ReplayName": "settled" }),
    )
    .await;
    assert_eq!(status, 200, "the settled replay's cancel: {status} {body}");
    // Each of those two is left until its own record says it has settled: the one
    // nobody stopped runs its whole progression to COMPLETED, the cancelled one
    // settles CANCELLED.
    poll_until_state(
        &emulator,
        "the replay the clock completed",
        "completed",
        state::REPLAY_STATE_COMPLETED,
    )
    .await;
    poll_until_state(
        &emulator,
        "the replay that settled cancelled",
        "settled",
        state::REPLAY_STATE_CANCELLED,
    )
    .await;
    // And one cancelled just now, so it is still CANCELLING.
    let (status, body) = start_replay(&emulator, &bus_arn, &archive_arn, "cancelling").await;
    assert_eq!(status, 200, "the cancelling replay's start: {status} {body}");
    let (status, body) = call(
        &emulator,
        "CancelReplay",
        &json!({ "ReplayName": "cancelling" }),
    )
    .await;
    assert_eq!(status, 200, "the cancelling replay's cancel: {status} {body}");

    // (case, the replay named, the state it is in now, the error code)
    let rows: [(&str, &str, Option<&str>, &str); 4] = [
        (
            "a name no replay was started under",
            "nobody-started-this",
            None,
            "ResourceNotFoundException",
        ),
        (
            "a replay the clock already completed",
            "completed",
            Some(state::REPLAY_STATE_COMPLETED),
            "IllegalStatusException",
        ),
        (
            "a replay that has already settled cancelled",
            "settled",
            Some(state::REPLAY_STATE_CANCELLED),
            "IllegalStatusException",
        ),
        (
            "a replay that is already cancelling",
            "cancelling",
            Some(state::REPLAY_STATE_CANCELLING),
            "IllegalStatusException",
        ),
    ];

    for (case, replay, found_in, expected_code) in rows {
        // The state the row says the replay is in is the state it reports, so the
        // refusal below really is the refusal for THAT state.
        if let Some(found_in) = found_in {
            let (status, body) =
                call(&emulator, "DescribeReplay", &json!({ "ReplayName": replay })).await;
            assert_eq!(
                body["State"],
                json!(found_in),
                "case {case}: expected the replay to be in {found_in}, got {status} {body}"
            );
        }
        let (status, body) =
            call(&emulator, "CancelReplay", &json!({ "ReplayName": replay })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case}: expected {expected_code}, got {status} {body}"
        );
        let expected = if found_in.is_some() {
            state::replay_not_cancellable(replay)
        } else {
            state::no_such_replay(replay)
        };
        assert_eq!(
            message(&body),
            expected,
            "case {case}: expected {expected:?}, got {body}"
        );
    }
}

/// How a `StartReplay` whose window is not open is refused: one fixed message,
/// whichever way the window is closed, and the open window is accepted.
#[tokio::test]
async fn start_replay_refuses_a_window_whose_end_is_not_after_its_start() {
    let emulator = emulator();
    let (bus_arn, _, archive_arn) = given_archive(&emulator, "window").await;
    let anchor = epoch_now();

    // The message has NO variable part: it names the member refused and the rule
    // the two members have to satisfy, not their values.
    let expected = "Parameter EventEndTime is not valid. \
                    Reason: EventStartTime must be before EventEndTime.";

    // (case, EventStartTime, EventEndTime, whether a replay starts)
    let rows: [(&str, i64, i64, bool); 4] = [
        ("an end an hour before the start", anchor, anchor - 3_600, false),
        ("an end one second before the start", anchor, anchor - 1, false),
        ("an end equal to the start", anchor, anchor, false),
        ("an end one second after the start", anchor - 1, anchor, true),
    ];

    for (index, (case, start, end, accepted)) in rows.into_iter().enumerate() {
        let name = format!("window-{index}");
        let (status, body) = call(
            &emulator,
            "StartReplay",
            &json!({
                "ReplayName": name,
                "EventSourceArn": archive_arn,
                "EventStartTime": start,
                "EventEndTime": end,
                "Destination": { "Arn": bus_arn },
            }),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case}: an open window starts a replay, got {status} {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {status} {body}"
        );
        assert_eq!(
            message(&body),
            expected,
            "case {case}: expected {expected:?}, got {body}"
        );
        // The refused call started nothing.
        let (status, body) =
            call(&emulator, "DescribeReplay", &json!({ "ReplayName": name })).await;
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case}: a refused StartReplay leaves no replay behind, got {status} {body}"
        );
        let found = listed(&emulator, case, &json!({ "NamePrefix": name })).await;
        assert!(
            found.is_empty(),
            "case {case}: a refused StartReplay leaves nothing to list, got {found:?}"
        );
    }
}

/// How a `StartReplay` whose `EventSourceArn` resolves to no archive is refused:
/// the member and the archive the ARN named, and no replay behind it.
#[tokio::test]
async fn start_replay_refuses_an_event_source_that_resolves_to_no_archive() {
    let emulator = emulator();
    let (bus_arn, archive, archive_arn) = given_archive(&emulator, "source").await;
    let anchor = epoch_now();
    let arn = |resource: &str| format!("arn:aws:events:{REGION}:{}:{resource}", account());

    // (case, the EventSourceArn, the reason clause it is refused with)
    let missing = format!("{archive}-does-not-exist");
    let rows: [(&str, String, String); 5] = [
        (
            "an archive ARN whose name has a suffix no archive carries",
            arn(&format!("archive/{missing}")),
            format!("Archive {missing} does not exist."),
        ),
        (
            "an archive ARN naming no archive at all",
            arn("archive/nope"),
            "Archive nope does not exist.".to_string(),
        ),
        (
            // `ReplayName`'s own pattern admits these characters, so an archive
            // name carrying them is refused by the same words as any other.
            "an archive ARN whose name carries dots, dashes and underscores",
            arn("archive/dots.and-dashes_1"),
            "Archive dots.and-dashes_1 does not exist.".to_string(),
        ),
        (
            "an event bus ARN, which names no archive resource",
            bus_arn.clone(),
            format!("{bus_arn:?} is not the ARN of an archive."),
        ),
        (
            "a rule ARN, which names no archive resource either",
            arn("rule/some-rule"),
            format!("{:?} is not the ARN of an archive.", arn("rule/some-rule")),
        ),
    ];

    for (index, (case, source, reason)) in rows.into_iter().enumerate() {
        let name = format!("source-{index}");
        let (status, body) = call(
            &emulator,
            "StartReplay",
            &json!({
                "ReplayName": name,
                "EventSourceArn": source,
                "EventStartTime": anchor - 3_600,
                "EventEndTime": anchor,
                "Destination": { "Arn": bus_arn },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {status} {body}"
        );
        let expected = format!("Parameter EventSourceArn is not valid. Reason: {reason}");
        assert_eq!(
            message(&body),
            expected,
            "case {case}: expected {expected:?}, got {body}"
        );
        // The refused call started nothing.
        let (status, body) =
            call(&emulator, "DescribeReplay", &json!({ "ReplayName": name })).await;
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case}: a refused StartReplay leaves no replay behind, got {status} {body}"
        );
        let found = listed(&emulator, case, &json!({ "NamePrefix": name })).await;
        assert!(
            found.is_empty(),
            "case {case}: a refused StartReplay leaves nothing to list, got {found:?}"
        );
    }

    // The positive control: the archive's real ARN starts a replay.
    let (status, body) = start_replay(&emulator, &bus_arn, &archive_arn, "the-real-one").await;
    assert_eq!(
        status, 200,
        "case the archive's own ARN: StartReplay answered {status} {body}"
    );
}

/// Which replays a `ListReplays` reports, under each filter it offers and none,
/// and how it refuses more than one filter at a time.
#[tokio::test]
async fn list_replays_reports_the_replays_one_filter_selects() {
    let emulator = emulator();
    let (bus_arn, _, archive_arn) = given_archive(&emulator, "listing").await;
    // A second archive on the same bus, so an `EventSourceArn` filter has a
    // source to exclude as well as one to select.
    let (status, body) = call(
        &emulator,
        "CreateArchive",
        &json!({ "ArchiveName": "listing-other-archive", "EventSourceArn": bus_arn }),
    )
    .await;
    assert_eq!(status, 200, "the second CreateArchive answered {status} {body}");
    let other_archive_arn = body["ArchiveArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no ArchiveArn in {body}"))
        .to_string();

    // Two replays over the first archive: one left to complete and one cancelled,
    // both settled by the polls below.
    for name in ["alpha-done", "alpha-settled"] {
        let (status, body) = start_replay(&emulator, &bus_arn, &archive_arn, name).await;
        assert_eq!(status, 200, "the start of {name} answered {status} {body}");
    }
    let (status, body) = call(
        &emulator,
        "CancelReplay",
        &json!({ "ReplayName": "alpha-settled" }),
    )
    .await;
    assert_eq!(status, 200, "the cancel of alpha-settled: {status} {body}");
    poll_until_state(
        &emulator,
        "the replay left to complete",
        "alpha-done",
        state::REPLAY_STATE_COMPLETED,
    )
    .await;
    poll_until_state(
        &emulator,
        "the replay left to settle cancelled",
        "alpha-settled",
        state::REPLAY_STATE_CANCELLED,
    )
    .await;
    // And a third started and cancelled only now, so it is still CANCELLING when
    // the listings below read it.
    let (status, body) = start_replay(&emulator, &bus_arn, &archive_arn, "beta-cancelling").await;
    assert_eq!(
        status, 200,
        "the start of beta-cancelling answered {status} {body}"
    );
    let (status, body) = call(
        &emulator,
        "CancelReplay",
        &json!({ "ReplayName": "beta-cancelling" }),
    )
    .await;
    assert_eq!(status, 200, "the cancel of beta-cancelling: {status} {body}");

    // (case, the filter, the replay names it reports, in name order)
    let rows: [(&str, Value, &[&str]); 6] = [
        (
            "no filter at all",
            json!({}),
            &["alpha-done", "alpha-settled", "beta-cancelling"],
        ),
        (
            "a NamePrefix two replays share",
            json!({ "NamePrefix": "alpha" }),
            &["alpha-done", "alpha-settled"],
        ),
        (
            "a NamePrefix no replay carries",
            json!({ "NamePrefix": "gamma" }),
            &[],
        ),
        (
            "the State the clock left one replay in",
            json!({ "State": state::REPLAY_STATE_COMPLETED }),
            &["alpha-done"],
        ),
        (
            "the EventSourceArn every replay was started from",
            json!({ "EventSourceArn": archive_arn }),
            &["alpha-done", "alpha-settled", "beta-cancelling"],
        ),
        (
            "an EventSourceArn no replay was started from",
            json!({ "EventSourceArn": other_archive_arn }),
            &[],
        ),
    ];

    for (case, filter, expected) in rows {
        let found = listed(&emulator, case, &filter).await;
        let names: Vec<String> = found
            .iter()
            .map(|replay| replay["ReplayName"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            names, expected,
            "case {case}: expected {expected:?}, got {names:?}"
        );
    }

    // The two states a cancel moved a replay into are reported from the same
    // records the reads use.
    for (case, replay, expected) in [
        (
            "a settled cancel",
            "alpha-settled",
            state::REPLAY_STATE_CANCELLED,
        ),
        (
            "a cancel still under way",
            "beta-cancelling",
            state::REPLAY_STATE_CANCELLING,
        ),
    ] {
        let found = listed(&emulator, case, &json!({ "State": expected })).await;
        let names: Vec<&str> = found
            .iter()
            .map(|entry| entry["ReplayName"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            names,
            vec![replay],
            "case {case}: expected the {expected} filter to select {replay}, got {names:?}"
        );
    }

    // One page at a time, resumed by the token the page carries.
    let (status, body) = call(&emulator, "ListReplays", &json!({ "Limit": 2 })).await;
    assert_eq!(status, 200, "the first page answered {status} {body}");
    let first: Vec<&str> = body["Replays"]
        .as_array()
        .unwrap_or_else(|| panic!("no Replays in {body}"))
        .iter()
        .map(|entry| entry["ReplayName"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        first,
        vec!["alpha-done", "alpha-settled"],
        "the first page holds the first two replays in name order, got {body}"
    );
    let token = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a page with more behind it carries a token, got {body}"))
        .to_string();
    let (status, body) = call(
        &emulator,
        "ListReplays",
        &json!({ "Limit": 2, "NextToken": token }),
    )
    .await;
    assert_eq!(status, 200, "the second page answered {status} {body}");
    let second: Vec<&str> = body["Replays"]
        .as_array()
        .unwrap_or_else(|| panic!("no Replays in {body}"))
        .iter()
        .map(|entry| entry["ReplayName"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        second,
        vec!["beta-cancelling"],
        "the page resumed from the token holds what is left, got {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "the last page carries no token, got {body}"
    );

    // And every request naming more than one filter is refused: "filter
    // parameters are exclusive".
    for (case, filter) in [
        (
            "a NamePrefix and a State",
            json!({ "NamePrefix": "alpha", "State": state::REPLAY_STATE_COMPLETED }),
        ),
        (
            "a NamePrefix and an EventSourceArn",
            json!({ "NamePrefix": "alpha", "EventSourceArn": archive_arn }),
        ),
        (
            "a State and an EventSourceArn",
            json!({
                "State": state::REPLAY_STATE_COMPLETED,
                "EventSourceArn": archive_arn,
            }),
        ),
        (
            "all three filters at once",
            json!({
                "NamePrefix": "alpha",
                "State": state::REPLAY_STATE_COMPLETED,
                "EventSourceArn": archive_arn,
            }),
        ),
    ] {
        let (status, body) = call(&emulator, "ListReplays", &filter).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {status} {body}"
        );
    }

    // A `State` the modelled enum does not have selects nothing silently, so it
    // is refused too.
    let (status, body) = call(&emulator, "ListReplays", &json!({ "State": "NOPE" })).await;
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case a State outside the modelled enum: expected ValidationException, got {status} {body}"
    );
}
