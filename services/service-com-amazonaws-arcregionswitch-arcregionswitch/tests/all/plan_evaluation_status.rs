// SPDX-License-Identifier: CC0-1.0

//! `GetPlanEvaluationStatus` over the wire.
//!
//! The rule under test: a plan covers exactly two Amazon Web Services Regions and a
//! switch shifts traffic between them in either direction, so every workflow step that
//! drives regional resources has to configure one in EACH of those Regions. The
//! evaluation of a plan therefore raises one active warning for each of the plan's
//! Regions a step configures no resource in, and one for each resource a step configures
//! in a Region the plan does not cover; the plan's evaluation state is `actionRequired`
//! while any warning is active and `passed` when none is, and both the version the
//! evaluation reports and the warnings themselves come from the stored plan record.
//!
//! Every plan here is performed as a role the account HOLDS, so the evaluation under test
//! is the one about the workflows; the warning a plan's own `executionRole` raises is
//! [`crate::plan_execution_role_evaluation`]'s subject.
//!
//! The request/response helpers are shared with [`crate::plan_lifecycle`]: both suites
//! drive the emulator through the same `MultiServiceRouter::dispatch` the router serves
//! on a socket (`awsJson1_0`, `POST /`, `X-Amz-Target: ArcRegionSwitch.<Operation>`).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_arcregionswitch_arcregionswitch::state::{
    EVALUATION_ACTION_REQUIRED, EVALUATION_PASSED, WARNING_STATUS_ACTIVE,
};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::plan_lifecycle::{call, create_request, default_workflows, error_code, ACCOUNT};
use crate::switch_support::given_role;

/// The role every plan in this suite names as its `executionRole`
/// ([`crate::plan_lifecycle::create_request`]). It is recorded in IAM before any plan is
/// created, because a plan whose `executionRole` resolves to no role of the account is
/// `actionRequired` on that ground alone — which is
/// [`crate::plan_execution_role_evaluation`]'s subject, not this suite's: here the
/// evaluation under test is the one about the WORKFLOWS.
const EXECUTION_ROLE: &str = "ArcRegionSwitchRole";

/// A router carrying this service, with [`EXECUTION_ROLE`] already recorded in the IAM
/// store of the same shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    given_role(&engine, EXECUTION_ROLE);
    let mut router = MultiServiceRouter::new();
    // This suite exercises operation behaviour, not request signing.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_arcregionswitch_arcregionswitch::EmulatedService::new(engine),
    );
    router
}

/// Every Region any case below names. A warning's Region is read back out of its message
/// against this catalog, so a case states which Region each warning is about without the
/// assertions knowing which message produced it. Each entry is a value of the model's
/// `Region` shape (pattern `^[a-z]{2}-[a-z-]+-\d+$`, from
/// `smithy_query(query="shape", shape="Region", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
const REGION_CATALOG: &[&str] = &[
    "us-east-1",
    "us-west-2",
    "eu-west-1",
    "eu-central-1",
    "ap-southeast-2",
    "sa-east-1",
];

/// A Lambda function ARN in `region`.
fn lambda_arn(region: &str) -> String {
    format!("arn:aws:lambda:{region}:{ACCOUNT}:function:switch")
}

/// An EC2 Auto Scaling group ARN in `region`, as `AsgArn`'s pattern spells one.
fn asg_arn(region: &str) -> String {
    format!(
        "arn:aws:autoscaling:{region}:{ACCOUNT}:autoScalingGroup:\
         12345678-1234-1234-1234-1234567890ab:autoScalingGroupName/web"
    )
}

/// An ECS service ARN in `region`, as `EcsServiceArn`'s pattern spells one.
fn ecs_service_arn(region: &str) -> String {
    format!("arn:aws:ecs:{region}:{ACCOUNT}:service/prod/web")
}

/// An ECS cluster ARN in `region`, as `EcsClusterArn`'s pattern spells one.
fn ecs_cluster_arn(region: &str) -> String {
    format!("arn:aws:ecs:{region}:{ACCOUNT}:cluster/prod")
}

/// An RDS database instance ARN in `region`, as `RdsDbInstanceArn`'s pattern spells one.
fn rds_instance_arn(region: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:db:orders-replica")
}

