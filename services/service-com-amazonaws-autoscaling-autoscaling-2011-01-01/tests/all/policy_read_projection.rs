// SPDX-License-Identifier: CC0-1.0

//! What a READ of a scaling policy reports, over the awsQuery wire.
//!
//! Two rules, both about the projection rather than about any one policy:
//!
//! 1. A read reports the policy the service RESOLVED, not the request that created it:
//!    every collection the shape declares is reported as the collection the policy holds
//!    — empty when it holds none — and every switch whose default the model documents is
//!    reported with that default when the put named none. So a client reads the effective
//!    policy back, and the documented update idiom (echo a described policy into
//!    `PutScalingPolicy`) changes nothing.
//! 2. A modelled floating-point member is written as a decimal carrying a fractional
//!    part, the form an AWS XML response carries: a whole value reads `50.0`, never `50`.
//!    The rule is the protocol's, so it is asserted on a second operation too —
//!    `DescribeAutoScalingGroups`, whose instance requirements state their limits as
//!    doubles.
//!
//! Each table spans what its rule generalises over — the policy type, which switch the put
//! states and which it leaves out, the value's sign, magnitude and precision, and the
//! operation reporting it — rather than one recorded row, and every assertion names its
//! case and the value actually reported.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch_service;

use crate::support::{borrowed, new_engine, p, query, tag_value, tag_values};

/// The launch source every group in this suite is created from — a launch TEMPLATE,
/// because no account can create a launch configuration.
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-projection");

/// The group every policy in this suite belongs to.
const GROUP: &str = "asg-projection";

/// A router carrying Amazon EC2 Auto Scaling and CloudWatch on ONE engine: putting a
/// target-tracking policy materialises its alarms in CloudWatch's own store, so that
/// service has to be mounted beside this one for the put to reach it.
fn router() -> MultiServiceRouter {
    let engine = new_engine();
    let mut router = MultiServiceRouter::new();
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(cloudwatch_service::EmulatedService::new(engine));
    router
}

/// Create the group every policy here is put against.
async fn create_group(router: &MultiServiceRouter) {
    let (status, body) = query(
        router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", GROUP),
            LAUNCH_SOURCE,
            ("MinSize", "0"),
            ("MaxSize", "4"),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the group must be created, got {status} with {body:?}"
    );
}

/// Put one policy against that group and answer the body of the read-back.
async fn put_and_read(
    router: &MultiServiceRouter,
    case: &str,
    policy: &str,
    extra: &[(String, String)],
) -> String {
    let mut params = vec![p("AutoScalingGroupName", GROUP), p("PolicyName", policy)];
    params.extend_from_slice(extra);
    let (status, body) = query(router, "PutScalingPolicy", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the put must succeed, got {status} with {body:?}"
    );
    read(router, case, policy).await
}

