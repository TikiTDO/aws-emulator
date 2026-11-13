// SPDX-License-Identifier: CC0-1.0

//! The whole METRIC-DRIVEN scaling scenario over an ECS service whose capacity was LAUNCHED:
//! from the Amazon EC2 instance that gives the cluster capacity, through the task the
//! scheduler places on it, to the datapoint that puts one alarm in `ALARM` and the scale-out
//! that follows — beside the twin alarm nothing feeds, which stays in `INSUFFICIENT_DATA` and
//! scales nothing.
//!
//! RULE: every leg of a metric-driven scaling scenario reaches an observable state on its
//! own clock, with no call but the reads a client makes:
//!
//! * an Amazon EC2 instance launched into a cluster (the container agent's `ECS_CLUSTER` in
//!   its user data) becomes that cluster's registered capacity, so the ECS service's tasks
//!   are placed and `Service$runningCount` reaches the count the caller asked for — "the EC2
//!   launch type runs your tasks on Amazon EC2 instances registered to your cluster";
//! * a CloudWatch alarm whose `AlarmActions` name a step scaling policy's ARN executes that
//!   policy when the datapoints published put the alarm in `ALARM`, which moves the capacity
//!   of the resource the scalable target scales and records ONE scaling activity;
//! * an alarm nothing publishes for cannot decide anything, so it stays in
//!   `INSUFFICIENT_DATA` ("if you specify `missing`, the alarm is treated as not breaching
//!   the threshold ... the alarm transitions to INSUFFICIENT_DATA") and executes nothing at
//!   all.
//!
//! Why this suite exists beside [`crate::published_breach_execution`], which pins the breach
//! rule over a table of its own: that suite gives its cluster capacity through the agent's
//! own registration call, so it says nothing about a client that provisions capacity the way
//! AWS documents — launching an instance into the cluster — and nothing about the
//! `runningCount` that follows a scale-out. This one drives exactly that chain, and the
//! negative leg (the unfed twin) beside it, so "correctly did not scale" is distinguishable
//! from "nothing works".
//!
//! Nothing here reaches into a state struct: the instance is created through Amazon EC2's own
//! API, the cluster and service through ECS's, the alarms and datapoints through
//! CloudWatch's, and every value asserted is read back through `DescribeServices`,
//! `DescribeAlarms` and `DescribeScalingActivities`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::emulator::{Emulator, REGION};

/// The cluster the capacity is launched into, and the service that runs on it.
const CLUSTER: &str = "metricdriven";
const SERVICE: &str = "web";

/// The namespace and dimension the scalable target is registered under: "ecs:service:
/// DesiredCount - The task count of an ECS service".
const NAMESPACE: &str = "ecs";
const DIMENSION: &str = "ecs:service:DesiredCount";

/// The window the target is registered with, and the count the service starts at.
const MIN_CAPACITY: i64 = 1;
const MAX_CAPACITY: i64 = 6;
const STARTING_COUNT: i64 = 1;

/// The step the policy takes when its alarm fires, and the capacity that lands on.
const ADJUSTMENT: i64 = 2;
const SCALED_COUNT: i64 = STARTING_COUNT + ADJUSTMENT;

/// The customer metric namespace the datapoint is published into, and the two metrics the
/// twin alarms watch: one the client feeds, one it deliberately does not.
const METRIC_NAMESPACE: &str = "Emulator/MetricDriven";
const FED_METRIC: &str = "FedLoad";
const UNFED_METRIC: &str = "UnfedLoad";

/// The threshold both alarms carry, and the value published for the fed one.
const THRESHOLD: f64 = 50.0;
const PUBLISHED_VALUE: f64 = THRESHOLD + 30.0;

/// The alarms' evaluation cadence, in seconds: a sub-minute period, so the window that
/// grades a just-published datapoint rolls past it inside the poll below.
const ALARM_PERIOD: i64 = 10;

/// The two alarms, both naming the same policy among their actions.
const FED_ALARM: &str = "metricdriven-fed";
const UNFED_ALARM: &str = "metricdriven-unfed";

/// The policy the alarms execute.
const POLICY: &str = "metricdriven-out";

/// How long a leg is given to reach its observable state, and how often it is looked at.
///
/// Nothing in the emulator sleeps for a leg: each poll is the TEST waiting for what the clock
/// owes (the agent's registration, the task's start, the alarm's evaluation, the execution
/// that follows), so a leg that never arrives fails on the value it last read. Generous, so a
/// loaded machine cannot turn a slow leg into a failure.
const LEG_BUDGET: Duration = Duration::from_secs(90);
/// See [`LEG_BUDGET`].
const LEG_POLL: Duration = Duration::from_millis(200);