/// An Aurora cluster ARN in `region`, as `AuroraClusterArn`'s pattern spells one.
fn aurora_cluster_arn(region: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:cluster:orders")
}

/// A routing control ARN. ARC routing controls are global, which is why the execution
/// block files them under the Region they are configured for.
fn routing_control_arn(suffix: &str) -> String {
    format!(
        "arn:aws:route53-recovery-control::{ACCOUNT}:controlpanel/cp1/routingcontrol/{suffix}"
    )
}

/// One step carrying `config` as its execution block.
fn step(name: &str, block_type: &str, config: Value) -> Value {
    json!({
        "name": name,
        "executionBlockType": block_type,
        "executionBlockConfiguration": config,
    })
}

/// One workflow performing `action` with `steps`.
fn workflow_of(action: &str, steps: Vec<Value>) -> Value {
    json!({
        "workflowTargetAction": action,
        "workflowDescription": format!("the {action} workflow"),
        "steps": steps,
    })
}

/// A manual-approval execution block: it waits inside this service and drives nothing
/// regional, so it configures no Region at all.
fn approval_block() -> Value {
    json!({"executionApprovalConfig": {
        "approvalRole": format!("arn:aws:iam::{ACCOUNT}:role/Approver"),
        "timeoutMinutes": 60,
    }})
}

/// A Lambda execution block naming one function per entry of `regions`. Members from
/// `smithy_query(query="shape", shape="CustomActionLambdaConfiguration", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`:
/// `lambdas` (`@length(1..2)`), `retryIntervalMinutes` and `regionToRun` are required.
fn lambda_block(regions: &[&str]) -> Value {
    let lambdas: Vec<Value> = regions
        .iter()
        .map(|region| json!({"arn": lambda_arn(region)}))
        .collect();
    json!({"customActionLambdaConfig": {
        "timeoutMinutes": 60,
        "retryIntervalMinutes": 1.0,
        "regionToRun": "activatingRegion",
        "lambdas": lambdas,
    }})
}

/// An EC2 Auto Scaling execution block naming one group per entry of `regions` (`asgs` is
/// `@length(2..2)`, so a caller names two groups whatever Regions they are in).
fn asg_block(regions: &[&str; 2]) -> Value {
    let asgs: Vec<Value> = regions
        .iter()
        .map(|region| json!({"arn": asg_arn(region)}))
        .collect();
    json!({"ec2AsgCapacityIncreaseConfig": {
        "timeoutMinutes": 60,
        "asgs": asgs,
        "targetPercent": 100,
        "capacityMonitoringApproach": "sampledMaxInLast24Hours",
    }})
}

/// An ECS execution block naming one service and its cluster per entry of `regions`
/// (`services` is `@length(2..2)`).
fn ecs_block(regions: &[&str; 2]) -> Value {
    let services: Vec<Value> = regions
        .iter()
        .map(|region| json!({
            "serviceArn": ecs_service_arn(region),
            "clusterArn": ecs_cluster_arn(region),
        }))
        .collect();
    json!({"ecsCapacityIncreaseConfig": {
        "timeoutMinutes": 60,
        "services": services,
        "targetPercent": 100,
        "capacityMonitoringApproach": "sampledMaxInLast24Hours",
    }})
}

/// An RDS promote-read-replica execution block, whose `dbInstanceArnMap` files one
/// instance under each Region it is configured for.
fn rds_block(regions: &[&str]) -> Value {
    let map: Value = regions
        .iter()
        .map(|region| ((*region).to_string(), json!(rds_instance_arn(region))))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    json!({"rdsPromoteReadReplicaConfig": {
        "timeoutMinutes": 60,
        "dbInstanceArnMap": map,
    }})
}

/// An ARC routing control execution block, whose `regionAndRoutingControls` files the
/// controls to flip under each Region it is configured for.
fn routing_control_block(regions: &[&str]) -> Value {
    let map: Value = regions
        .iter()
        .map(|region| {
            (
                (*region).to_string(),
                json!([{"routingControlArn": routing_control_arn(region), "state": "On"}]),
            )
        })
        .collect::<serde_json::Map<String, Value>>()
        .into();
    json!({"arcRoutingControlConfig": {
        "timeoutMinutes": 60,
        "regionAndRoutingControls": map,
    }})
}

