// SPDX-License-Identifier: CC0-1.0

//! The scaling policies of an Auto Scaling group: the resource `PutScalingPolicy`
//! creates or updates and `DescribePolicies` reports.
//!
//! A policy belongs to ONE group and is named uniquely within it, so a stored policy is
//! keyed by [`PolicyKey`] and the whole collection lives in a single map on
//! [`crate::state::AutoScalingState`] — nothing in this crate holds a second copy.
//!
//! # What a policy carries
//!
//! A policy's `PolicyType` decides which of the request's members are part of it: the
//! model documents `Cooldown` as "Valid only if the policy type is SimpleScaling",
//! `ScalingAdjustment` as "Required if the policy type is SimpleScaling. (Not used with
//! any other policy type.)", `StepAdjustments` and `MetricAggregationType` as the step
//! policy's own, and `EstimatedInstanceWarmup` as "Valid only if the policy type is
//! TargetTrackingScaling or StepScaling". So a put keeps only the members its type owns
//! ([`policy_members_of_type`]) and a read reports exactly those — a step policy never
//! reports a cooldown it cannot honour.
//!
//! # The alarms a target-tracking policy IS
//!
//! "The CloudWatch alarms created for the target tracking scaling policy"
//! (`PolicyARNType$Alarms`): a target-tracking policy is driven by CloudWatch alarms over
//! the metric it tracks, so putting one materialises those alarms in the account's OWN
//! CloudWatch store ([`sync_policy_alarms`]) and the policy reports back the alarms that
//! store holds for it. Nothing here invents an alarm name a CloudWatch read would not
//! resolve.
//!
//! LOCKING. This service's own store is locked by the handler; CloudWatch's is taken here
//! for the one pass that writes or removes the alarms and released when it ends, and
//! nothing awaits while either is held. CloudWatch never reaches this service, so the two
//! locks are never taken in the other order and no cycle exists.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

use crate::ops::types::{
    Alarm, CustomizedMetricSpecification, PredictiveScalingConfiguration, ScalingPolicy,
    StepAdjustment, TargetTrackingConfiguration,
};

// ---------------------------------------------------------------------------
// The policy types, and the members each of them owns
// ---------------------------------------------------------------------------

/// The default `PolicyType`, from the model's own documentation for
/// `PutScalingPolicyType$PolicyType`: "SimpleScaling (default)".
pub const POLICY_TYPE_SIMPLE_SCALING: &str = "SimpleScaling";

/// The step-scaling policy type (same documentation).
pub const POLICY_TYPE_STEP_SCALING: &str = "StepScaling";

/// The target-tracking policy type (same documentation).
pub const POLICY_TYPE_TARGET_TRACKING: &str = "TargetTrackingScaling";

/// The predictive-scaling policy type (same documentation).
pub const POLICY_TYPE_PREDICTIVE_SCALING: &str = "PredictiveScaling";

/// The policy types a request may name, from the model's own documentation for
/// `PutScalingPolicyType$PolicyType` ("One of the following policy types:
/// TargetTrackingScaling StepScaling SimpleScaling (default) PredictiveScaling"), in the
/// order that documentation lists them.
pub const POLICY_TYPES: &[&str] = &[
    POLICY_TYPE_TARGET_TRACKING,
    POLICY_TYPE_STEP_SCALING,
    POLICY_TYPE_SIMPLE_SCALING,
    POLICY_TYPE_PREDICTIVE_SCALING,
];

/// The aggregation types a `MetricAggregationType` may name, from the model's own
/// documentation for `PutScalingPolicyType$MetricAggregationType`: "The valid values are
/// Minimum, Maximum, and Average", in the order that documentation lists them.
pub const METRIC_AGGREGATION_TYPES: &[&str] = &["Minimum", "Maximum", "Average"];

/// The predefined metric types a target-tracking policy may track, from the model's
/// `com.amazonaws.autoscaling#MetricType` enum
/// (`smithy_query(query="shape",shape="MetricType")`).
pub const PREDEFINED_METRIC_TYPES: &[&str] = &[
    "ASGAverageCPUUtilization",
    "ASGAverageNetworkIn",
    "ASGAverageNetworkOut",
    ALB_REQUEST_COUNT_PER_TARGET,
];

