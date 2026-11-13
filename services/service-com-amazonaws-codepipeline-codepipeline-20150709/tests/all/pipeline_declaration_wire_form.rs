// SPDX-License-Identifier: CC0-1.0

//! The WIRE FORM of a `PipelineDeclaration`, end to end over the protocol this service
//! speaks (`awsJson1_1`, `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! Three rules, each asserted over a SET of declarations rather than one recorded call,
//! and each on EVERY operation that answers with a pipeline structure — `CreatePipeline`,
//! `UpdatePipeline` and `GetPipeline` report one projection, so a client that reads a
//! structure back reads what the create or the update answered:
//!
//! 1. RESOLVED MEMBERS — every member the SERVICE decides the value of is present on each
//!    reported action declaration: its `runOrder` (the one it declares, else the first
//!    order), its `configuration` (the entries it declares, else the empty map) and both
//!    artifact lists (empty where it declares no artifact).
//! 2. PIPELINE TYPE — a declaration that states a `pipelineType` keeps it, and one that
//!    states none is stored as the service's default type.
//! 3. SHAPE DISCRIMINATOR — wherever a response carries an `ActionTypeId` or an
//!    `EncryptionKey`, the structure carries the awsJson `__type` member naming its shape,
//!    and no structure enclosing one carries a discriminator of its own. The rule is per
//!    SHAPE, so it is also asserted on another plane's answer (`ListActionExecutions`).
//!
//! Every read goes through the router, so nothing here can pass by reading a handler's
//! internals.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::pipeline_lifecycle::{source_stage, Emulator, ROLE_ARN};

/// The run order an action that declares none is reported in: `ActionRunOrder` is
/// `@range(min: 1, max: 999)` (`smithy_query query="shape" shape="ActionRunOrder"`), so
/// the first order a stage can run anything in is 1.
const DEFAULT_RUN_ORDER: i64 = 1;

/// `ActionRunOrder`'s published range, from the same query.
const RUN_ORDER_RANGE: (i64, i64) = (1, 999);

/// The `PipelineType` the service stores a declaration that states none as.
const DEFAULT_PIPELINE_TYPE: &str = "V2";

/// The member names this service binds a discriminated structure to, with the shape name
/// each one's `__type` carries.
///
/// `ActionType$id` binds one too — the catalog's own suite asserts that
/// (`tests/all/action_type_wire_form.rs`) — and is left out here because `id` is a member
/// name several other shapes of this model use.
const DISCRIMINATED_MEMBERS: [(&str, &str); 2] = [
    ("actionTypeId", "ActionTypeId"),
    ("encryptionKey", "EncryptionKey"),
];

/// How long a poll waits for the execution plane to record something. Generous: the
/// condition is what is asserted, never the time it took.
const WAIT: Duration = Duration::from_secs(30);

/// One acting-stage action, as a row of the resolved-members table declares it.
struct ActionCase {
    /// What makes this row different from the others.
    case: &'static str,
    /// The action declaration sent to the service.
    declared: fn() -> Value,
    /// The `runOrder` the reported declaration must carry.
    run_order: i64,
    /// The `configuration` the reported declaration must carry.
    configuration: fn() -> Value,
}

/// A manual-approval action, which takes and produces no artifact and whose three
/// configuration parameters are all optional (`Approval/AWS/Manual` in the action-type
/// catalog `ListActionTypes` reports).
fn approval(configuration: Option<Value>, run_order: Option<i64>) -> Value {
    let mut action = json!({
        "name": "Approve",
        "actionTypeId": {
            "category": "Approval", "owner": "AWS", "provider": "Manual", "version": "1",
        },
    });
    if let Some(configuration) = configuration {
        action["configuration"] = configuration;
    }
    if let Some(run_order) = run_order {
        action["runOrder"] = json!(run_order);
    }
    action
}