/// An Aurora global database execution block naming one cluster per entry of `regions`.
fn aurora_block(regions: &[&str]) -> Value {
    let arns: Vec<String> = regions.iter().map(|region| aurora_cluster_arn(region)).collect();
    json!({"globalAuroraConfig": {
        "timeoutMinutes": 60,
        "behavior": "switchoverOnly",
        "globalClusterIdentifier": "orders-global",
        "databaseClusterArns": arns,
    }})
}

/// A Route 53 health check execution block whose record sets name `regions`.
fn route53_block(regions: &[&str]) -> Value {
    let record_sets: Vec<Value> = regions
        .iter()
        .map(|region| json!({"recordSetIdentifier": format!("rs-{region}"), "region": region}))
        .collect();
    json!({"route53HealthCheckConfig": {
        "timeoutMinutes": 60,
        "hostedZoneId": "Z123456789",
        "recordName": "www.example.com",
        "recordSets": record_sets,
    }})
}

/// A parallel execution block holding `steps`: each nested step is a step of the workflow
/// and is evaluated as one.
fn parallel_block(steps: Vec<Value>) -> Value {
    json!({"parallelConfig": {"steps": steps}})
}

/// The `(step, Region, resource)` triple one warning reports: the step it names, the
/// Region its message is about, and the resource ARN it carries (`""` when it carries
/// none, which is how a warning about a Region nothing is configured in reads).
type WarningFact = (String, String, String);

