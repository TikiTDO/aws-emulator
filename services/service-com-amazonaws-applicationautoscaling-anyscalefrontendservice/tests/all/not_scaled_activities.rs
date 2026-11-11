// SPDX-License-Identifier: CC0-1.0

//! WHAT the activity log says when a breaching alarm's scaling decision moves no capacity
//! because the target is already AT the bound the decision asks past.
//!
//! The rules under test:
//!
//! * a decision a registered bound withholds is recorded as a NOT SCALED activity: `StatusCode`
//!   `Failed`, the fixed description of a decision taken on an alarm, and one
//!   `NotScaledReasons` entry whose `Code` is the model's own reason for that bound beside the
//!   bound and the capacity the target was found at;
//! * such an activity is reported ONLY to a caller that asks for it — "specifies whether to
//!   include activities that aren't scaled" — so the default view of the log does not hold it;
//! * a breach that keeps breaching for the same reason adds no further entry; and
//! * widening the bound that withheld the movement delivers it, as a movement of the resource
//!   and a `Successful` activity of its own.
//!
//! The catalog spans both ends of the registered window — the ceiling a scale-out asks past and
//! the floor a scale-in asks past — because the rule is about the bound, not about one
//! direction.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use super::emulator::{Emulator, PolicyAlarm};

/// How long a case polls for what the clock performs: well past the quarter-second the
/// in-process clock waits between passes and the fulfilment span a movement takes.
const POLL_FOR: Duration = Duration::from_secs(25);

/// How long a poll sleeps between reads.
const POLL_EVERY: Duration = Duration::from_millis(200);

/// The metric the suite's alarms watch. Nothing is published for it: the alarm is driven into
/// `ALARM` directly, which is how a client breaches one without a datapoint.
const METRIC_NAMESPACE: &str = "AWS/ECS";
/// See [`METRIC_NAMESPACE`].
const METRIC_NAME: &str = "CPUUtilization";

/// The description every activity taken on an alarm breach that scaled nothing carries.
const NOT_SCALED_DESCRIPTION: &str = "Attempting to scale due to alarm triggered";

/// One breach a registered bound withholds.
struct WithheldRow {
    case: &'static str,
    /// The window the target is registered with, one end of which the live capacity sits at.
    registered: (i64, i64),
    /// The capacity the resource holds, which is that end of the window.
    live: i64,
    /// The adjustment the policy's step asks for, which is past that end.
    adjustment: i64,
    /// The comparison that puts the alarm in `ALARM`, which names the direction of the breach.
    comparison_operator: &'static str,
    /// WHICH end of the step's metric interval the bound `0` is: a breach above the threshold
    /// is measured upwards from it, one below it downwards — "if the metric value is above the
    /// breach threshold, the lower bound is inclusive […] otherwise, it's exclusive".
    step_interval_bound: &'static str,
    /// The `NotScaledReason$Code` the decision is recorded with.
    code: &'static str,
    /// The bound the reason reports, as `(MaxCapacity, MinCapacity)` — one of them, since only
    /// the bound that withheld the movement had any part in it.
    reported_bounds: (Option<i64>, Option<i64>),
    /// The window the target is re-registered with afterwards, which widens the bound that
    /// withheld the movement.
    widened: (i64, i64),
    /// The capacity the withheld movement then delivers.
    delivered: i64,
}

/// Both ends of the window a decision can be withheld by. The reason codes are the model's own
/// `NotScaledReason$Code` values for a target already at a bound.
const WITHHELD_ROWS: &[WithheldRow] = &[
    WithheldRow {
        case: "a scale-out against a ceiling the capacity already sits at",
        registered: (1, 5),
        live: 5,
        adjustment: 3,
        comparison_operator: "GreaterThanThreshold",
        step_interval_bound: "MetricIntervalLowerBound",
        code: "AlreadyAtMaxCapacity",
        reported_bounds: (Some(5), None),
        widened: (1, 9),
        delivered: 8,
    },
    WithheldRow {
        case: "a scale-in against a floor the capacity already sits at",
        registered: (4, 20),
        live: 4,
        adjustment: -2,
        comparison_operator: "LessThanThreshold",
        step_interval_bound: "MetricIntervalUpperBound",
        code: "AlreadyAtMinCapacity",
        reported_bounds: (None, Some(4)),
        widened: (1, 20),
        delivered: 2,
    },
];