/// A deploy action that consumes the source artifact, so a row can differ in carrying
/// several configuration entries and an input artifact (`Deploy/AWS/S3`, input 1..1).
fn deploy(run_order: Option<i64>) -> Value {
    let mut action = json!({
        "name": "Ship",
        "actionTypeId": {
            "category": "Deploy", "owner": "AWS", "provider": "S3", "version": "1",
        },
        "configuration": {
            "BucketName": "deploy-bucket",
            "ObjectKey": "delivered/app.zip",
            "Extract": "false",
        },
        "inputArtifacts": [{"name": "SourceOut"}],
    });
    if let Some(run_order) = run_order {
        action["runOrder"] = json!(run_order);
    }
    action
}

/// A declaration of `name` whose acting stage carries exactly `action`.
fn declaration_with(name: &str, action: Value) -> Value {
    json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": [source_stage("Source"), {"name": "Act", "actions": [action]}],
    })
}

/// The acting stage's single reported action of a `PipelineDeclaration`.
fn acting_action(pipeline: &Value) -> Value {
    pipeline["stages"][1]["actions"][0].clone()
}

/// RULE 1: every member the service resolves for an action declaration is present on the
/// structure each of the three operations reports.
///
/// Rows differ along what the rule generalises over: whether the action declares a run
/// order (and which, including both published bounds), whether it declares a
/// configuration (none, the empty map, one entry, several entries) and whether it
/// consumes an artifact.
#[tokio::test]
async fn every_reported_action_declaration_carries_the_members_the_service_resolves() {
    let cases: [ActionCase; 6] = [
        ActionCase {
            case: "an action declaring neither a run order nor a configuration",
            declared: || approval(None, None),
            run_order: DEFAULT_RUN_ORDER,
            configuration: || json!({}),
        },
        ActionCase {
            case: "an action declaring the empty configuration map",
            declared: || approval(Some(json!({})), None),
            run_order: DEFAULT_RUN_ORDER,
            configuration: || json!({}),
        },
        ActionCase {
            case: "an action declaring one configuration entry",
            declared: || approval(Some(json!({"CustomData": "ship it"})), None),
            run_order: DEFAULT_RUN_ORDER,
            configuration: || json!({"CustomData": "ship it"}),
        },
        ActionCase {
            case: "an action declaring the lowest published run order",
            declared: || approval(None, Some(RUN_ORDER_RANGE.0)),
            run_order: RUN_ORDER_RANGE.0,
            configuration: || json!({}),
        },
        ActionCase {
            case: "an action declaring the highest published run order",
            declared: || approval(None, Some(RUN_ORDER_RANGE.1)),
            run_order: RUN_ORDER_RANGE.1,
            configuration: || json!({}),
        },
        ActionCase {
            case: "an action declaring several configuration entries, an input artifact \
                   and a run order",
            declared: || deploy(Some(7)),
            run_order: 7,
            configuration: || {
                json!({
                    "BucketName": "deploy-bucket",
                    "ObjectKey": "delivered/app.zip",
                    "Extract": "false",
                })
            },
        },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        let declaration = declaration_with("demo", (case.declared)());
        let created = emulator
            .ok("CreatePipeline", &json!({"pipeline": declaration}))
            .await;
        let fetched = emulator.ok("GetPipeline", &json!({"name": "demo"})).await;
        // The same structure sent again: an update reports the projection a create and a
        // read report, so the three answers cannot disagree about it.
        let updated = emulator
            .ok("UpdatePipeline", &json!({"pipeline": declaration}))
            .await;
        let read_back = emulator.ok("GetPipeline", &json!({"name": "demo"})).await;

        for (operation, pipeline) in [
            ("CreatePipeline", &created["pipeline"]),
            ("GetPipeline", &fetched["pipeline"]),
            ("UpdatePipeline", &updated["pipeline"]),
            ("GetPipeline after the update", &read_back["pipeline"]),
        ] {
            let action = acting_action(pipeline);
            let name = case.case;
            assert_eq!(
                action["runOrder"],
                json!(case.run_order),
                "case {name:?}: {operation} must report the action's run order, got {action}"
            );
            assert_eq!(
                action["configuration"],
                (case.configuration)(),
                "case {name:?}: {operation} must report the action's configuration, got {action}"
            );
            for member in ["inputArtifacts", "outputArtifacts"] {
                assert!(
                    action[member].is_array(),
                    "case {name:?}: {operation} must report {member} as a list, got {action}"
                );
            }
        }
    }
}

