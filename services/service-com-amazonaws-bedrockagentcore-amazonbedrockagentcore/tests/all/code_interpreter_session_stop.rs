// SPDX-License-Identifier: CC0-1.0

//! What a `StopCodeInterpreterSession` changes, and what it leaves alone.
//!
//! THE RULE: a stop terminates EXACTLY the session addressed by the code interpreter in
//! its path and the `sessionId` in its query — that one session reads back `TERMINATED`
//! and serves no further tool, while every other session the emulator holds (a sibling
//! of the same code interpreter, or a session of another one) is untouched and still
//! runs tools. `sessionId` is scoped to the code interpreter the session was started on
//! (`PUT /code-interpreters/{codeInterpreterIdentifier}/sessions/stop?sessionId=…`,
//! source: smithy_query query="operation" operation="StopCodeInterpreterSession"), so a
//! stop aimed at another code interpreter resolves no session: it is refused and
//! terminates nothing. A stop refused for failing a modelled input constraint likewise
//! changes nothing. The `lastUpdatedAt` a served stop reports is the instant of that
//! stop, so it is never before the session's own `createdAt`.
//!
//! The rows span what the rule generalises over: which code interpreter the session was
//! started on (the one the service provides and two the control plane created), how many
//! sessions that interpreter holds, which of them is stopped, whether the stop carries a
//! `clientToken` and how long that token is, and which constraint a refused stop fails.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator serves
//! with, so what is asserted is the wire behaviour a client sees.

use aws_smithy_types::date_time::{DateTime, Format};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

use super::harness::{create_code_interpreter, label};

/// The code interpreter the service provides to every account; `codeInterpreterIdentifier`
/// is modelled as a plain string, so each identifier addresses its own session set.
const SYSTEM_INTERPRETER: &str = "aws.codeinterpreter.v1";
/// Two code interpreters the CONTROL plane created, so a case can show that a session
/// belongs to the interpreter it was started on.
const CUSTOM_ONE: &str = "demo_interpreter-1a2b3c4d5e";
const CUSTOM_TWO: &str = "report_interpreter-9f8e7d6c5b";
/// A code interpreter identifier nothing ever created.
const UNKNOWN_INTERPRETER: &str = "aws.codeinterpreter.v2";

/// The header `InvokeCodeInterpreter` binds `sessionId` to on the request (source:
/// smithy_query query="operation" operation="InvokeCodeInterpreter").
const SESSION_HEADER: &str = "x-amzn-code-interpreter-session-id";

/// The emulator, with request authentication off so a test signs nothing. The two
/// customer-created interpreters are registered the way the control plane's
/// `CreateCodeInterpreter` does, since a session cannot be started on one that does not
/// exist; the interpreter the service provides needs no such call.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    create_code_interpreter(&engine, CUSTOM_ONE);
    create_code_interpreter(&engine, CUSTOM_TWO);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(engine));
    router
}

/// One HTTP exchange: the status, the response headers and the parsed body.
struct Call {
    status: u16,
    headers: http::HeaderMap,
    body: Value,
}