/// The one predefined metric type whose series is counted per Application Load Balancer
/// target group rather than per Auto Scaling group, so it cannot be resolved without the
/// `ResourceLabel` that names that target group.
pub const ALB_REQUEST_COUNT_PER_TARGET: &str = "ALBRequestCountPerTarget";

/// Which of a put's members belong to a policy of `policy_type`.
///
/// Each flag is one sentence of the model's own documentation (see the module docs): a
/// member that is "not used with" or "valid only if" another type is not part of this
/// policy, so it is neither stored nor reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyMembers {
    /// `AdjustmentType`, `MinAdjustmentStep` and `MinAdjustmentMagnitude`: "Valid only if
    /// the policy type is StepScaling or SimpleScaling."
    pub adjustment: bool,
    /// `ScalingAdjustment`: "Required if the policy type is SimpleScaling. (Not used with
    /// any other policy type.)"
    pub scaling_adjustment: bool,
    /// `Cooldown`: "Valid only if the policy type is SimpleScaling."
    pub cooldown: bool,
    /// `StepAdjustments`: "Required if the policy type is StepScaling. (Not used with any
    /// other policy type.)", and `MetricAggregationType`: "Valid only if the policy type
    /// is StepScaling."
    pub step: bool,
    /// `EstimatedInstanceWarmup`: "Valid only if the policy type is
    /// TargetTrackingScaling or StepScaling."
    pub instance_warmup: bool,
    /// `TargetTrackingConfiguration`: "Required if the policy type is
    /// TargetTrackingScaling."
    pub target_tracking: bool,
    /// `PredictiveScalingConfiguration`: "Required if the policy type is
    /// PredictiveScaling."
    pub predictive: bool,
}

/// The members a policy of `policy_type` owns. An unknown type owns none — such a
/// request is refused before anything is stored.
#[must_use]
pub fn policy_members_of_type(policy_type: &str) -> PolicyMembers {
    let none = PolicyMembers {
        adjustment: false,
        scaling_adjustment: false,
        cooldown: false,
        step: false,
        instance_warmup: false,
        target_tracking: false,
        predictive: false,
    };
    match policy_type {
        POLICY_TYPE_SIMPLE_SCALING => PolicyMembers {
            adjustment: true,
            scaling_adjustment: true,
            cooldown: true,
            ..none
        },
        POLICY_TYPE_STEP_SCALING => PolicyMembers {
            adjustment: true,
            step: true,
            instance_warmup: true,
            ..none
        },
        POLICY_TYPE_TARGET_TRACKING => PolicyMembers {
            instance_warmup: true,
            target_tracking: true,
            ..none
        },
        POLICY_TYPE_PREDICTIVE_SCALING => PolicyMembers {
            predictive: true,
            ..none
        },
        _ => none,
    }
}

// ---------------------------------------------------------------------------
// The stored policy
// ---------------------------------------------------------------------------

/// A stored policy's identity: the group it belongs to and its name within that group.
///
/// Ordered, so the collection iterates group by group and, inside a group, policy name by
/// policy name — which is the order every read of it reports.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PolicyKey {
    pub auto_scaling_group_name: String,
    pub policy_name: String,
}

/// A stored scaling policy — the `ScalingPolicy` resource `PutScalingPolicy` creates or
/// updates.
#[derive(Debug, Clone)]
pub struct StoredScalingPolicy {
    /// The ARN minted when the policy was CREATED, which every later put of the same
    /// name keeps.
    pub policy_arn: String,
    /// The Region the policy (and the group it belongs to) lives in.
    pub region: String,
    pub policy_type: String,
    pub adjustment_type: Option<String>,
    pub min_adjustment_step: Option<i32>,
    pub min_adjustment_magnitude: Option<i32>,
    pub scaling_adjustment: Option<i32>,
    pub cooldown: Option<i32>,
    pub step_adjustments: Option<Vec<StepAdjustment>>,
    pub metric_aggregation_type: Option<String>,
    pub estimated_instance_warmup: Option<i32>,
    /// `Enabled`: "Indicates whether the scaling policy is enabled or disabled. The
    /// default is enabled."
    pub enabled: bool,
    pub target_tracking_configuration: Option<TargetTrackingConfiguration>,
    pub predictive_scaling_configuration: Option<PredictiveScalingConfiguration>,
    /// The alarms this policy holds in CloudWatch, as the last sync of them left it.
    pub alarms: Vec<Alarm>,
    /// The opaque segment the alarm names of a target-tracking policy carry, minted once
    /// per policy so a later put of the same policy keeps the alarms it already has.
    pub alarm_id: String,
    pub created_time: SystemTime,
}

