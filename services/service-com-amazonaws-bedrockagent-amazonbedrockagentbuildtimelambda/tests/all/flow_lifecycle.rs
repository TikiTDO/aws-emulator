// SPDX-License-Identifier: CC0-1.0

//! The flow slice, end to end over HTTP: `CreateFlow` persists a flow and
//! `GetFlow` reports that stored record back.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
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
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";

/// Members of the `GetFlow` response that describe the flow's metadata — every
/// member but `definition`, which is the one part a metadata-only read drops.
const METADATA_MEMBERS: [&str; 8] = [
    "name",
    "description",
    "executionRoleArn",
    "customerEncryptionKeyArn",
    "id",
    "arn",
    "status",
    "version",
];

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
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
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

    async fn create_flow(&self, request: Value) -> Answer {
        self.call("POST", "/flows/", Some(request)).await
    }

    /// `GetFlow` on any `FlowIdentifier` form, with or without `includedData`.
    async fn get_flow(&self, identifier: &str, included_data: Option<&str>) -> Answer {
        let escaped = identifier.replace(':', "%3A").replace('/', "%2F");
        let query = included_data
            .map(|value| format!("?includedData={value}"))
            .unwrap_or_default();
        self.call("GET", &format!("/flows/{escaped}/{query}"), None)
            .await
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

/// A two-branch definition: the same `Input` fanned out to two `Output` nodes,
/// so a row of the table differs from [`passthrough_definition`] in node and
/// connection COUNT, not just in values.
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

/// Assert `flow_id` is shaped as the model's `FlowId` demands
/// (`^[0-9a-zA-Z]{10}$`).
fn assert_well_formed_id(case: &str, flow_id: &str) {
    assert_eq!(
        flow_id.len(),
        10,
        "case {case}: expected a 10-character flowId, got {flow_id:?}"
    );
    assert!(
        flow_id.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case}: expected flowId to match ^[0-9a-zA-Z]{{10}}$, got {flow_id:?}"
    );
}

/// `CreateFlow` persists every member the request supplied, and `GetFlow`
/// reports that record back — over a table of requests that differ in which
/// optional members they carry, in the shape and size of the definition, and in
/// the form of the flow name.
#[tokio::test]
async fn created_flows_round_trip_through_get_flow() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: which optional members
    // are present, whether a definition is supplied and how large it is, and the
    // form of the modelled `FlowName`.
    let cases: Vec<(&str, Value)> = vec![
        (
            "minimal: only the required name and executionRoleArn",
            json!({ "name": "minimal-flow", "executionRoleArn": EXECUTION_ROLE }),
        ),
        (
            "a passthrough definition and a description",
            json!({
                "name": "passthrough-flow",
                "executionRoleArn": EXECUTION_ROLE,
                "description": "an input node wired to an output node",
                "definition": passthrough_definition(),
            }),
        ),
        (
            "a larger definition: three nodes, two connections",
            json!({
                "name": "fan_out_flow",
                "executionRoleArn": EXECUTION_ROLE,
                "definition": fan_out_definition(),
            }),
        ),
        (
            "every member of this slice supplied",
            json!({
                "name": "FullFlow1",
                "executionRoleArn": "arn:aws:iam::000000000000:role/FlowRole",
                "description": "a flow carrying every member of this slice",
                "customerEncryptionKeyArn":
                    "arn:aws:kms:eu-west-1:000000000000:key/12345678-1234-1234-1234-123456789012",
                "definition": passthrough_definition(),
                "tags": { "team": "platform" },
            }),
        ),
        (
            "an empty definition object",
            json!({
                "name": "empty-definition-flow",
                "executionRoleArn": EXECUTION_ROLE,
                "definition": {},
            }),
        ),
    ];

    for (case, request) in cases {
        let created = emulator.create_flow(request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected CreateFlow -> 201, got {created:?}"
        );

        let flow_id = created.string(&["id"]);
        assert_well_formed_id(case, &flow_id);
        assert_eq!(
            created.string(&["arn"]),
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:flow/{flow_id}"),
            "case {case}: the flow ARN must be built from the request's region, the account and the minted id"
        );
        assert_eq!(
            created.string(&["version"]),
            "DRAFT",
            "case {case}: a newly created flow has only the DRAFT version"
        );
        assert_eq!(
            created.string(&["status"]),
            "NotPrepared",
            "case {case}: a newly created flow has not been prepared"
        );
        // `createdAt`/`updatedAt` target `DateTimestamp`, which carries
        // `@timestampFormat("date-time")`
        // (`smithy_query(query="shape", shape="CreateFlowResponse")`), so each one is
        // populated AND carried as an RFC-3339 string rather than an epoch number.
        for member in ["createdAt", "updatedAt"] {
            let at = created.member(&[member]);
            let text = at.as_str().unwrap_or_else(|| {
                panic!("case {case}: expected {member} to be a date-time string, got {at:?}")
            });
            assert!(
                text.contains('T') && text.ends_with('Z'),
                "case {case}: expected {member} in the modelled date-time wire form, \
                 got {text:?}"
            );
        }

        // The round trip: the stored record, read back through GetFlow.
        let fetched = emulator.get_flow(&flow_id, None).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetFlow -> 200, got {fetched:?}"
        );
        for member in METADATA_MEMBERS
            .iter()
            .chain(["createdAt", "updatedAt", "definition"].iter())
        {
            assert_eq!(
                fetched.member(&[member]),
                created.member(&[member]),
                "case {case}: GetFlow must report {member} exactly as CreateFlow persisted it, got {:?}",
                fetched.member(&[member])
            );
        }

        // Every member the request supplied survives the round trip verbatim.
        for member in [
            "name",
            "description",
            "executionRoleArn",
            "customerEncryptionKeyArn",
            "definition",
        ] {
            if let Some(supplied) = request.get(member) {
                assert_eq!(
                    fetched.member(&[member]),
                    supplied,
                    "case {case}: expected {member} to round-trip as {supplied:?}, got {:?}",
                    fetched.member(&[member])
                );
            }
        }

        // A flow nothing has failed on reports an EMPTY findings list: the list
        // is the report, so its emptiness is the answer.
        assert_eq!(
            fetched.member(&["validations"]),
            &json!([]),
            "case {case}: expected a flow with no failed operation to report an empty \
             validations list, got {:?}",
            fetched.member(&["validations"])
        );
    }
}