#[tokio::test]
async fn a_decision_a_bound_withholds_is_recorded_as_a_not_scaled_activity() {
    for row in WITHHELD_ROWS {
        let case = row.case;
        let emulator = Emulator::empty();
        let (cluster, service) = ("scaled", "web");
        let resource_id = emulator.given_ecs_service(cluster, service, row.live).await;
        let target = json!({
            "ServiceNamespace": "ecs",
            "ResourceId": resource_id,
            "ScalableDimension": "ecs:service:DesiredCount",
        });
        register(&emulator, &target, row.registered, case).await;

        let mut policy = target.clone();
        policy["PolicyName"] = json!("the-policy");
        policy["PolicyType"] = json!("StepScaling");
        let mut step = json!({"ScalingAdjustment": row.adjustment});
        step[row.step_interval_bound] = json!(0);
        policy["StepScalingPolicyConfiguration"] = json!({
            "AdjustmentType": "ChangeInCapacity",
            "Cooldown": 0,
            "StepAdjustments": [step],
        });
        let (status, body) = emulator.call("PutScalingPolicy", policy).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutScalingPolicy to answer 200, got {status} {body:?}"
        );
        let policy_arn = body["PolicyARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a PolicyARN, got {body:?}"))
            .to_string();

        let alarm_name = "the-alarm";
        emulator
            .given_policy_alarm(&PolicyAlarm {
                name: alarm_name,
                policy_arn: &policy_arn,
                metric_namespace: METRIC_NAMESPACE,
                metric_name: METRIC_NAME,
                threshold: 50.0,
                comparison_operator: row.comparison_operator,
            })
            .await;
        emulator.set_alarm_state(alarm_name, "ALARM").await;

        // What the caller that asks for not-scaled activities sees: exactly one, naming the
        // reason the movement was withheld for.
        let withheld = poll_for_not_scaled(&emulator, &target, 1).await;
        assert_eq!(
            withheld.len(),
            1,
            "case {case:?}: expected exactly one not-scaled activity, got {withheld:?}"
        );
        let activity = &withheld[0];
        assert_eq!(
            activity["StatusCode"].as_str().unwrap_or_default(),
            "Failed",
            "case {case:?}: expected the withheld decision to settle Failed, got {activity:?}"
        );
        assert_eq!(
            activity["Description"].as_str().unwrap_or_default(),
            NOT_SCALED_DESCRIPTION,
            "case {case:?}: expected the fixed description of a decision taken on an alarm, got \
             {activity:?}"
        );
        let reason = &activity["NotScaledReasons"][0];
        assert_eq!(
            reason["Code"].as_str().unwrap_or_default(),
            row.code,
            "case {case:?}: expected the reason code {}, got {activity:?}",
            row.code
        );
        assert_eq!(
            (
                reason["MaxCapacity"].as_i64(),
                reason["MinCapacity"].as_i64()
            ),
            row.reported_bounds,
            "case {case:?}: expected the reason to report the bound that withheld the movement \
             as {:?}, got {activity:?}",
            row.reported_bounds
        );
        assert_eq!(
            reason["CurrentCapacity"].as_i64(),
            Some(row.live),
            "case {case:?}: expected the reason to report the live capacity {}, got {activity:?}",
            row.live
        );

        // The default view hides it, and the continuing breach adds nothing to either view.
        let default_view = activities(&emulator, &target, false).await;
        assert!(
            default_view.is_empty(),
            "case {case:?}: expected the default view to hide a not-scaled activity, got \
             {default_view:?}"
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
        let again = activities(&emulator, &target, true).await;
        assert_eq!(
            again.len(),
            1,
            "case {case:?}: expected the continuing breach to add no further entry, got {again:?}"
        );
        let count = emulator.ecs_desired_count(cluster, service).await;
        assert_eq!(
            count, row.live,
            "case {case:?}: expected the withheld decision to move nothing, got {count}"
        );

        // Widening the bound that withheld the movement delivers it.
        register(&emulator, &target, row.widened, case).await;
        let delivered = poll_for_capacity(&emulator, cluster, service, row.delivered).await;
        assert_eq!(
            delivered, row.delivered,
            "case {case:?}: expected the widened window to deliver the withheld movement to {}, \
             got {delivered}",
            row.delivered
        );
        let scaled: Vec<Value> = activities(&emulator, &target, true)
            .await
            .into_iter()
            .filter(|activity| activity["NotScaledReasons"].is_null())
            .collect();
        assert_eq!(
            scaled.len(),
            1,
            "case {case:?}: expected the delivered movement to be recorded once, got {scaled:?}"
        );
    }
}

/// Register `target` with the window `bounds`.
async fn register(emulator: &Emulator, target: &Value, bounds: (i64, i64), case: &str) {
    let mut request = target.clone();
    request["MinCapacity"] = json!(bounds.0);
    request["MaxCapacity"] = json!(bounds.1);
    let (status, body) = emulator.call("RegisterScalableTarget", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected RegisterScalableTarget to answer 200, got {status} {body:?}"
    );
}

/// Poll until the log holds at least `wanted` not-scaled activities for `target`, and answer
/// the ones it holds.
async fn poll_for_not_scaled(emulator: &Emulator, target: &Value, wanted: usize) -> Vec<Value> {
    let deadline = SystemTime::now() + POLL_FOR;
    let mut last = not_scaled(emulator, target).await;
    while last.len() < wanted && SystemTime::now() < deadline {
        tokio::time::sleep(POLL_EVERY).await;
        last = not_scaled(emulator, target).await;
    }
    last
}

/// The not-scaled activities of `target`, out of the view that holds them.
async fn not_scaled(emulator: &Emulator, target: &Value) -> Vec<Value> {
    activities(emulator, target, true)
        .await
        .into_iter()
        .filter(|activity| !activity["NotScaledReasons"].is_null())
        .collect()
}

/// The activities of `target`, with or without the ones that were not scaled.
async fn activities(emulator: &Emulator, target: &Value, include_not_scaled: bool) -> Vec<Value> {
    let mut request = target.clone();
    request["IncludeNotScaledActivities"] = json!(include_not_scaled);
    let (status, body) = emulator.call("DescribeScalingActivities", request).await;
    assert_eq!(
        status, 200,
        "case \"the activity log\": expected 200, got {status} {body:?}"
    );
    body["ScalingActivities"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Poll the ECS service's `desiredCount` until it reaches `wanted`, answering what it last read.
async fn poll_for_capacity(
    emulator: &Emulator,
    cluster: &str,
    service: &str,
    wanted: i64,
) -> i64 {
    let deadline = SystemTime::now() + POLL_FOR;
    let mut last = emulator.ecs_desired_count(cluster, service).await;
    while last != wanted && SystemTime::now() < deadline {
        tokio::time::sleep(POLL_EVERY).await;
        last = emulator.ecs_desired_count(cluster, service).await;
    }
    last
}