impl StoredScalingPolicy {
    /// The `ScalingPolicy` a read reports for this policy.
    ///
    /// Only the members the policy's own type owns are reported, which are the only ones
    /// a put stored (see the module docs).
    #[must_use]
    pub fn view(&self, key: &PolicyKey) -> ScalingPolicy {
        ScalingPolicy {
            auto_scaling_group_name: Some(key.auto_scaling_group_name.clone()),
            policy_name: Some(key.policy_name.clone()),
            policy_arn: Some(self.policy_arn.clone()),
            policy_type: Some(self.policy_type.clone()),
            adjustment_type: self.adjustment_type.clone(),
            min_adjustment_step: self.min_adjustment_step,
            min_adjustment_magnitude: self.min_adjustment_magnitude,
            scaling_adjustment: self.scaling_adjustment,
            cooldown: self.cooldown,
            step_adjustments: self.step_adjustments.clone(),
            metric_aggregation_type: self.metric_aggregation_type.clone(),
            estimated_instance_warmup: self.estimated_instance_warmup,
            alarms: Some(self.alarms.clone()),
            target_tracking_configuration: self.target_tracking_configuration.clone(),
            enabled: Some(self.enabled),
            predictive_scaling_configuration: self.predictive_scaling_configuration.clone(),
        }
    }
}

/// The ARN of a scaling policy, in the form the service reports:
/// `arn:aws:autoscaling:{region}:{account}:scalingPolicy:{id}:autoScalingGroupName/{group}:policyName/{policy}`.
#[must_use]
pub fn policy_arn(region: &str, resource_id: &str, key: &PolicyKey) -> String {
    ::engine::emulator_arn(
        "autoscaling",
        region,
        &format!(
            "scalingPolicy:{resource_id}:autoScalingGroupName/{}:policyName/{}",
            key.auto_scaling_group_name, key.policy_name
        ),
    )
}

/// How many CloudWatch alarms one target-tracking policy holds at most: the scale-out
/// alarm and the scale-in one ([`AlarmKind`]).
pub const ALARMS_PER_TARGET_TRACKING_POLICY: usize = 2;

/// The most scaling policies this emulator's store holds.
///
/// A capacity bound of the store, not an AWS quota: the operation models
/// `LimitExceededFault` for "You have already reached a limit for your Amazon EC2 Auto
/// Scaling resources", and this is the limit this store has. It is derived from the
/// alarms the policies may mint — CloudWatch's own store holds at most
/// `cloudwatch::state::MAX_ALARMS` of them — so a store full of target-tracking policies
/// cannot ask CloudWatch for more alarms than CloudWatch itself holds. Checked only when a
/// policy is CREATED, so an update of an existing policy still succeeds at the ceiling.
pub const MAX_SCALING_POLICIES: usize =
    cloudwatch::state::MAX_ALARMS / ALARMS_PER_TARGET_TRACKING_POLICY;

// ---------------------------------------------------------------------------
// The refusals a put answers
// ---------------------------------------------------------------------------

/// The sentence a put naming an `AdjustmentType` the service does not serve is refused
/// with: it reports the adjustment types that ARE valid and quotes nothing from the
/// request.
pub const VALID_ADJUSTMENT_TYPES_MESSAGE: &str =
    "Valid adjustment types are: [PercentChangeInCapacity, ChangeInCapacity, ExactCapacity].";

/// The sentence a step-scaling put carrying no `StepAdjustments` is refused with. Fixed
/// text: it names neither the group nor the policy.
pub const STEP_ADJUSTMENT_REQUIRED_MESSAGE: &str =
    "You should specify at least one StepAdjustment with a StepScaling policy.";

