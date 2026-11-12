// SPDX-License-Identifier: CC0-1.0

//! The quotas, the blue/green waits, the reserved host-name form and the
//! refusal sentences this service states, driven over the wire.
//!
//! RULES under test, each over a SET of inputs rather than one recorded request:
//!
//! * the request-size quota of EVERY `BatchGet*` reader is the one its shared
//!   `BatchLimitExceededException` states (100 names or ids), not the number of
//!   records the operation's prose says a call returns;
//! * a DEPLOYMENT's `overrideAlarmConfiguration` is held to the deployment alarm
//!   quota (50), which is not the deployment-group quota (10) the group's own
//!   `alarmConfiguration` is held to;
//! * `ContinueDeployment` refuses by the deployment's TYPE before anything about
//!   its progress, and continues the termination wait of a blue/green deployment
//!   that has one;
//! * a `targetId` that names no target of a deployment is refused as a target
//!   that does not exist, decided from the snapshot its group resolved and so
//!   before anything about the deployment's progress;
//! * the Amazon EC2 instance-id form is RESERVED for every on-premises
//!   `instanceName`, and a registration's ARN names the REGISTRATION;
//! * a refused request states the sentence the error owns, naming the value the
//!   request carried where the sentence is built from it.
//!
//! Every case goes through the same front door a client uses — the shared
//! [`Emulator`] harness of `codedeploy_lifecycle`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{
    deployment_on_free_group, error_code, s3_revision, seeded_group, Emulator,
};

/// The request-size quota every `BatchGet*` reader of this service enforces, as
/// `BatchLimitExceededException` states it ("The maximum number of names or IDs
/// allowed for this request (100) was exceeded"). A second copy of the
/// implementation's constant on purpose: this suite asserts the published
/// quota, not whatever the handler happens to hold.
const BATCH_QUOTA: usize = 100;

/// The sentence a `BatchLimitExceededException` carries, which states the quota
/// a batch has to fit in.
const BATCH_QUOTA_SENTENCE: &str = "BatchGet size limit is 100";

/// How many alarms one DEPLOYMENT may override its group's configuration with,
/// and how many a DEPLOYMENT GROUP's own configuration may carry. The group
/// figure is the model's (`AlarmConfiguration$alarms`: "A maximum of 10 alarms
/// can be added", and `AlarmsLimitExceededException`: "The maximum number of
/// alarms for a deployment group (10) was exceeded"); the deployment figure is
/// the one the deployment-level refusal states.
const DEPLOYMENT_ALARM_QUOTA: usize = 50;
/// See [`DEPLOYMENT_ALARM_QUOTA`].
const DEPLOYMENT_GROUP_ALARM_QUOTA: usize = 10;

/// The IAM role every deployment group here is created with.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// The tag an on-premises instance carries to be selected by the groups here.
const FLEET_TAG: (&str, &str) = ("Fleet", "deployable");

/// The error message of a failure body.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or("<no message>")
        .to_string()
}

/// An IAM user ARN for `name`.
fn user_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:user/{name}", engine::emulator_account_id())
}

/// Register `name` as an on-premises instance carrying [`FLEET_TAG`], so a
/// deployment group selecting that tag has it as a target.
async fn register_host(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            "RegisterOnPremisesInstance",
            json!({"instanceName": name, "iamUserArn": user_arn(name)}),
        )
        .await;
    emulator
        .ok(
            "AddTagsToOnPremisesInstances",
            json!({
                "instanceNames": [name],
                "tags": [{"Key": FLEET_TAG.0, "Value": FLEET_TAG.1}],
            }),
        )
        .await;
}

/// Create an application and a deployment group that selects [`FLEET_TAG`],
/// answering the group name. `style` and `blue_green` are added only when they
/// are not null.
async fn fleet_group(
    emulator: &Emulator,
    application: &str,
    style: Value,
    blue_green: Value,
) -> String {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": "Server"}),
        )
        .await;
    let group = format!("{application}-dg");
    let mut request = json!({
        "applicationName": application,
        "deploymentGroupName": group,
        "serviceRoleArn": SERVICE_ROLE,
        "onPremisesInstanceTagFilters": [
            {"Key": FLEET_TAG.0, "Value": FLEET_TAG.1, "Type": "KEY_AND_VALUE"},
        ],
    });
    if !style.is_null() {
        request["deploymentStyle"] = style;
    }
    if !blue_green.is_null() {
        request["blueGreenDeploymentConfiguration"] = blue_green;
    }
    emulator.ok("CreateDeploymentGroup", request).await;
    group
}