/// How long the unfed alarm is watched for a scaling activity that must not happen: long
/// enough to span many of the scaling clock's passes and several alarm evaluations.
const QUIET_WATCH: Duration = Duration::from_secs(3);

/// The AMI every launch boots: well-formed, which is all a launch needs.
const IMAGE_ID: &str = "ami-0123456789abcdef0";

/// The instance type the capacity comes from.
const INSTANCE_TYPE: &str = "t3.micro";

/// The three identity members of the `ecs` scalable target on `resource_id`.
fn target(resource_id: &str) -> Value {
    json!({
        "ServiceNamespace": NAMESPACE,
        "ResourceId": resource_id,
        "ScalableDimension": DIMENSION,
    })
}

/// The text of the FIRST `<name>` element of an `ec2Query` response.
fn element(xml: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml
        .find(&open)
        .unwrap_or_else(|| panic!("expected a <{name}> in {xml}"))
        + open.len();
    let end = xml[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("expected a </{name}> in {xml}"))
        + start;
    xml[start..end].to_string()
}

/// One Amazon EC2 call that must succeed, answering its XML body.
async fn ec2(emulator: &Emulator, action: &str, params: &[(&str, &str)]) -> String {
    let (status, body) = emulator.call_ec2(action, params).await;
    assert_eq!(
        status, 200,
        "case {action:?}: expected Amazon EC2 to answer 200, got {status} with {body}"
    );
    body
}

/// Create the VPC and the public subnet the instance is launched into, through Amazon EC2's
/// own API, and answer the subnet's id.
async fn network(emulator: &Emulator) -> String {
    let vpc = element(
        &ec2(emulator, "CreateVpc", &[("CidrBlock", "10.70.0.0/16")]).await,
        "vpcId",
    );
    let zone = format!("{REGION}a");
    let subnet = element(
        &ec2(
            emulator,
            "CreateSubnet",
            &[
                ("VpcId", vpc.as_str()),
                ("CidrBlock", "10.70.0.0/20"),
                ("AvailabilityZone", zone.as_str()),
            ],
        )
        .await,
        "subnetId",
    );
    // "To use an internet gateway, your subnet's route table must contain a route that
    // directs internet-bound traffic to the internet gateway" — the path the tasks' images
    // are fetched over, so the subnet the capacity sits in is a public one.
    let gateway = element(
        &ec2(emulator, "CreateInternetGateway", &[]).await,
        "internetGatewayId",
    );
    ec2(
        emulator,
        "AttachInternetGateway",
        &[("InternetGatewayId", gateway.as_str()), ("VpcId", vpc.as_str())],
    )
    .await;
    let route_table = element(
        &ec2(emulator, "CreateRouteTable", &[("VpcId", vpc.as_str())]).await,
        "routeTableId",
    );
    ec2(
        emulator,
        "AssociateRouteTable",
        &[
            ("RouteTableId", route_table.as_str()),
            ("SubnetId", subnet.as_str()),
        ],
    )
    .await;
    ec2(
        emulator,
        "CreateRoute",
        &[
            ("RouteTableId", route_table.as_str()),
            ("DestinationCidrBlock", "0.0.0.0/0"),
            ("GatewayId", gateway.as_str()),
        ],
    )
    .await;
    subnet
}

/// Launch ONE instance into `subnet` whose user data configures the container agent with
/// [`CLUSTER`], the documented way to put EC2 capacity into a cluster, and answer the
/// instance id Amazon EC2 minted.
async fn launch_capacity(emulator: &Emulator, subnet: &str) -> String {
    use ::base64::Engine as _;

    let user_data = ::base64::engine::general_purpose::STANDARD.encode(format!(
        "#!/bin/bash\necho ECS_CLUSTER={CLUSTER} >> /etc/ecs/ecs.config\n"
    ));
    let launched = ec2(
        emulator,
        "RunInstances",
        &[
            ("ImageId", IMAGE_ID),
            ("InstanceType", INSTANCE_TYPE),
            ("SubnetId", subnet),
            ("MinCount", "1"),
            ("MaxCount", "1"),
            ("UserData", user_data.as_str()),
        ],
    )
    .await;
    element(&launched, "instanceId")
}

/// Wait until the ECS service reports `wanted` tasks RUNNING, answering what it last read.
async fn awaited_running_count(emulator: &Emulator, wanted: i64) -> i64 {
    let deadline = Instant::now() + LEG_BUDGET;
    let mut last = emulator.ecs_running_count(CLUSTER, SERVICE).await;
    while last != wanted && Instant::now() < deadline {
        tokio::time::sleep(LEG_POLL).await;
        last = emulator.ecs_running_count(CLUSTER, SERVICE).await;
    }
    last
}

