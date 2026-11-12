// SPDX-License-Identifier: CC0-1.0

//! The Region switch plan lifecycle over the wire.
//!
//! Every case here drives the emulator through the same `MultiServiceRouter::dispatch`
//! the router serves on a socket, so what these tests observe is what an SDK observes:
//! `awsJson1_0`, `POST /`, `X-Amz-Target: ArcRegionSwitch.<Operation>`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_arcregionswitch_arcregionswitch::state::{
    EXECUTION_START_SPAN, MAX_PLANS, UNSUPPORTED_EXECUTION_REASON,
};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The canonical account id every ARN a generated emulator mints carries
/// (`engine::emulator_account_id`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// The values the model's `ExecutionState` enum declares, from
/// `smithy_query(query="shape", shape="ExecutionState", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`.
const EXECUTION_STATES: &[&str] = &[
    "inProgress",
    "pausedByFailedStep",
    "pausedByOperator",
    "completed",
    "completedWithExceptions",
    "canceled",
    "planExecutionTimedOut",
    "pendingManualApproval",
    "failed",
    "pending",
    "completedMonitoringApplicationHealth",
];

/// A router carrying this one service on a fresh shared engine.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // This suite exercises operation behaviour, not request signing.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_arcregionswitch_arcregionswitch::EmulatedService::new(engine),
    );
    router
}

/// Call one operation in the Region this suite's requests are scoped to, and return its
/// HTTP status and parsed body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    call_in_region(router, "us-east-1", operation, body).await
}

/// Call one operation with the request scoped to `region` — what a caller reaching that
/// Region's endpoint sends — and return its HTTP status and parsed body.
pub(crate) async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("ArcRegionSwitch.{operation}"))
        // The router keys on the signing name in the SigV4 credential scope.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDTEST/20260101/{region}/arc-region-switch/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of an error response: the `__type` member's shape name.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// How long a case polls for a transition the clock owes before it gives up.