/// Either `FlowIdentifier` form — the bare `flowId` or the flow's ARN — selects
/// the same stored flow, and `includedData` decides only whether the definition
/// comes with it.
#[tokio::test]
async fn get_flow_resolves_every_identifier_form_and_honours_included_data() {
    let emulator = Emulator::new();

    let created = emulator
        .create_flow(json!({
            "name": "identifier-forms-flow",
            "executionRoleArn": EXECUTION_ROLE,
            "description": "read back through both identifier forms",
            "definition": passthrough_definition(),
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "case setup: expected CreateFlow -> 201, got {created:?}"
    );
    let flow_id = created.string(&["id"]);
    let flow_arn = created.string(&["arn"]);

    // Rows: both modelled identifier forms x every `includedData` value,
    // the omitted member included.
    for (identifier_case, identifier) in [("bare flowId", &flow_id), ("flow ARN", &flow_arn)] {
        for (data_case, included_data, expect_definition) in [
            ("includedData omitted", None, true),
            ("includedData ALL_DATA", Some("ALL_DATA"), true),
            ("includedData METADATA_ONLY", Some("METADATA_ONLY"), false),
        ] {
            let case = format!("{identifier_case}, {data_case}");
            let fetched = emulator.get_flow(identifier, included_data).await;
            assert_eq!(
                fetched.status, 200,
                "case {case}: expected GetFlow -> 200, got {fetched:?}"
            );
            assert_eq!(
                fetched.string(&["id"]),
                flow_id,
                "case {case}: both identifier forms must name the same flow"
            );

            // The metadata is the same whatever was asked for.
            for member in METADATA_MEMBERS {
                assert_eq!(
                    fetched.member(&[member]),
                    created.member(&[member]),
                    "case {case}: expected {member} to be reported as stored, got {:?}",
                    fetched.member(&[member])
                );
            }

            if expect_definition {
                assert_eq!(
                    fetched.member(&["definition"]),
                    created.member(&["definition"]),
                    "case {case}: the full response carries the stored definition"
                );
            } else {
                assert!(
                    fetched.member(&["definition"]).is_null(),
                    "case {case}: a metadata-only read drops the definition, got {:?}",
                    fetched.member(&["definition"])
                );
            }
        }
    }

    // A read changes nothing a later reader sees: the metadata-only read above
    // did not drop the stored definition.
    let after = emulator.get_flow(&flow_id, None).await;
    assert_eq!(
        after.member(&["definition"]),
        created.member(&["definition"]),
        "case read-after-metadata-only: a read must not change what a later caller sees"
    );
}

/// `GetFlow` answers `ResourceNotFoundException` (404) for a well-formed
/// identifier no flow holds — in either identifier form, and for a flow that was
/// never created as well as for an identifier belonging to no flow in the store.
#[tokio::test]
async fn get_flow_refuses_an_identifier_no_flow_holds() {
    let emulator = Emulator::new();

    let created = emulator
        .create_flow(json!({ "name": "the-one-flow", "executionRoleArn": EXECUTION_ROLE }))
        .await;
    assert_eq!(
        created.status, 201,
        "case setup: expected CreateFlow -> 201, got {created:?}"
    );
    let stored_id = created.string(&["id"]);

    // Rows: both identifier forms, with ids the store does not hold (one all
    // digits, one mixed case, one differing from the stored id in its last
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
            let fetched = emulator.get_flow(&identifier, None).await;
            assert_eq!(
                fetched.status, 404,
                "case {case}: expected GetFlow -> 404, got {fetched:?}"
            );
            assert_eq!(
                fetched.error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case}: expected ResourceNotFoundException, got {fetched:?}"
            );
        }
    }

    // The flow that does exist is still readable.
    let fetched = emulator.get_flow(&stored_id, None).await;
    assert_eq!(
        fetched.status, 200,
        "case stored flow: expected GetFlow -> 200, got {fetched:?}"
    );
}