impl Call {
    /// The wire error code: the `x-amzn-errortype` header restJson1 sets, or the body's
    /// `__type`.
    fn error_code(&self) -> String {
        if let Some(value) = self.headers.get("x-amzn-errortype") {
            return value.to_str().unwrap_or_default().to_string();
        }
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> Call {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(value.to_string())
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://bedrock-agentcore.us-east-1.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router
        .dispatch(request.body(payload).expect("build the request"))
        .await;
    Call {
        status: response.status().as_u16(),
        headers: response.headers().clone(),
        body: serde_json::from_slice(response.body()).unwrap_or(Value::Null),
    }
}

/// `StartCodeInterpreterSession`: `PUT /code-interpreters/{id}/sessions/start`.
async fn start_session(router: &MultiServiceRouter, interpreter: &str, body: Value) -> Call {
    send(
        router,
        "PUT",
        &format!("/code-interpreters/{}/sessions/start", label(interpreter)),
        &[],
        Some(body),
    )
    .await
}

/// `GetCodeInterpreterSession`: `GET /code-interpreters/{id}/sessions/get?sessionId=…`.
async fn get_session(router: &MultiServiceRouter, interpreter: &str, session_id: &str) -> Call {
    send(
        router,
        "GET",
        &format!(
            "/code-interpreters/{}/sessions/get?sessionId={}",
            label(interpreter),
            label(session_id)
        ),
        &[],
        None,
    )
    .await
}

/// `StopCodeInterpreterSession`: `PUT /code-interpreters/{id}/sessions/stop?sessionId=…`.
async fn stop_session(
    router: &MultiServiceRouter,
    interpreter: &str,
    session_id: &str,
    client_token: Option<&str>,
) -> Call {
    let mut body = json!({});
    if let Some(token) = client_token {
        body["clientToken"] = json!(token);
    }
    send(
        router,
        "PUT",
        &format!(
            "/code-interpreters/{}/sessions/stop?sessionId={}",
            label(interpreter),
            label(session_id)
        ),
        &[],
        Some(body),
    )
    .await
}

/// `InvokeCodeInterpreter` with the cheapest tool that touches the session's own state,
/// used here only as the liveness check: a live session serves it (HTTP 200), a
/// terminated one conflicts.
async fn invoke_list_files(
    router: &MultiServiceRouter,
    interpreter: &str,
    session_id: &str,
) -> Call {
    send(
        router,
        "POST",
        &format!("/code-interpreters/{}/tools/invoke", label(interpreter)),
        &[(SESSION_HEADER, session_id)],
        Some(json!({"name": "listFiles", "arguments": {"path": ""}})),
    )
    .await
}

/// A session this suite started, as the fleet below holds it.
#[derive(Debug, Clone)]
struct Started {
    case: String,
    interpreter: String,
    session_id: String,
    created_at: String,
}

/// Start one session per row and return the fleet, every member of it live.
///
/// Rows differ along the code interpreter, how many sessions it holds and the
/// time-to-live each was started with. Every time-to-live here is generous: this suite
/// asserts what a STOP does, so nothing may terminate on its own while it runs
/// (`CodeInterpreterSessionTimeout` is range 1..28800 with default 900, source:
/// smithy_query query="operation" operation="StartCodeInterpreterSession").
async fn start_fleet(router: &MultiServiceRouter) -> Vec<Started> {
    let rows: Vec<(&str, &str, Value)> = vec![
        ("system-default-timeout", SYSTEM_INTERPRETER, json!({})),
        (
            "system-named-session",
            SYSTEM_INTERPRETER,
            json!({"name": "analysis", "sessionTimeoutSeconds": 3_600}),
        ),
        (
            "system-longest-timeout",
            SYSTEM_INTERPRETER,
            json!({"sessionTimeoutSeconds": 28_800}),
        ),
        (
            "custom-one-only-session",
            CUSTOM_ONE,
            json!({"sessionTimeoutSeconds": 1_800}),
        ),
        ("custom-two-first-session", CUSTOM_TWO, json!({})),
        (
            "custom-two-second-session",
            CUSTOM_TWO,
            json!({"name": "second", "sessionTimeoutSeconds": 7_200}),
        ),
    ];

    let mut fleet = Vec::new();
    for (case, interpreter, body) in rows {
        let started = start_session(router, interpreter, body).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartCodeInterpreterSession, got {} with {:?}",
            started.status, started.body
        );
        fleet.push(Started {
            case: case.to_string(),
            interpreter: interpreter.to_string(),
            session_id: started.text("sessionId"),
            created_at: started.text("createdAt"),
        });
    }
    fleet
}

/// Assert `session` is live: the read reports `READY` and a tool call is served.
async fn assert_live(router: &MultiServiceRouter, case: &str, session: &Started) {
    let read = get_session(router, &session.interpreter, &session.session_id).await;
    assert_eq!(
        (read.status, read.text("status").as_str()),
        (200, "READY"),
        "case {case:?}: session {:?} of {:?} must still be READY, got {} with {:?}",
        session.session_id,
        session.interpreter,
        read.status,
        read.body
    );
    let served = invoke_list_files(router, &session.interpreter, &session.session_id).await;
    assert_eq!(
        served.status, 200,
        "case {case:?}: session {:?} of {:?} must still serve a tool, got {} with {:?}",
        session.session_id, session.interpreter, served.status, served.body
    );
}

/// Assert `session` is terminated: the read reports `TERMINATED` and a tool call
/// conflicts with that state.
async fn assert_terminated(router: &MultiServiceRouter, case: &str, session: &Started) {
    let read = get_session(router, &session.interpreter, &session.session_id).await;
    assert_eq!(
        (read.status, read.text("status").as_str()),
        (200, "TERMINATED"),
        "case {case:?}: session {:?} of {:?} must read back TERMINATED, got {} with {:?}",
        session.session_id,
        session.interpreter,
        read.status,
        read.body
    );
    let refused = invoke_list_files(router, &session.interpreter, &session.session_id).await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (409, "ConflictException"),
        "case {case:?}: a terminated session serves no tool, got {} with {:?}",
        refused.status,
        refused.body
    );
}

