// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListMemoryExtractionJobs`, the listing of the long-term memory
//! extraction jobs a memory container holds.
//!
//! The rule under test: one job per SESSION of one actor, composed of the messages that
//! session's records carry for long-term extraction — the conversational turns of its
//! events (an event created with `extractionMode` `SKIP` is excluded from extraction and
//! composes none, and a payload item that is not a conversational turn is no message)
//! and of the content ingested into it — each message named by the id of the record it
//! belongs to and its position in that record's messages; a session composing no message
//! is no job. The optional `filter` narrows by every member it carries at once, the page
//! is bounded by `maxResults` with a `nextToken` resuming after it, and the listing
//! reports nothing about an extraction attempt (no `status`, no `failureReason`).
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator serves
//! with, so the wire shape (routes, status codes, `restJson1` bodies, modelled error
//! codes) is exercised, not just the handler functions.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use super::harness::{create_memory, label};

/// A memory id matching `MemoryId`'s pattern
/// `^(arn:...:memory/)?[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`.
const MEMORY: &str = "extraction-memory-0123456789";
/// The same memory addressed by its ARN spelling, which that same pattern admits.
const MEMORY_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:123456789012:memory/extraction-memory-0123456789";
/// A memory id the CONTROL plane never created, so nothing addresses a container.
const ABSENT_MEMORY: &str = "absent-memory-0123456789";

/// The emulator, with request authentication off so a test signs nothing.
///
/// The memory container this suite reads is created first, through the shared inventory
/// the CONTROL plane's `CreateMemory` writes: a call addressing a container that was
/// never created is refused.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    create_memory(&engine, MEMORY);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null` for an
/// empty body). The SigV4 credential scope is what routes the request to this service.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |v| bytes::Bytes::from(v.to_string()));
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://bedrock-agentcore.us-east-1.amazonaws.com{path}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

async fn create_event(router: &MultiServiceRouter, memory: &str, body: Value) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/memories/{}/events", label(memory)),
        Some(body),
    )
    .await
}

async fn list_extraction_jobs(
    router: &MultiServiceRouter,
    memory: &str,
    body: Value,
) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/memories/{}/extractionJobs", label(memory)),
        Some(body),
    )
    .await
}

async fn ingest_data(router: &MultiServiceRouter, memory: &str, body: Value) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/memories/{}/ingest", label(memory)),
        Some(body),
    )
    .await
}

async fn get_event(
    router: &MultiServiceRouter,
    memory: &str,
    actor: &str,
    session: &str,
    event: &str,
) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!(
            "/memories/{}/actor/{}/sessions/{}/events/{}",
            label(memory),
            label(actor),
            label(session),
            label(event)
        ),
        None,
    )
    .await
}

/// A conversational payload item. `Role`'s values are `ASSISTANT`, `USER`, `TOOL`,
/// `OTHER` (source: smithy_query shape="Role").
fn conversational(role: &str, text: &str) -> Value {
    json!({"conversational": {"role": role, "content": {"text": text}}})
}

/// A `json` payload item, which carries "non-conversational, JSON-formatted data"
/// (source: smithy_query shape="PayloadType", member="json") and is therefore no
/// message of an extraction job.
fn json_item(note: &str) -> Value {
    json!({"json": {"content": {"note": note}}})
}

fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

fn text(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One message of a job: the id of the record it belongs to and its index within that
/// record's messages.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Message {
    record_id: String,
    index: i64,
}

/// One job of an answer, as the members the rule is asserted over.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Job {
    job_id: String,
    actor_id: String,
    session_id: String,
    messages: Vec<Message>,
}

/// What one session must compose, in a case table.
struct Expected {
    actor: &'static str,
    session: &'static str,
    messages: Vec<Message>,
}

/// `EventId`'s modelled pattern, `^[0-9]+#[a-fA-F0-9]+$`, checked without a regex
/// dependency: the form every message's `eventId` is named in.
fn is_modelled_event_id(id: &str) -> bool {
    match id.split_once('#') {
        Some((millis, suffix)) => {
            !millis.is_empty()
                && millis.bytes().all(|b| b.is_ascii_digit())
                && !suffix.is_empty()
                && suffix.bytes().all(|b| b.is_ascii_hexdigit())
        }
        None => false,
    }
}

