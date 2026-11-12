// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two AGENT-INVOCATION routes of this plane —
//! `InvokeAgent` (`POST /agents/{agentId}/agentAliases/{agentAliasId}/sessions/{sessionId}/text`)
//! and `InvokeFlow` (`POST /flows/{flowIdentifier}/aliases/{flowAliasIdentifier}`).
//!
//! THE RULES UNDER TEST:
//!
//! * a request whose URI LABEL breaks the model's own constraint is refused with the
//!   modelled `ValidationException` (HTTP 400) before any resource is resolved, whichever
//!   label and whichever constraint class it is;
//! * an agent or an alias the Bedrock Agent build-time plane does not hold is the modelled
//!   `ResourceNotFoundException` (HTTP 404) with the fixed text this plane states for that
//!   class, and an agent it DOES hold is served — as an event stream carrying the one
//!   `chunk` the modelled `ResponseStream` declares for the agent's answer;
//! * `InvokeFlow` is modelled `@http(code: 200)` and its `FlowResponseStream` carries the
//!   service's modelled exceptions as EVENT MEMBERS, so every failure that arises ONCE the
//!   invocation has begun is delivered inside a 200 event stream rather than as an HTTP
//!   error — a flow the build-time plane does not hold included;
//! * a flow that IS prepared runs its own stored definition: the request's document enters
//!   at the flow's input node and leaves at the output nodes that node is connected to, one
//!   `flowOutputEvent` each, then the `flowCompletionEvent`;
//! * a leg of either request this emulator cannot compute is refused rather than ignored.
//!
//! The tables span what those rules generalise over: which label is wrong and which
//! constraint class it breaks, whether the agent exists and whether the alias is the draft
//! one every agent carries, the FORM of a flow identifier (a bare id, a flow ARN, an alias
//! ARN of another flow), the number of output nodes a flow's input reaches, and the status
//! the flow stands in.

use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};

use aws_smithy_eventstream::frame::read_message_from;
use aws_smithy_types::event_stream::HeaderValue;

use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state as agent_state;

use crate::agent_memory::{Emulator, Reply, ACCOUNT_KEY, REGION};

/// The content type of an event-stream body, in both directions.
pub(crate) const EVENT_STREAM: &str = "application/vnd.amazon.eventstream";

/// The draft (test) alias every agent carries, as the modelled `AgentAliasId` spells it.
const DRAFT_ALIAS: &str = "TSTALIASID";

/// The fixed text this plane states when the agent or the alias a request names is one the
/// build-time plane holds nothing for. Entirely fixed: it names neither half of the pair.
const RESOURCE_NOT_FOUND: &str = "Failed to retrieve resource because it doesn't exist. \
                                  Retry the request with a different resource identifier.";

/// A `sessionId` every modelled constraint on the label admits.
const SESSION: &str = "probe-session-01";

/// The `@pattern` a `sessionId` refusal quotes: the modelled `^[0-9a-zA-Z._:-]+$` with the
/// outer anchors dropped, which is this service's rule for quoting a pattern
/// (`tests/all/pattern_constraint_messages.rs`).
const SESSION_ID_PATTERN: &str = "[0-9a-zA-Z._:-]+";

/// The `@pattern` a `flowIdentifier` refusal quotes (`shape="FlowIdentifier"`), by the same
/// rule: both anchors go and the leftover alternation is grouped.
const FLOW_IDENTIFIER_PATTERN: &str =
    "((arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10}))";

/// An IAM role ARN matching the build-time plane's modelled `FlowExecutionRoleArn`.
const FLOW_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";

/// One frame of an event-stream body, read the way an SDK's decoder reads it.
pub(crate) struct Frame {
    /// `:message-type`: `event` for a data event, `exception` for an in-band modelled
    /// error.
    pub message_type: String,
    /// The `@streaming` union member the frame carries, from `:event-type` for a data event
    /// and from `:exception-type` for an in-band error.
    pub name: String,
    pub payload: Value,
}

/// Decode an event-stream body into its frames, asserting each one names a member.
pub(crate) fn decode_frames(case: &str, body: &[u8]) -> Vec<Frame> {
    let mut buffer = bytes::Bytes::copy_from_slice(body);
    let mut frames = Vec::new();
    while !buffer.is_empty() {
        let message = read_message_from(&mut buffer).unwrap_or_else(|error| {
            panic!("case {case:?}: the body must decode as event-stream frames: {error}")
        });
        let header = |name: &str| {
            message
                .headers()
                .iter()
                .find(|header| header.name().as_str() == name)
                .map(|header| match header.value() {
                    HeaderValue::String(value) => value.as_str().to_string(),
                    other => format!("{other:?}"),
                })
        };
        let message_type = header(":message-type")
            .unwrap_or_else(|| panic!("case {case:?}: a frame named no :message-type"));
        let name = header(":event-type")
            .or_else(|| header(":exception-type"))
            .unwrap_or_else(|| {
                panic!("case {case:?}: a frame named neither :event-type nor :exception-type")
            });
        let payload: Value = serde_json::from_slice(message.payload()).unwrap_or_else(|error| {
            panic!("case {case:?}: the {name} payload must be JSON: {error}")
        });
        frames.push(Frame {
            message_type,
            name,
            payload,
        });
    }
    frames
}

