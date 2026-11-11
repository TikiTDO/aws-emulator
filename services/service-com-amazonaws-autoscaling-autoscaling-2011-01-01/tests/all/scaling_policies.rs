// SPDX-License-Identifier: CC0-1.0

//! The scaling policies of an Auto Scaling group, over the awsQuery wire: what
//! `PutScalingPolicy` creates and `DescribePolicies` reports back.
//!
//! The rules under test:
//!
//! 1. A put against a stored group creates (or updates) a policy and answers its
//!    `PolicyARN`; the policy is then reported by `DescribePolicies` carrying exactly the
//!    members its `PolicyType` owns — the model documents `Cooldown` as "Valid only if
//!    the policy type is SimpleScaling", `ScalingAdjustment` and `StepAdjustments` as
//!    "Not used with any other policy type", `MetricAggregationType` as the step policy's
//!    own and `EstimatedInstanceWarmup` as "Valid only if the policy type is
//!    TargetTrackingScaling or StepScaling" — and every value it carries is reported as
//!    it was sent, whatever its magnitude or precision.
//! 2. Putting the same policy name again UPDATES that policy: the ARN and the creation
//!    identity survive, the members are replaced.
//! 3. A request that cannot describe a policy is refused with HTTP 400 `ValidationError`
//!    carrying the sentence the service states for that class, and NOTHING is stored.
//! 4. "The CloudWatch alarms created for the target tracking scaling policy"
//!    (`PolicyARNType$Alarms`): a target-tracking policy's alarms exist in the account's
//!    own CloudWatch store, so CloudWatch's own `DescribeAlarms` resolves every alarm the
//!    policy reports — and `DisableScaleIn` withholds the scale-in alarm, because a
//!    policy that cannot remove an instance owes no alarm that would ask it to.
//! 5. A policy only exists against a group that exists: deleting the group deletes its
//!    policies and retires their alarms.
//!
//! Each table spans what its rule generalises over (the policy type, the member set, the
//! value's magnitude and precision, the filter, the page size) rather than one recorded
//! row, and every assertion names its case and the value actually reported.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch_service;

use crate::support::{borrowed, new_engine, p, query, tag_value, tag_values, xml_unescape, REGION};

/// The launch source every group in this suite is created from — a launch TEMPLATE,
/// because no account can create a launch configuration (see
/// `launch_configurations_are_unavailable.rs`).
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-1");

/// The `x-amz-target` prefix CloudWatch's awsJson protocol names its operations under.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// The members one case's request carries beyond the group and policy names.
type Params = Vec<(String, String)>;

/// One re-put of a policy: the case, the members it carries, the members the read-back
/// must then report, and the members that must NOT survive the update.
type UpdateCase<'a> = (&'a str, Params, Vec<(&'a str, &'a str)>, Vec<&'a str>);

/// One target-tracking put: the case, the members it carries, and how many alarms the
/// policy it creates owes.
type AlarmCase<'a> = (&'a str, Params, usize);

/// One read: the case, the filter it carries, and the policy names it must report.
type SelectionCase<'a> = (&'a str, Params, Vec<&'a str>);

/// A router carrying Amazon EC2 Auto Scaling and CloudWatch on ONE engine: the alarms a
/// target-tracking policy mints live in CloudWatch's own store, so the readback goes
/// through CloudWatch's own API.
fn router() -> MultiServiceRouter {
    let engine = new_engine();
    let mut router = MultiServiceRouter::new();
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(cloudwatch_service::EmulatedService::new(Arc::clone(&engine)));
    router
}

/// Create one group and assert the create succeeded.
async fn create_group(router: &MultiServiceRouter, name: &str) {
    let (status, body) = query(
        router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", name),
            LAUNCH_SOURCE,
            ("MinSize", "0"),
            ("MaxSize", "4"),
            ("DesiredCapacity", "0"),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the group {name:?} must be created, got {status} with {body:?}"
    );
}

/// POST one `PutScalingPolicy` and answer its status and body.
async fn put_policy(
    router: &MultiServiceRouter,
    group: &str,
    policy: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = vec![
        p("AutoScalingGroupName", group),
        p("PolicyName", policy),
    ];
    params.extend_from_slice(extra);
    query(router, "PutScalingPolicy", &borrowed(&params)).await
}

/// POST one `DescribePolicies` and answer its status and body.
async fn describe_policies(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    query(router, "DescribePolicies", &borrowed(params)).await
}

/// The wire code and the unescaped message of a refusal.
fn refusal(body: &str) -> (String, String) {
    (
        tag_value(body, "Code"),
        xml_unescape(&tag_value(body, "Message")),
    )
}