/// The triple [`WarningFact`] names, read out of one warning of a response.
fn warning_fact(case: &str, warning: &Value) -> WarningFact {
    let message = warning["warningMessage"].as_str().unwrap_or_default();
    let regions: Vec<&str> = REGION_CATALOG
        .iter()
        .copied()
        .filter(|region| {
            message.contains(&format!("no resource in {region}."))
                || message.contains(&format!(" in {region}, which is not"))
        })
        .collect();
    assert_eq!(
        regions.len(),
        1,
        "case {case:?}: expected a warning message about exactly one Region, got {regions:?} \
         from {warning}"
    );
    (
        warning["stepName"].as_str().unwrap_or_default().to_string(),
        regions[0].to_string(),
        warning["resourceArn"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

/// The warnings a response carries, as sorted `(step, Region, resource)` triples.
fn warning_facts(case: &str, body: &Value) -> Vec<WarningFact> {
    let mut facts: Vec<WarningFact> = body["warnings"]
        .as_array()
        .map(|warnings| {
            warnings
                .iter()
                .map(|warning| warning_fact(case, warning))
                .collect()
        })
        .unwrap_or_default();
    facts.sort();
    facts
}

/// Create the plan a case describes and return its ARN.
async fn create(
    router: &smithy_server_runtime::multi_service_router::MultiServiceRouter,
    case: &str,
    name: &str,
    regions: [&str; 2],
    workflows: Value,
) -> String {
    let request = create_request(name, regions, "activePassive", workflows);
    let (status, created) = call(router, "CreatePlan", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreatePlan expected 200, got {status} with {created}"
    );
    created["plan"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One row of the evaluation table: a plan, and the warnings its stored configuration
/// has to raise, as `(step, Region, resource)` triples.
struct Row {
    case: &'static str,
    name: &'static str,
    regions: [&'static str; 2],
    workflows: Value,
    expected: Vec<WarningFact>,
}

/// A warning about one of the plan's Regions the step configures nothing in.
fn unconfigured(step_name: &str, region: &str) -> WarningFact {
    (step_name.to_string(), region.to_string(), String::new())
}

/// A warning about a resource the step configures outside the plan's Regions.
fn outside(step_name: &str, region: &str, arn: String) -> WarningFact {
    (step_name.to_string(), region.to_string(), arn)
}

/// One stage of the corrected-plan case: the workflows to write (`Value::Null` leaves the
/// plan as it stands), and the evaluation the record then reports.
struct Stage {
    case: &'static str,
    workflows: Value,
    state: &'static str,
    expected: Vec<WarningFact>,
}

/// The table: rows differ in the kind of execution block, in how a block says which
/// Region a resource is in (a resource ARN, a Region-keyed map, a record set's own
/// Region), in the Region pair the plan covers, in how many steps and workflows the plan
/// holds, and in whether the configuration is complete, short of a Region, or names a
/// Region the plan does not cover.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "manual approvals only: nothing regional to leave out",
            name: "approvals",
            regions: ["us-east-1", "us-west-2"],
            workflows: default_workflows(),
            expected: vec![],
        },
        Row {
            case: "a Lambda block naming a function in each of the plan's Regions",
            name: "lambda-both",
            regions: ["us-east-1", "us-west-2"],
            workflows: json!([workflow_of(
                "activate",
                vec![step(
                    "invoke",
                    "CustomActionLambda",
                    lambda_block(&["us-east-1", "us-west-2"]),
                )],
            )]),
            expected: vec![],
        },
        Row {
            case: "a Lambda block naming a function in only one of them",
            name: "lambda-one",
            regions: ["us-east-1", "us-west-2"],
            workflows: json!([workflow_of(
                "activate",
                vec![step("invoke", "CustomActionLambda", lambda_block(&["us-east-1"]))],
            )]),
            expected: vec![unconfigured("invoke", "us-west-2")],
        },
        Row {
            case: "two Auto Scaling groups, both in the same Region",
            name: "asgs-same",
            regions: ["eu-west-1", "eu-central-1"],
            workflows: json!([workflow_of(
                "activate",
                vec![step(
                    "scale-up",
                    "EC2AutoScaling",
                    asg_block(&["eu-west-1", "eu-west-1"]),
                )],
            )]),
            expected: vec![unconfigured("scale-up", "eu-central-1")],
        },
        Row {
            case: "a Region-keyed RDS map carrying one of the two Regions",
            name: "rds-one",
            regions: ["us-east-1", "us-west-2"],
            workflows: json!([workflow_of(
                "activate",
                vec![step("promote", "RdsPromoteReadReplica", rds_block(&["us-west-2"]))],
            )]),
            expected: vec![unconfigured("promote", "us-east-1")],
        },
        Row {
            case: "a Region-keyed routing control map carrying both Regions",
            name: "controls-both",
            regions: ["ap-southeast-2", "sa-east-1"],
            workflows: json!([workflow_of(
                "deactivate",
                vec![step(
                    "flip",
                    "ARCRoutingControl",
                    routing_control_block(&["ap-southeast-2", "sa-east-1"]),
                )],
            )]),
            expected: vec![],
        },
        Row {
            case: "Aurora clusters in both Regions, read out of their ARNs",
            name: "aurora-both",
            regions: ["us-east-1", "eu-west-1"],
            workflows: json!([workflow_of(
                "activate",
                vec![step(
                    "switchover",
                    "AuroraGlobalDatabase",
                    aurora_block(&["us-east-1", "eu-west-1"]),
                )],
            )]),
            expected: vec![],
        },
        Row {
            case: "record sets naming only one of the plan's Regions",
            name: "route53-one",
            regions: ["eu-west-1", "eu-central-1"],
            workflows: json!([workflow_of(
                "activate",
                vec![step("health", "Route53HealthCheck", route53_block(&["eu-west-1"]))],
            )]),
            expected: vec![unconfigured("health", "eu-central-1")],
        },
        Row {
            case: "ECS services, one of them in a Region the plan does not cover",
            name: "ecs-outside",
            regions: ["us-east-1", "us-west-2"],
            workflows: json!([workflow_of(
                "activate",
                vec![step(
                    "scale-ecs",
                    "ECSServiceScaling",
                    ecs_block(&["us-east-1", "sa-east-1"]),
                )],
            )]),
            expected: vec![
                unconfigured("scale-ecs", "us-west-2"),
                outside("scale-ecs", "sa-east-1", ecs_cluster_arn("sa-east-1")),
                outside("scale-ecs", "sa-east-1", ecs_service_arn("sa-east-1")),
            ],
        },
        Row {
            case: "a parallel block whose nested steps are evaluated as steps",
            name: "parallel",
            regions: ["us-east-1", "us-west-2"],
            workflows: json!([workflow_of(
                "activate",
                vec![step(
                    "fan-out",
                    "Parallel",
                    parallel_block(vec![
                        step(
                            "nested-complete",
                            "CustomActionLambda",
                            lambda_block(&["us-east-1", "us-west-2"]),
                        ),
                        step(
                            "nested-short",
                            "RdsPromoteReadReplica",
                            rds_block(&["us-east-1"]),
                        ),
                    ]),
                )],
            )]),
            expected: vec![unconfigured("nested-short", "us-west-2")],
        },
        Row {
            case: "two workflows, each holding one complete and one incomplete step",
            name: "two-workflows",
            regions: ["us-east-1", "us-west-2"],
            workflows: json!([
                workflow_of(
                    "activate",
                    vec![
                        step("approve-activate", "ManualApproval", approval_block()),
                        step(
                            "scale-up",
                            "EC2AutoScaling",
                            asg_block(&["us-east-1", "us-east-1"]),
                        ),
                    ],
                ),
                workflow_of(
                    "deactivate",
                    vec![
                        step(
                            "switchover",
                            "AuroraGlobalDatabase",
                            aurora_block(&["us-east-1", "us-west-2"]),
                        ),
                        step("scale-down", "CustomActionLambda", lambda_block(&["us-west-2"])),
                    ],
                ),
            ]),
            expected: vec![
                unconfigured("scale-down", "us-east-1"),
                unconfigured("scale-up", "us-west-2"),
            ],
        },
    ]
}

/// Every row's evaluation reports exactly the warnings its stored configuration raises,
/// and the state those warnings add up to.
#[tokio::test]
async fn evaluation_reports_the_warnings_the_stored_configuration_raises() {
    let router = router();
    for row in rows() {
        let arn = create(&router, row.case, row.name, row.regions, row.workflows.clone()).await;

        let (status, plan) = call(&router, "GetPlan", &json!({"arn": arn})).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetPlan expected 200, got {status} with {plan}",
            row.case
        );
        let version = plan["plan"]["version"].clone();
        let updated_at = plan["plan"]["updatedAt"].clone();

        let (status, body) = call(
            &router,
            "GetPlanEvaluationStatus",
            &json!({"planArn": arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} with {body}",
            row.case
        );
        assert_eq!(
            body["planArn"],
            json!(arn),
            "case {:?}: expected the plan ARN {arn}, got {body}",
            row.case
        );
        assert_eq!(
            body["lastEvaluatedVersion"], version,
            "case {:?}: expected the stored plan's version {version}, got {}",
            row.case, body["lastEvaluatedVersion"]
        );
        assert_eq!(
            body["lastEvaluationTime"], updated_at,
            "case {:?}: expected the instant the evaluated version was written \
             {updated_at}, got {}",
            row.case, body["lastEvaluationTime"]
        );
        assert_eq!(
            body["region"],
            json!("us-east-1"),
            "case {:?}: expected the Region the request reached, got {}",
            row.case, body["region"]
        );

        let mut expected = row.expected.clone();
        expected.sort();
        assert_eq!(
            warning_facts(row.case, &body),
            expected,
            "case {:?}: expected the warnings {expected:?}, got {}",
            row.case,
            body["warnings"]
        );

        let expected_state = if expected.is_empty() {
            EVALUATION_PASSED
        } else {
            EVALUATION_ACTION_REQUIRED
        };
        assert_eq!(
            body["evaluationState"],
            json!(expected_state),
            "case {:?}: expected the evaluation state {expected_state}, got {}",
            row.case, body["evaluationState"]
        );

        for warning in body["warnings"].as_array().unwrap_or(&Vec::new()) {
            assert_eq!(
                warning["warningStatus"],
                json!(WARNING_STATUS_ACTIVE),
                "case {:?}: expected an active warning, got {warning}",
                row.case
            );
            assert_eq!(
                warning["version"], version,
                "case {:?}: expected the warning to carry the evaluated version \
                 {version}, got {warning}",
                row.case
            );
            assert_eq!(
                warning["warningUpdatedTime"], updated_at,
                "case {:?}: expected the warning to be stamped with the instant the \
                 evaluated version was written {updated_at}, got {warning}",
                row.case
            );
            assert!(
                warning["workflow"]["action"].is_string(),
                "case {:?}: expected the warning to name the workflow's action, got {warning}",
                row.case
            );
        }
    }
}

/// The warnings page: every page size reports the same evaluation state and the same
/// warnings in the same order, and paging stops when the token is absent.
#[tokio::test]
async fn evaluation_warnings_paginate() {
    let router = router();
    // Four warnings: two steps short of a Region in each of two workflows.
    let arn = create(
        &router,
        "a plan with four warnings",
        "paged",
        ["us-east-1", "us-west-2"],
        json!([
            workflow_of(
                "activate",
                vec![
                    step("a1", "CustomActionLambda", lambda_block(&["us-east-1"])),
                    step("a2", "RdsPromoteReadReplica", rds_block(&["us-east-1"])),
                ],
            ),
            workflow_of(
                "deactivate",
                vec![
                    step("d1", "CustomActionLambda", lambda_block(&["us-west-2"])),
                    step("d2", "RdsPromoteReadReplica", rds_block(&["us-west-2"])),
                ],
            ),
        ]),
    )
    .await;

    let (status, whole) =
        call(&router, "GetPlanEvaluationStatus", &json!({"planArn": arn})).await;
    assert_eq!(
        status, 200,
        "the unpaged read expected 200, got {status} with {whole}"
    );
    let all = whole["warnings"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(all.len(), 4, "expected 4 warnings, got {}", whole["warnings"]);
    assert!(
        whole["nextToken"].is_null(),
        "expected no token when every warning fits on the page, got {whole}"
    );

    for page_size in [1usize, 2, 3, 4, 5] {
        let case = format!("maxResults {page_size}");
        let mut walked: Vec<Value> = Vec::new();
        let mut token = Value::Null;
        let mut pages = 0usize;
        loop {
            let mut request = json!({"planArn": arn, "maxResults": page_size});
            if let Some(token) = token.as_str() {
                request["nextToken"] = json!(token);
            }
            let (status, body) = call(&router, "GetPlanEvaluationStatus", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {body}"
            );
            assert_eq!(
                body["evaluationState"], whole["evaluationState"],
                "case {case:?}: expected every page to report the same state {}, got {}",
                whole["evaluationState"], body["evaluationState"]
            );
            let page = body["warnings"].as_array().cloned().unwrap_or_default();
            assert!(
                !page.is_empty() && page.len() <= page_size,
                "case {case:?}: expected 1..={page_size} warnings on a page, got {}",
                body["warnings"]
            );
            walked.extend(page);
            pages += 1;
            token = body["nextToken"].clone();
            if token.is_null() {
                break;
            }
            assert!(
                pages <= all.len(),
                "case {case:?}: paging did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            walked, all,
            "case {case:?}: expected paging to yield exactly the {} warnings in order, got {:?}",
            all.len(),
            walked
        );
        assert_eq!(
            pages,
            all.len().div_ceil(page_size),
            "case {case:?}: expected {} pages, got {pages}",
            all.len().div_ceil(page_size)
        );
    }
}

/// The refusals: an ARN no plan is stored under is the modelled 404, and a token this
/// service never issued is a 400.
#[tokio::test]
async fn evaluation_refuses_an_unknown_plan_and_an_unissued_token() {
    let router = router();
    let arn = create(
        &router,
        "a stored plan",
        "refusals",
        ["us-east-1", "us-west-2"],
        json!([workflow_of(
            "activate",
            vec![step("invoke", "CustomActionLambda", lambda_block(&["us-east-1"]))],
        )]),
    )
    .await;

    // Each row satisfies `PlanArn`'s pattern — the boundary validator refuses anything
    // else before the handler runs — and names no stored plan.
    let absent: Vec<(&str, String)> = vec![
        (
            "a name no plan was created with",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/absent:abc123"),
        ),
        (
            "the stored plan's name with another suffix",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/refusals:zzzzzz"),
        ),
        (
            "another account's plan",
            "arn:aws:arc-region-switch::210987654321:plan/refusals:abc123".to_string(),
        ),
    ];
    for (case, candidate) in absent {
        assert_ne!(
            candidate, arn,
            "case {case:?}: the case must not name the stored plan"
        );
        let (status, body) = call(
            &router,
            "GetPlanEvaluationStatus",
            &json!({"planArn": candidate}),
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

    // The plan raises exactly one warning, so no token resuming at 1 or beyond was ever
    // issued for it, and nothing this service issues is non-numeric.
    let tokens: &[(&str, &str)] = &[
        ("a token that is not a position", "not-a-token"),
        ("a position at the end of the list", "1"),
        ("a position past the end of the list", "7"),
    ];
    for (case, token) in tokens {
        let (status, body) = call(
            &router,
            "GetPlanEvaluationStatus",
            &json!({"planArn": arn, "nextToken": token}),
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
}

/// The evaluation follows the record: correcting the configuration through `UpdatePlan`
/// clears the warnings and moves the state to `passed`, and reading the evaluation twice
/// reports the same thing both times.
#[tokio::test]
async fn evaluation_follows_the_stored_plan_and_changes_nothing() {
    let router = router();
    let arn = create(
        &router,
        "a plan short of a Region",
        "corrected",
        ["us-east-1", "us-west-2"],
        json!([workflow_of(
            "activate",
            vec![step("promote", "RdsPromoteReadReplica", rds_block(&["us-east-1"]))],
        )]),
    )
    .await;

    // Rows differ in the configuration the plan holds, so each one is a different
    // evaluation of the same plan record.
    let stages: Vec<Stage> = vec![
        Stage {
            case: "the plan as created, short of a Region",
            workflows: Value::Null,
            state: EVALUATION_ACTION_REQUIRED,
            expected: vec![unconfigured("promote", "us-west-2")],
        },
        Stage {
            case: "the missing Region added",
            workflows: json!([workflow_of(
                "activate",
                vec![step(
                    "promote",
                    "RdsPromoteReadReplica",
                    rds_block(&["us-east-1", "us-west-2"]),
                )],
            )]),
            state: EVALUATION_PASSED,
            expected: vec![],
        },
        Stage {
            case: "a second step, naming a Region the plan does not cover",
            workflows: json!([workflow_of(
                "activate",
                vec![
                    step(
                        "promote",
                        "RdsPromoteReadReplica",
                        rds_block(&["us-east-1", "us-west-2"]),
                    ),
                    step("invoke", "CustomActionLambda", lambda_block(&["eu-west-1"])),
                ],
            )]),
            state: EVALUATION_ACTION_REQUIRED,
            expected: vec![
                unconfigured("invoke", "us-east-1"),
                unconfigured("invoke", "us-west-2"),
                outside("invoke", "eu-west-1", lambda_arn("eu-west-1")),
            ],
        },
    ];

    for Stage {
        case,
        workflows,
        state: expected_state,
        expected,
    } in stages
    {
        if !workflows.is_null() {
            let update = json!({
                "arn": arn,
                "executionRole": format!("arn:aws:iam::{ACCOUNT}:role/ArcRegionSwitchRole"),
                "workflows": workflows,
            });
            let (status, updated) = call(&router, "UpdatePlan", &update).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdatePlan expected 200, got {status} with {updated}"
            );
        }

        let (status, plan) = call(&router, "GetPlan", &json!({"arn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPlan expected 200, got {status} with {plan}"
        );
        let version = plan["plan"]["version"].clone();

        let mut expected = expected;
        expected.sort();
        let mut bodies = Vec::new();
        // Read twice: a report must not change what a later caller sees.
        for attempt in 1..=2 {
            let (status, body) =
                call(&router, "GetPlanEvaluationStatus", &json!({"planArn": arn})).await;
            assert_eq!(
                status, 200,
                "case {case:?} (read {attempt}): expected 200, got {status} with {body}"
            );
            assert_eq!(
                body["evaluationState"],
                json!(expected_state),
                "case {case:?} (read {attempt}): expected {expected_state}, got {}",
                body["evaluationState"]
            );
            assert_eq!(
                body["lastEvaluatedVersion"], version,
                "case {case:?} (read {attempt}): expected the stored version {version}, got {}",
                body["lastEvaluatedVersion"]
            );
            assert_eq!(
                warning_facts(case, &body),
                expected,
                "case {case:?} (read {attempt}): expected the warnings {expected:?}, got {}",
                body["warnings"]
            );
            bodies.push(body);
        }
        assert_eq!(
            bodies[0], bodies[1],
            "case {case:?}: expected two reads to report the same evaluation, got {:?} then {:?}",
            bodies[0], bodies[1]
        );
    }
}