/// The member names of the frames of one body, in order.
pub(crate) fn frame_names(frames: &[Frame]) -> Vec<&str> {
    frames.iter().map(|frame| frame.name.as_str()).collect()
}

impl Emulator {
    /// One `InvokeAgent` call over the modelled URI.
    async fn invoke_agent(&self, agent: &str, alias: &str, session: &str, body: Value) -> Reply {
        self.bedrock(
            ACCOUNT_KEY,
            "POST",
            &format!("/agents/{agent}/agentAliases/{alias}/sessions/{session}/text"),
            Some(body),
        )
        .await
    }

    /// One `InvokeFlow` call over the modelled URI.
    async fn invoke_flow(&self, flow: &str, alias: &str, body: Value) -> Reply {
        self.bedrock(
            ACCOUNT_KEY,
            "POST",
            &format!("/flows/{flow}/aliases/{alias}"),
            Some(body),
        )
        .await
    }

    /// Create an agent through the build-time plane's own `CreateAgent`, with the memory
    /// configuration `memory` when one is given, and answer its minted `agentId`.
    async fn agent_with_memory(&self, name: &str, memory: Option<Value>) -> String {
        let mut body = json!({ "agentName": name });
        if let Some(memory) = memory {
            body["memoryConfiguration"] = memory;
        }
        let reply = self.bedrock(ACCOUNT_KEY, "PUT", "/agents/", Some(body)).await;
        assert_eq!(
            reply.status, 202,
            "setup: CreateAgent must answer 202, got {} {:?}",
            reply.status,
            reply.json(name)
        );
        reply.json(name)["agent"]["agentId"]
            .as_str()
            .unwrap_or_else(|| panic!("setup: CreateAgent answered no agentId: {:?}", reply.json(name)))
            .to_string()
    }

    /// Create a flow carrying `definition` and answer its minted `(flowId, flowArn)`.
    async fn create_flow(&self, name: &str, definition: Value) -> (String, String) {
        let reply = self
            .bedrock(
                ACCOUNT_KEY,
                "POST",
                "/flows/",
                Some(json!({
                    "name": name,
                    "executionRoleArn": FLOW_ROLE,
                    "definition": definition,
                })),
            )
            .await;
        assert_eq!(
            reply.status, 201,
            "setup: CreateFlow must answer 201, got {} {:?}",
            reply.status,
            reply.json(name)
        );
        let created = reply.json(name);
        (
            created["id"].as_str().expect("a minted flowId").to_string(),
            created["arn"].as_str().expect("a minted flow ARN").to_string(),
        )
    }

    /// Prepare the flow and move the outcome the clock owes into the past, so it stands at
    /// `Prepared` without the test sleeping for the span.
    async fn prepare_flow(&self, case: &str, flow_id: &str) {
        let reply = self
            .bedrock(
                ACCOUNT_KEY,
                "POST",
                &format!("/flows/{flow_id}/"),
                Some(json!({})),
            )
            .await;
        assert_eq!(
            reply.status, 202,
            "case {case}: PrepareFlow must answer 202, got {} {:?}",
            reply.status,
            reply.json(case)
        );
        let state = agent_state::service_state(&*self.engine)
            .expect("the Bedrock Agent store is on this engine");
        let mut store = state.lock();
        let pending = store
            .flows
            .get_mut(flow_id)
            .expect("the stored flow")
            .pending_preparation
            .as_mut()
            .unwrap_or_else(|| panic!("case {case}: expected a preparation in flight"));
        pending.due = SystemTime::now() - Duration::from_secs(1);
    }

    /// Create an alias on `flow_id` routing to `flow_version`, and answer its
    /// `(aliasId, aliasArn)`.
    async fn create_flow_alias(&self, case: &str, flow_id: &str, flow_version: &str) -> (String, String) {
        let reply = self
            .bedrock(
                ACCOUNT_KEY,
                "POST",
                &format!("/flows/{flow_id}/aliases/"),
                Some(json!({
                    "name": "probe-alias",
                    "routingConfiguration": [{ "flowVersion": flow_version }],
                })),
            )
            .await;
        assert_eq!(
            reply.status, 201,
            "case {case}: CreateFlowAlias must answer 201, got {} {:?}",
            reply.status,
            reply.json(case)
        );
        let created = reply.json(case);
        (
            created["id"].as_str().expect("a minted aliasId").to_string(),
            created["arn"].as_str().expect("a minted alias ARN").to_string(),
        )
    }
}