/// Create a deployment of `group`, answering its id.
async fn deploy(emulator: &Emulator, application: &str, group: &str) -> String {
    emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "revision": s3_revision("artifacts", "app.zip"),
            }),
        )
        .await["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Poll `GetDeployment` until `ready` accepts the stored `deploymentInfo`. The
/// transitions are owed by the CLOCK, so this waits for them rather than asking
/// for them.
async fn poll_deployment(
    emulator: &Emulator,
    case: &str,
    deployment_id: &str,
    ready: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let info = emulator
            .ok("GetDeployment", json!({"deploymentId": deployment_id}))
            .await["deploymentInfo"]
            .clone();
        if ready(&info) {
            return info;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the deployment to reach the awaited state, got {info:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The batch request-size quota
// ---------------------------------------------------------------------------

/// RULE: every `BatchGet*` reader accepts a batch of up to the 100 names or ids
/// its `BatchLimitExceededException` states and refuses one past it with that
/// code and the sentence naming the quota — whatever smaller figure the
/// operation's own prose carries for how many records a call returns.
#[tokio::test]
async fn every_batch_reader_enforces_the_shared_request_size_quota() {
    let emulator = Emulator::new();
    let application = "batched";
    let group = seeded_group(&emulator, application, "Server").await;
    let deployment = deploy(&emulator, application, &group).await;
    let revision = s3_revision("artifacts", "batched.zip");
    emulator
        .ok(
            "RegisterApplicationRevision",
            json!({"applicationName": application, "revision": revision}),
        )
        .await;
    emulator
        .ok(
            "RegisterOnPremisesInstance",
            json!({"instanceName": "batched-host", "iamUserArn": user_arn("batched-host")}),
        )
        .await;

    // One row per batch reader of this service. Each builds a batch of `size`
    // entries that all exist, so nothing but the SIZE can refuse it.
    struct Reader<'a> {
        operation: &'a str,
        request: Box<dyn Fn(usize) -> Value + 'a>,
    }
    let readers: Vec<Reader> = vec![
        Reader {
            operation: "BatchGetDeployments",
            request: Box::new(|size| json!({"deploymentIds": vec![deployment.clone(); size]})),
        },
        Reader {
            operation: "BatchGetOnPremisesInstances",
            request: Box::new(|size| json!({"instanceNames": vec!["batched-host"; size]})),
        },
        Reader {
            operation: "BatchGetApplications",
            request: Box::new(|size| json!({"applicationNames": vec![application; size]})),
        },
        Reader {
            operation: "BatchGetApplicationRevisions",
            request: Box::new(|size| {
                json!({
                    "applicationName": application,
                    "revisions": vec![revision.clone(); size],
                })
            }),
        },
    ];

    // Sizes differ along the quota: one entry, the smaller figure an operation's
    // own prose names (25), one past that, the quota itself, and past it.
    let accepted: &[usize] = &[1, 25, 26, BATCH_QUOTA];
    let refused: &[usize] = &[BATCH_QUOTA + 1, BATCH_QUOTA * 2];

    for reader in &readers {
        for size in accepted {
            let (status, body) = emulator
                .call(reader.operation, (reader.request)(*size))
                .await;
            assert_eq!(
                status,
                200,
                "case {} with {size} entries: expected the batch to be accepted, got {status} \
                 {body:?}",
                reader.operation
            );
        }
        for size in refused {
            let (status, body) = emulator
                .call(reader.operation, (reader.request)(*size))
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "BatchLimitExceededException"),
                "case {} with {size} entries: expected the batch quota to refuse it, got \
                 {status} {body:?}",
                reader.operation
            );
            assert_eq!(
                error_message(&body),
                BATCH_QUOTA_SENTENCE,
                "case {} with {size} entries: expected the sentence naming the quota, got \
                 {body:?}",
                reader.operation
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The alarm quotas
// ---------------------------------------------------------------------------

/// `count` distinct alarms, as an enabled `AlarmConfiguration`; `count` of zero
/// is a DISABLED configuration, which is how alarm monitoring is turned off.
fn alarms(count: usize) -> Value {
    let names: Vec<Value> = (0..count)
        .map(|index| json!({"name": format!("alarm-{index:03}")}))
        .collect();
    json!({"enabled": count > 0, "alarms": names})
}

/// RULE: a deployment's `overrideAlarmConfiguration` is held to the DEPLOYMENT
/// alarm quota (50) and a NEW deployment group's own `alarmConfiguration` to the
/// deployment-group quota (10) the model states for it — past either, the
/// request is refused with `AlarmsLimitExceededException` stating the quota it
/// exceeded — while an UPDATE of an existing group's configuration is admitted
/// whatever its length.
#[tokio::test]
async fn a_deployment_overrides_alarms_under_its_own_quota() {
    let emulator = Emulator::new();
    let application = "alarmed";
    let group = seeded_group(&emulator, application, "Server").await;

    // Rows differ along how many alarms the override carries: none (monitoring
    // off), one, the deployment-group quota, one past it — which this operation
    // ACCEPTS — the deployment quota itself, and past it.
    let cases: &[(usize, bool)] = &[
        (0, true),
        (1, true),
        (DEPLOYMENT_GROUP_ALARM_QUOTA, true),
        (DEPLOYMENT_GROUP_ALARM_QUOTA + 1, true),
        (DEPLOYMENT_ALARM_QUOTA, true),
        (DEPLOYMENT_ALARM_QUOTA + 1, false),
        (DEPLOYMENT_ALARM_QUOTA * 2, false),
    ];

    for (count, accepted) in cases {
        let request = json!({
            "applicationName": application,
            "deploymentGroupName": group,
            "revision": s3_revision("artifacts", "app.zip"),
            "overrideAlarmConfiguration": alarms(*count),
        });
        if *accepted {
            // A deployment group runs one deployment at a time, so each accepted
            // row waits for the row before it exactly as a client would.
            let body = deployment_on_free_group(&emulator, request).await;
            // The override is persisted as sent, so a read answers it back.
            let deployment_id = body["deploymentId"].as_str().unwrap_or_default();
            let info = emulator
                .ok("GetDeployment", json!({"deploymentId": deployment_id}))
                .await["deploymentInfo"]
                .clone();
            let reported = info["overrideAlarmConfiguration"]["alarms"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default();
            assert_eq!(
                reported, *count,
                "case {count} overridden alarms: expected the override to read back, got {info:?}"
            );
            continue;
        }
        let (status, body) = emulator.call("CreateDeployment", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AlarmsLimitExceededException"),
            "case {count} overridden alarms: expected the deployment alarm quota to refuse it, \
             got {status} {body:?}"
        );
        assert_eq!(
            error_message(&body),
            format!("Deployments cannot contain more than {DEPLOYMENT_ALARM_QUOTA} alarms."),
            "case {count} overridden alarms: expected the sentence naming the deployment quota, \
             got {body:?}"
        );
    }

    // The DEPLOYMENT GROUP's own configuration AT the model's figure is accepted
    // by both operations that set one.
    for (case, operation, request) in [
        (
            "CreateDeploymentGroup at the group quota",
            "CreateDeploymentGroup",
            json!({
                "applicationName": application,
                "deploymentGroupName": "group-at-quota",
                "serviceRoleArn": SERVICE_ROLE,
                "alarmConfiguration": alarms(DEPLOYMENT_GROUP_ALARM_QUOTA),
            }),
        ),
        (
            "UpdateDeploymentGroup at the group quota",
            "UpdateDeploymentGroup",
            json!({
                "applicationName": application,
                "currentDeploymentGroupName": group,
                "alarmConfiguration": alarms(DEPLOYMENT_GROUP_ALARM_QUOTA),
            }),
        ),
    ] {
        let (status, body) = emulator.call(operation, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the group quota to accept it, got {status} {body:?}"
        );
    }
    // Past it, a NEW group is refused with the sentence naming the quota.
    let case = "CreateDeploymentGroup past the group quota";
    let (status, body) = emulator
        .call(
            "CreateDeploymentGroup",
            json!({
                "applicationName": application,
                "deploymentGroupName": "group-past-quota",
                "serviceRoleArn": SERVICE_ROLE,
                "alarmConfiguration": alarms(DEPLOYMENT_GROUP_ALARM_QUOTA + 1),
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "AlarmsLimitExceededException"),
        "case {case:?}: expected the group alarm quota to refuse it, got {status} {body:?}"
    );
    assert!(
        error_message(&body).contains(&format!("({DEPLOYMENT_GROUP_ALARM_QUOTA})")),
        "case {case:?}: expected the sentence naming the deployment-group quota, got {body:?}"
    );
    // An UPDATE of an existing group is admitted whatever its length, and stores
    // every entry it names; the count-past-the-figure cases are tabulated in
    // `super::deployment_group_alarm_counts`.
    for count in [DEPLOYMENT_GROUP_ALARM_QUOTA + 1, DEPLOYMENT_ALARM_QUOTA] {
        let (status, body) = emulator
            .call(
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": group,
                    "alarmConfiguration": alarms(count),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {count} alarms on a group update: expected it to be accepted, got {status} \
             {body:?}"
        );
        let stored = emulator
            .ok(
                "GetDeploymentGroup",
                json!({"applicationName": application, "deploymentGroupName": group}),
            )
            .await;
        let reported = stored["deploymentGroupInfo"]["alarmConfiguration"]["alarms"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            reported, count,
            "case {count} alarms on a group update: expected every alarm to be stored, got \
             {stored:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// ContinueDeployment
// ---------------------------------------------------------------------------

/// RULE: `ContinueDeployment` refuses a deployment whose TYPE does not reroute
/// traffic between two environments with
/// `UnsupportedActionForDeploymentTypeException` — before anything about how far
/// that deployment has got, because a deployment's type is fixed when it is
/// created — and answers each of its other preconditions with that
/// precondition's own modelled code.
#[tokio::test]
async fn continue_deployment_refuses_by_type_before_progress() {
    let emulator = Emulator::new();
    register_host(&emulator, "continue-host").await;

    // An in-place deployment (the default style), one a caller STOPPED, and a
    // blue/green one whose group keeps the original instances alive.
    let in_place_group = fleet_group(&emulator, "continue-in-place", Value::Null, Value::Null).await;
    let in_place = deploy(&emulator, "continue-in-place", &in_place_group).await;
    let stopped_group = fleet_group(&emulator, "continue-stopped", Value::Null, Value::Null).await;
    let stopped = deploy(&emulator, "continue-stopped", &stopped_group).await;
    emulator
        .ok(
            "StopDeployment",
            json!({"deploymentId": stopped, "autoRollbackEnabled": false}),
        )
        .await;
    let kept_group = fleet_group(
        &emulator,
        "continue-kept",
        json!({"deploymentType": "BLUE_GREEN", "deploymentOption": "WITH_TRAFFIC_CONTROL"}),
        json!({
            "terminateBlueInstancesOnDeploymentSuccess": {
                "action": "KEEP_ALIVE",
                "terminationWaitTimeInMinutes": 0,
            }
        }),
    )
    .await;
    let kept = deploy(&emulator, "continue-kept", &kept_group).await;

    // Rows differ along the deployment the request names (its type and its
    // progress), along the wait type it asks to continue past, and along the
    // form of the id itself.
    let cases: &[(&str, Value, &str)] = &[
        (
            "an in-place deployment asked to start shifting traffic",
            json!({"deploymentId": in_place, "deploymentWaitType": "READY_WAIT"}),
            "UnsupportedActionForDeploymentTypeException",
        ),
        (
            "an in-place deployment asked to stop waiting to terminate",
            json!({"deploymentId": in_place, "deploymentWaitType": "TERMINATION_WAIT"}),
            "UnsupportedActionForDeploymentTypeException",
        ),
        (
            // The TYPE refusal comes first: a stopped in-place deployment is
            // refused for its type, not for being complete.
            "a stopped in-place deployment",
            json!({"deploymentId": stopped, "deploymentWaitType": "READY_WAIT"}),
            "UnsupportedActionForDeploymentTypeException",
        ),
        (
            "a blue/green deployment that holds no Ready status",
            json!({"deploymentId": kept, "deploymentWaitType": "READY_WAIT"}),
            "DeploymentIsNotInReadyStateException",
        ),
        (
            "a blue/green deployment that terminates nothing",
            json!({"deploymentId": kept, "deploymentWaitType": "TERMINATION_WAIT"}),
            "InvalidDeploymentWaitTypeException",
        ),
        (
            "no wait type at all",
            json!({"deploymentId": kept}),
            "InvalidDeploymentWaitTypeException",
        ),
        (
            "no deployment id at all",
            json!({"deploymentWaitType": "READY_WAIT"}),
            "DeploymentIdRequiredException",
        ),
        (
            "a blank deployment id",
            json!({"deploymentId": "   ", "deploymentWaitType": "READY_WAIT"}),
            "DeploymentIdRequiredException",
        ),
        (
            "a value that is not a deployment id",
            json!({"deploymentId": "not-an-id", "deploymentWaitType": "READY_WAIT"}),
            "InvalidDeploymentIdException",
        ),
        (
            "a well-formed id no deployment has",
            json!({"deploymentId": "d-ABCDEFGHI", "deploymentWaitType": "READY_WAIT"}),
            "DeploymentDoesNotExistException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = emulator.call("ContinueDeployment", request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {case:?}: expected 400 {expected}, got {status} {body:?}"
        );
        if *expected == "UnsupportedActionForDeploymentTypeException" {
            let message = error_message(&body);
            assert!(
                message.contains("IN_PLACE"),
                "case {case:?}: expected the sentence to name the deployment's type, got \
                 {message:?}"
            );
        }
    }

    // Every refusal above left the deployments as they were: the in-place one
    // still reports its own type.
    let info = emulator
        .ok("GetDeployment", json!({"deploymentId": in_place}))
        .await["deploymentInfo"]
        .clone();
    assert_eq!(
        info["deploymentStyle"]["deploymentType"].as_str(),
        Some("IN_PLACE"),
        "case after the refusals: expected the deployment to be untouched, got {info:?}"
    );
}

/// RULE: continuing a blue/green deployment's TERMINATION_WAIT drops the wait it
/// still owes, so the completion the clock owes it moves back to the instant its
/// traffic routing is complete — and once the deployment is complete, there is
/// no wait left to continue past.
#[tokio::test]
async fn continuing_a_termination_wait_completes_the_deployment() {
    let emulator = Emulator::new();
    register_host(&emulator, "shifted-host").await;
    // Rows differ along the wait the group configured: both are far longer than
    // this test's own deadline, so only CONTINUING it can complete the
    // deployment.
    for minutes in [60, 2880] {
        let application = format!("continued-{minutes}");
        let group = fleet_group(
            &emulator,
            &application,
            json!({"deploymentType": "BLUE_GREEN", "deploymentOption": "WITH_TRAFFIC_CONTROL"}),
            json!({
                "terminateBlueInstancesOnDeploymentSuccess": {
                    "action": "TERMINATE",
                    "terminationWaitTimeInMinutes": minutes,
                }
            }),
        )
        .await;
        let deployment = deploy(&emulator, &application, &group).await;
        let case = format!("a {minutes}-minute termination wait");

        // The deployment has to have STARTED before its traffic is shifted: the
        // clock owes that, so this waits for it.
        let started = poll_deployment(&emulator, &case, &deployment, |info| {
            info["startTime"].as_f64().is_some()
        })
        .await;
        assert!(
            !matches!(started["status"].as_str(), Some("Succeeded" | "Failed")),
            "case {case:?}: expected the termination wait to hold the deployment open, got \
             {started:?}"
        );

        emulator
            .ok(
                "ContinueDeployment",
                json!({
                    "deploymentId": deployment,
                    "deploymentWaitType": "TERMINATION_WAIT",
                }),
            )
            .await;

        // The wait is gone, so the clock settles the deployment: a terminal
        // status, read back through the implemented reader.
        let settled = poll_deployment(&emulator, &case, &deployment, |info| {
            matches!(
                info["status"].as_str(),
                Some("Succeeded" | "Failed" | "Stopped")
            )
        })
        .await;
        assert_eq!(
            settled["status"].as_str(),
            Some("Succeeded"),
            "case {case:?}: expected the deployment to complete once the wait was continued, \
             got {settled:?}"
        );

        // Nothing left to continue past.
        let (status, body) = emulator
            .call(
                "ContinueDeployment",
                json!({
                    "deploymentId": deployment,
                    "deploymentWaitType": "TERMINATION_WAIT",
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "DeploymentAlreadyCompletedException"),
            "case {case:?}: expected a completed deployment to be refused, got {status} {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Target membership
// ---------------------------------------------------------------------------

/// RULE: whether a `targetId` names a target of a deployment is decided from the
/// snapshot that deployment's group resolved when it was created, so an id that
/// names none of them is refused with `DeploymentTargetDoesNotExistException`
/// however far the deployment has got — the single-target reader and the batch
/// reader alike — while a target the deployment HAS is reported once the clock
/// has started it.
#[tokio::test]
async fn a_target_id_outside_the_deployment_is_refused_whatever_its_progress() {
    let emulator = Emulator::new();
    register_host(&emulator, "member-host").await;
    // A second registered host the group does NOT select, so "registered" and
    // "a target of this deployment" cannot be confused.
    emulator
        .ok(
            "RegisterOnPremisesInstance",
            json!({"instanceName": "outside-host", "iamUserArn": user_arn("outside-host")}),
        )
        .await;
    let group = fleet_group(&emulator, "membership", Value::Null, Value::Null).await;
    let deployment = deploy(&emulator, "membership", &group).await;

    // Rows differ along the FORM of a well-formed target id that names no target
    // of this deployment: an EC2 instance id, a registered host the group does
    // not select, and a name nothing registered.
    let outside: &[&str] = &["i-0123456789abcdef0", "outside-host", "ghost-host"];

    // Asked straight after the deployment was created — before the clock starts
    // it — and again once it has started: the answer is the same both times.
    for stage in ["before the deployment started", "after it started"] {
        if stage == "after it started" {
            poll_deployment(&emulator, stage, &deployment, |info| {
                info["startTime"].as_f64().is_some()
            })
            .await;
        }
        for target_id in outside {
            for operation in ["GetDeploymentTarget", "BatchGetDeploymentTargets"] {
                let request = if operation == "GetDeploymentTarget" {
                    json!({"deploymentId": deployment, "targetId": target_id})
                } else {
                    json!({"deploymentId": deployment, "targetIds": [target_id]})
                };
                let (status, body) = emulator.call(operation, request).await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "DeploymentTargetDoesNotExistException"),
                    "case {operation} {target_id:?} {stage}: expected the target not to exist, \
                     got {status} {body:?}"
                );
            }
        }
    }

    // The target the deployment DOES have is reported, once the clock has
    // started the deployment.
    let deadline = Instant::now() + Duration::from_secs(20);
    let target = loop {
        let (status, body) = emulator
            .call(
                "GetDeploymentTarget",
                json!({"deploymentId": deployment, "targetId": "member-host"}),
            )
            .await;
        if status == 200 {
            break body["deploymentTarget"].clone();
        }
        assert_eq!(
            error_code(&body),
            "DeploymentTargetDoesNotExistException",
            "case member-host: expected the target to be reported or its record not to exist \
             yet, got {status} {body:?}"
        );
        assert!(
            Instant::now() < deadline,
            "case member-host: expected the deployment to start, got {body:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(
        target["instanceTarget"]["targetId"].as_str(),
        Some("member-host"),
        "case member-host: expected the target of this deployment, got {target:?}"
    );
}

// ---------------------------------------------------------------------------
// The reserved host-name form, and the ARN a registration carries
// ---------------------------------------------------------------------------

/// The sentence `InvalidInstanceNameException` carries for a name in the
/// reserved Amazon EC2 instance-id form.
const RESERVED_NAME_SENTENCE: &str = "Host name cannot start with \"i-\"";

/// One operation taking an on-premises `instanceName`, with the request that
/// hands it `name`.
type NamedRequest = (&'static str, fn(&str) -> Value);

/// RULE: the Amazon EC2 instance-id form (`i-…`) is RESERVED for an on-premises
/// `instanceName`, so every operation that takes one refuses it with
/// `InvalidInstanceNameException` before it asks the registry anything — never
/// as a successful no-op — while a name that merely begins with `i` is an
/// ordinary name.
#[tokio::test]
async fn the_ec2_instance_id_form_is_reserved_for_every_on_premises_operation() {
    let emulator = Emulator::new();

    // Rows differ along the operation taking the name, and along the form of a
    // reserved name: a full EC2 instance id, the short form, the prefix alone,
    // and the prefix in another casing.
    let reserved: &[&str] = &["i-1234567890abcdef0", "i-abc123", "i-", "I-1234567890"];
    let operations: &[NamedRequest] = &[
        ("RegisterOnPremisesInstance", |name| {
            json!({"instanceName": name, "iamUserArn": "arn:aws:iam::000000000000:user/host"})
        }),
        ("DeregisterOnPremisesInstance", |name| {
            json!({"instanceName": name})
        }),
        ("GetOnPremisesInstance", |name| {
            json!({"instanceName": name})
        }),
        ("BatchGetOnPremisesInstances", |name| {
            json!({"instanceNames": [name]})
        }),
        ("AddTagsToOnPremisesInstances", |name| {
            json!({"instanceNames": [name], "tags": [{"Key": "k", "Value": "v"}]})
        }),
        ("RemoveTagsFromOnPremisesInstances", |name| {
            json!({"instanceNames": [name], "tags": [{"Key": "k", "Value": "v"}]})
        }),
    ];

    for (operation, request) in operations {
        for name in reserved {
            let (status, body) = emulator.call(operation, request(name)).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidInstanceNameException"),
                "case {operation} {name:?}: expected the reserved name to be refused, got \
                 {status} {body:?}"
            );
            assert_eq!(
                error_message(&body),
                RESERVED_NAME_SENTENCE,
                "case {operation} {name:?}: expected the sentence naming the reserved prefix, \
                 got {body:?}"
            );
        }
    }

    // A name that merely begins with `i` is not reserved: it registers and reads
    // back.
    for name in ["i", "instance-7", "io-9"] {
        emulator
            .ok(
                "RegisterOnPremisesInstance",
                json!({"instanceName": name, "iamUserArn": user_arn(name)}),
            )
            .await;
        let info = emulator
            .ok("GetOnPremisesInstance", json!({"instanceName": name}))
            .await["instanceInfo"]
            .clone();
        assert_eq!(
            info["instanceName"].as_str(),
            Some(name),
            "case {name:?}: expected an ordinary name to register, got {info:?}"
        );
    }

    // A name outside the service's character set is the OTHER refusal that
    // error carries.
    let (status, body) = emulator
        .call(
            "DeregisterOnPremisesInstance",
            json!({"instanceName": "has a space"}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidInstanceNameException"),
        "case a name carrying a space: expected it to be refused, got {status} {body:?}"
    );
    assert_eq!(
        error_message(&body),
        "Instance name contains invalid characters.",
        "case a name carrying a space: expected the sentence naming the characters, got {body:?}"
    );
}

/// RULE: the `instanceArn` of an on-premises instance names the REGISTRATION —
/// the host name, then the id the service minted for that registration — so
/// every reader answers the ARN the registry minted, no two registrations share
/// one, and registering a name again after deregistering it mints a new one.
#[tokio::test]
async fn an_on_premises_instance_arn_names_its_registration() {
    let emulator = Emulator::new();
    let expected_prefix = |name: &str| {
        format!(
            "arn:aws:codedeploy:us-east-1:{}:instance/{name}_",
            engine::emulator_account_id()
        )
    };
    /// The registration id in `arn`, asserting the ARN is this service's and
    /// names `name`.
    fn registration_of(case: &str, prefix: &str, arn: Option<&str>) -> String {
        let arn = arn.unwrap_or("<no instanceArn>");
        let id = arn.strip_prefix(prefix).unwrap_or_else(|| {
            panic!("case {case:?}: expected an instanceArn starting {prefix:?}, got {arn:?}")
        });
        assert!(
            !id.is_empty(),
            "case {case:?}: expected a registration id after {prefix:?}, got {arn:?}"
        );
        id.to_string()
    }

    // Rows differ along the FORM of the host name, since the name is part of the
    // ARN: plain, punctuated, and the published maximum length.
    let longest = "n".repeat(100);
    let names: &[&str] = &["host-one", "a+b=c,d.e@f_g-h", longest.as_str()];
    let mut registrations: Vec<String> = Vec::new();

    for name in names {
        emulator
            .ok(
                "RegisterOnPremisesInstance",
                json!({"instanceName": name, "iamUserArn": user_arn(name)}),
            )
            .await;
        let prefix = expected_prefix(name);
        let read = emulator
            .ok("GetOnPremisesInstance", json!({"instanceName": name}))
            .await["instanceInfo"]
            .clone();
        let registration = registration_of(name, &prefix, read["instanceArn"].as_str());

        // Every reader answers the ARN the registry minted, not one rebuilt per
        // reader.
        let batched = emulator
            .ok(
                "BatchGetOnPremisesInstances",
                json!({"instanceNames": [name]}),
            )
            .await["instanceInfos"][0]
            .clone();
        assert_eq!(
            batched["instanceArn"].as_str(),
            read["instanceArn"].as_str(),
            "case {name:?}: expected the batch reader to answer the same ARN, got {batched:?}"
        );
        registrations.push(registration);
    }

    // Re-registering a name is a NEW registration, so its ARN carries a new id.
    let name = names[0];
    emulator
        .ok(
            "DeregisterOnPremisesInstance",
            json!({"instanceName": name}),
        )
        .await;
    emulator
        .ok(
            "RegisterOnPremisesInstance",
            json!({"instanceName": name, "iamUserArn": user_arn(name)}),
        )
        .await;
    let again = emulator
        .ok("GetOnPremisesInstance", json!({"instanceName": name}))
        .await["instanceInfo"]
        .clone();
    let reregistered = registration_of(
        "re-registered",
        &expected_prefix(name),
        again["instanceArn"].as_str(),
    );
    assert_ne!(
        reregistered, registrations[0],
        "case re-registered: expected a new registration id, got {reregistered:?} again"
    );

    registrations.push(reregistered);
    let unique: std::collections::BTreeSet<&String> = registrations.iter().collect();
    assert_eq!(
        unique.len(),
        registrations.len(),
        "expected one registration id per registration, got {registrations:?}"
    );
}

// ---------------------------------------------------------------------------
// Hook execution ids
// ---------------------------------------------------------------------------

/// RULE: a hook result is reported against a hook execution OF the deployment
/// named, so an id that names none of its hook executions is refused with
/// `InvalidLifecycleEventHookExecutionIdException` — which stays distinct from
/// the `DeploymentDoesNotExistException` a deployment id nothing created is
/// refused with.
#[tokio::test]
async fn a_hook_execution_id_the_deployment_never_issued_is_invalid() {
    let emulator = Emulator::new();
    // A deployment on the compute platform whose lifecycle events the CodeDeploy
    // agent runs: it issues no hook execution id, so no id names a hook
    // execution of it.
    let group = seeded_group(&emulator, "hooked", "Server").await;
    let deployment = deploy(&emulator, "hooked", &group).await;

    // Rows differ along the FORM of a fabricated execution id, and along the
    // result reported.
    let cases: &[(&str, &str)] = &[
        ("sim2real-hook-1", "Succeeded"),
        ("00000000-0000-0000-0000-000000000000", "Failed"),
        ("hook", "Succeeded"),
    ];
    for (hook_id, reported) in cases {
        let (status, body) = emulator
            .call(
                "PutLifecycleEventHookExecutionStatus",
                json!({
                    "deploymentId": deployment,
                    "lifecycleEventHookExecutionId": hook_id,
                    "status": reported,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidLifecycleEventHookExecutionIdException"),
            "case {hook_id:?}: expected the hook execution id to be refused, got {status} \
             {body:?}"
        );
    }

    // A deployment id nothing created is the OTHER refusal: the two stay
    // distinguishable.
    let (status, body) = emulator
        .call(
            "PutLifecycleEventHookExecutionStatus",
            json!({
                "deploymentId": "d-ABCDEFGHI",
                "lifecycleEventHookExecutionId": "sim2real-hook-1",
                "status": "Succeeded",
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "DeploymentDoesNotExistException"),
        "case an id nothing created: expected the deployment not to exist, got {status} {body:?}"
    );

    // And the refusals changed nothing: the deployment still reports no
    // lifecycle event of its own.
    let info = emulator
        .ok("GetDeployment", json!({"deploymentId": deployment}))
        .await["deploymentInfo"]
        .clone();
    assert!(
        info["errorInformation"]["code"].as_str() != Some("HOOK_EXECUTION_FAILURE"),
        "case after the refusals: expected no reported hook failure, got {info:?}"
    );
}

// ---------------------------------------------------------------------------
// The sentences a refusal states
// ---------------------------------------------------------------------------

/// RULE: a refused request states the sentence its error owns, and where that
/// sentence is built from the request it names the value the caller sent —
/// never the constraint validator's generic echo.
#[tokio::test]
async fn a_refusal_states_the_sentence_its_error_owns() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": "worded-a", "computePlatform": "Server"}),
        )
        .await;
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": "worded-b", "computePlatform": "Server"}),
        )
        .await;
    emulator
        .ok(
            "CreateDeploymentConfig",
            json!({
                "deploymentConfigName": "worded-config",
                "minimumHealthyHosts": {"type": "HOST_COUNT", "value": 1},
            }),
        )
        .await;
    emulator
        .ok(
            "RegisterOnPremisesInstance",
            json!({"instanceName": "worded-host", "iamUserArn": user_arn("worded-host")}),
        )
        .await;

    // Rows differ along the operation refusing, and — for a sentence built from
    // the request — along the VALUE the request carried, so a sentence that
    // ignored the request would fail a row.
    let cases: &[(&str, Value, &str, String)] = &[
        (
            "ListOnPremisesInstances",
            json!({"registrationStatus": "NotARealStatus"}),
            "InvalidRegistrationStatusException",
            "Registration status NotARealStatus is not valid.".to_string(),
        ),
        (
            "ListOnPremisesInstances",
            json!({"registrationStatus": "registered"}),
            "InvalidRegistrationStatusException",
            "Registration status registered is not valid.".to_string(),
        ),
        (
            "GetOnPremisesInstance",
            json!({"instanceName": "missing-host"}),
            "InstanceNotRegisteredException",
            "The requested host does not exist. Host: missing-host".to_string(),
        ),
        (
            "GetOnPremisesInstance",
            json!({"instanceName": "other-missing-host"}),
            "InstanceNotRegisteredException",
            "The requested host does not exist. Host: other-missing-host".to_string(),
        ),
        (
            "AddTagsToOnPremisesInstances",
            json!({
                "instanceNames": ["missing-host"],
                "tags": [{"Key": "k", "Value": "v"}],
            }),
            "InstanceNotRegisteredException",
            "Host missing-host is not registered".to_string(),
        ),
        (
            "RemoveTagsFromOnPremisesInstances",
            json!({
                "instanceNames": ["another-missing-host"],
                "tags": [{"Key": "k", "Value": "v"}],
            }),
            "InstanceNotRegisteredException",
            "Host another-missing-host is not registered".to_string(),
        ),
        (
            "GetDeploymentConfig",
            json!({"deploymentConfigName": "absent-config"}),
            "DeploymentConfigDoesNotExistException",
            "No deployment configuration found for name: absent-config".to_string(),
        ),
        (
            "CreateDeploymentConfig",
            json!({
                "deploymentConfigName": "worded-config",
                "minimumHealthyHosts": {"type": "HOST_COUNT", "value": 1},
            }),
            "DeploymentConfigAlreadyExistsException",
            "worded-config already exists.".to_string(),
        ),
        (
            "CreateDeploymentConfig",
            json!({
                "deploymentConfigName": "CodeDeployDefault.OneAtATime",
                "minimumHealthyHosts": {"type": "HOST_COUNT", "value": 1},
            }),
            "DeploymentConfigAlreadyExistsException",
            "CodeDeployDefault.OneAtATime already exists.".to_string(),
        ),
        (
            "CreateDeploymentConfig",
            json!({
                "deploymentConfigName": "server-routing",
                "computePlatform": "Server",
                "trafficRoutingConfig": {"type": "AllAtOnce"},
            }),
            "InvalidTrafficRoutingConfigurationException",
            "Traffic routing configuration should be null for Server deployment configuration"
                .to_string(),
        ),
        (
            "UpdateApplication",
            json!({"applicationName": "worded-a", "newApplicationName": "worded-b"}),
            "ApplicationAlreadyExistsException",
            "The application name: worded-b you want to update to already exist".to_string(),
        ),
        (
            "UpdateApplication",
            json!({"applicationName": "worded-b", "newApplicationName": "worded-a"}),
            "ApplicationAlreadyExistsException",
            "The application name: worded-a you want to update to already exist".to_string(),
        ),
        (
            "RegisterOnPremisesInstance",
            json!({
                "instanceName": "worded-host",
                "iamUserArn": "arn:aws:iam::000000000000:user/someone-else",
            }),
            "InstanceNameAlreadyRegisteredException",
            "Host name has already been registered or recently deregistered".to_string(),
        ),
        (
            "DeleteGitHubAccountToken",
            json!({}),
            "GitHubAccountTokenNameRequiredException",
            "Token name is missing".to_string(),
        ),
        (
            "DeleteGitHubAccountToken",
            json!({"tokenName": "   "}),
            "GitHubAccountTokenNameRequiredException",
            "Token name is missing".to_string(),
        ),
        (
            "ListApplicationRevisions",
            json!({"applicationName": "worded-a", "s3KeyPrefix": "builds/"}),
            "BucketNameFilterRequiredException",
            "S3 object key prefix filter specified without specifying bucket name".to_string(),
        ),
        (
            "CreateDeploymentGroup",
            json!({
                "applicationName": "worded-a",
                "deploymentGroupName": "rollback-no-events",
                "serviceRoleArn": SERVICE_ROLE,
                "autoRollbackConfiguration": {"enabled": true, "events": []},
            }),
            "InvalidAutoRollbackConfigException",
            "Deployment Groups need to have at least one event specified when auto rollback is \
             enabled"
                .to_string(),
        ),
        (
            "CreateDeploymentGroup",
            json!({
                "applicationName": "worded-a",
                "deploymentGroupName": "monitored-no-alarms",
                "serviceRoleArn": SERVICE_ROLE,
                "alarmConfiguration": {"enabled": true, "alarms": []},
            }),
            "InvalidAlarmConfigException",
            "Deployment Groups need to have at least one alarms attached if they are monitored."
                .to_string(),
        ),
    ];

    for (operation, request, expected_code, expected_message) in cases {
        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected_code),
            "case {operation} {request:?}: expected 400 {expected_code}, got {status} {body:?}"
        );
        assert_eq!(
            error_message(&body),
            *expected_message,
            "case {operation} {request:?}: expected the message {expected_message:?}, got \
             {body:?}"
        );
    }
}