/// The sentence a put naming a `PolicyType` the service does not serve is refused with,
/// in the same form as [`VALID_ADJUSTMENT_TYPES_MESSAGE`] — the service reports an
/// enumerated member's valid values and quotes nothing from the request.
#[must_use]
pub fn valid_policy_types_message() -> String {
    format!("Valid policy types are: [{}].", POLICY_TYPES.join(", "))
}

/// The same sentence for `MetricAggregationType`.
#[must_use]
pub fn valid_metric_aggregation_types_message() -> String {
    format!(
        "Valid metric aggregation types are: [{}].",
        METRIC_AGGREGATION_TYPES.join(", ")
    )
}

/// The sentence a target-tracking put that does not describe exactly one metric is
/// refused with. Fixed text, with no request-derived part: the defect is the same whether
/// the configuration carries both specifications or neither.
pub const ONE_METRIC_SPECIFICATION_MESSAGE: &str =
    "You should provide either a CustomizedMetricSpecification or a \
     PredefinedMetricSpecification";

/// The smallest `TargetValue` a target-tracking policy may track.
///
/// The model declares `MetricScale` as a bare double with no range, so the window is the
/// service's own and is reported verbatim by
/// [`TARGET_VALUE_RANGE_MESSAGE`].
pub const MIN_TARGET_VALUE: f64 = 8.515_92e-12;

/// The largest `TargetValue` a target-tracking policy may track (same window).
pub const MAX_TARGET_VALUE: f64 = 1.174_271e17;

/// The sentence a target-tracking put whose `TargetValue` is outside the service's window
/// is refused with: fixed text naming both bounds of that window.
pub const TARGET_VALUE_RANGE_MESSAGE: &str =
    "Target value must be between 8.51592E-12 and 1.174271E17";

/// Whether `target_value` is inside the window the service tracks
/// ([`MIN_TARGET_VALUE`]..=[`MAX_TARGET_VALUE`]).
#[must_use]
pub fn target_value_in_range(target_value: f64) -> bool {
    target_value.is_finite() && (MIN_TARGET_VALUE..=MAX_TARGET_VALUE).contains(&target_value)
}

/// The sentence a target-tracking put naming a predefined metric type whose series cannot
/// be resolved without the resource its label names is refused with. The metric type comes
/// from the request, so the sentence names whichever type needs one.
#[must_use]
pub fn resource_label_required_message(predefined_metric_type: &str) -> String {
    format!("Resource label should be specified for predefined metric type {predefined_metric_type}")
}

/// The service's standard constraint sentence for a member the request left null, naming
/// the member by the wire path the service reports it under.
#[must_use]
pub fn member_must_not_be_null_message(member_path: &str) -> String {
    validation_errors_message("null", member_path, "Member must not be null")
}

/// The same sentence for a member whose value is below the smallest the service accepts.
#[must_use]
pub fn member_below_minimum_message(member_path: &str, value: i32, minimum: i32) -> String {
    validation_errors_message(
        &format!("'{value}'"),
        member_path,
        &format!("Member must have value greater than or equal to {minimum}"),
    )
}

/// The envelope both constraint sentences above are stated in — the one the service
/// answers a member value outside its bounds with. ONE violation, because each refusal is
/// taken at the first member that fails.
fn validation_errors_message(value: &str, member_path: &str, constraint: &str) -> String {
    format!(
        "1 validation error detected: Value {value} at '{member_path}' failed to satisfy \
         constraint: {constraint}"
    )
}

/// The smallest `Cooldown` a simple-scaling policy may carry: a cooldown is a span of
/// seconds, so it is not negative.
pub const MIN_COOLDOWN_SECONDS: i32 = 0;

/// The wire path `Cooldown` is reported under in a constraint sentence.
pub const COOLDOWN_MEMBER_PATH: &str = "cooldown";

/// The wire path a target-tracking configuration's customized statistic is reported under.
pub const CUSTOMIZED_STATISTIC_MEMBER_PATH: &str =
    "targetTrackingConfiguration.customizedMetricSpecification.statistic";