/// A passthrough flow definition: one `Input` node wired straight to one `Output` node. The
/// node types, the union keys and the IO data types are the build-time model's own
/// `FlowNodeType` / `FlowNodeConfiguration` / `FlowNodeIODataType` values.
fn passthrough_definition() -> Value {
    json!({
        "nodes": [
            {
                "name": "FlowInput",
                "type": "Input",
                "configuration": { "input": {} },
                "outputs": [{ "name": "document", "type": "String" }],
            },
            {
                "name": "FlowOutput",
                "type": "Output",
                "configuration": { "output": {} },
                "inputs": [{ "name": "document", "type": "String", "expression": "$.data" }],
            },
        ],
        "connections": [
            {
                "name": "InputToOutput",
                "type": "Data",
                "source": "FlowInput",
                "target": "FlowOutput",
                "configuration": { "data": { "sourceOutput": "document", "targetInput": "document" } },
            },
        ],
    })
}

/// The same `Input` fanned out to TWO `Output` nodes, so one invocation delivers two
/// outputs.
fn fan_out_definition() -> Value {
    json!({
        "nodes": [
            {
                "name": "FlowInput",
                "type": "Input",
                "configuration": { "input": {} },
                "outputs": [{ "name": "document", "type": "Object" }],
            },
            {
                "name": "LeftOutput",
                "type": "Output",
                "configuration": { "output": {} },
                "inputs": [{ "name": "document", "type": "Object", "expression": "$.data.left" }],
            },
            {
                "name": "RightOutput",
                "type": "Output",
                "configuration": { "output": {} },
                "inputs": [{ "name": "document", "type": "Object", "expression": "$.data.right" }],
            },
        ],
        "connections": [
            {
                "name": "InputToLeft",
                "type": "Data",
                "source": "FlowInput",
                "target": "LeftOutput",
                "configuration": { "data": { "sourceOutput": "document", "targetInput": "document" } },
            },
            {
                "name": "InputToRight",
                "type": "Data",
                "source": "FlowInput",
                "target": "RightOutput",
                "configuration": { "data": { "sourceOutput": "document", "targetInput": "document" } },
            },
        ],
    })
}

/// One `InvokeFlow` request body entering the flow at `node` with `document`.
fn flow_request(node: &str, document: Value) -> Value {
    json!({ "inputs": [{ "nodeName": node, "nodeOutputName": "document", "content": { "document": document } }] })
}

/// The single-violation envelope over one `@pattern` clause, in the framework's own shape.
fn one_pattern_error(value: &str, member: &str, pattern: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {pattern}"
    )
}

/// RULE: an agent or an alias the Bedrock Agent build-time plane does not hold is the
/// modelled `ResourceNotFoundException` (404) with this plane's fixed text for that class,
/// whichever half of the pair is the missing one.
#[tokio::test]
async fn an_agent_or_alias_the_plane_does_not_hold_is_refused_as_not_found() {
    let emulator = Emulator::new();
    let existing = emulator.agent_with_memory("held-agent", None).await;

    // (case, agentId, agentAliasId) — every value satisfies its member's own constraints.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "an agent id the plane never minted",
            "ABCDEFGHIJ".to_string(),
            DRAFT_ALIAS.to_string(),
        ),
        (
            "a one-character agent id, the shortest the model admits",
            "A".to_string(),
            DRAFT_ALIAS.to_string(),
        ),
        (
            "a ten-character agent id, the longest the model admits",
            "0123456789".to_string(),
            DRAFT_ALIAS.to_string(),
        ),
        (
            "an agent the plane holds, an alias it does not carry",
            existing.clone(),
            "ZYXWVUTSRQ".to_string(),
        ),
        (
            "an agent the plane holds, a one-character alias",
            existing.clone(),
            "A".to_string(),
        ),
    ];

    for (case, agent, alias) in rows {
        let reply = emulator
            .invoke_agent(&agent, &alias, SESSION, json!({ "inputText": "hello" }))
            .await;
        assert_eq!(
            reply.status,
            404,
            "case {case:?}: expected 404, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "ResourceNotFoundException",
            "case {case:?}: expected the modelled not-found error, got {:?}",
            reply.json(case)
        );
        assert_eq!(
            reply.message(case),
            RESOURCE_NOT_FOUND,
            "case {case:?}: expected this plane's fixed not-found text, got {:?}",
            reply.message(case)
        );
        assert_eq!(
            reply.content_type, "application/json",
            "case {case:?}: a refusal is a JSON error, got {:?}",
            reply.content_type
        );
    }
}