/// An identifier of neither modelled `FlowIdentifier` form is a bad request, not
/// a missing flow: `GetFlow` refuses it with `ValidationException` (400).
#[tokio::test]
async fn get_flow_refuses_an_identifier_of_neither_modelled_form() {
    let emulator = Emulator::new();

    let created = emulator
        .create_flow(json!({ "name": "well-formed-flow", "executionRoleArn": EXECUTION_ROLE }))
        .await;
    assert_eq!(
        created.status, 201,
        "case setup: expected CreateFlow -> 201, got {created:?}"
    );

    // Rows differ in HOW the value misses the modelled forms: a flow name, an id
    // that is too short or too long, an id carrying a character outside the
    // alphabet, and ARNs wrong in the partition, the service, the account, the
    // resource type and the id length.
    let malformed = [
        ("a flow name, not an identifier", "sim2real-demo-flow-a1b2c3d4e5"),
        ("the name of the flow that exists", "well-formed-flow"),
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
    ];

    for (case, identifier) in malformed {
        let fetched = emulator.get_flow(identifier, None).await;
        assert_eq!(
            fetched.status, 400,
            "case {case} ({identifier}): expected GetFlow -> 400, got {fetched:?}"
        );
        assert_eq!(
            fetched.error_type.as_deref(),
            Some("ValidationException"),
            "case {case} ({identifier}): expected ValidationException, got {fetched:?}"
        );
    }
}

/// A repeated `@idempotencyToken` returns the flow the first call created, and
/// two flows that share a name are still two distinct flows — the model
/// constrains `FlowName`'s shape, not its uniqueness.
#[tokio::test]
async fn create_flow_honours_its_idempotency_token_and_mints_one_flow_per_call() {
    let emulator = Emulator::new();

    // Two tokens, each replayed: a replay returns the flow the first call
    // created and mints nothing new.
    for token in [
        "idempotency-token-for-the-first-flow",
        "IdempotencyTokenForTheSecondFlow000",
    ] {
        let request = json!({
            "name": "idempotent-flow",
            "executionRoleArn": EXECUTION_ROLE,
            "clientToken": token,
        });
        let once = emulator.create_flow(request.clone()).await;
        let again = emulator.create_flow(request).await;
        assert_eq!(
            once.status, 201,
            "case token {token}: expected 201 on the first call, got {once:?}"
        );
        assert_eq!(
            again.status, 201,
            "case token {token}: expected 201 on the replay, got {again:?}"
        );
        assert_eq!(
            again.string(&["id"]),
            once.string(&["id"]),
            "case token {token}: a replayed idempotency token must return the same flow"
        );
        assert_eq!(
            again.body, once.body,
            "case token {token}: a replay reports the stored flow, unchanged"
        );
    }

    // Same name, no token: distinct flows, each readable through GetFlow.
    let first = emulator
        .create_flow(json!({ "name": "shared-name-flow", "executionRoleArn": EXECUTION_ROLE }))
        .await;
    let second = emulator
        .create_flow(json!({ "name": "shared-name-flow", "executionRoleArn": EXECUTION_ROLE }))
        .await;
    assert_eq!(
        second.status, 201,
        "case duplicate name: the model constrains FlowName's pattern, not its uniqueness, got {second:?}"
    );
    assert_ne!(
        first.string(&["id"]),
        second.string(&["id"]),
        "case duplicate name: each create mints its own flowId"
    );
    for (case, answer) in [("first", &first), ("second", &second)] {
        let id = answer.string(&["id"]);
        let fetched = emulator.get_flow(&id, None).await;
        assert_eq!(
            fetched.string(&["arn"]),
            answer.string(&["arn"]),
            "case duplicate name {case}: GetFlow reports the flow that create minted"
        );
    }
}