/// POST one `awsJson1_0` CloudWatch call and answer its parsed body, requiring HTTP 200.
async fn cloudwatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://monitoring.{REGION}.amazonaws.com/"))
        .header("host", format!("monitoring.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/monitoring/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("a serializable request"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The alarm names CloudWatch's own store resolves for `names`, in the order it reports
/// them.
async fn resolved_alarm_names(router: &MultiServiceRouter, names: &[String]) -> Vec<String> {
    let answer = cloudwatch(
        router,
        "DescribeAlarms",
        &json!({"AlarmNames": names, "AlarmTypes": ["MetricAlarm"]}),
    )
    .await;
    answer
        .pointer("/MetricAlarms")
        .and_then(Value::as_array)
        .map(|alarms| {
            alarms
                .iter()
                .filter_map(|alarm| alarm.get("AlarmName").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// One accepted put: the case, the members it carries beyond the group and policy names,
/// the members the read-back must report, and the members it must NOT report because the
/// policy's type does not own them.
struct AcceptedCase {
    case: &'static str,
    params: Vec<(String, String)>,
    reported: Vec<(&'static str, String)>,
    withheld: Vec<&'static str>,
}

/// A simple-scaling put, which is also the put that names no `PolicyType` at all.
fn simple_scaling_params(adjustment_type: &str, adjustment: &str) -> Vec<(String, String)> {
    vec![
        p("PolicyType", "SimpleScaling"),
        p("AdjustmentType", adjustment_type),
        p("ScalingAdjustment", adjustment),
    ]
}

/// Every policy type, each with the members ITS type owns and a value set that differs in
/// magnitude, sign and precision from the others — plus the puts that exercise the
/// members a type does NOT own.
fn accepted_cases() -> Vec<AcceptedCase> {
    vec![
        AcceptedCase {
            case: "a simple scaling policy with no PolicyType at all, which is the default type",
            params: vec![
                p("AdjustmentType", "ChangeInCapacity"),
                p("ScalingAdjustment", "1"),
            ],
            reported: vec![
                ("PolicyType", "SimpleScaling".to_string()),
                ("AdjustmentType", "ChangeInCapacity".to_string()),
                ("ScalingAdjustment", "1".to_string()),
                ("Enabled", "true".to_string()),
            ],
            withheld: vec!["Cooldown", "StepAdjustments", "TargetTrackingConfiguration"],
        },
        AcceptedCase {
            case: "a simple scaling policy that scales in, with the smallest cooldown",
            params: {
                let mut params = simple_scaling_params("ChangeInCapacity", "-2");
                params.push(p("Cooldown", "0"));
                params
            },
            reported: vec![
                ("ScalingAdjustment", "-2".to_string()),
                ("Cooldown", "0".to_string()),
            ],
            withheld: vec!["StepAdjustments", "MetricAggregationType"],
        },
        AcceptedCase {
            case: "a simple scaling policy with a day-long cooldown and an exact capacity",
            params: {
                let mut params = simple_scaling_params("ExactCapacity", "3");
                params.push(p("Cooldown", "86400"));
                params
            },
            reported: vec![
                ("AdjustmentType", "ExactCapacity".to_string()),
                ("Cooldown", "86400".to_string()),
            ],
            withheld: vec!["EstimatedInstanceWarmup"],
        },
        AcceptedCase {
            case: "a percent-change simple scaling policy with a minimum magnitude",
            params: {
                let mut params = simple_scaling_params("PercentChangeInCapacity", "25");
                params.push(p("MinAdjustmentMagnitude", "2"));
                params
            },
            reported: vec![
                ("AdjustmentType", "PercentChangeInCapacity".to_string()),
                ("MinAdjustmentMagnitude", "2".to_string()),
            ],
            withheld: vec!["Cooldown"],
        },
        AcceptedCase {
            case: "a step scaling policy with one open-ended step",
            params: vec![
                p("PolicyType", "StepScaling"),
                p("AdjustmentType", "ChangeInCapacity"),
                p("StepAdjustments.member.1.MetricIntervalLowerBound", "0"),
                p("StepAdjustments.member.1.ScalingAdjustment", "1"),
                p("MetricAggregationType", "Maximum"),
                p("EstimatedInstanceWarmup", "90"),
            ],
            reported: vec![
                ("PolicyType", "StepScaling".to_string()),
                ("MetricAggregationType", "Maximum".to_string()),
                ("EstimatedInstanceWarmup", "90".to_string()),
                ("MetricIntervalLowerBound", "0".to_string()),
            ],
            // A step policy honours no cooldown of its own, so none is reported even
            // though the request carried one.
            withheld: vec!["Cooldown", "TargetTrackingConfiguration"],
        },
        AcceptedCase {
            case: "a step scaling policy with three steps covering the whole metric range",
            params: vec![
                p("PolicyType", "StepScaling"),
                p("AdjustmentType", "ChangeInCapacity"),
                p("Cooldown", "300"),
                p("StepAdjustments.member.1.MetricIntervalUpperBound", "-10"),
                p("StepAdjustments.member.1.ScalingAdjustment", "-2"),
                p("StepAdjustments.member.2.MetricIntervalLowerBound", "-10"),
                p("StepAdjustments.member.2.MetricIntervalUpperBound", "10"),
                p("StepAdjustments.member.2.ScalingAdjustment", "0"),
                p("StepAdjustments.member.3.MetricIntervalLowerBound", "10"),
                p("StepAdjustments.member.3.ScalingAdjustment", "3"),
            ],
            reported: vec![("PolicyType", "StepScaling".to_string())],
            withheld: vec!["Cooldown"],
        },
        AcceptedCase {
            case: "a target tracking policy on a predefined metric, with a whole target",
            params: vec![
                p("PolicyType", "TargetTrackingScaling"),
                p(
                    "TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType",
                    "ASGAverageCPUUtilization",
                ),
                p("TargetTrackingConfiguration.TargetValue", "50"),
            ],
            reported: vec![
                ("PolicyType", "TargetTrackingScaling".to_string()),
                ("PredefinedMetricType", "ASGAverageCPUUtilization".to_string()),
                ("TargetValue", "50".to_string()),
            ],
            withheld: vec!["AdjustmentType", "ScalingAdjustment", "Cooldown"],
        },
        AcceptedCase {
            case: "a target tracking policy whose target value carries decimals",
            params: vec![
                p("PolicyType", "TargetTrackingScaling"),
                p(
                    "TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType",
                    "ASGAverageNetworkIn",
                ),
                p("TargetTrackingConfiguration.TargetValue", "1234567.89"),
                p("TargetTrackingConfiguration.DisableScaleIn", "true"),
            ],
            reported: vec![
                ("TargetValue", "1234567.89".to_string()),
                ("DisableScaleIn", "true".to_string()),
            ],
            withheld: vec!["StepAdjustments"],
        },
        AcceptedCase {
            case: "a target tracking policy on a customized metric, disabled",
            params: vec![
                p("PolicyType", "TargetTrackingScaling"),
                p(
                    "TargetTrackingConfiguration.CustomizedMetricSpecification.Namespace",
                    "Acme/Queue",
                ),
                p(
                    "TargetTrackingConfiguration.CustomizedMetricSpecification.MetricName",
                    "BacklogPerInstance",
                ),
                p(
                    "TargetTrackingConfiguration.CustomizedMetricSpecification.Statistic",
                    "Sum",
                ),
                p("TargetTrackingConfiguration.TargetValue", "0.5"),
                p("Enabled", "false"),
            ],
            reported: vec![
                ("Namespace", "Acme/Queue".to_string()),
                ("MetricName", "BacklogPerInstance".to_string()),
                ("Statistic", "Sum".to_string()),
                ("TargetValue", "0.5".to_string()),
                ("Enabled", "false".to_string()),
            ],
            withheld: vec!["PredefinedMetricType"],
        },
        AcceptedCase {
            case: "a predictive scaling policy on a predefined metric pair",
            params: vec![
                p("PolicyType", "PredictiveScaling"),
                p(
                    "PredictiveScalingConfiguration.MetricSpecifications.member.1.TargetValue",
                    "40",
                ),
                p(
                    "PredictiveScalingConfiguration.MetricSpecifications.member.1\
                     .PredefinedMetricPairSpecification.PredefinedMetricType",
                    "ASGCPUUtilization",
                ),
                p("PredictiveScalingConfiguration.Mode", "ForecastOnly"),
                p("PredictiveScalingConfiguration.SchedulingBufferTime", "300"),
            ],
            reported: vec![
                ("PolicyType", "PredictiveScaling".to_string()),
                ("Mode", "ForecastOnly".to_string()),
                ("SchedulingBufferTime", "300".to_string()),
                ("TargetValue", "40".to_string()),
            ],
            withheld: vec!["AdjustmentType", "Cooldown", "TargetTrackingConfiguration"],
        },
    ]
}

/// RULE 1: every policy type round-trips through `DescribePolicies` carrying exactly the
/// members its type owns, with every value reported as it was sent.
#[tokio::test]
async fn every_policy_type_round_trips_with_the_members_its_type_owns() {
    let router = router();
    create_group(&router, "asg-policies").await;

    for (index, expected) in accepted_cases().into_iter().enumerate() {
        let case = expected.case;
        let policy_name = format!("policy-{index}");
        let (status, body) =
            put_policy(&router, "asg-policies", &policy_name, &expected.params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        let policy_arn = tag_value(&body, "PolicyARN");
        let arn_prefix = format!("arn:aws:autoscaling:{REGION}:");
        assert!(
            policy_arn.starts_with(&arn_prefix)
                && policy_arn.ends_with(&format!(
                    ":autoScalingGroupName/asg-policies:policyName/{policy_name}"
                )),
            "case {case:?}: expected a scaling-policy ARN of the group and policy, got \
             {policy_arn:?}"
        );

        // The read-back through the implemented reader: the policy is STATE, not just an
        // accepted request.
        let (status, body) = describe_policies(
            &router,
            &[
                p("AutoScalingGroupName", "asg-policies"),
                p("PolicyNames.member.1", policy_name.clone()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read-back must succeed, got {status} with {body:?}"
        );
        let reported_names = tag_values(&body, "PolicyName");
        assert_eq!(
            reported_names,
            vec![policy_name.clone()],
            "case {case:?}: expected exactly the policy {policy_name:?}, got {reported_names:?}"
        );
        assert_eq!(
            tag_value(&body, "PolicyARN"),
            policy_arn,
            "case {case:?}: the reported ARN must be the one the put answered, got {body:?}"
        );
        for (member, value) in &expected.reported {
            let reported = tag_value(&body, member);
            assert_eq!(
                &reported, value,
                "case {case:?}: expected {member} to be reported as {value:?}, got \
                 {reported:?} in {body:?}"
            );
        }
        for member in &expected.withheld {
            let reported = tag_values(&body, member);
            assert!(
                reported.is_empty(),
                "case {case:?}: {member} is not part of this policy's type, so it must not \
                 be reported, got {reported:?}"
            );
        }
    }
}

/// RULE 1, the value half: a value a policy carries is reported as it was sent, whatever
/// its magnitude or precision — the cooldown of a simple policy and the target value of a
/// target-tracking one, across the span each admits.
#[tokio::test]
async fn a_policys_numeric_values_are_reported_as_they_were_sent() {
    let router = router();
    create_group(&router, "asg-precision").await;

    // `Cooldown` counts seconds and the model constrains it only to be an integer, so the
    // rows span its whole span: the floor the service accepts, a minute, the documented
    // default, and a day.
    for cooldown in ["0", "1", "60", "300", "86400"] {
        let policy_name = format!("cooldown-{cooldown}");
        let mut params = simple_scaling_params("ChangeInCapacity", "1");
        params.push(p("Cooldown", cooldown));
        let (status, body) = put_policy(&router, "asg-precision", &policy_name, &params).await;
        assert_eq!(
            status, 200,
            "case {policy_name:?}: expected HTTP 200, got {status} with {body:?}"
        );
        let (_, body) = describe_policies(
            &router,
            &[p("PolicyNames.member.1", policy_name.clone())],
        )
        .await;
        let reported = tag_value(&body, "Cooldown");
        assert_eq!(
            reported, cooldown,
            "case {policy_name:?}: expected the cooldown {cooldown:?} back, got {reported:?}"
        );
    }

    // `TargetValue` is a bare double, so the rows span the window the service tracks:
    // each published bound, a sub-unit fraction, a whole number and a value with decimals.
    let target_values = [
        ("the smallest target value the service tracks", "8.51592E-12"),
        ("a sub-unit fraction", "0.5"),
        ("a whole number", "50"),
        ("a value carrying decimals", "1234567.89"),
        ("the largest target value the service tracks", "1.174271E17"),
    ];
    for (case, target_value) in target_values {
        let policy_name = format!("target-{target_value}");
        let params = vec![
            p("PolicyType", "TargetTrackingScaling"),
            p(
                "TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType",
                "ASGAverageCPUUtilization",
            ),
            p("TargetTrackingConfiguration.TargetValue", target_value),
        ];
        let (status, body) = put_policy(&router, "asg-precision", &policy_name, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        let (_, body) = describe_policies(
            &router,
            &[p("PolicyNames.member.1", policy_name.clone())],
        )
        .await;
        let reported: f64 = tag_value(&body, "TargetValue").parse().unwrap_or(f64::NAN);
        let sent: f64 = target_value.parse().expect("a parseable target value");
        assert!(
            (reported - sent).abs() <= sent.abs() * 1e-9,
            "case {case:?}: expected the target value {sent} back, got {reported} in {body:?}"
        );
    }
}

/// RULE 2: putting the same policy name again updates that policy — the ARN survives and
/// the members are the new ones, including the type's own.
#[tokio::test]
async fn putting_a_policy_again_updates_it_and_keeps_its_arn() {
    let router = router();
    create_group(&router, "asg-update").await;

    let first = simple_scaling_params("ChangeInCapacity", "1");
    let (status, body) = put_policy(&router, "asg-update", "steady", &first).await;
    assert_eq!(status, 200, "the first put must succeed, got {body:?}");
    let original_arn = tag_value(&body, "PolicyARN");

    // Each row re-puts the SAME name with a different shape, so the update is exercised
    // across a member change, a type change and a value change.
    let updates: Vec<UpdateCase<'_>> = vec![
        (
            "a new adjustment of the same type",
            {
                let mut params = simple_scaling_params("ExactCapacity", "4");
                params.push(p("Cooldown", "120"));
                params
            },
            vec![
                ("AdjustmentType", "ExactCapacity"),
                ("ScalingAdjustment", "4"),
                ("Cooldown", "120"),
            ],
            vec![],
        ),
        (
            "a different policy type, which drops the members the old type owned",
            vec![
                p("PolicyType", "TargetTrackingScaling"),
                p(
                    "TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType",
                    "ASGAverageNetworkOut",
                ),
                p("TargetTrackingConfiguration.TargetValue", "12.5"),
            ],
            vec![
                ("PolicyType", "TargetTrackingScaling"),
                ("TargetValue", "12.5"),
            ],
            vec!["AdjustmentType", "ScalingAdjustment", "Cooldown"],
        ),
        (
            "back to a step policy",
            vec![
                p("PolicyType", "StepScaling"),
                p("AdjustmentType", "ChangeInCapacity"),
                p("StepAdjustments.member.1.MetricIntervalLowerBound", "0"),
                p("StepAdjustments.member.1.ScalingAdjustment", "2"),
            ],
            vec![("PolicyType", "StepScaling")],
            vec!["TargetTrackingConfiguration", "Cooldown"],
        ),
    ];

    for (case, params, reported, withheld) in updates {
        let (status, body) = put_policy(&router, "asg-update", "steady", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update must succeed, got {status} with {body:?}"
        );
        assert_eq!(
            tag_value(&body, "PolicyARN"),
            original_arn,
            "case {case:?}: an update keeps the policy's ARN, got {body:?}"
        );

        let (_, body) = describe_policies(
            &router,
            &[p("AutoScalingGroupName", "asg-update")],
        )
        .await;
        let names = tag_values(&body, "PolicyName");
        assert_eq!(
            names,
            vec!["steady".to_string()],
            "case {case:?}: an update must not add a second policy, got {names:?}"
        );
        for (member, value) in reported {
            let seen = tag_value(&body, member);
            assert_eq!(
                seen, value,
                "case {case:?}: expected {member} to be {value:?}, got {seen:?} in {body:?}"
            );
        }
        for member in withheld {
            let seen = tag_values(&body, member);
            assert!(
                seen.is_empty(),
                "case {case:?}: {member} must not survive the update, got {seen:?}"
            );
        }
    }
}

/// One refused put: the case, the members it carries, and the whole message the service
/// answers it with.
struct RefusedCase {
    case: &'static str,
    params: Vec<(String, String)>,
    message: String,
}

/// The sentence the service answers a member the request left null with.
fn member_must_not_be_null(member_path: &str) -> String {
    format!(
        "1 validation error detected: Value null at '{member_path}' failed to satisfy \
         constraint: Member must not be null"
    )
}

/// Every rule a put is refused by, each with the sentence the service states for it.
fn refused_cases() -> Vec<RefusedCase> {
    vec![
        RefusedCase {
            case: "an adjustment type the service does not serve",
            params: simple_scaling_params("NotAType", "1"),
            message: "Valid adjustment types are: [PercentChangeInCapacity, ChangeInCapacity, \
                      ExactCapacity]."
                .to_string(),
        },
        RefusedCase {
            case: "an adjustment type differing only in case",
            params: simple_scaling_params("changeincapacity", "1"),
            message: "Valid adjustment types are: [PercentChangeInCapacity, ChangeInCapacity, \
                      ExactCapacity]."
                .to_string(),
        },
        RefusedCase {
            case: "a step scaling policy with no step adjustments at all",
            params: vec![
                p("PolicyType", "StepScaling"),
                p("AdjustmentType", "ChangeInCapacity"),
            ],
            message: "You should specify at least one StepAdjustment with a StepScaling policy."
                .to_string(),
        },
        RefusedCase {
            case: "a negative cooldown",
            params: {
                let mut params = simple_scaling_params("ChangeInCapacity", "1");
                params.push(p("Cooldown", "-1"));
                params
            },
            message: "1 validation error detected: Value '-1' at 'cooldown' failed to satisfy \
                      constraint: Member must have value greater than or equal to 0"
                .to_string(),
        },
        RefusedCase {
            case: "a cooldown far below zero",
            params: {
                let mut params = simple_scaling_params("ChangeInCapacity", "1");
                params.push(p("Cooldown", "-86400"));
                params
            },
            message: "1 validation error detected: Value '-86400' at 'cooldown' failed to \
                      satisfy constraint: Member must have value greater than or equal to 0"
                .to_string(),
        },
        RefusedCase {
            case: "a policy type the service does not serve",
            params: vec![p("PolicyType", "ChangeInCapacity")],
            message: "Valid policy types are: [TargetTrackingScaling, StepScaling, \
                      SimpleScaling, PredictiveScaling]."
                .to_string(),
        },
        RefusedCase {
            case: "a metric aggregation type the service does not serve",
            params: vec![
                p("PolicyType", "StepScaling"),
                p("AdjustmentType", "ChangeInCapacity"),
                p("StepAdjustments.member.1.MetricIntervalLowerBound", "0"),
                p("StepAdjustments.member.1.ScalingAdjustment", "1"),
                p("MetricAggregationType", "Median"),
            ],
            message: "Valid metric aggregation types are: [Minimum, Maximum, Average].".to_string(),
        },
        RefusedCase {
            case: "a simple scaling policy with no adjustment type",
            params: vec![
                p("PolicyType", "SimpleScaling"),
                p("ScalingAdjustment", "1"),
            ],
            message: member_must_not_be_null("adjustmentType"),
        },
        RefusedCase {
            case: "a simple scaling policy with no scaling adjustment",
            params: vec![
                p("PolicyType", "SimpleScaling"),
                p("AdjustmentType", "ChangeInCapacity"),
            ],
            message: member_must_not_be_null("scalingAdjustment"),
        },
        RefusedCase {
            case: "a target tracking policy with no configuration",
            params: vec![p("PolicyType", "TargetTrackingScaling")],
            message: member_must_not_be_null("targetTrackingConfiguration"),
        },
        RefusedCase {
            case: "a predictive scaling policy with no configuration",
            params: vec![p("PolicyType", "PredictiveScaling")],
            message: member_must_not_be_null("predictiveScalingConfiguration"),
        },
        RefusedCase {
            case: "a target tracking configuration carrying BOTH metric specifications",
            params: vec![
                p("PolicyType", "TargetTrackingScaling"),
                p(
                    "TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType",
                    "ASGAverageCPUUtilization",
                ),
                p(
                    "TargetTrackingConfiguration.CustomizedMetricSpecification.Namespace",
                    "Acme/Queue",
                ),
                p(
                    "TargetTrackingConfiguration.CustomizedMetricSpecification.MetricName",
                    "Backlog",
                ),
                p(
                    "TargetTrackingConfiguration.CustomizedMetricSpecification.Statistic",
                    "Maximum",
                ),
                p("TargetTrackingConfiguration.TargetValue", "50"),
            ],
            message: "You should provide either a CustomizedMetricSpecification or a \
                      PredefinedMetricSpecification"
                .to_string(),
        },
        RefusedCase {
            case: "a target tracking configuration carrying NEITHER metric specification",
            params: vec![
                p("PolicyType", "TargetTrackingScaling"),
                p("TargetTrackingConfiguration.TargetValue", "50"),
            ],
            message: "You should provide either a CustomizedMetricSpecification or a \
                      PredefinedMetricSpecification"
                .to_string(),
        },
        RefusedCase {
            case: "a customized metric naming a metric but no statistic",
            params: vec![
                p("PolicyType", "TargetTrackingScaling"),
                p(
                    "TargetTrackingConfiguration.CustomizedMetricSpecification.Namespace",
                    "Acme/Queue",
                ),
                p(
                    "TargetTrackingConfiguration.CustomizedMetricSpecification.MetricName",
                    "Backlog",
                ),
                p(
                    "TargetTrackingConfiguration.CustomizedMetricSpecification.Dimensions\
                     .member.1.Name",
                    "QueueName",
                ),
                p(
                    "TargetTrackingConfiguration.CustomizedMetricSpecification.Dimensions\
                     .member.1.Value",
                    "orders",
                ),
                p("TargetTrackingConfiguration.TargetValue", "50"),
            ],
            message: member_must_not_be_null(
                "targetTrackingConfiguration.customizedMetricSpecification.statistic",
            ),
        },
        RefusedCase {
            case: "the per-target request count without the resource label it is counted per",
            params: vec![
                p("PolicyType", "TargetTrackingScaling"),
                p(
                    "TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType",
                    "ALBRequestCountPerTarget",
                ),
                p("TargetTrackingConfiguration.TargetValue", "100"),
            ],
            message: "Resource label should be specified for predefined metric type \
                      ALBRequestCountPerTarget"
                .to_string(),
        },
        RefusedCase {
            case: "a zero target value, which is below the window the service tracks",
            params: vec![
                p("PolicyType", "TargetTrackingScaling"),
                p(
                    "TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType",
                    "ASGAverageCPUUtilization",
                ),
                p("TargetTrackingConfiguration.TargetValue", "0"),
            ],
            message: "Target value must be between 8.51592E-12 and 1.174271E17".to_string(),
        },
        RefusedCase {
            case: "a negative target value",
            params: vec![
                p("PolicyType", "TargetTrackingScaling"),
                p(
                    "TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType",
                    "ASGAverageCPUUtilization",
                ),
                p("TargetTrackingConfiguration.TargetValue", "-5"),
            ],
            message: "Target value must be between 8.51592E-12 and 1.174271E17".to_string(),
        },
        RefusedCase {
            case: "a target value above the window the service tracks",
            params: vec![
                p("PolicyType", "TargetTrackingScaling"),
                p(
                    "TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType",
                    "ASGAverageCPUUtilization",
                ),
                p("TargetTrackingConfiguration.TargetValue", "1.2E17"),
            ],
            message: "Target value must be between 8.51592E-12 and 1.174271E17".to_string(),
        },
    ]
}

/// RULE 3: each refused put answers HTTP 400 `ValidationError` with that class's whole
/// sentence, and leaves NOTHING behind — the group still reports no policy afterwards.
#[tokio::test]
async fn a_put_that_cannot_describe_a_policy_is_refused_and_stores_nothing() {
    let router = router();
    create_group(&router, "asg-refusals").await;

    for (index, refused) in refused_cases().into_iter().enumerate() {
        let case = refused.case;
        let policy_name = format!("refused-{index}");
        let (status, body) =
            put_policy(&router, "asg-refusals", &policy_name, &refused.params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code, "ValidationError",
            "case {case:?}: expected the ValidationError code, got {code:?} with {message:?}"
        );
        // The WHOLE message, not a substring: a sentence that merely contains the right
        // words would satisfy a `contains` check against a refusal for another reason.
        assert_eq!(
            message, refused.message,
            "case {case:?}: expected the message {:?}, got {message:?}",
            refused.message
        );
    }

    // Nothing any of those puts named was stored.
    let (status, body) = describe_policies(
        &router,
        &[p("AutoScalingGroupName", "asg-refusals")],
    )
    .await;
    assert_eq!(status, 200, "the read-back must succeed, got {body:?}");
    let names = tag_values(&body, "PolicyName");
    assert!(
        names.is_empty(),
        "a refused put must store nothing, got the policies {names:?}"
    );

    // The control: the otherwise identical put that IS valid still succeeds, so the
    // refusals above are not refusing everything.
    let (status, body) = put_policy(
        &router,
        "asg-refusals",
        "accepted",
        &simple_scaling_params("ChangeInCapacity", "1"),
    )
    .await;
    assert_eq!(
        status, 200,
        "the valid put must still succeed, got {status} with {body:?}"
    );
}

/// RULE 3, the group half: a put naming a group nothing answers to is refused with this
/// operation's own absent-group sentence, whatever else it carries.
#[tokio::test]
async fn a_put_naming_no_stored_group_is_refused() {
    let router = router();
    create_group(&router, "asg-present").await;

    const ABSENT_GROUP_MESSAGE: &str = "AutoScalingGroup name not found - null";

    // The names differ in length, case, character set and spelling, and the member sets
    // include ones that would themselves be refused if the group existed: the group is
    // resolved first, so the message is the same for all of them.
    for name in ["a", "ASG-UPPER", "null", "グループ-ünïcode", &"z".repeat(255)] {
        for (members_case, params) in [
            ("a valid simple policy", simple_scaling_params("ChangeInCapacity", "1")),
            ("a policy whose own members are invalid", simple_scaling_params("NotAType", "1")),
            ("no policy members at all", Vec::new()),
        ] {
            let case = format!("{name:?} with {members_case}");
            let (status, body) = put_policy(&router, name, "p", &params).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} with {body:?}"
            );
            let (code, message) = refusal(&body);
            assert_eq!(
                code, "ValidationError",
                "case {case}: expected the ValidationError code, got {code:?}"
            );
            assert_eq!(
                message, ABSENT_GROUP_MESSAGE,
                "case {case}: expected the message {ABSENT_GROUP_MESSAGE:?}, got {message:?}"
            );
        }
    }

    // A read scoped to a group nothing answers to is refused the same way, with the
    // detail form THAT operation reports.
    let (status, body) = describe_policies(&router, &[p("AutoScalingGroupName", "asg-absent")]).await;
    assert_eq!(status, 400, "expected HTTP 400, got {status} with {body:?}");
    let (code, message) = refusal(&body);
    assert_eq!(code, "ValidationError", "expected ValidationError, got {code:?}");
    assert_eq!(
        message, "AutoScalingGroup name not found - AutoScalingGroup asg-absent not found",
        "expected the read's own absent-group sentence, got {message:?}"
    );
}

/// RULE 4: a target-tracking policy's alarms exist in CloudWatch's own store, and
/// `DisableScaleIn` withholds the scale-in one.
#[tokio::test]
async fn a_target_tracking_policy_mints_alarms_cloudwatch_resolves() {
    let router = router();
    create_group(&router, "asg-alarms").await;

    // The rows differ along what decides the alarm SET (whether scaling in is disabled)
    // and along the series the alarms grade (a predefined metric, the per-target request
    // count that names a resource label, and a customized metric).
    let cases: Vec<AlarmCase<'_>> = vec![
        (
            "a predefined metric with scaling in enabled",
            vec![
                p("PolicyType", "TargetTrackingScaling"),
                p(
                    "TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType",
                    "ASGAverageCPUUtilization",
                ),
                p("TargetTrackingConfiguration.TargetValue", "40"),
            ],
            2,
        ),
        (
            "a predefined metric with scaling in disabled",
            vec![
                p("PolicyType", "TargetTrackingScaling"),
                p(
                    "TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType",
                    "ASGAverageNetworkIn",
                ),
                p("TargetTrackingConfiguration.TargetValue", "1000000"),
                p("TargetTrackingConfiguration.DisableScaleIn", "true"),
            ],
            1,
        ),
        (
            "the per-target request count, with the resource label it is counted per",
            vec![
                p("PolicyType", "TargetTrackingScaling"),
                p(
                    "TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType",
                    "ALBRequestCountPerTarget",
                ),
                p(
                    "TargetTrackingConfiguration.PredefinedMetricSpecification.ResourceLabel",
                    "app/my-alb/778d41231b141a0f/targetgroup/my-tg/943f017f100becff",
                ),
                p("TargetTrackingConfiguration.TargetValue", "100"),
            ],
            2,
        ),
        (
            "a customized metric",
            vec![
                p("PolicyType", "TargetTrackingScaling"),
                p(
                    "TargetTrackingConfiguration.CustomizedMetricSpecification.Namespace",
                    "Acme/Queue",
                ),
                p(
                    "TargetTrackingConfiguration.CustomizedMetricSpecification.MetricName",
                    "BacklogPerInstance",
                ),
                p(
                    "TargetTrackingConfiguration.CustomizedMetricSpecification.Statistic",
                    "Average",
                ),
                p("TargetTrackingConfiguration.TargetValue", "7.5"),
            ],
            2,
        ),
    ];

    for (index, (case, params, expected_alarms)) in cases.into_iter().enumerate() {
        let policy_name = format!("tracked-{index}");
        let (status, body) = put_policy(&router, "asg-alarms", &policy_name, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        let answered: Vec<String> = tag_values(&body, "AlarmName");
        assert_eq!(
            answered.len(),
            expected_alarms,
            "case {case:?}: expected {expected_alarms} alarm(s), got {answered:?}"
        );
        for name in &answered {
            assert!(
                name.starts_with("TargetTracking-asg-alarms-Alarm"),
                "case {case:?}: an alarm of a target-tracking policy is named for the group it \
                 steers, got {name:?}"
            );
        }

        // The alarms are CloudWatch's OWN resources: its own read resolves every one the
        // policy reports.
        let resolved = resolved_alarm_names(&router, &answered).await;
        let mut expected_sorted = answered.clone();
        expected_sorted.sort();
        let mut resolved_sorted = resolved.clone();
        resolved_sorted.sort();
        assert_eq!(
            resolved_sorted, expected_sorted,
            "case {case:?}: CloudWatch must hold every alarm the policy reports, got \
             {resolved:?} for {answered:?}"
        );

        // And the policy reports the same set on a later read.
        let (_, body) = describe_policies(
            &router,
            &[p("PolicyNames.member.1", policy_name.clone())],
        )
        .await;
        let mut reported = tag_values(&body, "AlarmName");
        reported.sort();
        assert_eq!(
            reported, expected_sorted,
            "case {case:?}: the read must report the alarms the put minted, got {reported:?}"
        );
    }

    // Re-putting a policy with scaling in disabled retires the scale-in alarm it held: the
    // alarm set CloudWatch holds is the one the policy now wants.
    let withdrawn = resolved_alarm_names(&router, &[]).await;
    let scale_in_alarms: Vec<&String> = withdrawn
        .iter()
        .filter(|name| name.contains("-AlarmLow-"))
        .collect();
    let before = scale_in_alarms.len();
    let (status, body) = put_policy(
        &router,
        "asg-alarms",
        "tracked-0",
        &[
            p("PolicyType", "TargetTrackingScaling"),
            p(
                "TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType",
                "ASGAverageCPUUtilization",
            ),
            p("TargetTrackingConfiguration.TargetValue", "40"),
            p("TargetTrackingConfiguration.DisableScaleIn", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "disabling scaling in must succeed, got {status} with {body:?}"
    );
    let after = resolved_alarm_names(&router, &[])
        .await
        .iter()
        .filter(|name| name.contains("-AlarmLow-"))
        .count();
    assert_eq!(
        after,
        before - 1,
        "disabling scaling in must retire exactly the policy's own scale-in alarm, went from \
         {before} to {after}"
    );
}

/// RULE 5: a policy only exists against a group that exists — deleting the group deletes
/// its policies and retires their alarms, while another group's are untouched.
#[tokio::test]
async fn deleting_a_group_deletes_its_policies_and_their_alarms() {
    let router = router();
    create_group(&router, "asg-doomed").await;
    create_group(&router, "asg-kept").await;

    let tracking = vec![
        p("PolicyType", "TargetTrackingScaling"),
        p(
            "TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType",
            "ASGAverageCPUUtilization",
        ),
        p("TargetTrackingConfiguration.TargetValue", "60"),
    ];
    let (_, doomed_body) = put_policy(&router, "asg-doomed", "cpu", &tracking).await;
    let doomed_alarms = tag_values(&doomed_body, "AlarmName");
    assert_eq!(
        doomed_alarms.len(),
        2,
        "the doomed group's policy must hold its two alarms, got {doomed_alarms:?}"
    );
    let (_, kept_body) = put_policy(&router, "asg-kept", "cpu", &tracking).await;
    let kept_alarms = tag_values(&kept_body, "AlarmName");
    assert_eq!(
        kept_alarms.len(),
        2,
        "the kept group's policy must hold its two alarms, got {kept_alarms:?}"
    );
    let (_, body) = put_policy(
        &router,
        "asg-doomed",
        "simple",
        &simple_scaling_params("ChangeInCapacity", "1"),
    )
    .await;
    assert_eq!(
        tag_values(&body, "AlarmName"),
        Vec::<String>::new(),
        "a simple scaling policy mints no alarm, got {body:?}"
    );

    let (status, body) = query(
        &router,
        "DeleteAutoScalingGroup",
        &[("AutoScalingGroupName", "asg-doomed")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the delete must succeed, got {status} with {body:?}"
    );

    // The deleted group's name answers the read's absent-group refusal, so its policies
    // are not reachable through it either.
    let (status, _) = describe_policies(&router, &[p("AutoScalingGroupName", "asg-doomed")]).await;
    assert_eq!(
        status, 400,
        "the deleted group must be refused by the read, got {status}"
    );
    // And the account-wide read reports only the surviving group's policy.
    let (status, body) = describe_policies(&router, &[]).await;
    assert_eq!(status, 200, "the account-wide read must succeed, got {body:?}");
    let groups = tag_values(&body, "AutoScalingGroupName");
    assert_eq!(
        groups,
        vec!["asg-kept".to_string()],
        "only the surviving group's policies may be reported, got {groups:?}"
    );

    let resolved = resolved_alarm_names(&router, &doomed_alarms).await;
    assert!(
        resolved.is_empty(),
        "the deleted group's alarms must be retired in CloudWatch, got {resolved:?}"
    );
    let surviving = resolved_alarm_names(&router, &kept_alarms).await;
    let mut surviving_sorted = surviving.clone();
    surviving_sorted.sort();
    let mut kept_sorted = kept_alarms.clone();
    kept_sorted.sort();
    assert_eq!(
        surviving_sorted, kept_sorted,
        "the surviving group's alarms must be untouched, got {surviving:?}"
    );
}

/// RULE 1, the filter and page half: a read selects by group, by policy name and by
/// policy type, pages over what it selected, and ignores a policy name nothing answers to.
#[tokio::test]
async fn a_read_filters_by_group_name_and_type_and_pages_over_the_rest() {
    let router = router();
    create_group(&router, "asg-a").await;
    create_group(&router, "asg-b").await;

    for (group, policy, params) in [
        ("asg-a", "simple-1", simple_scaling_params("ChangeInCapacity", "1")),
        ("asg-a", "simple-2", simple_scaling_params("ExactCapacity", "2")),
        (
            "asg-a",
            "stepped",
            vec![
                p("PolicyType", "StepScaling"),
                p("AdjustmentType", "ChangeInCapacity"),
                p("StepAdjustments.member.1.MetricIntervalLowerBound", "0"),
                p("StepAdjustments.member.1.ScalingAdjustment", "1"),
            ],
        ),
        ("asg-b", "simple-1", simple_scaling_params("ChangeInCapacity", "1")),
    ] {
        let (status, body) = put_policy(&router, group, policy, &params).await;
        assert_eq!(
            status, 200,
            "the put of {group}/{policy} must succeed, got {status} with {body:?}"
        );
    }

    let selections: Vec<SelectionCase<'_>> = vec![
        (
            "no filter at all reports every policy in the account",
            vec![],
            vec!["simple-1", "simple-2", "stepped", "simple-1"],
        ),
        (
            "a group name limits the results to that group",
            vec![p("AutoScalingGroupName", "asg-a")],
            vec!["simple-1", "simple-2", "stepped"],
        ),
        (
            "a policy name selects it in every group that holds it",
            vec![p("PolicyNames.member.1", "simple-1")],
            vec!["simple-1", "simple-1"],
        ),
        (
            "a group name and a policy name select one policy",
            vec![
                p("AutoScalingGroupName", "asg-b"),
                p("PolicyNames.member.1", "simple-1"),
            ],
            vec!["simple-1"],
        ),
        (
            "a policy type selects the policies of that type",
            vec![p("PolicyTypes.member.1", "StepScaling")],
            vec!["stepped"],
        ),
        (
            "two policy types select the union",
            vec![
                p("AutoScalingGroupName", "asg-a"),
                p("PolicyTypes.member.1", "StepScaling"),
                p("PolicyTypes.member.2", "SimpleScaling"),
            ],
            vec!["simple-1", "simple-2", "stepped"],
        ),
        (
            "a policy name nothing answers to is ignored with no error",
            vec![p("PolicyNames.member.1", "no-such-policy")],
            vec![],
        ),
        (
            "a policy type nothing answers to selects nothing",
            vec![p("PolicyTypes.member.1", "PredictiveScaling")],
            vec![],
        ),
    ];

    for (case, params, expected) in selections {
        let (status, body) = describe_policies(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        let reported = tag_values(&body, "PolicyName");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the policies {expected:?}, got {reported:?}"
        );
    }

    // Paging over the group's three policies: each page carries what was asked for and
    // the walk reaches every policy exactly once.
    for page_size in [1usize, 2, 3] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![
                p("AutoScalingGroupName", "asg-a"),
                p("MaxRecords", page_size.to_string()),
            ];
            if let Some(token) = token.as_deref() {
                params.push(p("NextToken", token));
            }
            let (status, body) = describe_policies(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {page_size}: expected HTTP 200, got {status} with {body:?}"
            );
            let page = tag_values(&body, "PolicyName");
            assert!(
                page.len() <= page_size,
                "case {page_size}: a page may hold at most {page_size} policies, got {page:?}"
            );
            walked.extend(page);
            let next = tag_value(&body, "NextToken");
            if next.is_empty() {
                break;
            }
            token = Some(next);
        }
        assert_eq!(
            walked,
            vec![
                "simple-1".to_string(),
                "simple-2".to_string(),
                "stepped".to_string()
            ],
            "case {page_size}: the page walk must reach every policy of the group exactly once"
        );
    }

    // A cursor this service never issued is refused as the modelled `InvalidNextToken`.
    let (status, body) = describe_policies(
        &router,
        &[p("NextToken", "sim2real-not-a-real-next-token")],
    )
    .await;
    assert_eq!(status, 400, "a bad cursor must be refused, got {body:?}");
    let (code, message) = refusal(&body);
    assert_eq!(
        code, "InvalidNextToken",
        "expected the InvalidNextToken code, got {code:?}"
    );
    assert_eq!(
        message, "The pagination token 'sim2real-not-a-real-next-token' is invalid",
        "the refusal must quote the token the caller sent, got {message:?}"
    );
}