/// RULE: a URI label outside its modelled constraint is refused with the modelled
/// `ValidationException` (400), naming the member, before any agent is resolved — so an
/// invalid argument never reads as a missing resource.
#[tokio::test]
async fn an_invoke_agent_label_outside_its_constraint_is_refused_as_invalid() {
    /// One malformed label, and what the refusal has to say about it.
    struct Row {
        case: &'static str,
        agent: String,
        alias: String,
        /// The `sessionId` as it travels in the URI.
        session: String,
        /// The member the clause must name.
        member: &'static str,
        /// The whole message, where this service's rule fixes it.
        message: Option<String>,
    }

    let rows = vec![
        Row {
            case: "a sessionId carrying a character outside its class",
            agent: "ABCDEFGHIJ".to_string(),
            alias: DRAFT_ALIAS.to_string(),
            session: "bad!session".to_string(),
            member: "sessionId",
            message: Some(one_pattern_error(
                "bad!session",
                "sessionId",
                SESSION_ID_PATTERN,
            )),
        },
        Row {
            case: "a sessionId carrying a space, percent-encoded as a client sends it",
            agent: "ABCDEFGHIJ".to_string(),
            alias: DRAFT_ALIAS.to_string(),
            session: "bad%20session%20id".to_string(),
            member: "sessionId",
            message: None,
        },
        Row {
            case: "a sessionId below the modelled minimum length",
            agent: "ABCDEFGHIJ".to_string(),
            alias: DRAFT_ALIAS.to_string(),
            session: "a".to_string(),
            member: "sessionId",
            message: None,
        },
        Row {
            case: "a sessionId above the modelled maximum length",
            agent: "ABCDEFGHIJ".to_string(),
            alias: DRAFT_ALIAS.to_string(),
            session: "s".repeat(101),
            member: "sessionId",
            message: None,
        },
        Row {
            case: "an agentId carrying a character outside its class",
            agent: "AGENT-0001".to_string(),
            alias: DRAFT_ALIAS.to_string(),
            session: SESSION.to_string(),
            member: "agentId",
            message: Some(one_pattern_error(
                "AGENT-0001",
                "agentId",
                "[0-9a-zA-Z]+",
            )),
        },
        Row {
            case: "an agentId above the modelled maximum length",
            agent: "A".repeat(11),
            alias: DRAFT_ALIAS.to_string(),
            session: SESSION.to_string(),
            member: "agentId",
            message: None,
        },
        Row {
            case: "an agentAliasId carrying a character outside its class",
            agent: "ABCDEFGHIJ".to_string(),
            alias: "TST_ALIAS".to_string(),
            session: SESSION.to_string(),
            member: "agentAliasId",
            message: None,
        },
    ];

    let emulator = Emulator::new();
    for row in rows {
        let reply = emulator
            .invoke_agent(&row.agent, &row.alias, &row.session, json!({}))
            .await;
        assert_eq!(
            reply.status,
            400,
            "case {:?}: expected 400, got {} {:?}",
            row.case,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(row.case),
            "ValidationException",
            "case {:?}: expected ValidationException, got {:?}",
            row.case,
            reply.json(row.case)
        );
        assert!(
            reply.message(row.case).contains(row.member),
            "case {:?}: the refusal must name {:?}, got {:?}",
            row.case,
            row.member,
            reply.message(row.case)
        );
        if let Some(expected) = row.message {
            assert_eq!(
                reply.message(row.case),
                expected,
                "case {:?}: expected the clause this service states, got {:?}",
                row.case,
                reply.message(row.case)
            );
        }
        assert_ne!(
            reply.content_type, EVENT_STREAM,
            "case {:?}: a refused request must not answer an event stream",
            row.case
        );
    }
}

