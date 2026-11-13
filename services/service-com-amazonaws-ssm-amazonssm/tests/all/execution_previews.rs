// SPDX-License-Identifier: CC0-1.0

//! Execution previews end to end: `StartExecutionPreview` working out what running an
//! Automation runbook WOULD do and persisting it, and `GetExecutionPreview` reporting that
//! preview back.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol").
//!
//! A preview is `Pending` until the service has worked it out, so the cases that look PAST
//! that instant move the clock through the service's own state ([`work_out`]) rather than
//! sleeping for it: what they assert is the preview, not how long a span is.
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — the step's action, the Regions and accounts the inputs name, the
//! targets they resolve, the status the clock has reached — rather than the single call a
//! recorded unit happened to make.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ssm_amazonssm::state::{
    execution_previews, ComAmazonawsSsmAmazonssmState, ManagedNodeRecord,
    RESOURCE_TYPE_EC2_INSTANCE, RESOURCE_TYPE_MANAGED_INSTANCE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request signs for unless a case says otherwise. Deliberately not the
/// emulator's default, so a preview served here proves the Region came from the request's
/// SigV4 credential scope.
const REGION: &str = "us-west-2";

/// A second Region, to pin that a preview is reachable only through the Region it was
/// started in.
const OTHER_REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AmazonSSM";

/// A well-formed `ExecutionPreviewId` (the modelled length is exactly 36, in the UUID
/// form) no preview was ever minted under.
const UNKNOWN_PREVIEW_ID: &str = "11111111-2222-3333-4444-555555555555";

/// A document Systems Manager publishes that is a COMMAND document, so it is no Automation
/// runbook at all.
const PUBLISHED_COMMAND: &str = "AWS-RunShellScript";

/// An EC2 node id whose suffix is 8 characters long (the modelled `InstanceId` pattern
/// admits 8 and 17).
const EC2_ONE: &str = "i-0abc1234";

/// A second EC2 node, with the 17-character suffix form.
const EC2_TWO: &str = "i-0123456789abcdef0";

/// A hybrid-activation node id: `mi-` and 17 characters (same pattern).
const HYBRID: &str = "mi-01234567890123456";

/// A node id that is well formed but names no registered node.
const UNREGISTERED: &str = "i-0dead999";

/// A router with just this service mounted, and the engine it was mounted on — the handle
/// a case seeds nodes through and moves the clock with.
fn service() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still reads the SigV4
    // credential scope below, which is where the Region comes from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Call `operation` signed for `region`, returning `(status, body)`.
async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{region}/ssm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // The error shape travels in `__type`; fold in the header the framework also sets so
    // one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Call `operation` signed for [`REGION`].
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, operation, body).await
}

/// The wire error code of a refusal.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no type>")
        .to_string()
}

/// The message of a refusal.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or("<no message>")
        .to_string()
}

/// A string member of a response body.
fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or("<unset>").to_string()
}

