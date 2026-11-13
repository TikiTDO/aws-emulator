// SPDX-License-Identifier: CC0-1.0

//! Which rules of the destination bus a REPLAY re-publishes onto — end to end
//! over the wire (`awsJson1_1`, `POST /` with `X-Amz-Target:
//! AWSEvents.<Operation>`) through the real `MultiServiceRouter`, with SQS
//! mounted on the same engine so a redelivery is observed where a client
//! observes it: on the queue a rule's target names.
//!
//! The RULE asserted here: "a list of ARNs for rules to replay events to"
//! (`ReplayDestination$FilterArns`) CONFINES a replay's routing to the rules it
//! names. The rules' own patterns do not enter into it — the rows below carry the
//! IDENTICAL pattern, so every one of them matches the replayed event — and a
//! rule the filters leave out therefore receives nothing from the replay while
//! staying live for events published afterwards. A destination carrying no
//! filters confines nothing: every matching rule of the bus receives the
//! redelivery. Each redelivered envelope carries the `replay-name` of the replay
//! that re-published it, and `DescribeReplay` reports the destination the request
//! named back unchanged once the replay has settled at `COMPLETED`.
//!
//! The rows differ along what the rule generalises over: how many of the bus's
//! rules the filters name (one, a subset, all of them by naming none), and which
//! of the identically-patterned rules those are.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-central-1";

/// The bus every rule below lives on.
const BUS: &str = "scoped";

/// The source and detail type every event below carries.
const SOURCE: &str = "com.demo.ledger";
const DETAIL_TYPE: &str = "Entry Posted";

/// How far back in EVENT time the archived event lies, and how wide the window
/// the replay selects it with is on either side.
const EVENT_AGE: i64 = -3_600;
const BAND: i64 = 300;

/// The emulator: EventBridge and SQS on one engine.
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
    assert_eq!(status, 200, "CreateQueue answered {status} {body}");
    let url = member(name, &body, "QueueUrl");
    let (status, body) = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(status, 200, "GetQueueAttributes answered {status} {body}");
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no QueueArn in {body}"))
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
    assert_eq!(status, 200, "SetQueueAttributes answered {status} {body}");
    (url, arn)
}

/// Every envelope waiting on the queue at `url`, parsed.
async fn drain(emulator: &Emulator, url: &str) -> Vec<Value> {
    let (status, body) = sqs(
        emulator,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    assert_eq!(status, 200, "ReceiveMessage answered {status} {body}");
    body["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let text = message["Body"].as_str().unwrap_or_default();
            serde_json::from_str::<Value>(text)
                .unwrap_or_else(|error| panic!("a delivered body is not JSON ({error}): {text:?}"))
        })
        .collect()
}

/// One rule of the scenario: its name, the ARN a filter names it by, and the url
/// of the queue its one target is.
struct Rule {
    name: String,
    arn: String,
    queue: String,
}

/// A rule called `name` on [`BUS`] carrying `pattern`, with one SQS queue target.
async fn given_rule(emulator: &Emulator, name: &str, pattern: &Value) -> Rule {
    let (queue, queue_arn) = given_queue(emulator, name).await;
    let (status, body) = events(
        emulator,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": BUS,
            "EventPattern": pattern.to_string(),
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
            "EventBusName": BUS,
            "Targets": [{ "Id": "t1", "Arn": queue_arn }],
        }),
    )
    .await;
    assert_eq!(status, 200, "case {name}: PutTargets answered {status} {body}");
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

/// Publish one event onto [`BUS`] with `detail`, timed `offset` seconds from
/// `anchor`, and answer its `EventId`.
async fn publish(emulator: &Emulator, detail: &Value, anchor: i64, offset: i64) -> String {
    let (status, body) = events(
        emulator,
        "PutEvents",
        &json!({
            "Entries": [{
                "EventBusName": BUS,
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": detail.to_string(),
                "Time": anchor + offset,
            }],
        }),
    )
    .await;
    assert_eq!(status, 200, "PutEvents answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "PutEvents reported a failed entry: {body}"
    );
    body["Entries"][0]["EventId"]
        .as_str()
        .unwrap_or_else(|| panic!("no EventId in {body}"))
        .to_string()
}