/// Read one policy back and answer the response body.
async fn read(router: &MultiServiceRouter, case: &str, policy: &str) -> String {
    let (status, body) = query(
        router,
        "DescribePolicies",
        &[
            ("AutoScalingGroupName", GROUP),
            ("PolicyNames.member.1", policy),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the read-back must succeed, got {status} with {body:?}"
    );
    body
}

/// The members one case's request carries beyond the group and policy names.
type Params = Vec<(String, String)>;

/// One row of the ECHO table: the case, the put that creates the policy, the member the
/// read reports, and the put parameter that member travels back in.
type EchoCase = (&'static str, Params, &'static str, &'static str);

/// One row of the COLLECTION table: the case, the put, whether the policy it creates holds
/// steps, and whether it holds alarms.
type CollectionCase = (&'static str, Params, bool, bool);

/// One row of the resolution table: the members the put carries, and the members the
/// read-back must report with them.
struct Resolved {
    case: &'static str,
    params: Params,
    /// `(member, reported value)` the read must answer — a value the put stated, or the
    /// default the service resolved for a member it did not.
    reported: Vec<(&'static str, &'static str)>,
}

/// A target-tracking put on a predefined metric, tracking `target_value`.
fn target_tracking(target_value: &str) -> Params {
    vec![
        p("PolicyType", "TargetTrackingScaling"),
        p(
            "TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType",
            "ASGAverageCPUUtilization",
        ),
        p("TargetTrackingConfiguration.TargetValue", target_value),
    ]
}

/// A step-scaling put with one open-ended step from `lower_bound` up.
fn step_scaling(lower_bound: &str) -> Params {
    vec![
        p("PolicyType", "StepScaling"),
        p("AdjustmentType", "ChangeInCapacity"),
        p("StepAdjustments.member.1.MetricIntervalLowerBound", lower_bound),
        p("StepAdjustments.member.1.ScalingAdjustment", "1"),
    ]
}

/// A predictive-scaling put on a predefined metric pair, targeting `target_value`.
fn predictive_scaling(target_value: &str) -> Params {
    vec![
        p("PolicyType", "PredictiveScaling"),
        p(
            "PredictiveScalingConfiguration.MetricSpecifications.member.1.TargetValue",
            target_value,
        ),
        p(
            "PredictiveScalingConfiguration.MetricSpecifications.member.1\
             .PredefinedMetricPairSpecification.PredefinedMetricType",
            "ASGCPUUtilization",
        ),
    ]
}

/// Every switch of a policy whose default the model documents, each exercised both ways:
/// the put that states it and the put that leaves it to the service.
///
/// The documented defaults, from this service's own model
/// (`smithy_query(query="shape",shape="TargetTrackingConfiguration,PredictiveScalingConfiguration")`
/// and `smithy_query(query="doc",shape="PutScalingPolicyType",member="MetricAggregationType")`):
/// `DisableScaleIn` "The default is false", `Mode` "Defaults to ForecastOnly if not
/// specified", `MaxCapacityBreachBehavior` "Defaults to HonorMaxCapacity if not
/// specified", `MetricAggregationType` "If the aggregation type is null, the value is
/// treated as Average". The values a row STATES are the other member of each enumerated
/// set, so no row's expectation is also its default.
fn resolution_rows() -> Vec<Resolved> {
    vec![
        Resolved {
            case: "a target-tracking put that names no DisableScaleIn",
            params: target_tracking("40"),
            reported: vec![("DisableScaleIn", "false")],
        },
        Resolved {
            case: "a target-tracking put that disables scaling in",
            params: {
                let mut params = target_tracking("40");
                params.push(p("TargetTrackingConfiguration.DisableScaleIn", "true"));
                params
            },
            reported: vec![("DisableScaleIn", "true")],
        },
        Resolved {
            case: "a target-tracking put that states the default itself",
            params: {
                let mut params = target_tracking("40");
                params.push(p("TargetTrackingConfiguration.DisableScaleIn", "false"));
                params
            },
            reported: vec![("DisableScaleIn", "false")],
        },
        Resolved {
            case: "a step-scaling put that names no MetricAggregationType",
            params: step_scaling("0"),
            reported: vec![("MetricAggregationType", "Average")],
        },
        Resolved {
            case: "a step-scaling put that names one of the other aggregation types",
            params: {
                let mut params = step_scaling("0");
                params.push(p("MetricAggregationType", "Minimum"));
                params
            },
            reported: vec![("MetricAggregationType", "Minimum")],
        },
        Resolved {
            case: "a step-scaling put that names the remaining aggregation type",
            params: {
                let mut params = step_scaling("0");
                params.push(p("MetricAggregationType", "Maximum"));
                params
            },
            reported: vec![("MetricAggregationType", "Maximum")],
        },
        Resolved {
            case: "a predictive put that names neither of its switches",
            params: predictive_scaling("30"),
            reported: vec![
                ("Mode", "ForecastOnly"),
                ("MaxCapacityBreachBehavior", "HonorMaxCapacity"),
            ],
        },
        Resolved {
            case: "a predictive put that names both of them",
            params: {
                let mut params = predictive_scaling("30");
                params.push(p("PredictiveScalingConfiguration.Mode", "ForecastAndScale"));
                params.push(p(
                    "PredictiveScalingConfiguration.MaxCapacityBreachBehavior",
                    "IncreaseMaxCapacity",
                ));
                // "Required if the MaxCapacityBreachBehavior property is set to
                // IncreaseMaxCapacity" (`PredictiveScalingConfiguration$MaxCapacityBuffer`).
                params.push(p("PredictiveScalingConfiguration.MaxCapacityBuffer", "10"));
                params
            },
            reported: vec![
                ("Mode", "ForecastAndScale"),
                ("MaxCapacityBreachBehavior", "IncreaseMaxCapacity"),
            ],
        },
        Resolved {
            case: "a predictive put that names only the capacity behaviour",
            params: {
                let mut params = predictive_scaling("30");
                params.push(p(
                    "PredictiveScalingConfiguration.MaxCapacityBreachBehavior",
                    "IncreaseMaxCapacity",
                ));
                params.push(p("PredictiveScalingConfiguration.MaxCapacityBuffer", "0"));
                params
            },
            reported: vec![
                ("Mode", "ForecastOnly"),
                ("MaxCapacityBreachBehavior", "IncreaseMaxCapacity"),
            ],
        },
        Resolved {
            case: "a simple-scaling put, which owns none of those switches",
            params: vec![
                p("AdjustmentType", "ChangeInCapacity"),
                p("ScalingAdjustment", "1"),
            ],
            reported: vec![("PolicyType", "SimpleScaling"), ("Enabled", "true")],
        },
    ]
}

/// RULE 1, the switch half: a member whose default the model documents is reported with
/// that default when the put named none, and with the stated value when it did.
#[tokio::test]
async fn a_read_reports_the_switches_the_service_resolved() {
    let router = router();
    create_group(&router).await;

    for (index, row) in resolution_rows().into_iter().enumerate() {
        let policy = format!("resolved-{index}");
        let body = put_and_read(&router, row.case, &policy, &row.params).await;
        for (member, expected) in &row.reported {
            let reported = tag_value(&body, member);
            assert_eq!(
                reported, *expected,
                "case {:?}: expected {member} to be reported as {expected:?}, got \
                 {reported:?} in {body:?}",
                row.case
            );
        }
    }
}

/// RULE 1, the idempotence half: echoing a described policy back into `PutScalingPolicy` —
/// the documented update idiom — reports the same policy again, because what the client
/// read back was the policy in force rather than its own request.
#[tokio::test]
async fn echoing_a_described_policy_back_changes_nothing() {
    let router = router();
    create_group(&router).await;

    // (case, the put that creates the policy, the member to echo, where the echoed member
    // travels in the put).
    let rows: [EchoCase; 3] = [
        (
            "the aggregation type of a step policy",
            step_scaling("0"),
            "MetricAggregationType",
            "MetricAggregationType",
        ),
        (
            "the scale-in switch of a target-tracking policy",
            target_tracking("40"),
            "DisableScaleIn",
            "TargetTrackingConfiguration.DisableScaleIn",
        ),
        (
            "the mode of a predictive policy",
            predictive_scaling("30"),
            "Mode",
            "PredictiveScalingConfiguration.Mode",
        ),
    ];

    for (index, (case, params, member, parameter)) in rows.into_iter().enumerate() {
        let policy = format!("echoed-{index}");
        let body = put_and_read(&router, case, &policy, &params).await;
        let first = tag_value(&body, member);
        assert!(
            !first.is_empty(),
            "case {case:?}: expected {member} to be reported at all, got {body:?}"
        );

        // The echo: the same request, now carrying the member the read reported.
        let mut echoed = params.clone();
        echoed.push(p(parameter, first.clone()));
        let body = put_and_read(&router, case, &policy, &echoed).await;
        let second = tag_value(&body, member);
        assert_eq!(
            second, first,
            "case {case:?}: echoing {member} back must report it unchanged, expected \
             {first:?}, got {second:?}"
        );
    }
}

/// RULE 1, the collection half: both collections the `ScalingPolicy` shape declares are
/// reported for every policy type, carrying what the policy holds — nothing for a policy
/// that holds none, rather than an absent member.
#[tokio::test]
async fn a_read_reports_every_collection_the_policy_shape_declares() {
    let router = router();
    create_group(&router).await;

    // (case, the put, whether the policy holds steps, whether it holds alarms) — one row
    // per policy type, so each collection is seen both empty and populated.
    let rows: [CollectionCase; 4] = [
        (
            "a simple-scaling policy holds neither steps nor alarms",
            vec![
                p("AdjustmentType", "ChangeInCapacity"),
                p("ScalingAdjustment", "1"),
            ],
            false,
            false,
        ),
        (
            "a step-scaling policy holds steps and no alarms",
            step_scaling("0"),
            true,
            false,
        ),
        (
            "a target-tracking policy holds alarms and no steps",
            target_tracking("40"),
            false,
            true,
        ),
        (
            "a predictive policy holds neither",
            predictive_scaling("30"),
            false,
            false,
        ),
    ];

    for (index, (case, params, holds_steps, holds_alarms)) in rows.into_iter().enumerate() {
        let policy = format!("collections-{index}");
        let body = put_and_read(&router, case, &policy, &params).await;
        for (member, holds) in [("StepAdjustments", holds_steps), ("Alarms", holds_alarms)] {
            let reported = tag_values(&body, member);
            assert_eq!(
                reported.len(),
                1,
                "case {case:?}: expected exactly one {member} member, got {reported:?} in \
                 {body:?}"
            );
            let populated = !reported[0].is_empty();
            assert_eq!(
                populated, holds,
                "case {case:?}: expected {member} to be populated == {holds}, got \
                 {reported:?}"
            );
        }
    }
}

/// RULE 2: every double a described policy carries is written with a fractional part.
#[tokio::test]
async fn a_described_policy_renders_its_doubles_as_decimals() {
    let router = router();
    create_group(&router).await;

    // (case, the value sent, the text the wire must carry) — the rows differ in sign,
    // magnitude and how many fractional digits the value already has: a whole value grows
    // one, a fractional value keeps the digits it has.
    let target_values: [(&str, &str, &str); 6] = [
        ("a whole single digit", "1", "1.0"),
        ("the recorded whole target", "50", "50.0"),
        ("a whole three-digit value", "100", "100.0"),
        ("a sub-unit fraction", "0.5", "0.5"),
        ("a value with two decimals", "12.75", "12.75"),
        ("a value with many digits", "1234567.89", "1234567.89"),
    ];
    for (index, (case, sent, expected)) in target_values.into_iter().enumerate() {
        let policy = format!("target-value-{index}");
        let body = put_and_read(&router, case, &policy, &target_tracking(sent)).await;
        let configuration = tag_value(&body, "TargetTrackingConfiguration");
        let reported = tag_value(&configuration, "TargetValue");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the target value {sent:?} to be reported as \
             {expected:?}, got {reported:?} in {body:?}"
        );
    }

    // The same rule on the bounds of a step, which are the doubles that take a NEGATIVE
    // value: a step below the alarm threshold has a negative upper bound.
    let bounds: [(&str, &str, &str, &str); 5] = [
        ("a lower bound at the threshold", "MetricIntervalLowerBound", "0", "0.0"),
        ("a whole lower bound", "MetricIntervalLowerBound", "10", "10.0"),
        ("a fractional lower bound", "MetricIntervalLowerBound", "2.5", "2.5"),
        ("a whole negative upper bound", "MetricIntervalUpperBound", "-10", "-10.0"),
        (
            "a fractional negative upper bound",
            "MetricIntervalUpperBound",
            "-2.5",
            "-2.5",
        ),
    ];
    for (index, (case, member, sent, expected)) in bounds.into_iter().enumerate() {
        let policy = format!("step-bound-{index}");
        let params = vec![
            p("PolicyType", "StepScaling"),
            p("AdjustmentType", "ChangeInCapacity"),
            p(&format!("StepAdjustments.member.1.{member}"), sent),
            p("StepAdjustments.member.1.ScalingAdjustment", "1"),
        ];
        let body = put_and_read(&router, case, &policy, &params).await;
        let reported = tag_value(&body, member);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {member} {sent:?} to be reported as {expected:?}, \
             got {reported:?} in {body:?}"
        );
    }

    // And on a predictive policy's target utilization, the third shape of this operation
    // that states a double.
    for (index, (case, sent, expected)) in [
        ("a whole predictive target", "30", "30.0"),
        ("a fractional predictive target", "30.5", "30.5"),
    ]
    .into_iter()
    .enumerate()
    {
        let policy = format!("predictive-target-{index}");
        let body = put_and_read(&router, case, &policy, &predictive_scaling(sent)).await;
        let configuration = tag_value(&body, "PredictiveScalingConfiguration");
        let reported = tag_value(&configuration, "TargetValue");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the predictive target {sent:?} to be reported as \
             {expected:?}, got {reported:?} in {body:?}"
        );
    }
}