/// The jobs one answer reports, in the order it reported them.
fn jobs_of(body: &Value) -> Vec<Job> {
    let Some(jobs) = body.get("jobs").and_then(Value::as_array) else {
        return Vec::new();
    };
    jobs.iter()
        .map(|job| Job {
            job_id: text(job, "jobID"),
            actor_id: text(job, "actorId"),
            session_id: text(job, "sessionId"),
            messages: job
                .get("messages")
                .and_then(|messages| messages.get("messagesList"))
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .map(|message| Message {
                            record_id: text(message, "eventId"),
                            index: message
                                .get("messageIndex")
                                .and_then(Value::as_i64)
                                .unwrap_or(-1),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

/// The sessions one answer reports a job for, in order.
fn sessions_of(body: &Value) -> Vec<String> {
    jobs_of(body).into_iter().map(|job| job.session_id).collect()
}

fn message(record_id: &str, index: i64) -> Message {
    Message {
        record_id: record_id.to_string(),
        index,
    }
}

/// Create one event and hand back the `eventId` the service minted for it.
async fn event_id(
    router: &MultiServiceRouter,
    actor: &str,
    session: &str,
    payload: Vec<Value>,
    extraction_mode: Option<&str>,
) -> String {
    let mut body = json!({
        "actorId": actor,
        "sessionId": session,
        "eventTimestamp": 1_762_000_000,
        "payload": payload,
    });
    if let Some(mode) = extraction_mode {
        body["extractionMode"] = json!(mode);
    }
    let (status, answer) = create_event(router, MEMORY, body).await;
    assert_eq!(
        status, 201,
        "creating the event for actor {actor:?} session {session:?} must answer 201, got \
         {status} {answer}"
    );
    text(&answer["event"], "eventId")
}

/// One job per session, composed of the conversational messages of that session's
/// events, in the order the container accepted them — over a table of sessions whose
/// payload shape, event count and message count differ along exactly what the rule
/// generalises over.
#[tokio::test]
async fn jobs_are_the_sessions_unextracted_messages() {
    let router = emulator();

    let one_turn = event_id(
        &router,
        "actor-a",
        "session-one-turn",
        vec![conversational("USER", "hello")],
        None,
    )
    .await;
    let three_turns = event_id(
        &router,
        "actor-a",
        "session-three-turns",
        vec![
            conversational("USER", "first"),
            conversational("ASSISTANT", "second"),
            conversational("TOOL", "third"),
        ],
        None,
    )
    .await;
    let mixed = event_id(
        &router,
        "actor-b",
        "session-mixed",
        vec![
            conversational("USER", "ask"),
            json_item("telemetry"),
            conversational("ASSISTANT", "answer"),
        ],
        None,
    )
    .await;
    let first_of_two = event_id(
        &router,
        "actor-b",
        "session-two-events",
        vec![conversational("USER", "turn one")],
        None,
    )
    .await;
    let second_of_two = event_id(
        &router,
        "actor-b",
        "session-two-events",
        vec![
            conversational("ASSISTANT", "turn two"),
            conversational("USER", "turn three"),
        ],
        None,
    )
    .await;
    // A session whose only event is excluded from extraction, and one whose only event
    // carries no conversational turn: neither is a unit of extraction work.
    let skipped = event_id(
        &router,
        "actor-c",
        "session-skipped",
        vec![conversational("USER", "not extracted")],
        Some("SKIP"),
    )
    .await;
    let json_only = event_id(
        &router,
        "actor-c",
        "session-json-only",
        vec![json_item("behavioural event")],
        None,
    )
    .await;

    let expected = [
        Expected {
            actor: "actor-a",
            session: "session-one-turn",
            messages: vec![message(&one_turn, 0)],
        },
        Expected {
            actor: "actor-a",
            session: "session-three-turns",
            messages: vec![
                message(&three_turns, 0),
                message(&three_turns, 1),
                message(&three_turns, 2),
            ],
        },
        Expected {
            actor: "actor-b",
            session: "session-mixed",
            // The `json` item between the two turns is no message, so the second turn is
            // message 1 of its event.
            messages: vec![message(&mixed, 0), message(&mixed, 1)],
        },
        Expected {
            actor: "actor-b",
            session: "session-two-events",
            messages: vec![
                message(&first_of_two, 0),
                message(&second_of_two, 0),
                message(&second_of_two, 1),
            ],
        },
    ];

    let (status, answer) = list_extraction_jobs(&router, MEMORY, json!({})).await;
    assert_eq!(
        status, 200,
        "listing the extraction jobs must answer 200, got {status} {answer}"
    );
    let jobs = jobs_of(&answer);
    assert_eq!(
        jobs.len(),
        expected.len(),
        "case all-sessions: expected {} jobs (one per session composing a message), got \
         {jobs:?}",
        expected.len()
    );
    for (job, wanted) in jobs.iter().zip(expected.iter()) {
        let case = wanted.session;
        assert_eq!(
            (job.actor_id.as_str(), job.session_id.as_str()),
            (wanted.actor, wanted.session),
            "case {case:?}: expected the job to name its actor and session, got {job:?}"
        );
        assert!(
            !job.job_id.is_empty(),
            "case {case:?}: expected the job to report a jobID, got {job:?}"
        );
        assert_eq!(
            job.messages, wanted.messages,
            "case {case:?}: expected the messages {:?}, got {:?}",
            wanted.messages, job.messages
        );
    }

    // No session of an excluded or non-conversational event is a job, and each of their
    // events is nonetheless stored: the listing withholds the WORK, not the event.
    let sessions = sessions_of(&answer);
    for (case, session, event) in [
        ("skip", "session-skipped", &skipped),
        ("json-only", "session-json-only", &json_only),
    ] {
        assert!(
            !sessions.iter().any(|held| held == session),
            "case {case:?}: expected session {session:?} to compose no job, got {sessions:?}"
        );
        let (status, stored) = get_event(&router, MEMORY, "actor-c", session, event).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the event itself must still be readable, got {status} {stored}"
        );
    }

    // A second identical listing answers the same: a listing reports, it does not write.
    let (_, again) = list_extraction_jobs(&router, MEMORY, json!({})).await;
    assert_eq!(
        jobs_of(&again),
        jobs,
        "case repeat: two listings of an unchanged container must answer the same"
    );
    // The ARN spelling of `MemoryId` addresses the same container.
    let (status, by_arn) = list_extraction_jobs(&router, MEMORY_ARN, json!({})).await;
    assert_eq!(
        (status, jobs_of(&by_arn)),
        (200, jobs),
        "case arn: the ARN spelling must address the same container, got {by_arn}"
    );
}

/// Content submitted for long-term extraction is a unit of work too: `IngestData`
/// submits the same conversational material an event carries, so its messages compose
/// the session's job beside the events', in the order the container accepted them.
#[tokio::test]
async fn ingested_content_composes_the_sessions_job() {
    let router = emulator();

    let (status, answer) = ingest_data(
        &router,
        MEMORY,
        json!({
            "actorId": "actor-ingest",
            "sessionId": "session-ingest-only",
            "contentTimestamp": 1_762_000_000,
            "source": {"inline": {"payload": [
                conversational("USER", "ingested one"),
                json_item("telemetry"),
                conversational("ASSISTANT", "ingested two"),
            ]}},
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "ingesting into session-ingest-only must answer 202, got {status} {answer}"
    );

    let event = event_id(
        &router,
        "actor-ingest",
        "session-event-then-ingest",
        vec![conversational("USER", "from the event")],
        None,
    )
    .await;
    let (status, answer) = ingest_data(
        &router,
        MEMORY,
        json!({
            "actorId": "actor-ingest",
            "sessionId": "session-event-then-ingest",
            "contentTimestamp": 1_762_000_001,
            "source": {"inline": {"payload": [conversational("ASSISTANT", "from the ingest")]}},
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "ingesting into session-event-then-ingest must answer 202, got {status} {answer}"
    );

    // A session whose ingested content carries no conversational turn composes no
    // message, so it is no unit of work.
    let (status, answer) = ingest_data(
        &router,
        MEMORY,
        json!({
            "actorId": "actor-ingest",
            "sessionId": "session-ingest-json-only",
            "contentTimestamp": 1_762_000_002,
            "source": {"inline": {"payload": [json_item("behavioural event")]}},
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "ingesting into session-ingest-json-only must answer 202, got {status} {answer}"
    );

    let (status, answer) = list_extraction_jobs(&router, MEMORY, json!({})).await;
    assert_eq!(
        status, 200,
        "listing the extraction jobs must answer 200, got {status} {answer}"
    );
    let jobs = jobs_of(&answer);
    assert_eq!(
        sessions_of(&answer),
        vec![
            "session-ingest-only".to_string(),
            "session-event-then-ingest".to_string(),
        ],
        "case ingested: only the sessions whose content carries a message compose a job, \
         and in acceptance order, got {jobs:?}"
    );

    // The ingest-only session: the `json` item between the two turns is no message, so
    // the ingested content composes two messages of one record, indexed from zero.
    let ingest_only = &jobs[0].messages;
    assert_eq!(
        ingest_only.iter().map(|one| one.index).collect::<Vec<_>>(),
        vec![0, 1],
        "case ingest-only: expected the two turns indexed 0 and 1, got {ingest_only:?}"
    );
    assert!(
        ingest_only
            .iter()
            .all(|one| is_modelled_event_id(&one.record_id))
            && ingest_only[0].record_id == ingest_only[1].record_id,
        "case ingest-only: both messages must name the one stored record, in the modelled \
         id form, got {ingest_only:?}"
    );

    // The event-then-ingest session: one job, the event's message first (it was accepted
    // first), then the ingested turn under a DIFFERENT record id.
    let mixed = &jobs[1].messages;
    assert_eq!(
        mixed.len(),
        2,
        "case event-then-ingest: both writes must compose the one job, got {mixed:?}"
    );
    assert_eq!(
        mixed[0],
        message(&event, 0),
        "case event-then-ingest: the event's turn must come first, got {mixed:?}"
    );
    assert_eq!(
        mixed[1].index, 0,
        "case event-then-ingest: the ingested turn is the first message of its own \
         record, got {mixed:?}"
    );
    assert!(
        is_modelled_event_id(&mixed[1].record_id) && mixed[1].record_id != event,
        "case event-then-ingest: the ingested turn must name its own stored record, got \
         {mixed:?}"
    );
}

/// Every member of `filter` narrows the listing, and several narrow together: "only
/// extraction jobs with this <member> are returned" (source: smithy_query
/// shape="ExtractionJobFilterInput").
#[tokio::test]
async fn filter_members_narrow_together() {
    let router = emulator();
    for (actor, session) in [
        ("actor-one", "session-a"),
        ("actor-one", "session-b"),
        ("actor-two", "session-c"),
    ] {
        event_id(
            &router,
            actor,
            session,
            vec![conversational("USER", "hello")],
            None,
        )
        .await;
    }

    // (case, the filter, the sessions it must keep)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no filter",
            Value::Null,
            vec!["session-a", "session-b", "session-c"],
        ),
        (
            "actorId",
            json!({"actorId": "actor-one"}),
            vec!["session-a", "session-b"],
        ),
        (
            "sessionId",
            json!({"sessionId": "session-c"}),
            vec!["session-c"],
        ),
        (
            "actorId and sessionId together",
            json!({"actorId": "actor-one", "sessionId": "session-b"}),
            vec!["session-b"],
        ),
        // The two narrow together, so a pair that belongs to no job keeps none.
        (
            "actorId and sessionId that do not pair",
            json!({"actorId": "actor-two", "sessionId": "session-a"}),
            vec![],
        ),
        (
            "actorId nothing holds",
            json!({"actorId": "actor-three"}),
            vec![],
        ),
        (
            "sessionId nothing holds",
            json!({"sessionId": "session-absent"}),
            vec![],
        ),
        // `ExtractionJobStatus`'s only value (source: smithy_query
        // shape="ExtractionJobStatus"). A unit of work nothing has attempted reports no
        // status, so a listing filtered on one keeps nothing.
        ("status FAILED", json!({"status": "FAILED"}), vec![]),
        // A memory strategy belongs to the CONTROL plane, so a job derived from stored
        // messages names none and a listing filtered on one keeps nothing.
        (
            "strategyId",
            json!({"strategyId": "facts-0123456789"}),
            vec![],
        ),
    ];

    for (case, filter, wanted) in cases {
        let body = if filter.is_null() {
            json!({})
        } else {
            json!({"filter": filter})
        };
        let (status, answer) = list_extraction_jobs(&router, MEMORY, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {answer}"
        );
        let sessions = sessions_of(&answer);
        assert_eq!(
            sessions, wanted,
            "case {case:?}: expected the jobs of {wanted:?}, got {sessions:?}"
        );
        for job in jobs_of(&answer) {
            assert!(
                !job.job_id.is_empty() && !job.messages.is_empty(),
                "case {case:?}: every job must name its id and its messages, got {job:?}"
            );
        }
    }

    // Nothing an extraction attempt would have produced is reported.
    let (_, answer) = list_extraction_jobs(&router, MEMORY, json!({})).await;
    for job in answer["jobs"].as_array().cloned().unwrap_or_default() {
        for member in ["status", "failureReason", "strategyId"] {
            assert!(
                job.get(member).is_none(),
                "case {member:?}: a job nothing has attempted must report no {member}, got \
                 {job}"
            );
        }
    }
}

/// `maxResults` bounds one page and `nextToken` resumes after it, over every page size
/// the range admits around the number of jobs the container holds — and the pages
/// together are the whole list, each job once.
#[tokio::test]
async fn pages_are_bounded_and_resume() {
    let router = emulator();
    let sessions = [
        "session-p1",
        "session-p2",
        "session-p3",
        "session-p4",
        "session-p5",
    ];
    for session in sessions {
        event_id(
            &router,
            "actor-paged",
            session,
            vec![conversational("USER", "hello")],
            None,
        )
        .await;
    }

    // `maxResults` is modelled `@range(min: 1, max: 50)` with a default of 20 (source:
    // smithy_query query="operation" operation="ListMemoryExtractionJobs"), so the page
    // sizes exercised are both published bounds and the sizes around the list's length.
    for max_results in [1_i64, 2, 4, 5, 6, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({"maxResults": max_results});
            if let Some(next) = token.as_ref() {
                body["nextToken"] = json!(next);
            }
            let (status, answer) = list_extraction_jobs(&router, MEMORY, body).await;
            assert_eq!(
                status, 200,
                "case maxResults={max_results}: expected 200, got {status} {answer}"
            );
            let page = sessions_of(&answer);
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or(usize::MAX),
                "case maxResults={max_results}: a page must hold at most {max_results} jobs, \
                 got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            token = answer
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= sessions.len() + 1,
                "case maxResults={max_results}: paging must terminate, got {pages} pages \
                 and token {token:?}"
            );
        }
        let wanted: Vec<String> = sessions.iter().map(ToString::to_string).collect();
        assert_eq!(
            seen, wanted,
            "case maxResults={max_results}: the pages together must be the whole list in \
             order, got {seen:?}"
        );
    }

    // A token this operation did not issue is refused rather than resuming at a guess.
    for (case, token) in [
        ("not base64", "{{{"),
        ("another list's token", "ZXZlbnRzOjE="),
        ("no offset", "ZXh0cmFjdGlvbkpvYnM="),
    ] {
        let (status, answer) =
            list_extraction_jobs(&router, MEMORY, json!({"nextToken": token})).await;
        assert_eq!(
            (status, error_code(&answer)),
            (400, "ValidationException"),
            "case {case:?}: a foreign nextToken must be refused, got {status} {answer}"
        );
    }
}

/// A container the CONTROL plane never created is refused with the resource it addressed,
/// and one it created but nothing has been written into holds no job.
#[tokio::test]
async fn container_is_resolved_before_the_listing() {
    let router = emulator();

    let (status, answer) = list_extraction_jobs(&router, ABSENT_MEMORY, json!({})).await;
    assert_eq!(
        (status, error_code(&answer)),
        (404, "ResourceNotFoundException"),
        "case absent: a container that was never created must be refused, got {status} {answer}"
    );
    assert_eq!(
        text(&answer, "message"),
        format!("Memory not found: {ABSENT_MEMORY}"),
        "case absent: the refusal must name the container the request addressed, got {answer}"
    );

    let (status, answer) = list_extraction_jobs(&router, MEMORY, json!({})).await;
    assert_eq!(
        (status, jobs_of(&answer).len()),
        (200, 0),
        "case empty: a container nothing has been written into holds no job, got {status} \
         {answer}"
    );
    assert!(
        answer.get("nextToken").is_none(),
        "case empty: an empty listing must offer no nextToken, got {answer}"
    );
}

/// A job's `jobID` is the identity of the unit of work, so it is the same across
/// listings and keeps naming the same session as that session grows.
#[tokio::test]
async fn job_ids_are_stable_for_a_session() {
    let router = emulator();
    event_id(
        &router,
        "actor-stable",
        "session-stable",
        vec![conversational("USER", "first")],
        None,
    )
    .await;

    let (_, first) = list_extraction_jobs(&router, MEMORY, json!({})).await;
    let before = jobs_of(&first);
    let (_, second) = list_extraction_jobs(&router, MEMORY, json!({})).await;
    assert_eq!(
        jobs_of(&second),
        before,
        "case repeat: two listings must report the same job for the same unit of work"
    );

    // A further event in that session joins the SAME unit of work: one job, one more
    // message, the same id.
    let added = event_id(
        &router,
        "actor-stable",
        "session-stable",
        vec![conversational("ASSISTANT", "second")],
        None,
    )
    .await;
    let (_, grown) = list_extraction_jobs(&router, MEMORY, json!({})).await;
    let after = jobs_of(&grown);
    assert_eq!(
        after.len(),
        1,
        "case grown: the session must still compose exactly one job, got {after:?}"
    );
    assert_eq!(
        after[0].job_id, before[0].job_id,
        "case grown: the jobID must not change when the session grows, got {after:?} after \
         {before:?}"
    );
    assert_eq!(
        after[0].messages.len(),
        before[0].messages.len() + 1,
        "case grown: the new event's message must join the unit, got {after:?}"
    );
    assert_eq!(
        after[0].messages.last(),
        Some(&message(&added, 0)),
        "case grown: the last message must be the new event's only turn, got {after:?}"
    );
}