/// Poll `DescribeReplay` until the replay has left the states a replay still
/// moves out of, and answer what it reports then. The wait is for that CONDITION
/// and is given generous room, so a loaded host does not decide the assertion.
async fn settled_replay(emulator: &Emulator, replay: &str) -> Value {
    let deadline = ::std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let (status, body) = events(
            emulator,
            "DescribeReplay",
            &json!({ "ReplayName": replay }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {replay}: DescribeReplay answered {status} {body}"
        );
        let state = body["State"].as_str().unwrap_or_default().to_string();
        if state::is_terminal_replay_state(&state) {
            return body;
        }
        assert!(
            ::std::time::Instant::now() < deadline,
            "case {replay}: the replay never reached a terminal state, last read {body}"
        );
        ::tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `FilterArns` confines a replay to the rules it names, whatever the rules'
/// patterns say: the rows below all carry the same pattern, so what decides
/// which queue receives the redelivery is the filter list alone.
#[tokio::test]
async fn filter_arns_confine_a_replay_to_the_rules_they_name() {
    let emulator = emulator();
    let (status, body) = events(&emulator, "CreateEventBus", &json!({ "Name": BUS })).await;
    assert_eq!(status, 200, "CreateEventBus answered {status} {body}");
    let bus_arn = member("bus", &body, "EventBusArn");

    // Three rules carrying the IDENTICAL pattern, each with its own queue: every
    // one of them matches every event this suite publishes.
    let pattern = json!({ "source": [SOURCE] });
    let mut rules: Vec<Rule> = Vec::new();
    for name in ["rule-a", "rule-b", "rule-c"] {
        rules.push(given_rule(&emulator, name, &pattern).await);
    }

    let (status, body) = events(
        &emulator,
        "CreateArchive",
        &json!({
            "ArchiveName": "scoped-archive",
            "EventSourceArn": bus_arn,
            "EventPattern": pattern.to_string(),
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateArchive answered {status} {body}");
    let archive_arn = member("archive", &body, "ArchiveArn");

    // The one event every replay below re-publishes, timed an hour ago so the
    // window selects it by its OWN time.
    let anchor = epoch_now();
    let archived = publish(&emulator, &json!({ "entry": "archived" }), anchor, EVENT_AGE).await;

    let (status, body) = events(
        &emulator,
        "DescribeArchive",
        &json!({ "ArchiveName": "scoped-archive" }),
    )
    .await;
    assert_eq!(status, 200, "DescribeArchive answered {status} {body}");
    assert_eq!(
        body["EventCount"].as_i64().unwrap_or_default(),
        1,
        "the archive has to hold the published event before it can be replayed: {body}"
    );

    // The LIVE deliveries: every rule matched the published event, and no live
    // envelope carries a replay name.
    for rule in &rules {
        let bodies = drain(&emulator, &rule.queue).await;
        assert_eq!(
            bodies.len(),
            1,
            "case live delivery to {}: the published event matches this rule, got {bodies:?}",
            rule.name
        );
        assert!(
            bodies[0].get(state::REPLAY_NAME_FIELD).is_none(),
            "case live delivery to {}: a live delivery carries no replay name, got {}",
            rule.name,
            bodies[0]
        );
    }

    // Each row: the case, the replay's name, and which of the three rules the
    // destination's filters name — `None` for a destination carrying none at all.
    let rows: [(&str, &str, Option<Vec<usize>>); 4] = [
        ("one of three rules", "replay-one", Some(vec![0])),
        ("another single rule", "replay-other", Some(vec![2])),
        ("two of three rules", "replay-two", Some(vec![1, 2])),
        ("no filters at all", "replay-all", None),
    ];

    for (case, replay, named) in rows {
        let filters: Option<Vec<String>> = named
            .as_ref()
            .map(|indexes| indexes.iter().map(|at| rules[*at].arn.clone()).collect());
        let mut destination = json!({ "Arn": bus_arn });
        if let Some(filters) = &filters {
            destination["FilterArns"] = json!(filters);
        }
        let (status, body) = events(
            &emulator,
            "StartReplay",
            &json!({
                "ReplayName": replay,
                "EventSourceArn": archive_arn,
                "EventStartTime": anchor + EVENT_AGE - BAND,
                "EventEndTime": anchor + EVENT_AGE + BAND,
                "Destination": destination,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartReplay answered {status} {body}"
        );

        // The replay settles at COMPLETED, and the destination it reports is the
        // one the request named — filters included, unchanged.
        let reported = settled_replay(&emulator, replay).await;
        assert_eq!(
            reported["State"],
            json!(state::REPLAY_STATE_COMPLETED),
            "case {case:?}: expected the replay to settle at COMPLETED, got {reported}"
        );
        match &filters {
            Some(filters) => assert_eq!(
                reported["Destination"]["FilterArns"],
                json!(filters),
                "case {case:?}: expected DescribeReplay to report the filters unchanged, got \
                 {reported}"
            ),
            None => assert!(
                reported["Destination"]["FilterArns"].is_null(),
                "case {case:?}: a destination that named no filters reports none, got {reported}"
            ),
        }

        // Only the named rules received the redelivery; a rule the filters left
        // out received nothing at all, even though its pattern matches.
        for (at, rule) in rules.iter().enumerate() {
            let confined = named.as_ref().is_none_or(|indexes| indexes.contains(&at));
            let bodies = drain(&emulator, &rule.queue).await;
            if confined {
                assert_eq!(
                    bodies.len(),
                    1,
                    "case {case:?}: expected one redelivery to {}, got {bodies:?}",
                    rule.name
                );
                assert_eq!(
                    bodies[0]["id"],
                    json!(archived),
                    "case {case:?}: the redelivered event is the archived one, got {}",
                    bodies[0]
                );
                assert_eq!(
                    bodies[0][state::REPLAY_NAME_FIELD],
                    json!(replay),
                    "case {case:?}: a redelivered event carries the replay's name, got {}",
                    bodies[0]
                );
            } else {
                assert!(
                    bodies.is_empty(),
                    "case {case:?}: the filters leave {} out, so it receives nothing from the \
                     replay, got {bodies:?}",
                    rule.name
                );
            }
        }

        // A SENTINEL, so the silences above are not read off a path that had
        // died: one more live event reaches EVERY rule, including the ones the
        // filters left out.
        let live = publish(&emulator, &json!({ "entry": case }), anchor, 0).await;
        for rule in &rules {
            let bodies = drain(&emulator, &rule.queue).await;
            assert_eq!(
                bodies.len(),
                1,
                "case {case:?}: a live event published after the replay reaches {}, so a \
                 redelivery would have been seen had there been one, got {bodies:?}",
                rule.name
            );
            assert_eq!(
                bodies[0]["id"],
                json!(live),
                "case {case:?}: the live delivery to {} is the event just published, got {}",
                rule.name,
                bodies[0]
            );
        }
    }
}