/// RULE 1 at the edge of the published range: a run order outside `ActionRunOrder`'s
/// `@range` is refused rather than stored, on a create and on an update alike.
#[tokio::test]
async fn a_run_order_outside_the_published_range_is_refused() {
    // (case, the declared runOrder, whether the declaration is accepted)
    let cases: [(&str, i64, bool); 4] = [
        ("the lowest published order", RUN_ORDER_RANGE.0, true),
        ("the highest published order", RUN_ORDER_RANGE.1, true),
        ("one below the lowest", RUN_ORDER_RANGE.0 - 1, false),
        ("one above the highest", RUN_ORDER_RANGE.1 + 1, false),
    ];

    for (case, run_order, accepted) in cases {
        let emulator = Emulator::new();
        let declaration = declaration_with("demo", approval(None, Some(run_order)));
        let (status, response) = emulator
            .call("CreatePipeline", &json!({"pipeline": declaration}))
            .await;
        if accepted {
            assert_eq!(
                (status, response["pipeline"]["stages"][1]["actions"][0]["runOrder"].clone()),
                (200, json!(run_order)),
                "case {case:?}: runOrder {run_order} is inside the published range, got \
                 HTTP {status} with {response}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: runOrder {run_order} is outside the published range \
             {RUN_ORDER_RANGE:?}, got HTTP {status} with {response}"
        );
        // Nothing was stored: the refused declaration left no pipeline behind.
        let (status, response) = emulator.call("GetPipeline", &json!({"name": "demo"})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create stores no pipeline, got HTTP {status} with \
             {response}"
        );
    }
}

/// One row of the pipeline-type table: what the create states, what the update states,
/// and the type stored after each.
struct TypeCase {
    /// What makes this row different from the others.
    case: &'static str,
    /// The `pipelineType` the create states, `None` for a declaration stating none.
    created_type: Option<&'static str>,
    /// The type stored after that create.
    stored: &'static str,
    /// The `pipelineType` the update states, `None` for a declaration stating none.
    updated_type: Option<&'static str>,
    /// The type stored after that update.
    stored_after: &'static str,
}

/// RULE 2: the `pipelineType` a declaration is stored as, on a create and on an update,
/// read back through `GetPipeline` and through the `ListPipelines` summary.
///
/// Rows differ along what the rule generalises over: which type the declaration states,
/// including none at all, and what a later update states.
#[tokio::test]
async fn a_declaration_keeps_the_pipeline_type_it_states_and_defaults_to_the_services_type() {
    let cases: [TypeCase; 4] = [
        TypeCase {
            case: "no type stated at all",
            created_type: None,
            stored: DEFAULT_PIPELINE_TYPE,
            updated_type: None,
            stored_after: DEFAULT_PIPELINE_TYPE,
        },
        TypeCase {
            case: "the default type stated explicitly",
            created_type: Some("V2"),
            stored: "V2",
            updated_type: Some("V2"),
            stored_after: "V2",
        },
        TypeCase {
            case: "the older type stated explicitly",
            created_type: Some("V1"),
            stored: "V1",
            // An update is how a pipeline's type changes, so a V1 pipeline updated with a
            // declaration stating no type takes the default.
            updated_type: None,
            stored_after: DEFAULT_PIPELINE_TYPE,
        },
        TypeCase {
            case: "an update down to the older type",
            created_type: None,
            stored: DEFAULT_PIPELINE_TYPE,
            updated_type: Some("V1"),
            stored_after: "V1",
        },
    ];

    for TypeCase {
        case,
        created_type,
        stored,
        updated_type,
        stored_after,
    } in cases
    {
        let emulator = Emulator::new();
        let mut declaration = declaration_with("demo", approval(None, None));
        if let Some(created_type) = created_type {
            declaration["pipelineType"] = json!(created_type);
        }
        let created = emulator
            .ok("CreatePipeline", &json!({"pipeline": declaration.clone()}))
            .await;
        assert_eq!(
            created["pipeline"]["pipelineType"],
            json!(stored),
            "case {case:?}: CreatePipeline must report the stored type, got {created}"
        );
        assert_reads_report_type(&emulator, case, "the create", stored).await;

        let mut declaration = declaration_with("demo", approval(None, None));
        if let Some(updated_type) = updated_type {
            declaration["pipelineType"] = json!(updated_type);
        }
        let answered = emulator
            .ok("UpdatePipeline", &json!({"pipeline": declaration}))
            .await;
        assert_eq!(
            answered["pipeline"]["pipelineType"],
            json!(stored_after),
            "case {case:?}: UpdatePipeline must report the stored type, got {answered}"
        );
        assert_reads_report_type(&emulator, case, "the update", stored_after).await;
    }
}

/// Every reader of the stored pipeline reports `expected` as its type.
async fn assert_reads_report_type(emulator: &Emulator, case: &str, after: &str, expected: &str) {
    let fetched = emulator.ok("GetPipeline", &json!({"name": "demo"})).await;
    assert_eq!(
        fetched["pipeline"]["pipelineType"],
        json!(expected),
        "case {case:?}: GetPipeline after {after} must report the stored type, got {fetched}"
    );
    let listed = emulator.ok("ListPipelines", &json!({})).await;
    assert_eq!(
        listed["pipelines"][0]["pipelineType"],
        json!(expected),
        "case {case:?}: the summary after {after} must report the stored type, got {listed}"
    );
}

/// RULE 3, as an INVARIANT over a whole response: every structure bound to a
/// discriminated member carries its shape name in `__type`, and no other structure carries
/// a `__type` at all.
///
/// `found` collects the path of each discriminator, so a case can assert the walk was not
/// vacuous.
fn assert_discriminators(
    case: &str,
    path: &str,
    bound_to: Option<&str>,
    value: &Value,
    found: &mut BTreeMap<String, String>,
) {
    match value {
        Value::Object(members) => {
            let shape = bound_to.and_then(|name| {
                DISCRIMINATED_MEMBERS
                    .iter()
                    .find(|(bound, _)| *bound == name)
                    .map(|(_, shape)| *shape)
            });
            match shape {
                Some(shape) => {
                    assert_eq!(
                        members.get("__type").and_then(Value::as_str),
                        Some(shape),
                        "case {case:?}: {path} must carry the shape discriminator \
                         {shape:?}, got {value}"
                    );
                    found.insert(path.to_string(), shape.to_string());
                }
                None => assert!(
                    !members.contains_key("__type"),
                    "case {case:?}: {path} must carry no shape discriminator, got {value}"
                ),
            }
            for (name, member) in members {
                if name != "__type" {
                    assert_discriminators(
                        case,
                        &format!("{path}.{name}"),
                        Some(name),
                        member,
                        found,
                    );
                }
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                // A list's elements are the member's own shape, so they are judged
                // against the member that binds the list.
                assert_discriminators(case, &format!("{path}[{index}]"), bound_to, item, found);
            }
        }
        _ => {}
    }
}

/// RULE 3 over every operation that answers with a pipeline structure, for declarations
/// differing in how many actions they carry, in which form of artifact store they use and
/// in whether a store names an encryption key.
#[tokio::test]
async fn every_discriminated_structure_of_a_pipeline_structure_carries_its_shape_name() {
    // (case, declaration, the discriminated paths the answer must carry)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "one artifact store with an encryption key",
            json!({
                "name": "demo",
                "roleArn": ROLE_ARN,
                "artifactStore": {
                    "type": "S3",
                    "location": "my-bucket",
                    "encryptionKey": {"id": "alias/cp", "type": "KMS"},
                },
                "stages": [source_stage("Source"), {"name": "Act", "actions": [approval(None, None)]}],
            }),
            vec![
                "pipeline.artifactStore.encryptionKey",
                "pipeline.stages[0].actions[0].actionTypeId",
                "pipeline.stages[1].actions[0].actionTypeId",
            ],
        ),
        (
            "one artifact store with no encryption key, and two acting actions",
            json!({
                "name": "demo",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [
                    source_stage("Source"),
                    {"name": "Act", "actions": [approval(None, None), deploy(Some(2))]},
                ],
            }),
            vec![
                "pipeline.stages[0].actions[0].actionTypeId",
                "pipeline.stages[1].actions[0].actionTypeId",
                "pipeline.stages[1].actions[1].actionTypeId",
            ],
        ),
        (
            "the cross-region artifactStores map, one store carrying an encryption key",
            json!({
                "name": "demo",
                "roleArn": ROLE_ARN,
                "artifactStores": {
                    "us-east-1": {
                        "type": "S3",
                        "location": "east",
                        "encryptionKey": {"id": "alias/east", "type": "KMS"},
                    },
                    "eu-west-1": {"type": "S3", "location": "west"},
                },
                "stages": [source_stage("Source"), {"name": "Act", "actions": [approval(None, None)]}],
            }),
            vec![
                "pipeline.artifactStores.us-east-1.encryptionKey",
                "pipeline.stages[0].actions[0].actionTypeId",
                "pipeline.stages[1].actions[0].actionTypeId",
            ],
        ),
    ];

    for (case, declaration, expected) in cases {
        let emulator = Emulator::new();
        let created = emulator
            .ok("CreatePipeline", &json!({"pipeline": declaration.clone()}))
            .await;
        let fetched = emulator.ok("GetPipeline", &json!({"name": "demo"})).await;
        let updated = emulator
            .ok("UpdatePipeline", &json!({"pipeline": declaration}))
            .await;

        for (operation, answer) in [
            ("CreatePipeline", &created),
            ("GetPipeline", &fetched),
            ("UpdatePipeline", &updated),
        ] {
            let case = format!("{case} ({operation})");
            let mut found = BTreeMap::new();
            assert_discriminators(&case, "pipeline", Some("pipeline"), &answer["pipeline"], &mut found);
            let paths: Vec<&str> = found.keys().map(String::as_str).collect();
            let mut wanted = expected.clone();
            wanted.sort_unstable();
            assert_eq!(
                paths, wanted,
                "case {case:?}: unexpected set of discriminated structures, got {answer}"
            );
        }
    }
}