/// Wait until the ECS service's `desiredCount` reaches `wanted`, answering what it last read.
async fn awaited_desired_count(emulator: &Emulator, wanted: i64) -> i64 {
    let deadline = Instant::now() + LEG_BUDGET;
    let mut last = emulator.ecs_desired_count(CLUSTER, SERVICE).await;
    while last != wanted && Instant::now() < deadline {
        tokio::time::sleep(LEG_POLL).await;
        last = emulator.ecs_desired_count(CLUSTER, SERVICE).await;
    }
    last
}

/// The state CloudWatch reports for one alarm.
async fn alarm_state(emulator: &Emulator, alarm_name: &str) -> String {
    let (status, body) = emulator
        .call_cloudwatch("DescribeAlarms", json!({"AlarmNames": [alarm_name]}))
        .await;
    assert_eq!(
        status, 200,
        "case {alarm_name:?}: expected DescribeAlarms to answer 200, got {status} {body:?}"
    );
    body["MetricAlarms"][0]["StateValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Wait until one alarm reports `wanted`, answering what it last read.
async fn awaited_alarm_state(emulator: &Emulator, alarm_name: &str, wanted: &str) -> String {
    let deadline = Instant::now() + LEG_BUDGET;
    let mut last = alarm_state(emulator, alarm_name).await;
    while last != wanted && Instant::now() < deadline {
        tokio::time::sleep(LEG_POLL).await;
        last = alarm_state(emulator, alarm_name).await;
    }
    last
}

/// The scaling activities the target's log holds.
async fn activities(emulator: &Emulator, resource_id: &str) -> Vec<Value> {
    let (status, body) = emulator
        .call("DescribeScalingActivities", target(resource_id))
        .await;
    assert_eq!(
        status, 200,
        "expected DescribeScalingActivities to answer 200, got {status} {body:?}"
    );
    body["ScalingActivities"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a ScalingActivities list, got {body:?}"))
        .clone()
}

/// Put one alarm watching `metric_name`, with `policy_arn` among the actions its transition
/// into `ALARM` executes.
async fn put_alarm(emulator: &Emulator, alarm_name: &str, metric_name: &str, policy_arn: &str) {
    let (status, body) = emulator
        .call_cloudwatch(
            "PutMetricAlarm",
            json!({
                "AlarmName": alarm_name,
                "Namespace": METRIC_NAMESPACE,
                "MetricName": metric_name,
                "Dimensions": [{"Name": "ServiceName", "Value": SERVICE}],
                "Statistic": "Maximum",
                "Period": ALARM_PERIOD,
                "EvaluationPeriods": 1,
                "DatapointsToAlarm": 1,
                "Threshold": THRESHOLD,
                "ComparisonOperator": "GreaterThanThreshold",
                // "If you specify `missing`, the alarm is treated as not breaching the
                // threshold": a window with no datapoint of its own decides nothing, which
                // is what leaves the unfed twin in INSUFFICIENT_DATA and what makes the fed
                // alarm's ALARM a statement about the datapoint it really graded.
                "TreatMissingData": "missing",
                "AlarmActions": [policy_arn],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {alarm_name:?}: expected PutMetricAlarm to answer 200, got {status} {body:?}"
    );
}

#[tokio::test]
async fn a_metric_driven_policy_scales_an_ecs_service_running_on_launched_ec2_capacity() {
    let emulator = Emulator::with_ec2();
    let subnet = network(&emulator).await;

    // LEG ONE: the estate. A cluster with an ECS service that wants one task, and the EC2
    // capacity launched into that cluster — in that order, so the service is created with
    // nowhere to run a task and the capacity is what lets the scheduler place it.
    let resource_id = emulator
        .given_ecs_service_without_capacity(CLUSTER, SERVICE, STARTING_COUNT)
        .await;
    let instance_id = launch_capacity(&emulator, &subnet).await;
    let running = awaited_running_count(&emulator, STARTING_COUNT).await;
    assert_eq!(
        running, STARTING_COUNT,
        "expected the service to run its {STARTING_COUNT} task(s) on the capacity launched \
         into its cluster ({instance_id}), got {running}"
    );

    // LEG TWO: the scalable target and the step scaling policy over it.
    let mut registration = target(&resource_id);
    registration["MinCapacity"] = json!(MIN_CAPACITY);
    registration["MaxCapacity"] = json!(MAX_CAPACITY);
    let (status, body) = emulator.call("RegisterScalableTarget", registration).await;
    assert_eq!(
        status, 200,
        "expected RegisterScalableTarget to answer 200, got {status} {body:?}"
    );
    let mut request = target(&resource_id);
    request["PolicyName"] = json!(POLICY);
    request["PolicyType"] = json!("StepScaling");
    request["StepScalingPolicyConfiguration"] = json!({
        "AdjustmentType": "ChangeInCapacity",
        "MetricAggregationType": "Maximum",
        // Nothing here breaches twice, so no wait a previous activity is given comes into it.
        "Cooldown": 0,
        "StepAdjustments": [{
            "MetricIntervalLowerBound": 0.0,
            "ScalingAdjustment": ADJUSTMENT,
        }],
    });
    let (status, body) = emulator.call("PutScalingPolicy", request).await;
    assert_eq!(
        status, 200,
        "expected PutScalingPolicy to answer 200, got {status} {body:?}"
    );
    let policy_arn = body["PolicyARN"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a PolicyARN, got {body:?}"))
        .to_string();

    // LEG THREE: the twin alarms, both naming that policy — and one datapoint, published for
    // ONE of the two metrics the way a client publishes one (no time stamp, so it is stamped
    // at the instant of the call).
    put_alarm(&emulator, FED_ALARM, FED_METRIC, &policy_arn).await;
    put_alarm(&emulator, UNFED_ALARM, UNFED_METRIC, &policy_arn).await;
    let (status, body) = emulator
        .call_cloudwatch(
            "PutMetricData",
            json!({
                "Namespace": METRIC_NAMESPACE,
                "MetricData": [{
                    "MetricName": FED_METRIC,
                    "Dimensions": [{"Name": "ServiceName", "Value": SERVICE}],
                    "Value": PUBLISHED_VALUE,
                }],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected PutMetricData to answer 200, got {status} {body:?}"
    );

    // The fed alarm reaches ALARM on the datapoint it graded, and the policy it names scales
    // the service out: the desired count moves and the tasks the new count wants run.
    let state = awaited_alarm_state(&emulator, FED_ALARM, "ALARM").await;
    assert_eq!(
        state, "ALARM",
        "expected the fed alarm to grade the published datapoint into ALARM, got {state:?}"
    );
    let desired = awaited_desired_count(&emulator, SCALED_COUNT).await;
    assert_eq!(
        desired, SCALED_COUNT,
        "expected the breach to scale the service out to {SCALED_COUNT}, got {desired}"
    );
    let running = awaited_running_count(&emulator, SCALED_COUNT).await;
    assert_eq!(
        running, SCALED_COUNT,
        "expected the scaled-out service to run {SCALED_COUNT} tasks on its cluster's \
         capacity, got {running}"
    );

    // The negative leg: the twin nothing published for stays in INSUFFICIENT_DATA, and the
    // whole scenario records exactly ONE scaling activity — the one the FED alarm caused.
    let unfed = awaited_alarm_state(&emulator, UNFED_ALARM, "INSUFFICIENT_DATA").await;
    assert_eq!(
        unfed, "INSUFFICIENT_DATA",
        "expected the alarm nothing publishes for to stay in INSUFFICIENT_DATA, got {unfed:?}"
    );
    // The invariant is WATCHED rather than waited for: the log and the capacity are re-read
    // until the watch is up, so a second execution appearing at any point inside it fails
    // here rather than being missed by a single later look.
    let watch_until = Instant::now() + QUIET_WATCH;
    loop {
        let listed = activities(&emulator, &resource_id).await;
        assert_eq!(
            listed.len(),
            1,
            "expected the fed alarm's scale-out to be the only scaling activity, got {listed:?}"
        );
        let cause = listed[0]["Cause"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            cause,
            format!("monitor alarm {FED_ALARM} in state ALARM triggered policy {POLICY}"),
            "expected the one activity to be caused by the fed alarm, got {cause:?}"
        );
        assert!(
            !cause.contains(UNFED_ALARM),
            "expected no activity caused by the alarm nothing fed, got {cause:?}"
        );
        // And the capacity stays where the one execution put it: the unfed alarm scales
        // nothing.
        let settled = emulator.ecs_desired_count(CLUSTER, SERVICE).await;
        assert_eq!(
            settled, SCALED_COUNT,
            "expected the capacity to stay at {SCALED_COUNT} while the unfed alarm was \
             watched, got {settled}"
        );
        let state = alarm_state(&emulator, UNFED_ALARM).await;
        assert_eq!(
            state, "INSUFFICIENT_DATA",
            "expected the unfed alarm to stay in INSUFFICIENT_DATA while it was watched, got \
             {state:?}"
        );
        if Instant::now() >= watch_until {
            break;
        }
        tokio::time::sleep(LEG_POLL).await;
    }
}