/// Whether a customized metric specification describes its metric by NAMING one (a
/// namespace and a metric name) rather than as a metric data query.
///
/// The two forms are alternatives — `Metrics` is "the metrics to include in the target
/// tracking scaling policy, as a metric data query" — and only the named form owes a
/// `Statistic`, which is why the statistic is required of this form alone.
#[must_use]
pub fn names_one_metric(customized: &CustomizedMetricSpecification) -> bool {
    customized.metric_name.is_some() && customized.namespace.is_some()
}

// ---------------------------------------------------------------------------
// The CloudWatch alarms of a target-tracking policy
// ---------------------------------------------------------------------------

/// The prefix every alarm a target-tracking policy mints carries, as Amazon EC2 Auto
/// Scaling names them: `TargetTracking-{group}-Alarm{High,Low}-{id}`.
pub const TARGET_TRACKING_ALARM_PREFIX: &str = "TargetTracking";

/// The aggregation period, in seconds, of the alarms a target-tracking policy mints.
///
/// From the model's own documentation for `CustomizedMetricSpecification$Period`: "The
/// period of the metric in seconds. The default value is 60."
pub const ALARM_PERIOD_SECONDS: i32 = 60;

/// How many consecutive periods the scale-OUT alarm grades before it fires.
///
/// Chosen, not documented: the model states no evaluation window for a policy's alarms.
/// Scale-out is graded over fewer periods than scale-in so a policy adds capacity sooner
/// than it removes it, which is the asymmetry target tracking is specified to have.
pub const SCALE_OUT_EVALUATION_PERIODS: i32 = 3;

/// How many consecutive periods the scale-IN alarm grades before it fires (same
/// reasoning).
pub const SCALE_IN_EVALUATION_PERIODS: i32 = 15;

/// How far below the target value the scale-in alarm's threshold sits, as a fraction of
/// the target value: a policy scales in only once the metric has fallen clear of the
/// target rather than the moment it dips under it.
///
/// Chosen, not documented, like the evaluation windows above.
pub const SCALE_IN_THRESHOLD_FRACTION: f64 = 0.9;

/// Which of a target-tracking policy's two alarms one spec describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlarmKind {
    /// The alarm that fires when the tracked metric is ABOVE the target value, which is
    /// what makes the policy scale out.
    High,
    /// The alarm that fires when the tracked metric has fallen clear of the target value,
    /// which is what makes the policy scale in. Withheld by `DisableScaleIn`.
    Low,
}

impl AlarmKind {
    /// The segment this kind contributes to an alarm's name.
    #[must_use]
    pub fn name_segment(self) -> &'static str {
        match self {
            Self::High => "AlarmHigh",
            Self::Low => "AlarmLow",
        }
    }

    /// The comparison the alarm grades the metric against its threshold with, spelled as
    /// CloudWatch's own `ComparisonOperator` enum does.
    #[must_use]
    pub fn comparison_operator(self) -> &'static str {
        match self {
            Self::High => "GreaterThanThreshold",
            Self::Low => "LessThanThreshold",
        }
    }

    /// How many consecutive periods this alarm grades before it fires.
    #[must_use]
    pub fn evaluation_periods(self) -> i32 {
        match self {
            Self::High => SCALE_OUT_EVALUATION_PERIODS,
            Self::Low => SCALE_IN_EVALUATION_PERIODS,
        }
    }

    /// The threshold this alarm grades against, for a policy tracking `target_value`.
    #[must_use]
    pub fn threshold(self, target_value: f64) -> f64 {
        match self {
            Self::High => target_value,
            Self::Low => target_value * SCALE_IN_THRESHOLD_FRACTION,
        }
    }
}

/// The name of one alarm of a target-tracking policy.
#[must_use]
pub fn policy_alarm_name(group_name: &str, alarm_id: &str, kind: AlarmKind) -> String {
    format!(
        "{TARGET_TRACKING_ALARM_PREFIX}-{group_name}-{}-{alarm_id}",
        kind.name_segment()
    )
}

