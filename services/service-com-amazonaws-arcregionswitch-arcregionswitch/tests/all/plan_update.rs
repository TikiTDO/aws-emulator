// SPDX-License-Identifier: CC0-1.0

//! `UpdatePlan` over the wire.
//!
//! The rule under test: an accepted `UpdatePlan` writes the plan's mutable configuration
//! to exactly what the request carries — every member of `UpdatePlanRequest` other than
//! `arn`, each `@cfnMutability: full`, so an omitted optional member leaves the plan
//! without that configuration — leaves every member the request does not carry as it
//! was, and makes a new server-owned `version` and `updatedAt`; an ARN no plan is stored
//! under is refused with the modelled `ResourceNotFoundException` (HTTP 404), carrying the
//! service's own plan-not-found message built from the ARN the request named, and writes
//! nothing.
//!
//! The request/response helpers are shared with [`crate::plan_lifecycle`]: both suites
//! drive the emulator through the same `MultiServiceRouter::dispatch` the router serves
//! on a socket (`awsJson1_0`, `POST /`, `X-Amz-Target: ArcRegionSwitch.<Operation>`).

use serde_json::{json, Value};

use crate::plan_lifecycle::{
    call, create_plan, default_workflows, error_code, error_message, router, workflow, ACCOUNT,
};

/// The `Plan` members `UpdatePlan` writes: every member of `UpdatePlanRequest` other than
/// `arn`, from
/// `smithy_query(query="operation", operation="UpdatePlan", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`.
const MUTABLE_MEMBERS: &[&str] = &[
    "description",
    "workflows",
    "executionRole",
    "recoveryTimeObjectiveMinutes",
    "associatedAlarms",
    "triggers",
    "reportConfiguration",
    "serviceQuotaChecksEnabled",
];

/// The `Plan` members `UpdatePlanRequest` does NOT carry (the plan's identity, topology
/// and owner), so an update must leave each of them as it was.
const IMMUTABLE_MEMBERS: &[&str] = &[
    "arn",
    "name",
    "regions",
    "recoveryApproach",
    "primaryRegion",
    "owner",
];