/// RULE 2 is the PROTOCOL's, not one operation's: a group's instance requirements state
/// their limits as doubles (`NullablePositiveDouble`), and `DescribeAutoScalingGroups`
/// renders them the same way — while the INTEGER limits beside them keep their integer
/// form, which is what AWS answers for an integer member.
#[tokio::test]
async fn a_described_group_renders_its_instance_requirement_doubles_as_decimals() {
    let router = router();

    let group = "asg-instance-requirements";
    let prefix = "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.InstanceRequirements";
    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", group),
            ("MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification\
              .LaunchTemplateName", "lt-projection"),
            ("MinSize", "0"),
            ("MaxSize", "4"),
            ("AvailabilityZones.member.1", "us-west-2a"),
            (&format!("{prefix}.VCpuCount.Min"), "2"),
            (&format!("{prefix}.VCpuCount.Max"), "8"),
            (&format!("{prefix}.MemoryMiB.Min"), "1024"),
            (&format!("{prefix}.MemoryGiBPerVCpu.Min"), "0.5"),
            (&format!("{prefix}.MemoryGiBPerVCpu.Max"), "4"),
            (&format!("{prefix}.TotalLocalStorageGB.Max"), "100"),
            (&format!("{prefix}.NetworkBandwidthGbps.Min"), "1"),
            (&format!("{prefix}.NetworkBandwidthGbps.Max"), "12.5"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the group must be created, got {status} with {body:?}"
    );

    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", group)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the group must be reported, got {status} with {body:?}"
    );

    // (case, the limit's structure, the member, the text the wire must carry) — the three
    // double-valued limits, plus the integer ones that must NOT grow a fractional part.
    let rows: [(&str, &str, &str, &str); 6] = [
        ("a sub-unit double limit", "MemoryGiBPerVCpu", "Min", "0.5"),
        ("a whole double limit", "MemoryGiBPerVCpu", "Max", "4.0"),
        ("a three-digit double limit", "TotalLocalStorageGB", "Max", "100.0"),
        ("a whole bandwidth limit", "NetworkBandwidthGbps", "Min", "1.0"),
        ("a fractional bandwidth limit", "NetworkBandwidthGbps", "Max", "12.5"),
        ("an INTEGER limit, unchanged", "VCpuCount", "Min", "2"),
    ];
    for (case, structure, member, expected) in rows {
        let section = tag_value(&body, structure);
        let reported = tag_value(&section, member);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {structure}.{member} to be reported as {expected:?}, \
             got {reported:?} in {body:?}"
        );
    }
}