///
/// Generous on purpose: it bounds a POLL LOOP that ends as soon as the condition holds, so
/// a fast machine never waits for it and a loaded one is not failed by it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long such a loop waits between looks.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Poll `GetPlanExecution` until the execution reports one of `states`, or
/// [`SETTLE_DEADLINE`] passes.
///
/// Waits for the CONDITION the clock owes rather than for a duration, so nothing about the
/// case depends on how fast the host runs.
async fn poll_until_execution_state(
    router: &MultiServiceRouter,
    case: &str,
    plan_arn: &str,
    execution_id: &str,
    states: &[&str],
) {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, execution) = call(
            router,
            "GetPlanExecution",
            &json!({"planArn": plan_arn, "executionId": execution_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPlanExecution expected 200, got {status} with {execution}"
        );
        let state = execution["executionState"].as_str().unwrap_or_default();
        if states.contains(&state) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected the execution to reach one of {states:?}, it reports \
             {state:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `message` of an error response.
pub(crate) fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// One workflow with `step_names` manual-approval steps.
pub(crate) fn workflow(action: &str, step_names: &[&str]) -> Value {
    let steps: Vec<Value> = step_names
        .iter()
        .map(|name| {
            json!({
                "name": name,
                "executionBlockType": "ManualApproval",
                "executionBlockConfiguration": {
                    "executionApprovalConfig": {
                        "approvalRole": format!("arn:aws:iam::{ACCOUNT}:role/Approver"),
                        // Sent explicitly: the member carries `@default: 60`, which the
                        // framework materialises, so an omitted one comes back filled in.
                        "timeoutMinutes": 60,
                    }
                }
            })
        })
        .collect();
    json!({
        "workflowTargetAction": action,
        "workflowDescription": format!("the {action} workflow"),
        "steps": steps,
    })
}

/// The workflow set most cases use: one per traffic-shifting action.
pub(crate) fn default_workflows() -> Value {
    json!([
        workflow("activate", &["scale-up", "shift-traffic"]),
        workflow("deactivate", &["shift-traffic", "scale-down"]),
    ])
}

/// One workflow whose `step_names` steps are each a Lambda execution block — a block that
/// drives ANOTHER service's resources, which this emulator does not run, so the clock
/// fails the execution when it reaches the first of them.
///
/// Members from `smithy_query(query="shape", shape="CustomActionLambdaConfiguration",
/// model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`:
/// `lambdas`, `retryIntervalMinutes` and `regionToRun` are required, `timeoutMinutes`
/// carries `@default: 60`.
fn unrunnable_workflow(action: &str, step_names: &[&str]) -> Value {
    let steps: Vec<Value> = step_names
        .iter()
        .map(|name| {
            json!({
                "name": name,
                "executionBlockType": "CustomActionLambda",
                "executionBlockConfiguration": {
                    "customActionLambdaConfig": {
                        "timeoutMinutes": 60,
                        "retryIntervalMinutes": 1.0,
                        "regionToRun": "activatingRegion",
                        "lambdas": [{
                            "arn": format!("arn:aws:lambda:us-west-2:{ACCOUNT}:function:switch"),
                        }],
                    }
                }
            })
        })
        .collect();
    json!({
        "workflowTargetAction": action,
        "workflowDescription": format!("the {action} workflow"),
        "steps": steps,
    })
}

/// A `CreatePlan` request carrying only the required members plus `workflows`.
pub(crate) fn create_request(
    name: &str,
    regions: [&str; 2],
    approach: &str,
    workflows: Value,
) -> Value {
    json!({
        "name": name,
        "regions": regions,
        "recoveryApproach": approach,
        "executionRole": format!("arn:aws:iam::{ACCOUNT}:role/ArcRegionSwitchRole"),
        "workflows": workflows,
    })
}

/// Create a plan and return its ARN.
pub(crate) async fn create_plan(
    router: &MultiServiceRouter,
    name: &str,
    regions: [&str; 2],
) -> String {
    let request = create_request(name, regions, "activePassive", default_workflows());
    let (status, created) = call(router, "CreatePlan", &request).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreatePlan expected 200, got {status} with {created}"
    );
    created["plan"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Create a plan whose workflows this emulator cannot run a single step of, and return
/// its ARN: an execution of it reaches a state it never leaves, which is what a case
/// about a FINISHED execution needs.
async fn create_unrunnable_plan(
    router: &MultiServiceRouter,
    name: &str,
    regions: [&str; 2],
) -> String {
    let request = create_request(
        name,
        regions,
        "activePassive",
        json!([
            unrunnable_workflow("activate", &["scale-up", "shift-traffic"]),
            unrunnable_workflow("deactivate", &["shift-traffic", "scale-down"]),
        ]),
    );
    let (status, created) = call(router, "CreatePlan", &request).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreatePlan expected 200, got {status} with {created}"
    );
    created["plan"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `PlanArn`'s pattern:
/// `arn:aws<partition>:arc-region-switch::<12 digits>:plan/<name>:<6 of [a-z0-9]>`.
fn assert_plan_arn(case: &str, arn: &str, name: &str) {
    let prefix = format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/{name}:");
    let Some(suffix) = arn.strip_prefix(&prefix) else {
        panic!("case {case:?}: expected a plan ARN starting {prefix:?}, got {arn:?}");
    };
    assert_eq!(
        suffix.len(),
        6,
        "case {case:?}: expected a 6-character ARN suffix, got {suffix:?}"
    );
    assert!(
        suffix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "case {case:?}: expected an [a-z0-9] ARN suffix, got {suffix:?}"
    );
}

/// Every member a plan was created with comes back from the store, with the server-owned
/// `arn`, `owner`, `version` and `updatedAt` filled in — and `GetPlan` reports the same
/// record `CreatePlan` answered with.
#[tokio::test]
async fn created_plans_read_back_member_for_member_through_get_plan() {
    let router = router();
    // The rows differ along what the rule generalises over: which optional members are
    // present, the recovery approach, the Region pair and its order, the name's length,
    // and the number of workflows and steps.
    let cases: Vec<(&str, Value)> = vec![
        (
            "required members only",
            create_request(
                "a",
                ["us-east-1", "us-west-2"],
                "activePassive",
                json!([workflow("activate", &["only-step"])]),
            ),
        ),
        (
            "active/active, reversed Region order, every optional member",
            json!({
                "name": "maximal-plan-with-a-32char-nam",
                "regions": ["eu-central-1", "eu-west-1"],
                "recoveryApproach": "activeActive",
                "primaryRegion": "eu-west-1",
                "description": "a plan carrying every optional member",
                "executionRole": format!("arn:aws:iam::{ACCOUNT}:role/ArcRegionSwitchRole"),
                "recoveryTimeObjectiveMinutes": 42,
                "serviceQuotaChecksEnabled": true,
                "workflows": default_workflows(),
                "associatedAlarms": {
                    "app-health": {
                        "resourceIdentifier": format!("arn:aws:cloudwatch:eu-west-1:{ACCOUNT}:alarm:app"),
                        "alarmType": "applicationHealth",
                    }
                },
                "triggers": [{
                    "targetRegion": "eu-central-1",
                    "action": "activate",
                    "minDelayMinutesBetweenExecutions": 5,
                    "conditions": [{"associatedAlarmName": "app-health", "condition": "red"}],
                }],
                "reportConfiguration": {
                    "reportOutput": [{"s3Configuration": {"bucketPath": "s3://reports/arc"}}]
                },
                "tags": {"env": "test", "team": "resilience"},
            }),
        ),
        (
            "a plan with a workflow for every action",
            create_request(
                "three-workflow-plan",
                ["ap-northeast-1", "ap-southeast-2"],
                "activePassive",
                json!([
                    workflow("activate", &["a1", "a2", "a3"]),
                    workflow("deactivate", &["d1"]),
                    workflow("postRecovery", &["p1", "p2"]),
                ]),
            ),
        ),
    ];

    for (case, request) in cases {
        let (status, created) = call(&router, "CreatePlan", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreatePlan expected 200, got {status} with {created}"
        );
        let plan = created["plan"].clone();
        let name = request["name"].as_str().unwrap_or_default();
        let arn = plan["arn"].as_str().unwrap_or_default().to_string();
        assert_plan_arn(case, &arn, name);
        assert_eq!(
            plan["owner"],
            json!(ACCOUNT),
            "case {case:?}: expected owner {ACCOUNT}, got {}",
            plan["owner"]
        );
        assert!(
            plan["version"].is_string(),
            "case {case:?}: expected a server-owned version string, got {}",
            plan["version"]
        );
        assert!(
            plan["updatedAt"].is_number(),
            "case {case:?}: expected a server-owned updatedAt timestamp, got {}",
            plan["updatedAt"]
        );

        // Every member of the request that the `Plan` shape carries is reported back.
        for (member, expected) in request.as_object().expect("the request is an object") {
            if member == "tags" {
                // `tags` is not a member of the `Plan` shape; `ListTagsForResource`
                // reports them and is still a stub.
                continue;
            }
            assert_eq!(
                &plan[member], expected,
                "case {case:?}: member {member} expected {expected}, got {}",
                plan[member]
            );
        }

        let (status, fetched) = call(&router, "GetPlan", &json!({"arn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPlan expected 200, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["plan"], plan,
            "case {case:?}: GetPlan expected the stored plan {plan}, got {}",
            fetched["plan"]
        );
    }
}

/// `GetPlan` for an ARN no plan is stored under is the modelled 404.
#[tokio::test]
async fn get_plan_for_an_unknown_arn_is_resource_not_found() {
    let router = router();
    let stored = create_plan(&router, "present", ["us-east-1", "us-west-2"]).await;
    let cases = [
        (
            "never created",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/absent:aaaaaa"),
        ),
        (
            "a stored plan's name with another suffix",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/present:zzzzzz"),
        ),
    ];
    for (case, arn) in cases {
        assert_ne!(
            arn, stored,
            "case {case:?}: the case must not name the stored plan"
        );
        let (status, body) = call(&router, "GetPlan", &json!({"arn": arn})).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// `ListPlans` reports every stored plan exactly once across its pages, whatever the
/// page size, and paging terminates.
#[tokio::test]
async fn list_plans_pages_over_every_stored_plan_exactly_once() {
    for plan_count in [0usize, 1, 2, 5] {
        for page_size in [None, Some(1usize), Some(2), Some(100)] {
            let case = format!("{plan_count} plans, maxResults {page_size:?}");
            let router = router();
            let mut expected = BTreeSet::new();
            for index in 0..plan_count {
                let arn = create_plan(
                    &router,
                    &format!("plan-{index}"),
                    ["us-east-1", "us-west-2"],
                )
                .await;
                expected.insert(arn);
            }

            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            for _ in 0..plan_count + 2 {
                let mut request = json!({});
                if let Some(size) = page_size {
                    request["maxResults"] = json!(size);
                }
                if let Some(token) = &token {
                    request["nextToken"] = json!(token);
                }
                let (status, page) = call(&router, "ListPlans", &request).await;
                assert_eq!(
                    status, 200,
                    "case {case}: ListPlans expected 200, got {status} with {page}"
                );
                let entries = page["plans"].as_array().cloned().unwrap_or_default();
                match page_size {
                    // Every page is as full as it can be, except for the last one.
                    Some(size) => {
                        let remaining = plan_count - seen.len();
                        assert_eq!(
                            entries.len(),
                            remaining.min(size),
                            "case {case}: expected {} entries on this page, got {}",
                            remaining.min(size),
                            entries.len()
                        );
                    }
                    // An absent `maxResults` asks for as many as the member's `@range`
                    // allows (100) — NOT the clamp floor of 1 — so one page carries them all.
                    None => {
                        assert_eq!(
                            entries.len(),
                            plan_count,
                            "case {case}: an absent maxResults expected all {plan_count} plans on \
                             one page, got {}",
                            entries.len()
                        );
                        assert!(
                            page["nextToken"].is_null(),
                            "case {case}: an absent maxResults expected no nextToken, got {}",
                            page["nextToken"]
                        );
                    }
                }
                for entry in entries {
                    assert_eq!(
                        entry["owner"],
                        json!(ACCOUNT),
                        "case {case}: expected owner {ACCOUNT} on the AbbreviatedPlan, got {entry}"
                    );
                    assert!(
                        entry["executionRole"].is_string(),
                        "case {case}: expected executionRole on the AbbreviatedPlan, got {entry}"
                    );
                    assert!(
                        entry["version"].is_string(),
                        "case {case}: expected version on the AbbreviatedPlan, got {entry}"
                    );
                    seen.push(entry["arn"].as_str().unwrap_or_default().to_string());
                }
                token = page["nextToken"].as_str().map(str::to_string);
                if token.is_none() {
                    break;
                }
            }

            assert!(
                token.is_none(),
                "case {case}: paging did not terminate, still holding {token:?}"
            );
            let unique: BTreeSet<String> = seen.iter().cloned().collect();
            assert_eq!(
                unique.len(),
                seen.len(),
                "case {case}: a plan appeared on two pages: {seen:?}"
            );
            assert_eq!(
                unique, expected,
                "case {case}: expected {expected:?}, got {unique:?}"
            );
        }
    }
}

/// One row of the Region-derivation table.
struct RegionRule {
    case: &'static str,
    regions: [&'static str; 2],
    action: &'static str,
    target: &'static str,
    activate: Option<&'static str>,
    deactivate: Option<&'static str>,
}

/// The Regions an execution reports come from the plan and the requested action: the
/// target Region takes the role the action names and the plan's other Region takes the
/// complementary one, while `postRecovery` shifts no traffic and so names neither.
#[tokio::test]
async fn start_plan_execution_derives_the_regions_from_the_plan_and_the_action() {
    let cases = vec![
        RegionRule {
            case: "activate the second Region",
            regions: ["us-east-1", "us-west-2"],
            action: "activate",
            target: "us-west-2",
            activate: Some("us-west-2"),
            deactivate: Some("us-east-1"),
        },
        RegionRule {
            case: "activate the first Region",
            regions: ["us-east-1", "us-west-2"],
            action: "activate",
            target: "us-east-1",
            activate: Some("us-east-1"),
            deactivate: Some("us-west-2"),
        },
        RegionRule {
            case: "deactivate the first Region",
            regions: ["eu-west-1", "eu-central-1"],
            action: "deactivate",
            target: "eu-west-1",
            activate: Some("eu-central-1"),
            deactivate: Some("eu-west-1"),
        },
        RegionRule {
            case: "deactivate the second Region",
            regions: ["ap-southeast-2", "ap-northeast-1"],
            action: "deactivate",
            target: "ap-northeast-1",
            activate: Some("ap-southeast-2"),
            deactivate: Some("ap-northeast-1"),
        },
        RegionRule {
            case: "post-recovery shifts no traffic",
            regions: ["sa-east-1", "us-east-2"],
            action: "postRecovery",
            target: "sa-east-1",
            activate: None,
            deactivate: None,
        },
    ];

    for RegionRule {
        case,
        regions,
        action,
        target,
        activate,
        deactivate,
    } in cases
    {
        let router = router();
        let plan_arn = create_plan(&router, "region-rule", regions).await;
        let mut request = json!({
            "planArn": plan_arn,
            "targetRegion": target,
            "action": action,
            "mode": "graceful",
            "comment": format!("case {case}"),
        });
        if action == "postRecovery" {
            // The model documents recoveryExecutionId as required for a post-recovery
            // execution; its pattern is `<region>/<16 hex>`.
            request["recoveryExecutionId"] = json!(format!("{target}/0123456789abcdef"));
        }

        let (status, started) = call(&router, "StartPlanExecution", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartPlanExecution expected 200, got {status} with {started}"
        );
        assert_eq!(
            started["activateRegion"].as_str(),
            activate,
            "case {case:?}: expected activateRegion {activate:?}, got {}",
            started["activateRegion"]
        );
        assert_eq!(
            started["deactivateRegion"].as_str(),
            deactivate,
            "case {case:?}: expected deactivateRegion {deactivate:?}, got {}",
            started["deactivateRegion"]
        );
        assert_eq!(
            started["plan"].as_str(),
            Some(plan_arn.as_str()),
            "case {case:?}: expected plan {plan_arn}, got {}",
            started["plan"]
        );
        assert!(
            started["planVersion"].is_string(),
            "case {case:?}: expected the plan's version, got {}",
            started["planVersion"]
        );
        let execution_id = started["executionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            execution_id.starts_with(&format!("{target}/")),
            "case {case:?}: expected an executionId in the Region it runs in, got {execution_id:?}"
        );

        let (status, got) = call(
            &router,
            "GetPlanExecution",
            &json!({"planArn": plan_arn, "executionId": execution_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPlanExecution expected 200, got {status} with {got}"
        );
        assert_eq!(
            got["planArn"],
            json!(plan_arn),
            "case {case:?}: expected planArn {plan_arn}, got {}",
            got["planArn"]
        );
        assert_eq!(
            got["executionAction"],
            json!(action),
            "case {case:?}: expected executionAction {action}, got {}",
            got["executionAction"]
        );
        assert_eq!(
            got["executionRegion"],
            json!(target),
            "case {case:?}: expected executionRegion {target}, got {}",
            got["executionRegion"]
        );
        assert_eq!(
            got["mode"],
            json!("graceful"),
            "case {case:?}: expected mode graceful, got {}",
            got["mode"]
        );
        assert_eq!(
            got["comment"],
            json!(format!("case {case}")),
            "case {case:?}: expected the recorded comment, got {}",
            got["comment"]
        );
        assert!(
            got["startTime"].is_number(),
            "case {case:?}: expected a startTime, got {}",
            got["startTime"]
        );
        let state = got["executionState"].as_str().unwrap_or_default();
        assert!(
            EXECUTION_STATES.contains(&state),
            "case {case:?}: expected a modelled ExecutionState, got {state:?}"
        );
        assert_eq!(
            got["plan"]["arn"],
            json!(plan_arn),
            "case {case:?}: expected the execution to carry its plan, got {}",
            got["plan"]
        );
    }
}

/// `StartPlanExecution` refuses, with the error the model declares, a request the plan's
/// own state or the model's own documentation rules out.
#[tokio::test]
async fn start_plan_execution_refuses_what_the_model_declares_an_error_for() {
    let router = router();
    let plan_arn = create_plan(&router, "refusals", ["us-east-1", "us-west-2"]).await;
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a plan ARN that was never created",
            json!({
                "planArn": format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/absent:aaaaaa"),
                "targetRegion": "us-east-1",
                "action": "activate",
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a target Region outside the plan's two Regions",
            json!({
                "planArn": plan_arn,
                "targetRegion": "eu-west-1",
                "action": "activate",
            }),
            400,
            "IllegalArgumentException",
        ),
        (
            "post-recovery without recoveryExecutionId",
            json!({
                "planArn": plan_arn,
                "targetRegion": "us-east-1",
                "action": "postRecovery",
            }),
            400,
            "IllegalArgumentException",
        ),
    ];

    for (case, request, expected_status, expected_code) in cases {
        let (status, body) = call(&router, "StartPlanExecution", &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // And nothing was stored for any of them: the plan is still free to execute.
    let (status, started) = call(
        &router,
        "StartPlanExecution",
        &json!({"planArn": plan_arn, "targetRegion": "us-west-2", "action": "activate"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "a refused start must not hold the plan: expected 200, got {status} with {started}"
    );
}

/// A replayed `clientToken` returns the first execution; the same token with different
/// parameters is the modelled client-token conflict; and while one execution holds the
/// plan, a second one is refused.
#[tokio::test]
async fn a_client_token_maps_to_one_execution_and_a_plan_runs_one_at_a_time() {
    let router = router();
    // A plan whose steps this emulator cannot run, so the first execution reaches a state
    // it never leaves and releases the plan.
    let plan_arn = create_unrunnable_plan(&router, "idempotent", ["us-east-1", "us-west-2"]).await;
    let request = json!({
        "planArn": plan_arn,
        "targetRegion": "us-west-2",
        "action": "activate",
        "mode": "graceful",
        "clientToken": "token-one",
    });

    let (status, first) = call(&router, "StartPlanExecution", &request).await;
    assert_eq!(
        status, 200,
        "first start expected 200, got {status} with {first}"
    );
    let (status, replay) = call(&router, "StartPlanExecution", &request).await;
    assert_eq!(
        status, 200,
        "a replayed clientToken expected 200, got {status} with {replay}"
    );
    assert_eq!(
        replay, first,
        "a replayed clientToken expected the first response {first}, got {replay}"
    );

    let mut different = request.clone();
    different["comment"] = json!("different parameters under the same token");
    let (status, body) = call(&router, "StartPlanExecution", &different).await;
    assert_eq!(
        status, 409,
        "a token reused with different parameters expected 409, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "a token reused with different parameters expected ConflictException, got {body}"
    );

    let mut fresh_token = request.clone();
    fresh_token["clientToken"] = json!("token-two");
    let (status, body) = call(&router, "StartPlanExecution", &fresh_token).await;
    assert_eq!(
        status, 400,
        "a second concurrent execution expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "IllegalStateException",
        "a second concurrent execution expected IllegalStateException, got {body}"
    );

    // One execution exists, and ListPlans reports it as the plan's active one.
    let (status, page) = call(&router, "ListPlans", &json!({})).await;
    assert_eq!(
        status, 200,
        "ListPlans expected 200, got {status} with {page}"
    );
    assert_eq!(
        page["plans"][0]["activePlanExecution"],
        first["executionId"],
        "expected activePlanExecution {}, got {}",
        first["executionId"],
        page["plans"][0]["activePlanExecution"]
    );

    // Once the first execution reaches a state it never leaves, the plan is no longer
    // held: it reports no active execution and a fresh token starts a second one.
    tokio::time::sleep(EXECUTION_START_SPAN + Duration::from_millis(250)).await;
    let (status, page) = call(&router, "ListPlans", &json!({})).await;
    assert_eq!(
        status, 200,
        "ListPlans expected 200, got {status} with {page}"
    );
    assert!(
        page["plans"][0]["activePlanExecution"].is_null(),
        "after the first execution finished, expected no activePlanExecution, got {}",
        page["plans"][0]["activePlanExecution"]
    );
    let (status, second) = call(&router, "StartPlanExecution", &fresh_token).await;
    assert_eq!(
        status, 200,
        "a second execution after the first finished expected 200, got {status} with {second}"
    );
    assert_ne!(
        second["executionId"], first["executionId"],
        "a second execution expected a new executionId, got {}",
        second["executionId"]
    );
}

/// An execution is created in a transient state and the clock — not the read — moves it
/// on, so a caller acting straight after the start still sees the transient state. An
/// execution whose next step is a block this emulator does not run reaches the service's
/// modelled FAILURE state, with the reason recorded; and failing RELEASES the plan.
#[tokio::test]
async fn an_execution_starts_pending_and_the_clock_settles_it_into_the_failure_state() {
    let router = router();
    let plan_arn = create_unrunnable_plan(&router, "clock", ["us-east-1", "us-west-2"]).await;
    let (status, started) = call(
        &router,
        "StartPlanExecution",
        &json!({"planArn": plan_arn, "targetRegion": "us-west-2", "action": "activate"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartPlanExecution expected 200, got {status} with {started}"
    );
    let execution_id = started["executionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let get = json!({"planArn": plan_arn, "executionId": execution_id});

    let (_, immediately) = call(&router, "GetPlanExecution", &get).await;
    assert_eq!(
        immediately["executionState"],
        json!("pending"),
        "case \"straight after the start\": expected pending, got {}",
        immediately["executionState"]
    );
    assert!(
        immediately["endTime"].is_null(),
        "case \"straight after the start\": expected no endTime, got {}",
        immediately["endTime"]
    );

    tokio::time::sleep(EXECUTION_START_SPAN + Duration::from_millis(250)).await;

    let (_, later) = call(&router, "GetPlanExecution", &get).await;
    assert_eq!(
        later["executionState"],
        json!("failed"),
        "case \"after the outcome came due\": expected failed, got {}",
        later["executionState"]
    );
    assert!(
        later["endTime"].is_number(),
        "case \"after the outcome came due\": expected an endTime, got {}",
        later["endTime"]
    );
    assert_eq!(
        later["startTime"], immediately["startTime"],
        "case \"after the outcome came due\": startTime expected {}, got {}",
        immediately["startTime"], later["startTime"]
    );
    // The step it stopped at is the one that was not performed; the one behind it was
    // never reached.
    let stopped: Vec<(&str, &str)> = later["stepStates"]
        .as_array()
        .map(|steps| {
            steps
                .iter()
                .map(|step| {
                    (
                        step["name"].as_str().unwrap_or_default(),
                        step["status"].as_str().unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        stopped,
        vec![("scale-up", "failed"), ("shift-traffic", "notStarted")],
        "case \"after the outcome came due\": expected the first step failed and the rest \
         unreached, got {stopped:?}"
    );

    // The reason is told, not left to guess: the executionFailed event carries it.
    let (status, events) = call(&router, "ListPlanExecutionEvents", &get).await;
    assert_eq!(
        status, 200,
        "ListPlanExecutionEvents expected 200, got {status} with {events}"
    );
    let failed = events["items"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|event| event["type"] == json!("executionFailed"))
        })
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        failed["error"],
        json!(UNSUPPORTED_EXECUTION_REASON),
        "case \"the failure reason\": expected the unsupported-execution reason, got {failed}"
    );

    // Failing released the plan: it is deletable and executable again.
    let (status, restarted) = call(
        &router,
        "StartPlanExecution",
        &json!({"planArn": plan_arn, "targetRegion": "us-east-1", "action": "activate"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a second execution after the first finished\": expected 200, got {status} with \
         {restarted}"
    );
    assert_ne!(
        restarted["executionId"], started["executionId"],
        "case \"a second execution after the first finished\": expected a new executionId, got {}",
        restarted["executionId"]
    );
    tokio::time::sleep(EXECUTION_START_SPAN + Duration::from_millis(250)).await;
    let (status, deleted) = call(&router, "DeletePlan", &json!({"arn": plan_arn})).await;
    assert_eq!(
        status, 200,
        "case \"delete once no execution holds the plan\": expected 200, got {status} with \
         {deleted}"
    );
}

/// `stepStates` are the steps of the workflow the action runs, none of them started, and
/// they page on `maxResults`/`nextToken` without losing or repeating a step.
#[tokio::test]
async fn get_plan_execution_pages_the_step_states_of_the_workflow_it_runs() {
    for step_count in [1usize, 3, 7] {
        for page_size in [None, Some(1usize), Some(2), Some(100)] {
            let case = format!("{step_count} steps, maxResults {page_size:?}");
            let router = router();
            let names: Vec<String> = (0..step_count).map(|i| format!("step-{i}")).collect();
            let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
            let request = create_request(
                "stepped",
                ["us-east-1", "us-west-2"],
                "activePassive",
                json!([
                    workflow("activate", &borrowed),
                    workflow("deactivate", &["unrelated"]),
                ]),
            );
            let (status, created) = call(&router, "CreatePlan", &request).await;
            assert_eq!(
                status, 200,
                "case {case}: CreatePlan expected 200, got {status} with {created}"
            );
            let plan_arn = created["plan"]["arn"]
                .as_str()
                .unwrap_or_default()
                .to_string();

            let (status, started) = call(
                &router,
                "StartPlanExecution",
                &json!({"planArn": plan_arn, "targetRegion": "us-west-2", "action": "activate"}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: StartPlanExecution expected 200, got {status} with {started}"
            );
            let execution_id = started["executionId"]
                .as_str()
                .unwrap_or_default()
                .to_string();

            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            for _ in 0..step_count + 2 {
                let mut get = json!({
                    "planArn": plan_arn,
                    "executionId": execution_id,
                });
                if let Some(size) = page_size {
                    get["maxResults"] = json!(size);
                }
                if let Some(token) = &token {
                    get["nextToken"] = json!(token);
                }
                let (status, got) = call(&router, "GetPlanExecution", &get).await;
                assert_eq!(
                    status, 200,
                    "case {case}: GetPlanExecution expected 200, got {status} with {got}"
                );
                let page = got["stepStates"].as_array().cloned().unwrap_or_default();
                pages += 1;
                match page_size {
                    // The page is exactly as full as it can be, except for the last one:
                    // this is what distinguishes paging by `maxResults` from paging by one.
                    Some(size) => {
                        let remaining = step_count - seen.len();
                        assert_eq!(
                            page.len(),
                            remaining.min(size),
                            "case {case}: page {pages} expected {} step states, got {}",
                            remaining.min(size),
                            page.len()
                        );
                    }
                    // An absent `maxResults` takes the model's own `@default` of 100 — NOT
                    // the clamp floor of 1 — so every step arrives on the first page.
                    None => {
                        assert_eq!(
                            page.len(),
                            step_count,
                            "case {case}: an absent maxResults expected all {step_count} step \
                             states on page {pages}, got {}",
                            page.len()
                        );
                        assert!(
                            got["nextToken"].is_null(),
                            "case {case}: an absent maxResults expected no nextToken, got {}",
                            got["nextToken"]
                        );
                    }
                }
                for step in page {
                    assert_eq!(
                        step["status"],
                        json!("notStarted"),
                        "case {case}: a step nothing ran expected notStarted, got {step}"
                    );
                    assert_eq!(
                        step["stepMode"],
                        json!("graceful"),
                        "case {case}: expected the execution's mode on the step, got {step}"
                    );
                    seen.push(step["name"].as_str().unwrap_or_default().to_string());
                }
                token = got["nextToken"].as_str().map(str::to_string);
                if token.is_none() {
                    break;
                }
            }

            assert!(
                token.is_none(),
                "case {case}: step-state paging did not terminate, still holding {token:?}"
            );
            assert_eq!(
                seen, names,
                "case {case}: expected the activate workflow's steps {names:?}, got {seen:?}"
            );
        }
    }
}

/// Deleting a plan removes it, and everything recorded against it, from every reader.
#[tokio::test]
async fn delete_plan_removes_the_plan_from_every_reader() {
    let router = router();
    let kept = create_plan(&router, "kept", ["us-east-1", "us-west-2"]).await;
    let doomed = create_plan(&router, "doomed", ["eu-west-1", "eu-central-1"]).await;

    let (status, body) = call(&router, "DeletePlan", &json!({"arn": doomed})).await;
    assert_eq!(
        status, 200,
        "DeletePlan expected 200, got {status} with {body}"
    );

    let (status, body) = call(&router, "GetPlan", &json!({"arn": doomed})).await;
    assert_eq!(
        status, 404,
        "case \"GetPlan after delete\": expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"GetPlan after delete\": expected ResourceNotFoundException, got {body}"
    );

    // @idempotent: repeating the delete is safe, and names a resource that is gone.
    let (status, body) = call(&router, "DeletePlan", &json!({"arn": doomed})).await;
    assert_eq!(
        status, 404,
        "case \"DeletePlan repeated\": expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"DeletePlan repeated\": expected ResourceNotFoundException, got {body}"
    );

    let (status, page) = call(&router, "ListPlans", &json!({})).await;
    assert_eq!(
        status, 200,
        "ListPlans expected 200, got {status} with {page}"
    );
    let listed: Vec<&str> = page["plans"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["arn"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        listed,
        vec![kept.as_str()],
        "case \"ListPlans after delete\": expected only the kept plan, got {listed:?}"
    );
}

/// A plan an execution still holds cannot be deleted, and the refusal leaves it readable.
#[tokio::test]
async fn a_plan_held_by_an_execution_cannot_be_deleted() {
    let router = router();
    let plan_arn = create_unrunnable_plan(&router, "held", ["us-east-1", "us-west-2"]).await;
    let (status, started) = call(
        &router,
        "StartPlanExecution",
        &json!({"planArn": plan_arn, "targetRegion": "us-west-2", "action": "activate"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartPlanExecution expected 200, got {status} with {started}"
    );

    let (status, body) = call(&router, "DeletePlan", &json!({"arn": plan_arn})).await;
    assert_eq!(
        status, 400,
        "case \"delete while an execution holds the plan\": expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "IllegalStateException",
        "case \"delete while an execution holds the plan\": expected IllegalStateException, got {body}"
    );

    let (status, fetched) = call(&router, "GetPlan", &json!({"arn": plan_arn})).await;
    assert_eq!(
        status, 200,
        "case \"the plan after a refused delete\": expected 200, got {status} with {fetched}"
    );

    // The hold is temporary, not a dead end: once the clock settles the execution into a
    // state it never leaves, the plan is released and the delete succeeds.
    tokio::time::sleep(EXECUTION_START_SPAN + Duration::from_millis(250)).await;
    let (status, deleted) = call(&router, "DeletePlan", &json!({"arn": plan_arn})).await;
    assert_eq!(
        status, 200,
        "case \"delete once the execution finished\": expected 200, got {status} with {deleted}"
    );
    let (status, gone) = call(&router, "GetPlan", &json!({"arn": plan_arn})).await;
    assert_eq!(
        status, 404,
        "case \"the plan after the delete\": expected 404, got {status} with {gone}"
    );
}

/// The event timeline is read back in order and pages without losing or repeating an
/// event; `name` selects one step's events; an unknown execution is the modelled 404.
#[tokio::test]
async fn list_plan_execution_events_reports_the_timeline_the_execution_recorded() {
    let router = router();
    let plan_arn = create_unrunnable_plan(&router, "timeline", ["us-east-1", "us-west-2"]).await;
    let (status, started) = call(
        &router,
        "StartPlanExecution",
        &json!({"planArn": plan_arn, "targetRegion": "us-west-2", "action": "activate"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartPlanExecution expected 200, got {status} with {started}"
    );
    let execution_id = started["executionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Straight after the start only the pending event exists; after the outcome comes
    // due the failure event joins it.
    let (status, opening) = call(
        &router,
        "ListPlanExecutionEvents",
        &json!({"planArn": plan_arn, "executionId": execution_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"straight after the start\": expected 200, got {status} with {opening}"
    );
    let opening_types: Vec<&str> = opening["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|event| event["type"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        opening_types,
        vec!["executionPending"],
        "case \"straight after the start\": expected only the pending event, got \
         {opening_types:?}"
    );

    // Wait for the CONDITION the clock owes — the execution reaching the state its
    // unperformable step decides — rather than for a duration.
    poll_until_execution_state(
        &router,
        "the timeline once the outcome came due",
        &plan_arn,
        &execution_id,
        &["failed"],
    )
    .await;

    // Page the whole timeline, one event at a time and all at once.
    for page_size in [None, Some(1usize), Some(100)] {
        let case = format!("maxResults {page_size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut ids: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..4 {
            let mut request = json!({"planArn": plan_arn, "executionId": execution_id});
            if let Some(size) = page_size {
                request["maxResults"] = json!(size);
            }
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let (status, page) = call(&router, "ListPlanExecutionEvents", &request).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} with {page}"
            );
            let items = page["items"].as_array().cloned().unwrap_or_default();
            if let Some(size) = page_size {
                assert!(
                    items.len() <= size,
                    "case {case}: a page carried {} events, more than maxResults {size}",
                    items.len()
                );
            }
            for event in items {
                assert!(
                    event["timestamp"].is_number(),
                    "case {case}: expected a timestamp on every event, got {event}"
                );
                ids.push(event["eventId"].as_str().unwrap_or_default().to_string());
                seen.push(event["type"].as_str().unwrap_or_default().to_string());
            }
            token = page["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case}: event paging did not terminate, still holding {token:?}"
        );
        assert_eq!(
            seen,
            vec!["executionPending", "executionStarted", "executionFailed"],
            "case {case}: expected the recorded timeline in order, got {seen:?}"
        );
        let unique: BTreeSet<&String> = ids.iter().collect();
        assert_eq!(
            unique.len(),
            ids.len(),
            "case {case}: two events share an eventId: {ids:?}"
        );
    }

    // `name` selects one step's events; the execution's own lifecycle events carry no step
    // name, so a step filter answers an empty timeline here.
    let (status, filtered) = call(
        &router,
        "ListPlanExecutionEvents",
        &json!({"planArn": plan_arn, "executionId": execution_id, "name": "scale-up"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"filtered by step name\": expected 200, got {status} with {filtered}"
    );
    assert_eq!(
        filtered["items"],
        json!([]),
        "case \"filtered by step name\": expected no step events, got {}",
        filtered["items"]
    );

    let (status, unreached) = call(
        &router,
        "ListPlanExecutionEvents",
        &json!({"planArn": plan_arn, "executionId": execution_id, "name": "shift-traffic"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"filtered by an unreached step\": expected 200, got {status} with {unreached}"
    );
    assert_eq!(
        unreached["items"],
        json!([]),
        "case \"filtered by an unreached step\": expected no events, got {}",
        unreached["items"]
    );

    let (status, body) = call(
        &router,
        "ListPlanExecutionEvents",
        &json!({"planArn": plan_arn, "executionId": "us-east-1/0123456789abcdef"}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"an execution that was never started\": expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"an execution that was never started\": expected ResourceNotFoundException, got \
         {body}"
    );
}

/// `GetPlanExecution` for an execution the store does not hold is the modelled 404.
#[tokio::test]
async fn get_plan_execution_for_an_unknown_execution_is_resource_not_found() {
    let router = router();
    let plan_arn = create_plan(&router, "no-exec", ["us-east-1", "us-west-2"]).await;
    let cases = [
        ("no execution was ever started", "us-east-1/0123456789abcdef"),
        ("an id in the plan's other Region", "us-west-2/fedcba9876543210"),
    ];
    for (case, execution_id) in cases {
        let (status, body) = call(
            &router,
            "GetPlanExecution",
            &json!({"planArn": plan_arn, "executionId": execution_id}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// One row of the client-token fingerprint table: two parameter sets a naive
/// separator-joined fingerprint renders identically.
struct TokenPair {
    case: &'static str,
    first: (&'static str, &'static str),
    second: (&'static str, &'static str),
}

/// A `clientToken` is bound to the whole parameter set, including free-form members a
/// caller can fill with any byte — so parameter sets that only differ in where a
/// separator falls are still different parameter sets.
#[tokio::test]
async fn a_client_token_distinguishes_parameter_sets_that_share_a_separator() {
    // In each row the bytes move between `comment` and `latestVersion`.
    let cases = [
        TokenPair {
            case: "a unit separator inside the comment",
            first: ("x\u{1f}y", "z"),
            second: ("x", "y\u{1f}z"),
        },
        TokenPair {
            case: "an empty field beside a separator",
            first: ("", "a\u{1f}b"),
            second: ("\u{1f}a", "b"),
        },
    ];
    for TokenPair {
        case,
        first,
        second,
    } in cases
    {
        let router = router();
        let plan_arn = create_plan(&router, "separator", ["us-east-1", "us-west-2"]).await;
        let request = |comment: &str, latest: &str| {
            json!({
                "planArn": plan_arn,
                "targetRegion": "us-west-2",
                "action": "activate",
                "mode": "graceful",
                "clientToken": "token-one",
                "comment": comment,
                "latestVersion": latest,
            })
        };

        let (status, started) =
            call(&router, "StartPlanExecution", &request(first.0, first.1)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first start expected 200, got {status} with {started}"
        );
        let (status, body) =
            call(&router, "StartPlanExecution", &request(second.0, second.1)).await;
        assert_eq!(
            status, 409,
            "case {case:?}: the same token on a different parameter set expected 409, \
             got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
    }
}

/// A `nextToken` this service never issued is refused, whatever shape it takes.
#[tokio::test]
async fn get_plan_execution_refuses_a_next_token_it_never_issued() {
    let router = router();
    let request = create_request(
        "token-check",
        ["us-east-1", "us-west-2"],
        "activePassive",
        json!([workflow("activate", &["one", "two"])]),
    );
    let (status, created) = call(&router, "CreatePlan", &request).await;
    assert_eq!(
        status, 200,
        "CreatePlan expected 200, got {status} with {created}"
    );
    let plan_arn = created["plan"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, started) = call(
        &router,
        "StartPlanExecution",
        &json!({"planArn": plan_arn, "targetRegion": "us-west-2", "action": "activate"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartPlanExecution expected 200, got {status} with {started}"
    );
    let execution_id = started["executionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The execution has two step states, so position 1 is the only token ever handed out.
    let cases = [
        ("not a number", "page-two"),
        ("negative", "-1"),
        ("exactly at the end of the step list", "2"),
        ("one past the end", "3"),
        ("far past the end", "999"),
    ];
    for (case, next_token) in cases {
        let (status, body) = call(
            &router,
            "GetPlanExecution",
            &json!({
                "planArn": plan_arn,
                "executionId": execution_id,
                "nextToken": next_token,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    // And the token it DOES issue is accepted.
    let (status, first) = call(
        &router,
        "GetPlanExecution",
        &json!({"planArn": plan_arn, "executionId": execution_id, "maxResults": 1}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the first page\": expected 200, got {status} with {first}"
    );
    let issued = first["nextToken"].as_str().unwrap_or_default().to_string();
    let (status, second) = call(
        &router,
        "GetPlanExecution",
        &json!({
            "planArn": plan_arn,
            "executionId": execution_id,
            "maxResults": 1,
            "nextToken": issued,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the issued token\": expected 200, got {status} with {second}"
    );
}

/// The plan store is bounded: once it holds `MAX_PLANS`, `CreatePlan` refuses rather than
/// growing, and deleting one makes room again.
#[tokio::test]
async fn the_plan_store_refuses_to_grow_past_its_ceiling() {
    let router = router();
    let mut first = String::new();
    for index in 0..MAX_PLANS {
        let arn = create_plan(&router, &format!("plan-{index}"), ["us-east-1", "us-west-2"]).await;
        if index == 0 {
            first = arn;
        }
    }

    let request = create_request(
        "one-too-many",
        ["us-east-1", "us-west-2"],
        "activePassive",
        default_workflows(),
    );
    let (status, body) = call(&router, "CreatePlan", &request).await;
    assert_eq!(
        status, 400,
        "case \"at the ceiling\": expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "IllegalStateException",
        "case \"at the ceiling\": expected IllegalStateException, got {body}"
    );

    let (status, deleted) = call(&router, "DeletePlan", &json!({"arn": first})).await;
    assert_eq!(
        status, 200,
        "case \"making room\": DeletePlan expected 200, got {status} with {deleted}"
    );
    let (status, created) = call(&router, "CreatePlan", &request).await;
    assert_eq!(
        status, 200,
        "case \"after making room\": expected 200, got {status} with {created}"
    );
}