/// RULE: an agent the build-time plane HOLDS is served, as an event stream carrying the one
/// `chunk` the modelled `ResponseStream` declares, whose text says no model ran and names
/// the alias that answered — never the caller's own `inputText` handed back.
#[tokio::test]
async fn an_invocation_of_a_held_agent_streams_one_chunk() {
    /// One request body, and the text it must NOT be answered with.
    struct Row {
        case: &'static str,
        body: Value,
        /// The `inputText` the request carried, which the answer may not echo.
        echoed: Option<&'static str>,
    }

    let rows = vec![
        Row {
            case: "a bare invocation with input text",
            body: json!({ "inputText": "what is the pump service interval" }),
            echoed: Some("what is the pump service interval"),
        },
        Row {
            case: "an invocation with no input text at all",
            body: json!({}),
            echoed: None,
        },
        Row {
            case: "an invocation naming the latency profile of the model",
            body: json!({
                "inputText": "status please",
                "bedrockModelConfigurations": { "performanceConfig": { "latency": "optimized" } },
            }),
            echoed: Some("status please"),
        },
        Row {
            case: "an invocation asking for the final response to be streamed",
            body: json!({
                "inputText": "status please",
                "streamingConfigurations": { "streamFinalResponse": true },
            }),
            echoed: Some("status please"),
        },
        Row {
            case: "an invocation naming the agent it is made on behalf of",
            body: json!({ "endSession": false }),
            echoed: None,
        },
    ];

    let emulator = Emulator::new();
    let agent = emulator.agent_with_memory("served-agent", None).await;
    for row in rows {
        let reply = emulator
            .invoke_agent(&agent, DRAFT_ALIAS, SESSION, row.body.clone())
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {:?}: expected 200, got {} {:?}",
            row.case,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.content_type, EVENT_STREAM,
            "case {:?}: expected an event-stream body, got {:?}",
            row.case, reply.content_type
        );
        let frames = decode_frames(row.case, &reply.body);
        assert_eq!(
            frame_names(&frames),
            vec!["chunk"],
            "case {:?}: expected the one modelled chunk event",
            row.case
        );
        assert_eq!(
            frames[0].message_type, "event",
            "case {:?}: a chunk is a data event, got {:?}",
            row.case, frames[0].message_type
        );
        let encoded = frames[0].payload["bytes"].as_str().unwrap_or_else(|| {
            panic!(
                "case {:?}: the chunk must carry bytes, got {:?}",
                row.case, frames[0].payload
            )
        });
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap_or_else(|error| {
                panic!("case {:?}: the chunk's bytes must be base64: {error}", row.case)
            });
        let text = String::from_utf8(decoded)
            .unwrap_or_else(|error| panic!("case {:?}: the chunk must be UTF-8: {error}", row.case));
        assert!(
            text.contains(&agent) && text.contains(DRAFT_ALIAS),
            "case {:?}: the answer must name the alias that answered, got {text:?}",
            row.case
        );
        if let Some(echoed) = row.echoed {
            assert!(
                !text.contains(echoed),
                "case {:?}: the answer must not be the submitted input handed back, got \
                 {text:?}",
                row.case
            );
        }
        assert!(
            frames[0].payload.get("attribution").is_none(),
            "case {:?}: no knowledge base was queried, so no attribution may be reported, \
             got {:?}",
            row.case,
            frames[0].payload
        );
    }
}