/// The series one alarm of a target-tracking policy grades: where it lives in CloudWatch,
/// which statistic of it is graded, and with which dimensions.
#[derive(Debug, Clone)]
struct TrackedSeries {
    namespace: String,
    metric_name: String,
    statistic: String,
    unit: Option<String>,
    period: i32,
    dimensions: Vec<(String, String)>,
}

/// The CloudWatch namespace Amazon EC2 publishes an instance metric under.
const EC2_METRIC_NAMESPACE: &str = "AWS/EC2";

/// The namespace an Application Load Balancer publishes its request counts under.
const APPLICATION_ELB_METRIC_NAMESPACE: &str = "AWS/ApplicationELB";

/// The dimension an Amazon EC2 metric of a whole Auto Scaling group is published under.
const GROUP_NAME_DIMENSION: &str = "AutoScalingGroupName";

/// The series the policy's `TargetTrackingConfiguration` tracks, or `None` when it is a
/// metric data query, which names no single series.
///
/// A predefined metric type IS a documented series of one service's resources, from the
/// model's own documentation for `PredefinedMetricSpecification$PredefinedMetricType`:
/// "ASGAverageCPUUtilization - Average CPU utilization of the Auto Scaling group",
/// "ASGAverageNetworkIn - Average number of bytes received on all network interfaces by
/// the Auto Scaling group", "ASGAverageNetworkOut - Average number of bytes sent out on
/// all network interfaces by the Auto Scaling group", "ALBRequestCountPerTarget - Average
/// Application Load Balancer request count per target for your Auto Scaling group".
fn tracked_series(
    group_name: &str,
    configuration: &TargetTrackingConfiguration,
) -> Option<TrackedSeries> {
    if let Some(predefined) = configuration.predefined_metric_specification.as_ref() {
        let group_dimension = vec![(GROUP_NAME_DIMENSION.to_string(), group_name.to_string())];
        return match predefined.predefined_metric_type.as_str() {
            "ASGAverageCPUUtilization" => Some(TrackedSeries {
                namespace: EC2_METRIC_NAMESPACE.to_string(),
                metric_name: "CPUUtilization".to_string(),
                statistic: "Average".to_string(),
                unit: Some("Percent".to_string()),
                period: ALARM_PERIOD_SECONDS,
                dimensions: group_dimension,
            }),
            "ASGAverageNetworkIn" => Some(TrackedSeries {
                namespace: EC2_METRIC_NAMESPACE.to_string(),
                metric_name: "NetworkIn".to_string(),
                statistic: "Average".to_string(),
                unit: Some("Bytes".to_string()),
                period: ALARM_PERIOD_SECONDS,
                dimensions: group_dimension,
            }),
            "ASGAverageNetworkOut" => Some(TrackedSeries {
                namespace: EC2_METRIC_NAMESPACE.to_string(),
                metric_name: "NetworkOut".to_string(),
                statistic: "Average".to_string(),
                unit: Some("Bytes".to_string()),
                period: ALARM_PERIOD_SECONDS,
                dimensions: group_dimension,
            }),
            // The request count per target is counted per target GROUP, which is the
            // resource the configuration's `ResourceLabel` names — a put that names this
            // metric type without one is refused, so a stored policy always carries it.
            ALB_REQUEST_COUNT_PER_TARGET => Some(TrackedSeries {
                namespace: APPLICATION_ELB_METRIC_NAMESPACE.to_string(),
                metric_name: "RequestCountPerTarget".to_string(),
                statistic: "Sum".to_string(),
                unit: Some("Count".to_string()),
                period: ALARM_PERIOD_SECONDS,
                dimensions: resource_label_dimensions(
                    predefined.resource_label.as_deref().unwrap_or_default(),
                ),
            }),
            _ => None,
        };
    }
    let customized = configuration.customized_metric_specification.as_ref()?;
    if !names_one_metric(customized) {
        return None;
    }
    Some(TrackedSeries {
        namespace: customized.namespace.clone().unwrap_or_default(),
        metric_name: customized.metric_name.clone().unwrap_or_default(),
        // A put of the named form without a statistic is refused, so a stored policy of
        // this form always carries one.
        statistic: customized.statistic.clone().unwrap_or_default(),
        unit: customized.unit.clone(),
        period: customized.period.unwrap_or(ALARM_PERIOD_SECONDS),
        dimensions: customized
            .dimensions
            .iter()
            .flatten()
            .map(|dimension| (dimension.name.clone(), dimension.value.clone()))
            .collect(),
    })
}