/// RULE 3 is a property of the SHAPE and not of the pipeline-structure operations: the
/// same `ActionTypeId` carries its discriminator in the execution plane's own answer.
#[tokio::test]
async fn an_action_type_id_carries_its_shape_name_in_an_action_execution_record() {
    let emulator = Emulator::new();
    let declaration = declaration_with("demo", approval(None, None));
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration}))
        .await;
    emulator
        .ok("StartPipelineExecution", &json!({"name": "demo"}))
        .await;

    // Poll for the first record the plane writes: the CONDITION is what is waited for,
    // never a fixed span.
    let deadline = Instant::now() + WAIT;
    let details = loop {
        let listed = emulator
            .ok("ListActionExecutions", &json!({"pipelineName": "demo"}))
            .await;
        let details: Vec<Value> = listed["actionExecutionDetails"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if !details.is_empty() {
            break details;
        }
        assert!(
            Instant::now() < deadline,
            "no action execution was recorded for the started execution; last answer was \
             {listed}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    for detail in &details {
        let case = format!(
            "the record of {}",
            detail["actionName"].as_str().unwrap_or("<unnamed>")
        );
        let mut found = BTreeMap::new();
        assert_discriminators(&case, "input", Some("input"), &detail["input"], &mut found);
        assert_eq!(
            found.get("input.actionTypeId").map(String::as_str),
            Some("ActionTypeId"),
            "case {case:?}: the recorded input's action type id must carry its shape name, \
             got {detail}"
        );
    }
}