/// Create an Automation runbook of `name` with `content`, asserting it succeeded.
async fn runbook(router: &MultiServiceRouter, name: &str, content: &Value) {
    let (status, created) = call(
        router,
        "CreateDocument",
        &json!({
            "Name": name,
            "DocumentType": "Automation",
            "Content": content.to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the runbook {name} must be created first, got {status} with {created:?}"
    );
}

/// Start a preview, asserting it succeeded, and answering the id it minted.
async fn started(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, response) = call(router, "StartExecutionPreview", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {response:?}"
    );
    let id = text(&response, "ExecutionPreviewId");
    assert_eq!(
        id.chars().count(),
        36,
        "case {case:?}: expected the modelled 36-character preview id, got {id:?}"
    );
    id
}

/// Read a preview back through [`REGION`], asserting the read succeeded.
async fn read(router: &MultiServiceRouter, case: &str, preview_id: &str) -> Value {
    let (status, response) = call(
        router,
        "GetExecutionPreview",
        &json!({"ExecutionPreviewId": preview_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {response:?}"
    );
    response
}

/// How many previews the store holds, to pin that a refused start persisted nothing.
fn stored(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine
        .state::<ComAmazonawsSsmAmazonssmState>()
        .lock()
        .execution_previews
        .len()
}

/// Let the clock reach the instant every preview of `region` is due to have been worked
/// out at.
///
/// The service owes a started preview ONE transition, a span after the start
/// (`execution_previews::COMPLETION_DELAY`); a case that asserts what the preview says
/// moves the clock to it through the store rather than sleeping for it.
fn work_out(engine: &Arc<engine::InMemoryEngine>, region: &str) {
    let state = engine.state::<ComAmazonawsSsmAmazonssmState>();
    let mut store = state.lock();
    let long_ago = SystemTime::now() - Duration::from_secs(3_600);
    for ((held, _), preview) in store.execution_previews.iter_mut() {
        if held == region {
            preview.ready_at = long_ago;
        }
    }
}

/// Start a preview, let the clock reach the instant it is due, and answer what the read
/// reports for it.
async fn previewed(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    body: &Value,
) -> Value {
    let id = started(router, case, body).await;
    work_out(engine, REGION);
    let reported = read(router, case, &id).await;
    assert_eq!(
        text(&reported, "Status"),
        "Success",
        "case {case:?}: expected a worked-out preview, got {reported}"
    );
    reported["ExecutionPreview"]["Automation"].clone()
}

/// A runbook of one step running `action` with `inputs`.
fn one_step_runbook(action: &str, inputs: Value) -> Value {
    json!({
        "schemaVersion": "0.3",
        "mainSteps": [{"name": "only", "action": action, "inputs": inputs, "isEnd": true}],
    })
}

/// A runbook that declares the parameter a rate-controlled preview binds its targets to.
fn targeting_runbook() -> Value {
    json!({
        "schemaVersion": "0.3",
        "parameters": {"Node": {"type": "String"}},
        "mainSteps": [{
            "name": "stop",
            "action": "aws:changeInstanceState",
            "inputs": {"InstanceIds": ["{{ Node }}"], "DesiredState": "stopped"},
            "isEnd": true,
        }],
    })
}

/// Register `record` in `region` the way a node's own registration would.
fn register(engine: &Arc<engine::InMemoryEngine>, region: &str, record: ManagedNodeRecord) {
    assert!(
        engine
            .state::<ComAmazonawsSsmAmazonssmState>()
            .lock()
            .insert_managed_node(region, record),
        "a seeded node must carry the node id it is stored under"
    );
}

/// Seed the nodes the target cases resolve against: two EC2 instances and one hybrid
/// managed node, the hybrid one tagged so a tag criterion can select across both kinds.
fn seed_nodes(engine: &Arc<engine::InMemoryEngine>) {
    let at = SystemTime::now();
    for (id, resource_type, env) in [
        (EC2_ONE, RESOURCE_TYPE_EC2_INSTANCE, "prod"),
        (EC2_TWO, RESOURCE_TYPE_EC2_INSTANCE, "qa"),
        (HYBRID, RESOURCE_TYPE_MANAGED_INSTANCE, "prod"),
    ] {
        let mut record = ManagedNodeRecord::registering(id, resource_type, at);
        record.tags.insert("env".to_string(), env.to_string());
        register(engine, REGION, record);
    }
}

/// One row of the step-impact table: the action a step runs, the inputs it carries and the
/// `ImpactType` previewing it reports.
struct ImpactRow {
    case: &'static str,
    action: &'static str,
    inputs: Value,
    impact: &'static str,
}

/// A preview counts a runbook's steps by the impact running each one WOULD have, which is
/// decided by the step's action.
///
/// The rows are the three `ImpactType` classes the model publishes
/// (`AutomationExecutionPreview$StepPreviews`: a step that creates, modifies or deletes
/// resources; one that only retrieves data; one whose work another orchestration service or
/// a third-party API performs), each exercised through several of the actions the Systems
/// Manager Automation action reference publishes — including an action no reference
/// publishes at all, whose outcome is therefore not known either.
#[tokio::test]
async fn a_preview_counts_each_step_by_the_impact_its_action_would_have() {
    let (router, engine) = service();
    let rows = [
        ImpactRow {
            case: "an action that creates a resource",
            action: "aws:createImage",
            inputs: json!({"InstanceId": "i-0abc1234"}),
            impact: execution_previews::IMPACT_MUTATING,
        },
        ImpactRow {
            case: "an action that deletes a resource",
            action: "aws:deleteStack",
            inputs: json!({"StackNameOrId": "a-stack"}),
            impact: execution_previews::IMPACT_MUTATING,
        },
        ImpactRow {
            case: "an API call that reads",
            action: "aws:executeAwsApi",
            inputs: json!({"Service": "ec2", "Api": "DescribeInstances"}),
            impact: execution_previews::IMPACT_NON_MUTATING,
        },
        ImpactRow {
            case: "an API call that changes a resource",
            action: "aws:executeAwsApi",
            inputs: json!({"Service": "ec2", "Api": "StopInstances"}),
            impact: execution_previews::IMPACT_MUTATING,
        },
        ImpactRow {
            case: "an action that asserts a property",
            action: "aws:assertAwsResourceProperty",
            inputs: json!({"Service": "ec2", "Api": "DescribeInstances"}),
            impact: execution_previews::IMPACT_NON_MUTATING,
        },
        ImpactRow {
            case: "an action that only waits",
            action: "aws:sleep",
            inputs: json!({"Duration": "PT0S"}),
            impact: execution_previews::IMPACT_NON_MUTATING,
        },
        ImpactRow {
            case: "an action another service performs",
            action: "aws:invokeLambdaFunction",
            inputs: json!({"FunctionName": "a-function"}),
            impact: execution_previews::IMPACT_UNDETERMINED,
        },
        ImpactRow {
            case: "an action Run Command performs",
            action: "aws:runCommand",
            inputs: json!({"DocumentName": "AWS-RunShellScript"}),
            impact: execution_previews::IMPACT_UNDETERMINED,
        },
        ImpactRow {
            case: "an action no reference publishes",
            action: "aws:somethingNobodyPublishes",
            inputs: json!({}),
            impact: execution_previews::IMPACT_UNDETERMINED,
        },
    ];

    for (position, row) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("Impact-{position}");
        runbook(
            &router,
            &name,
            &one_step_runbook(row.action, row.inputs.clone()),
        )
        .await;
        let preview = previewed(&router, &engine, case, &json!({"DocumentName": name})).await;
        assert_eq!(
            preview["StepPreviews"],
            json!({row.impact: 1}),
            "case {case:?}: expected the one step counted as {}, got {preview}",
            row.impact
        );
    }

    // The counts are per impact type over the WHOLE runbook, so a runbook mixing them
    // reports one count each and they add up to the number of steps it declares.
    let mixed = json!({
        "schemaVersion": "0.3",
        "mainSteps": [
            {"name": "look", "action": "aws:executeAwsApi",
             "inputs": {"Service": "ec2", "Api": "DescribeInstances"}},
            {"name": "tag", "action": "aws:createTags", "inputs": {"Tags": []}},
            {"name": "stop", "action": "aws:changeInstanceState",
             "inputs": {"DesiredState": "stopped"}},
            {"name": "hand-off", "action": "aws:executeStateMachine",
             "inputs": {"stateMachineArn": "a-machine"}, "isEnd": true},
        ],
    });
    runbook(&router, "Mixed", &mixed).await;
    let case = "a runbook mixing the three classes";
    let preview = previewed(&router, &engine, case, &json!({"DocumentName": "Mixed"})).await;
    assert_eq!(
        preview["StepPreviews"],
        json!({"NonMutating": 1, "Mutating": 2, "Undetermined": 1}),
        "case {case:?}: expected one count per impact type over every step, got {preview}"
    );
}

/// One row of the Regions-and-accounts table: the execution inputs a start carries, and
/// the Regions and account count previewing them reports.
struct ScopeRow {
    case: &'static str,
    inputs: Value,
    regions: Vec<&'static str>,
    total_accounts: i64,
}

/// A preview reports the Regions and the accounts the execution would run in: the ones its
/// target locations name, and the Region the start was signed for with this one account
/// when they name none.
#[tokio::test]
async fn a_preview_reports_the_regions_and_accounts_the_execution_would_run_in() {
    let (router, engine) = service();
    runbook(&router, "Scoped", &one_step_runbook("aws:sleep", json!({"Duration": "PT0S"}))).await;

    let rows = [
        ScopeRow {
            case: "no execution inputs at all",
            inputs: Value::Null,
            regions: vec![REGION],
            total_accounts: 1,
        },
        ScopeRow {
            case: "automation inputs naming no target location",
            inputs: json!({"Automation": {}}),
            regions: vec![REGION],
            total_accounts: 1,
        },
        ScopeRow {
            case: "one target location over two Regions and two accounts",
            inputs: json!({"Automation": {"TargetLocations": [{
                "Regions": ["eu-west-1", "ap-south-1"],
                "Accounts": ["111122223333", "444455556666"],
            }]}}),
            regions: vec!["eu-west-1", "ap-south-1"],
            total_accounts: 2,
        },
        ScopeRow {
            case: "two target locations naming one Region twice",
            inputs: json!({"Automation": {"TargetLocations": [
                {"Regions": ["eu-west-1"], "Accounts": ["111122223333"]},
                {"Regions": ["eu-west-1", "us-east-2"], "Accounts": ["111122223333"]},
            ]}}),
            regions: vec!["eu-west-1", "us-east-2"],
            total_accounts: 1,
        },
        ScopeRow {
            case: "a target location excluding one of its accounts",
            inputs: json!({"Automation": {"TargetLocations": [{
                "Regions": ["eu-west-1"],
                "Accounts": ["111122223333", "444455556666"],
                "ExcludeAccounts": ["444455556666"],
            }]}}),
            regions: vec!["eu-west-1"],
            total_accounts: 1,
        },
        ScopeRow {
            case: "a target location excluding every account it names",
            inputs: json!({"Automation": {"TargetLocations": [{
                "Regions": ["eu-west-1"],
                "Accounts": ["111122223333"],
                "ExcludeAccounts": ["111122223333"],
            }]}}),
            regions: vec!["eu-west-1"],
            total_accounts: 0,
        },
    ];

    for row in rows {
        let case = row.case;
        let mut body = json!({"DocumentName": "Scoped"});
        if !row.inputs.is_null() {
            body["ExecutionInputs"] = row.inputs.clone();
        }
        let preview = previewed(&router, &engine, case, &body).await;
        assert_eq!(
            preview["Regions"],
            json!(row.regions),
            "case {case:?}: expected the Regions the execution would run in, got {preview}"
        );
        // `TotalAccounts` is modelled with a default of 0, so a count of none may travel as
        // the absent member; either spelling means the same number of accounts.
        assert_eq!(
            preview["TotalAccounts"].as_i64().unwrap_or(0),
            row.total_accounts,
            "case {case:?}: expected {} accounts, got {preview}",
            row.total_accounts
        );
    }
}

/// One row of the target table: the targets a start carries, and the resource counts
/// previewing them reports, keyed by resource type.
struct TargetRow {
    case: &'static str,
    inputs: Value,
    counts: Value,
}

/// A preview counts the resources its targets resolve to, grouped by the type each of them
/// is: the criteria are resolved against the nodes this Region holds, so criteria that
/// describe no registered node contribute no resource.
#[tokio::test]
async fn a_preview_counts_the_resources_its_targets_resolve_to() {
    let (router, engine) = service();
    seed_nodes(&engine);
    runbook(&router, "Targeting", &targeting_runbook()).await;

    let rows = [
        TargetRow {
            case: "no targets at all",
            inputs: json!({"Automation": {"Parameters": {"Node": [EC2_ONE]}}}),
            counts: json!([]),
        },
        TargetRow {
            case: "node ids across both resource types",
            inputs: json!({"Automation": {
                "TargetParameterName": "Node",
                "Targets": [{"Key": "InstanceIds", "Values": [EC2_ONE, EC2_TWO, HYBRID]}],
            }}),
            counts: json!([
                {"TargetType": "AWS::EC2::Instance", "Count": 2},
                {"TargetType": "AWS::SSM::ManagedInstance", "Count": 1},
            ]),
        },
        TargetRow {
            case: "every EC2 instance of the Region",
            inputs: json!({"Automation": {
                "TargetParameterName": "Node",
                "Targets": [{"Key": "AWS::EC2::Instance", "Values": ["*"]}],
            }}),
            counts: json!([{"TargetType": "AWS::EC2::Instance", "Count": 2}]),
        },
        TargetRow {
            case: "a tag criterion spanning both resource types",
            inputs: json!({"Automation": {
                "TargetParameterName": "Node",
                "Targets": [{"Key": "tag:env", "Values": ["prod"]}],
            }}),
            counts: json!([
                {"TargetType": "AWS::EC2::Instance", "Count": 1},
                {"TargetType": "AWS::SSM::ManagedInstance", "Count": 1},
            ]),
        },
        TargetRow {
            case: "a criterion no registered node answers",
            inputs: json!({"Automation": {
                "TargetParameterName": "Node",
                "Targets": [{"Key": "InstanceIds", "Values": [UNREGISTERED]}],
            }}),
            counts: json!([]),
        },
        TargetRow {
            case: "parameter values, only one of which names a node",
            inputs: json!({"Automation": {
                "TargetParameterName": "Node",
                "Targets": [{"Key": "ParameterValues", "Values": [EC2_ONE, "a-plain-value"]}],
            }}),
            counts: json!([{"TargetType": "AWS::EC2::Instance", "Count": 1}]),
        },
    ];

    for row in rows {
        let case = row.case;
        let preview = previewed(
            &router,
            &engine,
            case,
            &json!({"DocumentName": "Targeting", "ExecutionInputs": row.inputs}),
        )
        .await;
        assert_eq!(
            preview["TargetPreviews"],
            row.counts,
            "case {case:?}: expected the resources the targets resolve to, got {preview}"
        );
    }

    // The nodes of ANOTHER Region are not the ones this preview resolves against: the same
    // criterion through this endpoint resolves only what this Region holds.
    let case = "a node registered in another Region";
    let at = SystemTime::now();
    register(
        &engine,
        OTHER_REGION,
        ManagedNodeRecord::registering("i-0eeeeeee", RESOURCE_TYPE_EC2_INSTANCE, at),
    );
    let preview = previewed(
        &router,
        &engine,
        case,
        &json!({"DocumentName": "Targeting", "ExecutionInputs": {"Automation": {
            "TargetParameterName": "Node",
            "Targets": [{"Key": "AWS::EC2::Instance", "Values": ["*"]}],
        }}}),
    )
    .await;
    assert_eq!(
        preview["TargetPreviews"],
        json!([{"TargetType": "AWS::EC2::Instance", "Count": 2}]),
        "case {case:?}: expected only this Region's nodes to be counted, got {preview}"
    );
}

/// A started preview is `Pending` with nothing to report until the service has had time to
/// work it out, and reports the same computation and the same end instant at every read
/// after that.
#[tokio::test]
async fn a_preview_is_pending_until_the_service_has_worked_it_out() {
    let (router, engine) = service();
    runbook(
        &router,
        "Pending",
        &one_step_runbook("aws:createImage", json!({"InstanceId": EC2_ONE})),
    )
    .await;

    let case = "straight after the start";
    let id = started(&router, case, &json!({"DocumentName": "Pending"})).await;
    let pending = read(&router, case, &id).await;
    assert_eq!(
        (
            text(&pending, "ExecutionPreviewId"),
            text(&pending, "Status")
        ),
        (id.clone(), "Pending".to_string()),
        "case {case:?}: expected the preview reported as pending under its own id, got \
         {pending}"
    );
    assert!(
        pending["ExecutionPreview"].is_null() && pending["EndedAt"].is_null(),
        "case {case:?}: expected a pending preview to report no result and no end instant, \
         got {pending}"
    );

    let case = "once the service has worked it out";
    work_out(&engine, REGION);
    let first = read(&router, case, &id).await;
    assert_eq!(
        text(&first, "Status"),
        "Success",
        "case {case:?}: expected the preview worked out, got {first}"
    );
    assert_eq!(
        first["ExecutionPreview"]["Automation"]["StepPreviews"],
        json!({"Mutating": 1}),
        "case {case:?}: expected the computation the start persisted, got {first}"
    );
    assert!(
        first["EndedAt"].is_number(),
        "case {case:?}: expected the instant the preview ended at, got {first}"
    );

    // A read reports; it does not move the preview on. A second read answers the same
    // thing, end instant included.
    let case = "a second read";
    let second = read(&router, case, &id).await;
    assert_eq!(
        second, first,
        "case {case:?}: expected a read to report what the preview holds, unchanged"
    );
}

/// A read of a preview id this Region holds none of is the modelled miss, and so is a read
/// through another Region's endpoint of a preview this one started.
#[tokio::test]
async fn a_read_of_a_preview_nothing_started_is_the_modelled_miss() {
    let (router, engine) = service();
    runbook(&router, "Missing", &one_step_runbook("aws:sleep", json!({"Duration": "PT0S"}))).await;
    let id = started(&router, "region isolation", &json!({"DocumentName": "Missing"})).await;

    for (case, region, preview_id) in [
        ("an id nothing minted", REGION, UNKNOWN_PREVIEW_ID),
        ("another Region's endpoint", OTHER_REGION, id.as_str()),
    ] {
        let (status, response) = call_in_region(
            &router,
            region,
            "GetExecutionPreview",
            &json!({"ExecutionPreviewId": preview_id}),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected the modelled miss, got {status} with {response}"
        );
        assert_eq!(
            error_message(&response),
            format!("Execution preview {preview_id} does not exist."),
            "case {case:?}: expected the refusal to name the id asked for, got {response}"
        );
    }
    assert_eq!(
        stored(&engine),
        1,
        "a refused read must not persist a preview of its own"
    );
}

/// A start that resolves no runbook, no version or no parameter set is refused as invalid
/// and persists nothing: a preview is of something, so there is nothing to work out.
#[tokio::test]
async fn a_start_that_resolves_nothing_is_refused_and_persists_nothing() {
    let (router, engine) = service();
    runbook(&router, "Resolving", &targeting_runbook()).await;
    let held = stored(&engine);

    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a name no document carries",
            json!({"DocumentName": "No-Such-Runbook"}),
            "Document No-Such-Runbook does not exist.".to_string(),
        ),
        (
            "a document that is no Automation runbook",
            json!({"DocumentName": PUBLISHED_COMMAND}),
            format!("Document {PUBLISHED_COMMAND} does not exist."),
        ),
        (
            "a version the runbook does not hold",
            json!({"DocumentName": "Resolving", "DocumentVersion": "7"}),
            "Document Resolving version 7 does not exist.".to_string(),
        ),
        (
            "a parameter the runbook does not declare",
            json!({"DocumentName": "Resolving", "ExecutionInputs": {"Automation": {
                "Parameters": {"Nope": ["a-value"]},
            }}}),
            "Document Resolving doesn't support a parameter named Nope".to_string(),
        ),
        (
            "targets with no parameter to bind them to",
            json!({"DocumentName": "Resolving", "ExecutionInputs": {"Automation": {
                "Targets": [{"Key": "InstanceIds", "Values": [EC2_ONE]}],
            }}}),
            "TargetParameterName must be specified with Targets".to_string(),
        ),
        (
            "a target key an automation does not resolve by",
            json!({"DocumentName": "Resolving", "ExecutionInputs": {"Automation": {
                "TargetParameterName": "Node",
                "Targets": [{"Key": "NotAKey", "Values": [EC2_ONE]}],
            }}}),
            "Target key 'NotAKey' isn't one of the target keys an automation resolves its \
             targets by"
                .to_string(),
        ),
    ];

    for (case, body, says) in rows {
        let (status, response) = call(&router, "StartExecutionPreview", &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} with {response}"
        );
        let message = error_message(&response);
        assert!(
            message.contains(&says),
            "case {case:?}: expected the refusal to say {says:?}, got {message:?}"
        );
        assert_eq!(
            stored(&engine),
            held,
            "case {case:?}: expected a refused start to persist nothing"
        );
    }
}