/// RULE: an invocation that ENDS its session under a `memoryId` leaves the session's summary
/// in the memory of the alias it reached — and only when the stored agent is configured to
/// keep one — so `GetAgentMemory` reports a session that was really invoked.
#[tokio::test]
async fn ending_a_session_under_a_memory_id_is_what_fills_the_alias_memory() {
    /// One invocation, and the sessions the alias's memory must hold afterwards.
    struct Row {
        case: &'static str,
        /// The agent's `memoryConfiguration`, or `None` for an agent that keeps none.
        memory: Option<Value>,
        /// `(sessionId, endSession, memoryId)` of each invocation, in order.
        invocations: Vec<(&'static str, bool, Option<&'static str>)>,
        /// The session ids the memory holds once they have all run.
        remembered: Vec<&'static str>,
    }

    /// The memory configuration of an agent that keeps session summaries. `storageDays` is
    /// the build-time model's own `default` for the member.
    fn session_summary_memory() -> Value {
        json!({ "enabledMemoryTypes": ["SESSION_SUMMARY"], "storageDays": 30 })
    }

    let rows = vec![
        Row {
            case: "one session ended under a memory id",
            memory: Some(session_summary_memory()),
            invocations: vec![("session-one", true, Some("memory-one"))],
            remembered: vec!["session-one"],
        },
        Row {
            case: "two sessions ended under the same memory id",
            memory: Some(session_summary_memory()),
            invocations: vec![
                ("session-one", true, Some("memory-one")),
                ("session-two", true, Some("memory-one")),
            ],
            remembered: vec!["session-one", "session-two"],
        },
        Row {
            case: "the same session ended twice is remembered once",
            memory: Some(session_summary_memory()),
            invocations: vec![
                ("session-one", true, Some("memory-one")),
                ("session-one", true, Some("memory-one")),
            ],
            remembered: vec!["session-one"],
        },
        Row {
            case: "a session that was not ended",
            memory: Some(session_summary_memory()),
            invocations: vec![("session-one", false, Some("memory-one"))],
            remembered: vec![],
        },
        Row {
            case: "a session ended with no memory id to record it against",
            memory: Some(session_summary_memory()),
            invocations: vec![("session-one", true, None)],
            remembered: vec![],
        },
        Row {
            case: "an agent configured to keep no session summary",
            memory: None,
            invocations: vec![("session-one", true, Some("memory-one"))],
            remembered: vec![],
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let agent = emulator
            .agent_with_memory(&format!("memory-agent-{index}"), row.memory.clone())
            .await;
        for (session, end_session, memory_id) in &row.invocations {
            let mut body = json!({ "inputText": "hello", "endSession": end_session });
            if let Some(memory_id) = memory_id {
                body["memoryId"] = json!(memory_id);
            }
            let reply = emulator
                .invoke_agent(&agent, DRAFT_ALIAS, session, body)
                .await;
            assert_eq!(
                reply.status,
                200,
                "case {:?}: expected the invocation to be served, got {} {:?}",
                row.case,
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
        }
        // Read back through the implemented reader, which is the only way a client sees it.
        let read = emulator
            .bedrock(
                ACCOUNT_KEY,
                "GET",
                &format!(
                    "/agents/{agent}/agentAliases/{DRAFT_ALIAS}/memories\
                     ?memoryType=SESSION_SUMMARY&memoryId=memory-one"
                ),
                None,
            )
            .await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: expected GetAgentMemory to answer 200, got {} {:?}",
            row.case,
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let body = read.json(row.case);
        let mut sessions: Vec<String> = body["memoryContents"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|content| {
                content["sessionSummary"]["sessionId"]
                    .as_str()
                    .map(str::to_string)
            })
            .collect();
        sessions.sort();
        let mut expected: Vec<String> = row.remembered.iter().map(|id| (*id).to_string()).collect();
        expected.sort();
        assert_eq!(
            sessions, expected,
            "case {:?}: expected the memory to hold {expected:?}, got {sessions:?} ({body:?})",
            row.case
        );
    }
}

/// RULE: a leg of an agent invocation this emulator cannot compute is refused with the
/// modelled `ValidationException` (400) naming the member, never served with the member
/// ignored.
#[tokio::test]
async fn an_agent_invocation_leg_this_service_cannot_compute_is_refused() {
    // (case, body, the member the refusal must name)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a session state carrying attributes a prompt would read",
            json!({ "sessionState": { "sessionAttributes": { "unit": "bay-2" } } }),
            "sessionState",
        ),
        (
            "a session state returning an action group's results",
            json!({ "sessionState": { "invocationId": "abc" } }),
            "sessionState",
        ),
        (
            "a request asking for the orchestration trace",
            json!({ "enableTrace": true }),
            "enableTrace",
        ),
        (
            "a request setting the guardrail interval",
            json!({ "streamingConfigurations": { "applyGuardrailInterval": 20 } }),
            "applyGuardrailInterval",
        ),
        (
            "a request selecting which earlier turns populate the prompt",
            json!({ "promptCreationConfigurations": { "previousConversationTurnsToInclude": 2 } }),
            "promptCreationConfigurations",
        ),
        (
            "a request excluding earlier thinking steps from the prompt",
            json!({ "promptCreationConfigurations": { "excludePreviousThinkingSteps": true } }),
            "promptCreationConfigurations",
        ),
    ];

    let emulator = Emulator::new();
    let agent = emulator.agent_with_memory("refusing-agent", None).await;
    for (case, body, member) in rows {
        let reply = emulator
            .invoke_agent(&agent, DRAFT_ALIAS, SESSION, body)
            .await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected 400, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            reply.json(case)
        );
        assert!(
            reply.message(case).contains(member),
            "case {case:?}: the refusal must name {member:?}, got {:?}",
            reply.message(case)
        );
    }
}

/// RULE: once an `InvokeFlow` invocation has begun, a modelled exception is delivered INSIDE
/// the stream as the `FlowResponseStream` member of its name, in a 200
/// `application/vnd.amazon.eventstream` response — never as a transport-level HTTP error.
#[tokio::test]
async fn a_flow_the_plane_does_not_hold_is_delivered_inside_the_stream() {
    let emulator = Emulator::new();
    let (held_flow, held_arn) = emulator
        .create_flow("held-flow", passthrough_definition())
        .await;
    emulator.prepare_flow("setup", &held_flow).await;
    let (held_alias, held_alias_arn) = emulator
        .create_flow_alias("setup", &held_flow, "DRAFT")
        .await;
    let (other_flow, _) = emulator
        .create_flow("other-flow", passthrough_definition())
        .await;

    // (case, flowIdentifier, flowAliasIdentifier) — every value satisfies its own pattern.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a bare flow id the plane never minted",
            "FLOW123456".to_string(),
            held_alias.clone(),
        ),
        (
            "a flow ARN naming a flow the plane never minted",
            format!("arn:aws:bedrock:{REGION}:000000000000:flow/FLOW123456"),
            held_alias.clone(),
        ),
        (
            "a flow the plane holds, an alias it does not carry",
            held_flow.clone(),
            "ALIAS12345".to_string(),
        ),
        (
            "a flow the plane holds, addressed by its ARN, with an unknown alias",
            held_arn.clone(),
            "ALIAS12345".to_string(),
        ),
        (
            "an alias ARN naming another flow than the path does",
            other_flow.clone(),
            held_alias_arn.clone(),
        ),
        (
            "the draft alias token, which no flow here has an alias under",
            held_flow.clone(),
            "TSTALIASID".to_string(),
        ),
    ];

    for (case, flow, alias) in rows {
        let reply = emulator
            .invoke_flow(
                &label(&flow),
                &label(&alias),
                flow_request("FlowInput", json!({ "data": "hello" })),
            )
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: an invocation that has begun answers 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.content_type, EVENT_STREAM,
            "case {case:?}: expected an event-stream body, got {:?}",
            reply.content_type
        );
        let frames = decode_frames(case, &reply.body);
        assert_eq!(
            frame_names(&frames),
            vec!["resourceNotFoundException"],
            "case {case:?}: expected the in-stream not-found event"
        );
        assert_eq!(
            frames[0].message_type, "exception",
            "case {case:?}: a modelled exception travels as an exception frame, got {:?}",
            frames[0].message_type
        );
        assert_eq!(
            frames[0].payload["message"].as_str(),
            Some(RESOURCE_NOT_FOUND),
            "case {case:?}: expected this plane's fixed not-found text, got {:?}",
            frames[0].payload
        );
    }
}