/// An IAM role ARN as `IamRoleArn`'s pattern requires it.
fn role(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// The plan's `version`, as the server-owned revision string the model reports.
fn version_of(case: &str, plan: &Value) -> u64 {
    let raw = plan["version"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a version string, got {plan}"));
    raw.parse::<u64>()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a numeric version, got {raw:?}"))
}

/// The plan's `updatedAt`, as the epoch timestamp the protocol reports.
fn updated_at_of(case: &str, plan: &Value) -> f64 {
    plan["updatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt timestamp, got {plan}"))
}

/// An accepted update writes every mutable member to what the request carries (clearing
/// the ones it omits), leaves the plan's identity alone, makes a new version, and is what
/// `GetPlan` reads back.
#[tokio::test]
async fn update_plan_replaces_the_mutable_configuration_and_versions_the_plan() {
    let router = router();
    let arn = create_plan(&router, "updatable", ["us-east-1", "us-west-2"]).await;
    let (status, created) = call(&router, "GetPlan", &json!({"arn": arn})).await;
    assert_eq!(
        status, 200,
        "the plan under test must read back: got {status} with {created}"
    );
    let created = created["plan"].clone();

    // The rows differ along what the rule generalises over: which optional members the
    // update carries, the number of workflows, steps, triggers and alarms, the value of
    // the recovery time objective (both published bounds of its `@range(1..10080)`), the
    // boolean's value, and whether a collection is empty. They are applied in order to
    // ONE plan, so each row also observes what the row before it had stored.
    let updates: Vec<(&str, Value)> = vec![
        (
            "required members only: every optional member cleared",
            json!({
                "executionRole": role("FirstUpdatedRole"),
                "workflows": [workflow("activate", &["only-step"])],
            }),
        ),
        (
            "every optional member, recovery time objective at its low bound",
            json!({
                "executionRole": role("SecondUpdatedRole"),
                "workflows": default_workflows(),
                "description": "the plan after its second update",
                "recoveryTimeObjectiveMinutes": 1,
                "serviceQuotaChecksEnabled": false,
                "associatedAlarms": {
                    "app-health": {
                        "resourceIdentifier":
                            format!("arn:aws:cloudwatch:us-east-1:{ACCOUNT}:alarm:app"),
                        "alarmType": "applicationHealth",
                    },
                    "routing-health": {
                        "resourceIdentifier":
                            format!("arn:aws:cloudwatch:us-west-2:{ACCOUNT}:alarm:routing"),
                        "alarmType": "applicationHealth",
                    },
                },
                "triggers": [{
                    "targetRegion": "us-west-2",
                    "action": "activate",
                    "minDelayMinutesBetweenExecutions": 5,
                    "conditions": [{"associatedAlarmName": "app-health", "condition": "red"}],
                }],
                "reportConfiguration": {
                    "reportOutput": [{"s3Configuration": {"bucketPath": "s3://reports/arc"}}]
                },
            }),
        ),
        (
            "three workflows, recovery time objective at its high bound",
            json!({
                "executionRole": role("ThirdUpdatedRole"),
                "workflows": [
                    workflow("activate", &["a1", "a2", "a3"]),
                    workflow("deactivate", &["d1"]),
                    workflow("postRecovery", &["p1", "p2"]),
                ],
                "description": "",
                "recoveryTimeObjectiveMinutes": 10080,
                "serviceQuotaChecksEnabled": true,
            }),
        ),
        (
            "the description alone, so the previous row's alarms and triggers are cleared",
            json!({
                "executionRole": role("FourthUpdatedRole"),
                "workflows": [workflow("deactivate", &["only-step"])],
                "description": "the plan after its fourth update",
            }),
        ),
    ];

    let mut previous_version = version_of("the created plan", &created);
    let mut previous_updated_at = updated_at_of("the created plan", &created);
    for (case, update) in updates {
        let mut request = update.clone();
        request["arn"] = json!(arn);
        let (status, body) = call(&router, "UpdatePlan", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdatePlan expected 200, got {status} with {body}"
        );
        let plan = body["plan"].clone();

        // Every mutable member is exactly what this request carried; one the request
        // omitted is absent (`Value::Null` on both sides of the comparison).
        for member in MUTABLE_MEMBERS {
            assert_eq!(
                &plan[member], &update[member],
                "case {case:?}: member {member} expected {}, got {}",
                update[member], plan[member]
            );
        }

        // Nothing the request does not carry moved.
        for member in IMMUTABLE_MEMBERS {
            assert_eq!(
                &plan[member], &created[member],
                "case {case:?}: member {member} expected the created plan's {}, got {}",
                created[member], plan[member]
            );
        }

        // The write is a new revision of the plan, at the instant of the write.
        let version = version_of(case, &plan);
        assert_eq!(
            version,
            previous_version + 1,
            "case {case:?}: expected the version after {previous_version}, got {version}"
        );
        let updated_at = updated_at_of(case, &plan);
        assert!(
            updated_at >= previous_updated_at,
            "case {case:?}: expected updatedAt at or after {previous_updated_at}, got {updated_at}"
        );
        previous_version = version;
        previous_updated_at = updated_at;

        // Round trip: the update is what a later reader sees, not just what it answered.
        let (status, fetched) = call(&router, "GetPlan", &json!({"arn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPlan expected 200, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["plan"], plan,
            "case {case:?}: GetPlan expected the updated plan {plan}, got {}",
            fetched["plan"]
        );
    }
}

/// `UpdatePlan` for an ARN no plan is stored under is the modelled 404, and the plans the
/// store does hold are untouched by the refusal.
#[tokio::test]
async fn update_plan_for_an_unknown_arn_is_resource_not_found_and_writes_nothing() {
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
        (
            "another account's plan",
            "arn:aws:arc-region-switch::123456789012:plan/present:aaaaaa".to_string(),
        ),
        (
            "another partition",
            format!("arn:aws-us-gov:arc-region-switch::{ACCOUNT}:plan/present:aaaaaa"),
        ),
    ];
    for (case, arn) in cases {
        assert_ne!(
            arn, stored,
            "case {case:?}: the case must not name the stored plan"
        );
        let request = json!({
            "arn": arn,
            "executionRole": role("UpdatedRole"),
            "workflows": default_workflows(),
            "description": "an update of a plan that is not there",
        });
        let (status, body) = call(&router, "UpdatePlan", &request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        // The service's single plan-not-found sentence, built from the ARN the REQUEST
        // carried: every operation that resolves a plan by ARN words it the same way.
        assert_eq!(
            error_message(&body),
            format!("Plan not found for ARN: {arn}"),
            "case {case:?}: expected the request-derived not-found message, got {body}"
        );

        // The refusal wrote nothing: the plan the store holds is still at its creation
        // revision, with the configuration it was created with.
        let (status, fetched) = call(&router, "GetPlan", &json!({"arn": stored})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPlan expected 200, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["plan"]["version"],
            json!("1"),
            "case {case:?}: expected the stored plan still at version 1, got {}",
            fetched["plan"]["version"]
        );
        assert_eq!(
            fetched["plan"]["description"],
            Value::Null,
            "case {case:?}: expected the stored plan to carry no description, got {}",
            fetched["plan"]["description"]
        );
    }
}

/// `recoveryTimeObjectiveMinutes` is `@range(min: 1, max: 10080)` on `UpdatePlanRequest`
/// (`smithy_query(query="operation", operation="UpdatePlan", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`),
/// so both published bounds are accepted and a value one past either bound is refused
/// without writing a new version of the plan.
#[tokio::test]
async fn update_plan_holds_the_recovery_time_objective_to_its_modelled_range() {
    let cases: [(&str, i64, u16); 6] = [
        ("one below the low bound", 0, 400),
        ("the low bound", 1, 200),
        ("one above the low bound", 2, 200),
        ("one below the high bound", 10_079, 200),
        ("the high bound", 10_080, 200),
        ("one above the high bound", 10_081, 400),
    ];
    for (case, minutes, expected_status) in cases {
        let router = router();
        let arn = create_plan(&router, "bounded", ["us-east-1", "us-west-2"]).await;
        let request = json!({
            "arn": arn,
            "executionRole": role("UpdatedRole"),
            "workflows": default_workflows(),
            "recoveryTimeObjectiveMinutes": minutes,
        });
        let (status, body) = call(&router, "UpdatePlan", &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: recoveryTimeObjectiveMinutes {minutes} expected {expected_status}, \
             got {status} with {body}"
        );
        let (_, fetched) = call(&router, "GetPlan", &json!({"arn": arn})).await;
        if expected_status == 200 {
            assert_eq!(
                fetched["plan"]["recoveryTimeObjectiveMinutes"],
                json!(minutes),
                "case {case:?}: expected the stored plan at {minutes} minutes, got {}",
                fetched["plan"]["recoveryTimeObjectiveMinutes"]
            );
            assert_eq!(
                fetched["plan"]["version"],
                json!("2"),
                "case {case:?}: expected the accepted update at version 2, got {}",
                fetched["plan"]["version"]
            );
        } else {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            assert_eq!(
                fetched["plan"]["version"],
                json!("1"),
                "case {case:?}: expected the refused update to leave version 1, got {}",
                fetched["plan"]["version"]
            );
        }
    }
}
