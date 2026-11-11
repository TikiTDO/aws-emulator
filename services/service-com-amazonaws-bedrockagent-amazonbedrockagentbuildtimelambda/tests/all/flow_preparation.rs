// SPDX-License-Identifier: CC0-1.0

//! `PrepareFlow`, end to end over HTTP: packaging the DRAFT version of a stored
//! flow, and the outcome the clock then reports through `GetFlow`.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.
//!
//! The one thing the tests reach into the store for is the INSTANT a pending
//! preparation comes due: the handler stores it rather than sleeping, so a test
//! moves it into the past instead of waiting out
//! [`FLOW_PREPARATION_SPAN`](service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::FLOW_PREPARATION_SPAN).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::{
    service_state, FLOW_PREPARATION_SPAN,
};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on, and the source of the region its ARNs
/// carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// The region of [`AUTHORIZATION`]'s credential scope.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A role ARN matching the modelled `FlowExecutionRoleArn` pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/(service-role/)?.+$`.
const EXECUTION_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype` or the body's
    /// `__type`; `None` on a success.
    error_type: Option<String>,
    body: Value,
}

impl Answer {
    fn member(&self, path: &[&str]) -> &Value {
        let mut at = &self.body;
        for key in path {
            at = &at[*key];
        }
        at
    }

    fn string(&self, path: &[&str]) -> String {
        self.member(path)
            .as_str()
            .unwrap_or_else(|| panic!("{path:?} is not a string in {:?}", self.body))
            .to_string()
    }

    /// The `type` of every reported `FlowValidation`, in the order the response
    /// listed them.
    fn validation_types(&self) -> Vec<String> {
        self.member(&["validations"])
            .as_array()
            .map(|findings| {
                findings
                    .iter()
                    .map(|finding| finding["type"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `severity` of every reported `FlowValidation`.
    fn validation_severities(&self) -> Vec<String> {
        self.member(&["validations"])
            .as_array()
            .map(|findings| {
                findings
                    .iter()
                    .map(|finding| finding["severity"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let bytes = body
            .map(|value| Bytes::from(serde_json::to_vec(&value).expect("serialize request body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "bedrock-agent.eu-west-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes)
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let raw = response.into_body();
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|e| {
                panic!(
                    "{method} {uri} -> {status} body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        let error_type = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        Answer {
            status,
            error_type,
            body,
        }
    }

    /// `CreateFlow`, which mints the flow a preparation is asked for.
    async fn create_flow(&self, case: &str, request: Value) -> Answer {
        let created = self.call("POST", "/flows/", Some(request)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected CreateFlow -> 201, got {created:?}"
        );
        created
    }

    async fn update_flow(&self, identifier: &str, request: Value) -> Answer {
        let escaped = Self::escape(identifier);
        self.call("PUT", &format!("/flows/{escaped}/"), Some(request))
            .await
    }

    async fn prepare_flow(&self, identifier: &str) -> Answer {
        let escaped = Self::escape(identifier);
        self.call("POST", &format!("/flows/{escaped}/"), Some(json!({})))
            .await
    }

    async fn get_flow(&self, identifier: &str) -> Answer {
        let escaped = Self::escape(identifier);
        self.call("GET", &format!("/flows/{escaped}/"), None).await
    }

    fn escape(identifier: &str) -> String {
        identifier.replace(':', "%3A").replace('/', "%2F")
    }

    /// When the preparation in flight for `flow_id` comes due, or `None` when the
    /// store owes the flow nothing.
    fn preparation_due(&self, flow_id: &str) -> Option<SystemTime> {
        let state = service_state(&*self.engine).expect("the bedrock-agent store");
        let state = state.lock();
        state
            .flows
            .get(flow_id)
            .expect("the stored flow")
            .pending_preparation
            .as_ref()
            .map(|pending| pending.due)
    }

    /// Move the instant the pending preparation comes due into the past, so the
    /// next caller observes the outcome without the test sleeping for the span.
    fn make_preparation_due(&self, case: &str, flow_id: &str) {
        let state = service_state(&*self.engine).expect("the bedrock-agent store");
        let mut state = state.lock();
        let pending = state
            .flows
            .get_mut(flow_id)
            .expect("the stored flow")
            .pending_preparation
            .as_mut()
            .unwrap_or_else(|| panic!("case {case}: expected a preparation in flight"));
        pending.due = SystemTime::now() - Duration::from_secs(1);
    }
}

/// A passthrough flow definition: an `Input` node wired straight to an `Output`
/// node. The node types, the union keys and the IO data types are the modelled
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

/// The same `Input` fanned out to two `Output` nodes: a valid definition that
/// differs from [`passthrough_definition`] in node and connection COUNT.
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

/// A definition with a starting node and nothing to end at.
fn input_only_definition() -> Value {
    json!({
        "nodes": [
            {
                "name": "FlowInput",
                "type": "Input",
                "configuration": { "input": {} },
                "outputs": [{ "name": "document", "type": "String" }],
            },
        ],
    })
}

/// A passthrough whose connection names a target node the definition never
/// declares, so a reference does not resolve.
fn dangling_connection_definition() -> Value {
    let mut definition = passthrough_definition();
    definition["connections"][0]["target"] = json!("NoSuchNode");
    definition
}

/// A passthrough plus an extra `Output` node nothing connects to: the definition
/// carries a finding, but a NON-fatal one (the `UnreachableNode` check is
/// modelled at `Warning` severity).
fn unreachable_node_definition() -> Value {
    let mut definition = passthrough_definition();
    definition["nodes"].as_array_mut().expect("the node list").push(json!({
        "name": "OrphanOutput",
        "type": "Output",
        "configuration": { "output": {} },
        "inputs": [{ "name": "document", "type": "String", "expression": "$.data" }],
    }));
    definition
}

/// The outcome a preparation of one definition reaches, and the findings it
/// publishes once that outcome is due.
struct Case {
    /// What makes this row different from the others.
    name: &'static str,
    /// The `definition` member `CreateFlow` is given, or `None` to omit it.
    definition: Option<Value>,
    /// The `FlowStatus` the flow reaches once the preparation is due.
    outcome: &'static str,
    /// `FlowValidation$type` values the flow must report at that point. The
    /// names are the validator's own
    /// (`services/.../src/ops/validate_flow_definition.rs`), which is what
    /// `ValidateFlowDefinition` reports for the same definition.
    expected_types: &'static [&'static str],
}

/// The table: rows differ in whether a definition is stored at all, in its size,
/// and in HOW it is defective — a missing starting node, a missing ending node,
/// a reference that does not resolve, and a defect that is not fatal.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "no definition stored at all",
            definition: None,
            outcome: "Failed",
            expected_types: &["MissingStartingNodes", "MissingEndingNodes"],
        },
        Case {
            name: "an empty definition object",
            definition: Some(json!({})),
            outcome: "Failed",
            expected_types: &["MissingStartingNodes", "MissingEndingNodes"],
        },
        Case {
            name: "a passthrough definition",
            definition: Some(passthrough_definition()),
            outcome: "Prepared",
            expected_types: &[],
        },
        Case {
            name: "a larger valid definition: three nodes, two connections",
            definition: Some(fan_out_definition()),
            outcome: "Prepared",
            expected_types: &[],
        },
        Case {
            name: "a definition with no ending node",
            definition: Some(input_only_definition()),
            outcome: "Failed",
            expected_types: &["MissingEndingNodes"],
        },
        Case {
            name: "a connection naming a node the definition does not declare",
            definition: Some(dangling_connection_definition()),
            outcome: "Failed",
            // The unresolved reference is one defect; the defects that follow
            // from the connection landing nowhere are reported beside it.
            expected_types: &[
                "UnfulfilledNodeInput",
                "UnknownConnectionTarget",
                "UnreachableNode",
            ],
        },
        Case {
            name: "a definition carrying a finding that is not fatal",
            definition: Some(unreachable_node_definition()),
            outcome: "Prepared",
            expected_types: &[],
        },
    ]
}

/// `PrepareFlow` accepts the call with the flow in `Preparing`, and the CLOCK
/// then carries it to the outcome the stored definition decides: `Prepared` when
/// nothing fatal is wrong with it, `Failed` carrying the findings when something
/// is.
#[tokio::test]
async fn prepare_flow_accepts_and_the_clock_reports_the_outcome() {
    let emulator = Emulator::new();

    for case in cases() {
        let name = case.name;
        let mut request = json!({ "name": "prepared-flow", "executionRoleArn": EXECUTION_ROLE });
        if let Some(definition) = case.definition.clone() {
            request["definition"] = definition;
        }
        let created = emulator.create_flow(name, request).await;
        let flow_id = created.string(&["id"]);

        let prepared = emulator.prepare_flow(&flow_id).await;
        assert_eq!(
            prepared.status, 202,
            "case {name}: expected PrepareFlow -> 202, got {prepared:?}"
        );
        assert_eq!(
            prepared.string(&["id"]),
            flow_id,
            "case {name}: PrepareFlow reports the flow it prepared, got {:?}",
            prepared.body
        );
        assert_eq!(
            prepared.string(&["status"]),
            "Preparing",
            "case {name}: an accepted preparation leaves the flow being prepared, got {:?}",
            prepared.body
        );

        // The transient status is observable: the outcome is owed at an instant
        // still in the future, within the span the store publishes.
        let due = emulator
            .preparation_due(&flow_id)
            .unwrap_or_else(|| panic!("case {name}: expected a preparation in flight"));
        let now = SystemTime::now();
        assert!(
            due > now,
            "case {name}: expected the outcome to come due later than the accepting call, \
             got {due:?} against {now:?}"
        );
        assert!(
            due <= now + FLOW_PREPARATION_SPAN,
            "case {name}: expected the outcome within {FLOW_PREPARATION_SPAN:?} of the call, \
             got {due:?} against {now:?}"
        );

        // Read back while the preparation is still in flight: the flow is being
        // prepared, and nothing has failed yet, so it reports an EMPTY findings
        // list rather than no list at all.
        let during = emulator.get_flow(&flow_id).await;
        assert_eq!(
            during.status, 200,
            "case {name}: expected GetFlow -> 200, got {during:?}"
        );
        assert_eq!(
            during.string(&["status"]),
            "Preparing",
            "case {name}: a flow still being prepared reports Preparing, got {:?}",
            during.body
        );
        assert_eq!(
            during.member(&["validations"]),
            &json!([]),
            "case {name}: expected an empty findings list while the flow is being prepared, \
             got {:?}",
            during.member(&["validations"])
        );

        // The clock reaches the instant the outcome was owed at.
        emulator.make_preparation_due(name, &flow_id);

        let after = emulator.get_flow(&flow_id).await;
        assert_eq!(
            after.string(&["status"]),
            case.outcome,
            "case {name}: expected the flow to reach {}, got {:?}",
            case.outcome,
            after.body
        );
        let reported = after.validation_types();
        for expected in case.expected_types {
            assert!(
                reported.iter().any(|kind| kind == expected),
                "case {name}: expected a {expected} finding, got {reported:?}"
            );
        }
        if case.expected_types.is_empty() {
            assert_eq!(
                after.member(&["validations"]),
                &json!([]),
                "case {name}: expected a preparation that succeeded to publish an empty \
                 findings list, got {:?}",
                after.member(&["validations"])
            );
        } else {
            assert!(
                after
                    .validation_severities()
                    .iter()
                    .any(|severity| severity == "Error"),
                "case {name}: a failed preparation publishes at least one Error finding, got {:?}",
                after.member(&["validations"])
            );
        }

        // The outcome is stable: reading it again neither re-runs the
        // preparation nor changes what the next caller sees.
        let again = emulator.get_flow(&flow_id).await;
        assert_eq!(
            again.body, after.body,
            "case {name}: a second read reports the same flow"
        );
    }
}

/// Preparation is repeatable, and each `PrepareFlow` packages the definition as
/// it stands: a flow that already reached one outcome goes back through
/// `Preparing` and reaches the outcome its CURRENT definition decides.
#[tokio::test]
async fn preparing_again_packages_the_definition_as_it_stands() {
    let emulator = Emulator::new();

    // Rows: the two orders in which a flow can cross between outcomes, so
    // neither direction depends on the one before it.
    let journeys: [(&str, [(Value, &str); 2]); 2] = [
        (
            "defective, then valid",
            [
                (input_only_definition(), "Failed"),
                (passthrough_definition(), "Prepared"),
            ],
        ),
        (
            "valid, then defective",
            [
                (fan_out_definition(), "Prepared"),
                (dangling_connection_definition(), "Failed"),
            ],
        ),
    ];

    for (journey, steps) in journeys {
        let created = emulator
            .create_flow(
                journey,
                json!({
                    "name": "re-prepared-flow",
                    "executionRoleArn": EXECUTION_ROLE,
                    "definition": steps[0].0.clone(),
                }),
            )
            .await;
        let flow_id = created.string(&["id"]);

        for (step, (definition, outcome)) in steps.iter().enumerate() {
            let case = format!("{journey} step {step}");
            if step > 0 {
                let updated = emulator
                    .update_flow(
                        &flow_id,
                        json!({
                            "name": "re-prepared-flow",
                            "executionRoleArn": EXECUTION_ROLE,
                            "definition": definition.clone(),
                        }),
                    )
                    .await;
                assert_eq!(
                    updated.status, 200,
                    "case {case}: expected UpdateFlow -> 200, got {updated:?}"
                );
                assert_eq!(
                    updated.string(&["status"]),
                    "NotPrepared",
                    "case {case}: an updated flow needs preparing again, got {:?}",
                    updated.body
                );
            }

            let prepared = emulator.prepare_flow(&flow_id).await;
            assert_eq!(
                prepared.string(&["status"]),
                "Preparing",
                "case {case}: expected the preparation to be accepted, got {prepared:?}"
            );
            emulator.make_preparation_due(&case, &flow_id);

            let after = emulator.get_flow(&flow_id).await;
            assert_eq!(
                after.string(&["status"]),
                *outcome,
                "case {case}: expected the flow to reach {outcome}, got {:?}",
                after.body
            );
        }
    }
}

/// An `UpdateFlow` cancels a preparation still in flight: what was being
/// packaged was the definition the update replaced, so the flow stays
/// `NotPrepared` instead of later reporting an outcome for a definition that is
/// no longer stored.
#[tokio::test]
async fn updating_a_flow_cancels_the_preparation_in_flight() {
    let emulator = Emulator::new();

    // Rows: the update replaces a valid definition with a defective one and the
    // other way round, so neither a cancelled success nor a cancelled failure
    // can leak into the flow's status.
    for (case, first, second) in [
        (
            "a valid definition replaced",
            passthrough_definition(),
            input_only_definition(),
        ),
        (
            "a defective definition replaced",
            input_only_definition(),
            fan_out_definition(),
        ),
    ] {
        let created = emulator
            .create_flow(
                case,
                json!({
                    "name": "cancelled-preparation-flow",
                    "executionRoleArn": EXECUTION_ROLE,
                    "definition": first,
                }),
            )
            .await;
        let flow_id = created.string(&["id"]);

        let prepared = emulator.prepare_flow(&flow_id).await;
        assert_eq!(
            prepared.string(&["status"]),
            "Preparing",
            "case {case}: expected the preparation to be accepted, got {prepared:?}"
        );

        let updated = emulator
            .update_flow(
                &flow_id,
                json!({
                    "name": "cancelled-preparation-flow",
                    "executionRoleArn": EXECUTION_ROLE,
                    "definition": second,
                }),
            )
            .await;
        assert_eq!(
            updated.string(&["status"]),
            "NotPrepared",
            "case {case}: an updated flow needs preparing again, got {:?}",
            updated.body
        );
        assert!(
            emulator.preparation_due(&flow_id).is_none(),
            "case {case}: the update must leave no preparation owed"
        );

        let after = emulator.get_flow(&flow_id).await;
        assert_eq!(
            after.string(&["status"]),
            "NotPrepared",
            "case {case}: a cancelled preparation never reports an outcome, got {:?}",
            after.body
        );
        assert_eq!(
            after.member(&["validations"]),
            &json!([]),
            "case {case}: the update succeeded, so the flow reports an empty findings list, \
             got {:?}",
            after.member(&["validations"])
        );
    }
}

/// A definition carrying `nodes` nodes and `connections` connections, used to
/// drive the modelled bounds on how large a definition can be. The first node is
/// the flow's `Input`; the rest are `Output` nodes, and each connection runs from
/// the input to one of them.
fn definition_with_counts(nodes: usize, connections: usize) -> Value {
    let mut node_list = vec![json!({
        "name": "FlowInput",
        "type": "Input",
        "configuration": { "input": {} },
        "outputs": [{ "name": "document", "type": "String" }],
    })];
    for index in 1..nodes {
        node_list.push(json!({
            "name": format!("Output{index}"),
            "type": "Output",
            "configuration": { "output": {} },
            "inputs": [{ "name": "document", "type": "String", "expression": "$.data" }],
        }));
    }
    let mut connection_list: Vec<Value> = Vec::new();
    if nodes > 1 {
        for index in 0..connections {
            let target = format!("Output{}", index % (nodes - 1) + 1);
            connection_list.push(json!({
                "name": format!("Connection{index}"),
                "type": "Data",
                "source": "FlowInput",
                "target": target,
                "configuration": {
                    "data": { "sourceOutput": "document", "targetInput": "document" },
                },
            }));
        }
    }
    json!({ "nodes": node_list, "connections": connection_list })
}

/// The work `PrepareFlow` does is bounded by the model, not by the caller: the
/// definition it traverses is the one a request stored, and
/// `FlowDefinition$nodes` is modelled `length ..40` while
/// `FlowDefinition$connections` is modelled `length ..20`
/// (`smithy_query(query="shape", shape="FlowDefinition")`). A definition at
/// either published bound is stored and prepares; one past it never reaches the
/// store at all.
#[tokio::test]
async fn the_definition_a_preparation_traverses_is_bounded_by_the_model() {
    let emulator = Emulator::new();

    // Rows: each published bound, and one past it.
    for (case, nodes, connections, expected_status) in [
        ("nodes at the published bound", 40, 1, 201),
        ("one node past the published bound", 41, 1, 400),
        ("connections at the published bound", 2, 20, 201),
        ("one connection past the published bound", 2, 21, 400),
    ] {
        let created = emulator
            .call(
                "POST",
                "/flows/",
                Some(json!({
                    "name": "bounded-definition-flow",
                    "executionRoleArn": EXECUTION_ROLE,
                    "definition": definition_with_counts(nodes, connections),
                })),
            )
            .await;
        assert_eq!(
            created.status, expected_status,
            "case {case} ({nodes} nodes, {connections} connections): \
             expected CreateFlow -> {expected_status}, got {created:?}"
        );
        if expected_status != 201 {
            assert_eq!(
                created.error_type.as_deref(),
                Some("ValidationException"),
                "case {case}: a definition past a modelled bound is refused at the boundary, \
                 got {created:?}"
            );
            continue;
        }

        // A definition at the bound is what a preparation can have to walk, and
        // it walks it.
        let flow_id = created.string(&["id"]);
        let prepared = emulator.prepare_flow(&flow_id).await;
        assert_eq!(
            prepared.status, 202,
            "case {case}: expected PrepareFlow -> 202, got {prepared:?}"
        );
        emulator.make_preparation_due(case, &flow_id);
        let after = emulator.get_flow(&flow_id).await;
        assert_ne!(
            after.string(&["status"]),
            "Preparing",
            "case {case}: the preparation must reach an outcome, got {:?}",
            after.body
        );
    }
}

/// `PrepareFlow` refuses a well-formed identifier no flow holds with
/// `ResourceNotFoundException` (404), and an identifier of neither modelled
/// `FlowIdentifier` form with `ValidationException` (400) — and neither refusal
/// touches the flow that does exist.
#[tokio::test]
async fn prepare_flow_refuses_an_identifier_that_names_no_flow() {
    let emulator = Emulator::new();

    let created = emulator
        .create_flow(
            "setup",
            json!({
                "name": "the-one-flow",
                "executionRoleArn": EXECUTION_ROLE,
                "definition": passthrough_definition(),
            }),
        )
        .await;
    let stored_id = created.string(&["id"]);

    // Rows: ids the store does not hold, in both modelled identifier forms (one
    // all digits, one mixed case, one differing from the stored id in its last
    // character only).
    let mut unknown_ids = vec!["0123456789".to_string(), "aZ09aZ09aZ".to_string()];
    let mut nearly = stored_id.clone();
    nearly.pop();
    nearly.push(if stored_id.ends_with('X') { 'Y' } else { 'X' });
    if nearly != stored_id {
        unknown_ids.push(nearly);
    }

    for unknown in unknown_ids {
        for (form, identifier) in [
            ("bare flowId", unknown.clone()),
            (
                "flow ARN",
                format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:flow/{unknown}"),
            ),
        ] {
            let case = format!("{form} {identifier}");
            let refused = emulator.prepare_flow(&identifier).await;
            assert_eq!(
                refused.status, 404,
                "case {case}: expected PrepareFlow -> 404, got {refused:?}"
            );
            assert_eq!(
                refused.error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case}: expected ResourceNotFoundException, got {refused:?}"
            );
        }
    }

    // Rows differ in HOW the value misses the modelled forms: a flow name, an id
    // of the wrong length, an id carrying a character outside the alphabet, and
    // ARNs wrong in the partition, the service, the account and the resource
    // type.
    for (case, identifier) in [
        ("a flow name, not an identifier", "the-one-flow"),
        ("nine characters", "012345678"),
        ("eleven characters", "0123456789a"),
        ("a character outside the id alphabet", "0123_56789"),
        (
            "an ARN whose id is too short",
            "arn:aws:bedrock:eu-west-1:000000000000:flow/012345678",
        ),
        (
            "an ARN naming another service",
            "arn:aws:s3:eu-west-1:000000000000:flow/0123456789",
        ),
        (
            "an ARN whose account is not 12 digits",
            "arn:aws:bedrock:eu-west-1:0000:flow/0123456789",
        ),
        (
            "an ARN naming another resource type",
            "arn:aws:bedrock:eu-west-1:000000000000:agent/0123456789",
        ),
        (
            "an ARN in another partition",
            "arn:aws-cn:bedrock:eu-west-1:000000000000:flow/0123456789",
        ),
    ] {
        let refused = emulator.prepare_flow(identifier).await;
        assert_eq!(
            refused.status, 400,
            "case {case} ({identifier}): expected PrepareFlow -> 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case} ({identifier}): expected ValidationException, got {refused:?}"
        );
    }

    // A refused call changed nothing: the stored flow was never prepared.
    let untouched = emulator.get_flow(&stored_id).await;
    assert_eq!(
        untouched.string(&["status"]),
        "NotPrepared",
        "case stored flow: a refused PrepareFlow must prepare nothing, got {:?}",
        untouched.body
    );
    assert!(
        emulator.preparation_due(&stored_id).is_none(),
        "case stored flow: a refused PrepareFlow must leave no preparation owed"
    );

    // And the flow that does exist still prepares.
    let prepared = emulator.prepare_flow(&stored_id).await;
    assert_eq!(
        prepared.status, 202,
        "case stored flow: expected PrepareFlow -> 202, got {prepared:?}"
    );
    // Either modelled identifier form names the same flow.
    let arn = created.string(&["arn"]);
    let by_arn = emulator.prepare_flow(&arn).await;
    assert_eq!(
        by_arn.string(&["id"]),
        stored_id,
        "case flow ARN: both identifier forms must name the same flow, got {:?}",
        by_arn.body
    );
}

/// `GetFlow` reports the flow's `validations` LIST whatever status the flow is
/// in: the member carries the findings of the last operation on the flow, and
/// an empty list is the report that there are none — not an absent member a
/// polling client cannot tell from "nothing reported".
///
/// The rows differ along every status the modelled `FlowStatus` admits —
/// `NotPrepared` (freshly created and freshly updated), `Preparing`, `Prepared`
/// and `Failed` — crossed with each `IncludedData` value the model admits
/// (`ALL_DATA`, `METADATA_ONLY`, `smithy_query(query="shape",
/// shape="IncludedData")`) and the omitted member.
#[tokio::test]
async fn get_flow_always_reports_the_validations_list() {
    let emulator = Emulator::new();

    /// How far a row drives the flow before it is read back.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Drive {
        /// Read the flow as `CreateFlow` left it.
        AsCreated,
        /// Store a new definition first, as `UpdateFlow` leaves it.
        Updated,
        /// Accept a preparation and read while it is still in flight.
        Preparing,
        /// Accept a preparation and let the clock reach its outcome.
        Settled,
    }

    // Rows: the status to expect, how to get the flow there, and the definition
    // stored for it — valid definitions of two different sizes, and two
    // different fatal defects.
    let rows: [(&str, Drive, Value, &str); 6] = [
        (
            "freshly created",
            Drive::AsCreated,
            passthrough_definition(),
            "NotPrepared",
        ),
        (
            "freshly updated",
            Drive::Updated,
            fan_out_definition(),
            "NotPrepared",
        ),
        (
            "preparation in flight",
            Drive::Preparing,
            passthrough_definition(),
            "Preparing",
        ),
        (
            "preparation succeeded",
            Drive::Settled,
            fan_out_definition(),
            "Prepared",
        ),
        (
            "preparation failed on a missing ending node",
            Drive::Settled,
            input_only_definition(),
            "Failed",
        ),
        (
            "preparation failed on a connection that does not resolve",
            Drive::Settled,
            dangling_connection_definition(),
            "Failed",
        ),
    ];

    for (row, drive, definition, expected_status) in rows {
        for included_data in [None, Some("ALL_DATA"), Some("METADATA_ONLY")] {
            let case = format!("{row} / includedData {included_data:?}");
            let created = emulator
                .create_flow(
                    &case,
                    json!({
                        "name": "validations-member-flow",
                        "executionRoleArn": EXECUTION_ROLE,
                        "definition": if drive == Drive::Updated {
                            passthrough_definition()
                        } else {
                            definition.clone()
                        },
                    }),
                )
                .await;
            let flow_id = created.string(&["id"]);

            match drive {
                Drive::AsCreated => {}
                Drive::Updated => {
                    let updated = emulator
                        .update_flow(
                            &flow_id,
                            json!({
                                "name": "validations-member-flow",
                                "executionRoleArn": EXECUTION_ROLE,
                                "definition": definition.clone(),
                            }),
                        )
                        .await;
                    assert_eq!(
                        updated.status, 200,
                        "case {case}: expected UpdateFlow -> 200, got {updated:?}"
                    );
                }
                Drive::Preparing => {
                    let prepared = emulator.prepare_flow(&flow_id).await;
                    assert_eq!(
                        prepared.status, 202,
                        "case {case}: expected PrepareFlow -> 202, got {prepared:?}"
                    );
                }
                Drive::Settled => {
                    let prepared = emulator.prepare_flow(&flow_id).await;
                    assert_eq!(
                        prepared.status, 202,
                        "case {case}: expected PrepareFlow -> 202, got {prepared:?}"
                    );
                    emulator.make_preparation_due(&case, &flow_id);
                }
            }

            let uri = match included_data {
                None => format!("/flows/{flow_id}/"),
                Some(value) => format!("/flows/{flow_id}/?includedData={value}"),
            };
            let fetched = emulator.call("GET", &uri, None).await;
            assert_eq!(
                fetched.status, 200,
                "case {case}: expected GetFlow -> 200, got {fetched:?}"
            );
            assert_eq!(
                fetched.string(&["status"]),
                expected_status,
                "case {case}: expected status {expected_status}, got {:?}",
                fetched.body
            );

            let reported = fetched.member(&["validations"]);
            let findings = reported.as_array().unwrap_or_else(|| {
                panic!("case {case}: expected a validations array, got {reported:?}")
            });
            if expected_status == "Failed" {
                assert!(
                    !findings.is_empty(),
                    "case {case}: a failed operation reports what it found, got {reported:?}"
                );
                assert!(
                    fetched
                        .validation_severities()
                        .iter()
                        .any(|severity| severity == "Error"),
                    "case {case}: a failed preparation reports at least one Error finding, \
                     got {reported:?}"
                );
            } else {
                assert!(
                    findings.is_empty(),
                    "case {case}: nothing has failed, so the list is empty, got {reported:?}"
                );
            }
        }
    }
}