/// The CloudWatch dimensions a `ResourceLabel` names.
///
/// The label is "the final portion of the load balancer ARN and the final portion of the
/// target group ARN into a single value, separated by a forward slash (/)", in the form
/// `app/<name>/<id>/targetgroup/<name>/<id>`
/// (`PredefinedMetricSpecification$ResourceLabel`), and the request-count-per-target
/// series is published under those two resources.
fn resource_label_dimensions(resource_label: &str) -> Vec<(String, String)> {
    let mut dimensions = Vec::new();
    if let Some((load_balancer, target_group)) = resource_label.split_once("/targetgroup/") {
        dimensions.push(("LoadBalancer".to_string(), load_balancer.to_string()));
        dimensions.push((
            "TargetGroup".to_string(),
            format!("targetgroup/{target_group}"),
        ));
    }
    dimensions
}

/// Which alarms a target-tracking policy wants: the scale-out alarm always, and the
/// scale-in alarm unless the configuration disabled scaling in.
///
/// "Indicates whether scaling in by the target tracking scaling policy is disabled. If
/// scaling in is disabled, the target tracking scaling policy doesn't remove instances
/// from the Auto Scaling group" (`TargetTrackingConfiguration$DisableScaleIn`) — a policy
/// that cannot remove an instance owes no alarm that would ask it to.
#[must_use]
pub fn wanted_alarm_kinds(configuration: &TargetTrackingConfiguration) -> Vec<AlarmKind> {
    if configuration.disable_scale_in.unwrap_or(false) {
        vec![AlarmKind::High]
    } else {
        vec![AlarmKind::High, AlarmKind::Low]
    }
}