/// RULE: an `InvokeFlow` label outside its modelled constraint is refused with the modelled
/// `ValidationException` (400) before the invocation begins — so it is an HTTP error and not
/// an in-stream event.
#[tokio::test]
async fn an_invoke_flow_label_outside_its_constraint_is_refused_as_invalid() {
    // (case, flowIdentifier, flowAliasIdentifier, member, whole message where fixed)
    let rows: Vec<(&str, String, String, &str, Option<String>)> = vec![
        (
            "a flowIdentifier that is neither a flow ARN nor a flow id",
            "bad".to_string(),
            "ALIAS12345".to_string(),
            "flowIdentifier",
            Some(one_pattern_error("bad", "flowIdentifier", FLOW_IDENTIFIER_PATTERN)),
        ),
        (
            "a flowIdentifier of nine characters, one short of the identifier form",
            "FLOW12345".to_string(),
            "ALIAS12345".to_string(),
            "flowIdentifier",
            Some(one_pattern_error(
                "FLOW12345",
                "flowIdentifier",
                FLOW_IDENTIFIER_PATTERN,
            )),
        ),
        (
            "a flowIdentifier carrying a character outside the identifier class",
            "FLOW-12345".to_string(),
            "ALIAS12345".to_string(),
            "flowIdentifier",
            None,
        ),
        (
            "a flowAliasIdentifier ending outside its character class",
            "FLOW123456".to_string(),
            "badalias.".to_string(),
            "flowAliasIdentifier",
            None,
        ),
    ];

    let emulator = Emulator::new();
    for (case, flow, alias, member, message) in rows {
        let reply = emulator
            .invoke_flow(
                &flow,
                &alias,
                flow_request("FlowInput", json!({ "data": "hello" })),
            )
            .await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected 400, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            reply.json(case)
        );
        assert!(
            reply.message(case).contains(member),
            "case {case:?}: the refusal must name {member:?}, got {:?}",
            reply.message(case)
        );
        if let Some(expected) = message {
            assert_eq!(
                reply.message(case),
                expected,
                "case {case:?}: expected the clause this service states, got {:?}",
                reply.message(case)
            );
        }
        assert_ne!(
            reply.content_type, EVENT_STREAM,
            "case {case:?}: a refusal decided before the stream opens is an HTTP error"
        );
    }
}