/// The epoch seconds of an RFC 3339 timestamp a response reported, asserting the wire
/// form at the same time: a `DateTimestamp` member carries
/// `@timestampFormat("date-time")` (source: smithy_query query="shape"
/// shape="DateTimestamp").
fn date_time_secs(case: &str, member_name: &str, value: &str) -> i64 {
    let parsed = DateTime::from_str(value, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: expected {member_name} as an RFC 3339 string, got {value:?}: {error}")
    });
    parsed.secs()
}

/// A stop terminates the session it names and NOTHING else: every other session the
/// emulator holds — a sibling of the same code interpreter and a session of another one
/// — stays live, and the stop reports the identity it was asked for with the instant of
/// the stop.
#[tokio::test]
async fn a_stop_terminates_only_the_session_it_names() {
    let router = emulator();
    let fleet = start_fleet(&router).await;

    // Rows differ along which member of the fleet is stopped (so along the code
    // interpreter, and along whether siblings of that interpreter remain) and along the
    // `clientToken` the stop carries: `ClientToken` is len 33..256, pattern
    // ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$ (source: smithy_query query="operation"
    // operation="StopCodeInterpreterSession").
    let targets: Vec<(usize, Option<String>)> = vec![
        (0, None),
        (3, Some("stop-token-0123456789-abcdefghij-0".to_string())),
        (4, Some("a".repeat(33))),
        (1, Some("z".repeat(256))),
    ];

    let mut stopped: Vec<usize> = Vec::new();
    for (index, token) in targets {
        let target = &fleet[index];
        let case = format!("stop-{}", target.case);

        let stop = stop_session(
            &router,
            &target.interpreter,
            &target.session_id,
            token.as_deref(),
        )
        .await;
        assert_eq!(
            stop.status, 200,
            "case {case:?}: expected HTTP 200 from StopCodeInterpreterSession, got {} with {:?}",
            stop.status, stop.body
        );
        assert_eq!(
            stop.text("sessionId"),
            target.session_id,
            "case {case:?}: expected the stopped session back, got {:?}",
            stop.body
        );
        assert_eq!(
            stop.text("codeInterpreterIdentifier"),
            target.interpreter,
            "case {case:?}: expected the code interpreter the request named, got {:?}",
            stop.body
        );
        let last_updated = date_time_secs(&case, "lastUpdatedAt", &stop.text("lastUpdatedAt"));
        let created = date_time_secs(&case, "createdAt", &target.created_at);
        assert!(
            last_updated >= created,
            "case {case:?}: the instant of the stop is not before the session was created, \
             got lastUpdatedAt {last_updated} and createdAt {created}"
        );

        stopped.push(index);
        assert_terminated(&router, &case, target).await;

        // Every session that was not stopped is untouched, whichever code interpreter it
        // belongs to.
        for (other, session) in fleet.iter().enumerate() {
            if stopped.contains(&other) {
                continue;
            }
            assert_live(&router, &case, session).await;
        }

        // And every session already stopped stays stopped.
        for other in &stopped {
            assert_terminated(&router, &case, &fleet[*other]).await;
        }
    }
}