/// Materialise the alarms `policy` wants in the account's own CloudWatch store, remove the
/// ones it no longer wants, and answer the alarm list the policy reports.
///
/// Call it with this service's store lock RELEASED. A policy of any other type wants no
/// alarm, so this removes whatever it previously held and answers an empty list. An engine
/// carrying no CloudWatch store holds no alarm, so none is reported for it: a policy never
/// reports an alarm a CloudWatch read would not resolve.
pub fn sync_policy_alarms<Eng: Send + Sync + 'static>(
    engine: &Eng,
    key: &PolicyKey,
    policy: &StoredScalingPolicy,
) -> Vec<Alarm> {
    let Some(store) = cloudwatch::state::service_state(engine) else {
        return Vec::new();
    };
    let configuration = policy
        .target_tracking_configuration
        .as_ref()
        .filter(|_| policy.policy_type == POLICY_TYPE_TARGET_TRACKING);
    let series = configuration.and_then(|configuration| {
        tracked_series(&key.auto_scaling_group_name, configuration)
    });
    let wanted: Vec<AlarmKind> = match (configuration, series.as_ref()) {
        (Some(configuration), Some(_)) => wanted_alarm_kinds(configuration),
        _ => Vec::new(),
    };

    let mut store = store.lock();
    let now = cloudwatch::state::now_secs();
    // The alarms this policy holds and no longer wants go first, so the alarm set left in
    // CloudWatch is exactly the one the policy reports.
    for kind in [AlarmKind::High, AlarmKind::Low] {
        if !wanted.contains(&kind) {
            store.alarms.remove(&policy_alarm_name(
                &key.auto_scaling_group_name,
                &policy.alarm_id,
                kind,
            ));
        }
    }

    let mut minted = Vec::with_capacity(ALARMS_PER_TARGET_TRACKING_POLICY);
    let (Some(configuration), Some(series)) = (configuration, series.as_ref()) else {
        return minted;
    };
    for kind in wanted {
        let name = policy_alarm_name(&key.auto_scaling_group_name, &policy.alarm_id, kind);
        let arn = cloudwatch::state::alarm_arn(&policy.region, &name);
        // An alarm that already exists keeps the state an evaluation gave it and the tags
        // a caller put on it; only its configuration is rewritten, exactly as
        // `PutMetricAlarm` does.
        let held = store.alarms.get(&name).map(|stored| {
            (
                stored.alarm.state_value.clone(),
                stored.alarm.state_reason.clone(),
                stored.alarm.state_reason_data.clone(),
                stored.alarm.state_updated_timestamp,
                stored.alarm.state_transitioned_timestamp,
                stored.tags.clone(),
                stored.creation_id.clone(),
            )
        });
        let (
            state_value,
            state_reason,
            state_reason_data,
            state_updated,
            state_transitioned,
            tags,
            creation_id,
        ) = held.unwrap_or_else(|| {
            (
                Some(cloudwatch::state::ALARM_STATE_INSUFFICIENT_DATA.to_string()),
                Some("Unchecked: initial alarm creation".to_string()),
                None,
                Some(cloudwatch::state::to_timestamp(now)),
                Some(cloudwatch::state::to_timestamp(now)),
                Vec::new(),
                cloudwatch::state::alarm_creation_id(now),
            )
        });
        let alarm = cloudwatch::ops::types::MetricAlarm {
            alarm_name: Some(name.clone()),
            alarm_arn: Some(arn.clone()),
            alarm_description: Some(format!(
                "DO NOT EDIT OR DELETE. For {} policy {} of Auto Scaling group {}.",
                POLICY_TYPE_TARGET_TRACKING, key.policy_name, key.auto_scaling_group_name
            )),
            alarm_configuration_updated_timestamp: Some(cloudwatch::state::to_timestamp(now)),
            actions_enabled: Some(true),
            // The alarm's one action names the scaling policy it executes: that wiring is
            // what makes a target-tracking policy alarm-driven.
            alarm_actions: Some(vec![policy.policy_arn.clone()]),
            state_value,
            state_reason,
            state_reason_data,
            state_updated_timestamp: state_updated,
            state_transitioned_timestamp: state_transitioned,
            metric_name: Some(series.metric_name.clone()),
            namespace: Some(series.namespace.clone()),
            statistic: Some(series.statistic.clone()),
            unit: series.unit.clone(),
            dimensions: Some(
                series
                    .dimensions
                    .iter()
                    .map(|(name, value)| cloudwatch::ops::types::Dimension {
                        name: name.clone(),
                        value: value.clone(),
                    })
                    .collect(),
            ),
            period: Some(series.period),
            evaluation_periods: Some(kind.evaluation_periods()),
            threshold: Some(kind.threshold(configuration.target_value)),
            comparison_operator: Some(kind.comparison_operator().to_string()),
            ..cloudwatch::ops::types::MetricAlarm::default()
        };
        store.alarms.insert(
            name.clone(),
            cloudwatch::state::StoredAlarm {
                alarm,
                tags,
                creation_id,
                // The alarm CloudWatch evaluates from the stored metric data, the same
                // span after this write as for one `PutMetricAlarm` created.
                evaluation_due_secs: now
                    + cloudwatch::state::alarm_evaluation::EVALUATION_DELAY_SECS,
                // Nothing has forced this alarm's state, so no forced span is standing
                // that its evaluation must not cut short.
                forced_state_until_secs: 0.0,
            },
        );
        minted.push(Alarm {
            alarm_name: Some(name),
            alarm_arn: Some(arn),
        });
    }
    minted
}

/// Remove every alarm the policies in `policies` hold in the account's own CloudWatch
/// store — what a group's deletion owes, since a deleted group's policies are gone with
/// it and so are the alarms that would execute them.
///
/// Call it with this service's store lock RELEASED.
pub fn remove_policy_alarms<Eng: Send + Sync + 'static>(
    engine: &Eng,
    policies: &BTreeMap<PolicyKey, StoredScalingPolicy>,
) {
    if policies.is_empty() {
        return;
    }
    let Some(store) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let mut store = store.lock();
    for (key, policy) in policies {
        for kind in [AlarmKind::High, AlarmKind::Low] {
            store.alarms.remove(&policy_alarm_name(
                &key.auto_scaling_group_name,
                &policy.alarm_id,
                kind,
            ));
        }
    }
}