/// RULE: a PREPARED flow runs its own stored definition — the request's document enters at
/// the flow's input node and leaves at the output nodes that node is connected to, one
/// `flowOutputEvent` each, then the `flowCompletionEvent` the modelled
/// `FlowCompletionReason` names for a run that reached the end.
#[tokio::test]
async fn a_prepared_flow_delivers_the_outputs_its_own_definition_reaches() {
    /// One flow, and the output nodes an invocation of it must reach.
    struct Row {
        case: &'static str,
        definition: Value,
        /// The output nodes, in the order the stream reports them.
        outputs: Vec<&'static str>,
        /// The document the invocation carries.
        document: Value,
    }

    let rows = vec![
        Row {
            case: "an input node wired to one output node",
            definition: passthrough_definition(),
            outputs: vec!["FlowOutput"],
            document: json!({ "data": "the pump is serviced every 90 days" }),
        },
        Row {
            case: "an input node fanned out to two output nodes",
            definition: fan_out_definition(),
            outputs: vec!["LeftOutput", "RightOutput"],
            document: json!({ "data": { "left": 1, "right": [true, null, "x"] } }),
        },
        Row {
            case: "the same fan-out carrying a scalar document",
            definition: fan_out_definition(),
            outputs: vec!["LeftOutput", "RightOutput"],
            document: json!("a bare string"),
        },
    ];

    let emulator = Emulator::new();
    for (index, row) in rows.into_iter().enumerate() {
        let (flow, _) = emulator
            .create_flow(&format!("ran-flow-{index}"), row.definition.clone())
            .await;
        emulator.prepare_flow(row.case, &flow).await;
        let (alias, _) = emulator.create_flow_alias(row.case, &flow, "DRAFT").await;

        let reply = emulator
            .invoke_flow(
                &flow,
                &alias,
                flow_request("FlowInput", row.document.clone()),
            )
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {:?}: expected 200, got {} {:?}",
            row.case,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.content_type, EVENT_STREAM,
            "case {:?}: expected an event-stream body, got {:?}",
            row.case, reply.content_type
        );
        let frames = decode_frames(row.case, &reply.body);
        let mut expected: Vec<&str> = vec!["flowOutputEvent"; row.outputs.len()];
        expected.push("flowCompletionEvent");
        assert_eq!(
            frame_names(&frames),
            expected,
            "case {:?}: expected one output event per output node, then the completion",
            row.case
        );
        for (frame, node) in frames.iter().zip(&row.outputs) {
            assert_eq!(
                frame.payload["nodeName"].as_str(),
                Some(*node),
                "case {:?}: expected the output to name {node:?}, got {:?}",
                row.case,
                frame.payload
            );
            assert_eq!(
                frame.payload["nodeType"].as_str(),
                Some("FlowOutputNode"),
                "case {:?}: an output leaves at a node of that type, got {:?}",
                row.case,
                frame.payload
            );
            assert_eq!(
                frame.payload["content"]["document"], row.document,
                "case {:?}: expected the document that entered the flow, got {:?}",
                row.case, frame.payload
            );
        }
        let completion = &frames[frames.len() - 1].payload;
        assert_eq!(
            completion["completionReason"].as_str(),
            Some("SUCCESS"),
            "case {:?}: expected the run to complete successfully, got {completion:?}",
            row.case
        );
    }
}

/// RULE: a flow invocation this service cannot run is refused INSIDE the stream, as the
/// `validationException` member of `FlowResponseStream`, naming what could not be computed.
#[tokio::test]
async fn a_flow_invocation_this_service_cannot_run_is_refused_inside_the_stream() {
    let emulator = Emulator::new();
    // A flow that was never prepared: its definition is stored, but nothing has turned it
    // into something invocable.
    let (unprepared, _) = emulator
        .create_flow("unprepared-flow", passthrough_definition())
        .await;
    // `CreateFlowAlias` routes to the DRAFT version, which every stored flow has.
    let (unprepared_alias, _) = emulator
        .create_flow_alias("setup", &unprepared, "DRAFT")
        .await;

    let (ready, _) = emulator
        .create_flow("ready-flow", passthrough_definition())
        .await;
    emulator.prepare_flow("setup", &ready).await;
    let (ready_alias, _) = emulator.create_flow_alias("setup", &ready, "DRAFT").await;

    // (case, flowId, aliasId, body, what the refusal must name)
    let rows: Vec<(&str, &String, &String, Value, &str)> = vec![
        (
            "a flow nothing has prepared",
            &unprepared,
            &unprepared_alias,
            flow_request("FlowInput", json!({ "data": "hello" })),
            "NotPrepared",
        ),
        (
            "an input node the flow does not hold",
            &ready,
            &ready_alias,
            flow_request("NoSuchNode", json!({ "data": "hello" })),
            "NoSuchNode",
        ),
        (
            "a request asking for the node-level trace",
            &ready,
            &ready_alias,
            {
                let mut body = flow_request("FlowInput", json!({ "data": "hello" }));
                body["enableTrace"] = json!(true);
                body
            },
            "enableTrace",
        ),
        (
            "a request naming an execution to continue",
            &ready,
            &ready_alias,
            {
                let mut body = flow_request("FlowInput", json!({ "data": "hello" }));
                body["executionId"] = json!("exec-1");
                body
            },
            "executionId",
        ),
    ];

    for (case, flow, alias, body, names) in rows {
        let reply = emulator.invoke_flow(flow, alias, body).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: an invocation that has begun answers 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let frames = decode_frames(case, &reply.body);
        assert_eq!(
            frame_names(&frames),
            vec!["validationException"],
            "case {case:?}: expected the in-stream validation event"
        );
        let message = frames[0].payload["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(names),
            "case {case:?}: the refusal must name {names:?}, got {message:?}"
        );
    }
}

/// A flow or alias ARN as a client percent-encodes a non-greedy `@httpLabel`: an ARN carries
/// `:` and `/`, which would otherwise split the path segment.
fn label(identifier: &str) -> String {
    identifier.replace(':', "%3A").replace('/', "%2F")
}