/// A `sessionId` is scoped to the code interpreter the session was started on: a stop
/// aimed at any other code interpreter resolves no session, so it is refused with a 404
/// `ResourceNotFoundException` and terminates nothing — and the same id stopped on the
/// right code interpreter is served.
#[tokio::test]
async fn a_stop_aimed_at_another_code_interpreter_terminates_nothing() {
    let router = emulator();
    let fleet = start_fleet(&router).await;

    // Rows: the code interpreter the session lives on × the one the stop is aimed at,
    // including an identifier nothing ever created.
    let rows: Vec<(usize, &str)> = vec![
        (0, CUSTOM_ONE),
        (3, SYSTEM_INTERPRETER),
        (4, CUSTOM_ONE),
        (5, UNKNOWN_INTERPRETER),
    ];

    for (index, aimed_at) in rows {
        let target = &fleet[index];
        let case = format!("{}-stopped-on-{aimed_at}", target.case);

        let refused = stop_session(&router, aimed_at, &target.session_id, None).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404 for a session that code interpreter does not \
             hold, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.body
        );

        // Nothing was terminated: not the session that carries this id on its own code
        // interpreter, and not anything else the emulator holds.
        for session in &fleet {
            assert_live(&router, &case, session).await;
        }
    }

    // Each of them stops on the code interpreter it was started on.
    for session in &fleet {
        let case = format!("{}-stopped-where-it-lives", session.case);
        let stop = stop_session(&router, &session.interpreter, &session.session_id, None).await;
        assert_eq!(
            stop.status, 200,
            "case {case:?}: expected HTTP 200 from the stop on its own code interpreter, got {} \
             with {:?}",
            stop.status, stop.body
        );
        assert_terminated(&router, &case, session).await;
    }
}

/// A stop whose input is outside a modelled constraint is refused with a 400
/// `ValidationException` and changes nothing: the session it addressed is still live
/// afterwards, and a well-formed stop still terminates it.
#[tokio::test]
async fn a_stop_refused_for_a_modelled_constraint_leaves_the_session_live() {
    let router = emulator();
    let fleet = start_fleet(&router).await;
    let target = &fleet[0];

    // Rows: each side of `ClientToken`'s published length 33..256, its pattern
    // ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$, and `CodeInterpreterSessionId`'s pattern
    // ^[0-9a-zA-Z]{1,40}$ past its length and outside its character set (source:
    // smithy_query query="operation" operation="StopCodeInterpreterSession").
    let rows: Vec<(&str, String, Option<String>)> = vec![
        (
            "client-token-one-short-of-its-length",
            target.session_id.clone(),
            Some("a".repeat(32)),
        ),
        (
            "client-token-one-past-its-length",
            target.session_id.clone(),
            Some("a".repeat(257)),
        ),
        (
            "client-token-outside-its-pattern",
            target.session_id.clone(),
            Some(format!("token_with_underscores{}", "0".repeat(16))),
        ),
        ("session-id-one-past-its-length", "a".repeat(41), None),
        (
            "session-id-outside-its-character-set",
            "session-with-hyphens".to_string(),
            None,
        ),
    ];

    for (case, session_id, token) in rows {
        let refused =
            stop_session(&router, &target.interpreter, &session_id, token.as_deref()).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
        // A refused stop stores nothing: every session is as it was.
        for session in &fleet {
            assert_live(&router, case, session).await;
        }
    }

    // The same session still stops once the request satisfies every constraint.
    let stop = stop_session(
        &router,
        &target.interpreter,
        &target.session_id,
        Some("valid-token-0123456789-abcdefghij"),
    )
    .await;
    assert_eq!(
        stop.status, 200,
        "after every refusal a well-formed stop must be served, got {} with {:?}",
        stop.status, stop.body
    );
    assert_terminated(&router, "after-the-refusals", target).await;
}
