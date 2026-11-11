// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-autoscaling-autoscaling-2011-01-01 service.
//!
//! The Auto Scaling control plane keeps two named collections — launch
//! configurations and Auto Scaling groups — plus the scaling activities the
//! capacity changes of those groups produce and the instance refreshes started on
//! them. Every handler in `src/ops/` reads and writes THIS struct through
//! [`service_state`]; nothing in this crate holds a second copy of a resource.
//!
//! # The clock this service owes
//!
//! These changes are owed by the clock rather than by a later request, so they are
//! stored as a DUE INSTANT and materialised by [`AutoScalingState::materialize_due`]
//! (called by every handler, including the reporting ones, which may only
//! materialise what the clock already made due):
//!
//! * an instance Amazon EC2 Auto Scaling just launched is reported `Pending` and
//!   becomes `InService`; one it is scaling in is reported `Terminating` and then
//!   leaves the group ([`LIFECYCLE_TRANSITION`]);
//! * a capacity change that asked to honour the group's cooldown waits for that
//!   cooldown to elapse before it is applied (`HonorCooldown`);
//! * a running instance refresh replaces one instance of its group per
//!   [`INSTANCE_REFRESH_REPLACEMENT`] and succeeds once it has replaced them all
//!   ([`StoredInstanceRefresh`]);
//! * a refresh a caller cancelled is reported `Cancelling` and becomes `Cancelled`
//!   ([`INSTANCE_REFRESH_CANCELLATION`]);
//! * a refresh a caller rolled back replaces its OWN replacements back, one per
//!   [`INSTANCE_REFRESH_REPLACEMENT`], and is reported `RollbackSuccessful` once it
//!   has replaced them all ([`AutoScalingState::roll_back_instance_refresh`]).
//!
//! # The instances a group holds
//!
//! An instance of a group is an Amazon EC2 instance OF THE ACCOUNT: a capacity change
//! launches it in the account's own Amazon EC2 store and terminates it there when the
//! group scales in ([`ec2_instances`]), so every id a group reports is one Amazon EC2
//! resolves. This struct holds the group's own view of it — its lifecycle state, its
//! health and the zone it was placed in — and nothing that Amazon EC2's record owns.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AvailabilityZoneDistribution, AvailabilityZoneImpairmentPolicy, BlockDeviceMapping,
    CapacityReservationSpecification, DesiredConfiguration, InstanceLifecyclePolicy,
    InstanceMaintenancePolicy, InstanceMetadataOptions, LaunchTemplateSpecification,
    MixedInstancesPolicy, Operator, RefreshPreferences, Tag, TrafficSourceIdentifier,
};

/// The Availability Zones of the subnets a request's `VPCZoneIdentifier` names, read
/// from the account's own Amazon EC2 store.
pub mod ec2_subnets;

/// The Amazon EC2 instances a capacity change launches and terminates, in the
/// account's own Amazon EC2 store: an instance id a group reports names an instance of
/// the account, not one of this service's own invention.
pub mod ec2_instances;

/// The IAM instance profile the launch templates of a group name, resolved against the
/// account's own IAM store: a reference Amazon EC2 stores verbatim and this service, as
/// the consumer of the template, is what resolves.
pub mod iam_instance_profiles;

/// The instance lifecycle events this service publishes onto the account's default
/// EventBridge bus as a group launches and terminates its instances, and the clock pump
/// that puts them on the bus as they fall due.
pub mod lifecycle_events;

/// The scaling policies of a group — what `PutScalingPolicy` creates and
/// `DescribePolicies` reports — and the CloudWatch alarms a target-tracking policy is
/// driven by, which live in the account's own CloudWatch store.
pub mod scaling_policies;

pub use scaling_policies::{PolicyKey, StoredScalingPolicy};

pub use lifecycle_events::{
    drive_lifecycle_clock, InstanceLifecycle, InstanceLifecycleChange, PendingLifecycleEvent,
};

/// How long a lifecycle transition this service owes takes to come due.
///
/// Amazon EC2 Auto Scaling reports a freshly launched instance as `Pending` and an
/// instance being scaled in as `Terminating` before either settles, so a caller
/// that reads the group straight after a capacity change must still see the
/// transient state. The real wait depends on the instance; this emulator picks a
/// short, fixed span (chosen, not documented) and never sleeps for it.
pub const LIFECYCLE_TRANSITION: Duration = Duration::from_millis(500);

/// `DefaultCooldown` when the request omits it.
///
/// From the model's own documentation for `CreateAutoScalingGroup$DefaultCooldown`:
/// "Default: 300 seconds".
pub const DEFAULT_COOLDOWN_SECONDS: i32 = 300;

/// `HealthCheckGracePeriod` when the request omits it.
///
/// From the model's own documentation for
/// `CreateAutoScalingGroup$HealthCheckGracePeriod`: "Default: 0 seconds".
pub const DEFAULT_HEALTH_CHECK_GRACE_PERIOD: i32 = 0;

/// `HealthCheckType` when the request omits it.
///
/// From the model's own documentation for `CreateAutoScalingGroup$HealthCheckType`:
/// "EC2 is the default health check and cannot be disabled."
pub const DEFAULT_HEALTH_CHECK_TYPE: &str = "EC2";

/// `TerminationPolicies` when the request omits it: the `Default` policy, which
/// the model documents as a valid `TerminationPolicies` value and which the
/// service applies to a group that configures none.
pub const DEFAULT_TERMINATION_POLICY: &str = "Default";

// The three `LifecycleState` values this slice moves an instance through, spelled
// exactly as the model's `com.amazonaws.autoscaling#LifecycleState` enum does
// (smithy_query(query="shape",shape="LifecycleState")).
/// An instance that has been launched and is not yet in service.
pub const LIFECYCLE_PENDING: &str = "Pending";
/// An instance that is in service.
pub const LIFECYCLE_IN_SERVICE: &str = "InService";
/// An instance that is being scaled in.
pub const LIFECYCLE_TERMINATING: &str = "Terminating";

/// The `HealthStatus` of an instance the service has no failed health check for.
pub const HEALTH_STATUS_HEALTHY: &str = "Healthy";

/// How long ONE instance replacement of an instance refresh takes to come due.
///
/// A replacement is a termination and a launch, so it is owed on the same transient
/// span as any other lifecycle transition this service owes ([`LIFECYCLE_TRANSITION`]):
/// chosen, not documented, and never slept for. A refresh of a group holding several
/// instances therefore takes one span per instance, which is what lets a caller that
/// reads it straight after starting it still see it running.
pub const INSTANCE_REFRESH_REPLACEMENT: Duration = LIFECYCLE_TRANSITION;

/// How long the cancellation of an instance refresh takes to come due.
///
/// A cancelled refresh is reported `Cancelling` before it is reported `Cancelled`, so
/// a caller that reads it straight after cancelling it still sees the transient
/// status. The span is this service's own transient span ([`LIFECYCLE_TRANSITION`]):
/// chosen, not documented, and never slept for.
pub const INSTANCE_REFRESH_CANCELLATION: Duration = LIFECYCLE_TRANSITION;

// The `InstanceRefreshStatus` values this slice moves an instance refresh through,
// spelled exactly as the model's `com.amazonaws.autoscaling#InstanceRefreshStatus`
// enum does (smithy_query(query="shape",shape="InstanceRefreshStatus")).
/// A refresh whose request was created but which has not started replacing yet.
pub const REFRESH_PENDING: &str = "Pending";
/// A refresh that is replacing the group's instances.
pub const REFRESH_IN_PROGRESS: &str = "InProgress";
/// A refresh that replaced every instance it set out to replace.
pub const REFRESH_SUCCESSFUL: &str = "Successful";
/// A refresh that could not finish.
pub const REFRESH_FAILED: &str = "Failed";
/// A refresh whose cancellation has been asked for and has not completed.
pub const REFRESH_CANCELLING: &str = "Cancelling";
/// A refresh whose cancellation has completed.
pub const REFRESH_CANCELLED: &str = "Cancelled";
/// A refresh that is being rolled back: the replacements it made are themselves
/// being replaced, with the configuration the group was using before it started.
pub const REFRESH_ROLLBACK_IN_PROGRESS: &str = "RollbackInProgress";
/// A refresh whose rollback replaced every instance the refresh had replaced.
pub const REFRESH_ROLLBACK_SUCCESSFUL: &str = "RollbackSuccessful";
/// A refresh whose rollback could not finish.
pub const REFRESH_ROLLBACK_FAILED: &str = "RollbackFailed";

/// What `RollbackDetails$RollbackReason` reports for a rollback a CALLER asked for.
///
/// The member reports "the reason for this instance refresh rollback (for example,
/// whether a manual or automatic rollback was initiated)"
/// (`smithy_query(query="doc",shape="RollbackDetails",member="RollbackReason")`), and
/// every rollback this service starts is one a caller asked for: nothing here starts
/// one on its own.
pub const MANUAL_ROLLBACK_REASON: &str = "A manual rollback was initiated.";

/// The prefix an Amazon EC2 launch template's `ImageId` carries when it names a
/// Systems Manager parameter rather than an AMI id is Amazon EC2's own reading of that
/// member ([`ec2_instances::resolves_image_through_ssm_parameter`]), so nothing here
/// spells the form out a second time.
///
/// The `Version` of a launch template specification that means "whichever version is
/// the default when the instance is launched", from the model's own documentation for
/// `LaunchTemplateSpecification$Version`: "The version number, `$Latest`, or
/// `$Default`. … The default value is `$Default`" — so a specification carrying no
/// version at all uses this one.
pub const LAUNCH_TEMPLATE_VERSION_DEFAULT: &str = "$Default";

/// The `Version` that means "whichever version is the latest when the instance is
/// launched" (same documentation).
pub const LAUNCH_TEMPLATE_VERSION_LATEST: &str = "$Latest";

/// `Strategy` when a `StartInstanceRefresh` request omits it.
///
/// From the model's own documentation for `StartInstanceRefreshType$Strategy`: "The
/// default value is Rolling."
pub const DEFAULT_REFRESH_STRATEGY: &str = "Rolling";

/// The most instance refreshes this service retains; appending past it drops the
/// oldest.
///
/// A bound on this process, for the same reason as [`MAX_RETAINED_ACTIVITIES`]: a
/// group may be refreshed again and again and nothing removes a finished refresh, so
/// an unbounded log would grow for the life of the process. Amazon EC2 Auto Scaling
/// itself reports the refreshes "from the previous six weeks"
/// (`smithy_query(query="operation",operation="DescribeInstanceRefreshes")`), so a
/// retention bound is the shape of the real behaviour too.
pub const MAX_RETAINED_INSTANCE_REFRESHES: usize = 1_000;

// The `ScalingActivityStatusCode` values an activity this slice records moves
// through, from the model's `com.amazonaws.autoscaling#ScalingActivityStatusCode`.
/// A scaling activity that has started and not yet finished.
pub const ACTIVITY_IN_PROGRESS: &str = "InProgress";
/// A scaling activity that finished.
pub const ACTIVITY_SUCCESSFUL: &str = "Successful";
/// A scaling activity that did not place the instance it was started for.
pub const ACTIVITY_FAILED: &str = "Failed";

/// `Activity$AutoScalingGroupState` for a group that still exists.
pub const GROUP_STATE_IN_SERVICE: &str = "InService";
/// `Activity$AutoScalingGroupState` for a group that has been deleted.
pub const GROUP_STATE_DELETED: &str = "Deleted";

/// The most instances this emulator will hold for one Auto Scaling group, and so
/// the largest `MinSize`, `MaxSize` or `DesiredCapacity` any operation accepts.
///
/// A BOUND ON THIS PROCESS, not a value AWS documents. Every unit of capacity is
/// a stored instance plus a stored scaling activity, so a single well-formed
/// request carrying a capacity in the billions would exhaust this process's
/// memory and wedge every other caller: the bounds the operations check against
/// each other (`MinSize <= MaxSize`, `DesiredCapacity` inside them) are all
/// relative and cannot stop that. Amazon EC2 Auto Scaling bounds the same growth
/// with an adjustable account quota and answers `LimitExceededFault` past it;
/// this ceiling plays that role, at a size an in-memory emulator can hold.
pub const MAX_GROUP_CAPACITY: i32 = 1_000;

/// The most Auto Scaling groups this emulator will hold, for the same reason as
/// [`MAX_GROUP_CAPACITY`]: a bound on this process, chosen at the order of the
/// service's own adjustable per-Region quota, past which `CreateAutoScalingGroup`
/// answers its modelled `LimitExceededFault`.
pub const MAX_GROUPS: usize = 500;

/// The launch-configuration limit this service REPORTS for the account, through
/// `DescribeAccountLimits`'s `MaxNumberOfLaunchConfigurations`.
///
/// Same order as the service's own adjustable per-Region quota, like
/// [`MAX_GROUPS`]. Nothing enforces it: `CreateLaunchConfiguration` is refused
/// with `UnsupportedOperation`, so the reported count stays 0 and no caller can
/// approach the limit.
pub const MAX_LAUNCH_CONFIGURATIONS: usize = 200;

/// How fast the account's API call-rate bucket refills, in calls per second, as
/// `DescribeAccountLimits` reports it under `ThrottlingLimits/AccountMaxCallRate`.
///
/// The subtree is RECORDED behaviour rather than modelled: the live awsQuery
/// response carries it while `DescribeAccountLimitsAnswer` declares only the four
/// counts (`smithy_query(query="shape", shape="DescribeAccountLimitsAnswer")`), so
/// the shapes it is written through are hand-authored beside that answer in
/// `ops/types/describe_account_limits_types.rs`.
///
/// The figure is what the account REPORTS, like [`MAX_GROUPS`]: a rate rather than a
/// counter, so nothing about it moves as calls arrive, and nothing in this emulator
/// paces a caller against it. A client that reads its own call budget before pacing
/// its calls needs a rate and a burst size here instead of a null.
pub const ACCOUNT_MAX_CALL_RATE_REFILL_RATE: f64 = 5.0;

/// How many calls the account's API call-rate bucket holds — its burst size — as
/// `DescribeAccountLimits` reports it under `ThrottlingLimits/AccountMaxCallRate`,
/// on the same footing as [`ACCOUNT_MAX_CALL_RATE_REFILL_RATE`].
pub const ACCOUNT_MAX_CALL_RATE_BUCKET_SIZE: i32 = 30;

/// The most target groups ONE call may name, which bounds both the legacy
/// target-group attach and the legacy detach.
///
/// From the model's own documentation for
/// `DetachLoadBalancerTargetGroupsType$TargetGroupARNs`: "You can specify up to 10
/// target groups." The member carries no modelled length, so the bound is the
/// service's own and is checked by the handler.
pub const MAX_TARGET_GROUPS_PER_CALL: usize = 10;

/// The most scaling activities this service retains; appending past it drops the
/// oldest.
///
/// Also a bound on this process: activities are appended by every capacity change
/// and never removed by one, so an unbounded log would grow for the life of the
/// process. Amazon EC2 Auto Scaling itself only reports activities from the last
/// six weeks, so a retention bound is the shape of the real behaviour too.
pub const MAX_RETAINED_ACTIVITIES: usize = 10_000;

/// How far back a read of the scaling activities reaches.
///
/// From the model's own documentation for `DescribeScalingActivitiesType$ActivityIds`:
/// "Only activities started within the last six weeks can be returned regardless of the
/// activity IDs specified" — stated again on each of the start-time filters, so it is a
/// window the read itself applies rather than a property of any one criterion.
pub const ACTIVITY_WINDOW: Duration = Duration::from_secs(6 * 7 * 24 * 60 * 60);

/// The most activity ids one read of the scaling activities may name.
///
/// From the model's own documentation for `DescribeScalingActivitiesType$ActivityIds`:
/// "Array Members: Maximum number of 50 IDs." The member carries no modelled length, so
/// the bound is the service's own and is checked by the handler.
pub const MAX_ACTIVITY_IDS_PER_CALL: usize = 50;

/// The message a read of the scaling activities refuses more than
/// [`MAX_ACTIVITY_IDS_PER_CALL`] ids with: the standard sentence this service states for
/// a member value outside its bounds, naming the member as the wire names it and the
/// bound the request missed.
pub fn too_many_activity_ids_message(count: usize) -> String {
    format!(
        "1 validation error detected: Value with {count} items at 'activityIds' failed to \
         satisfy constraint: Member must have length less than or equal to \
         {MAX_ACTIVITY_IDS_PER_CALL}"
    )
}

/// A stored launch configuration — the `LaunchConfiguration` resource.
///
/// NOTHING in this crate constructs one: `CreateLaunchConfiguration` is refused
/// with `UnsupportedOperation`, so [`ComAmazonawsAutoscalingAutoscaling20110101State::launch_configurations`]
/// stays empty. This is the shape its readers project from
/// (`DescribeLaunchConfigurations`, a group's launch source), which is why they
/// report what the store holds instead of assuming it is empty.
#[derive(Debug, Clone)]
pub struct StoredLaunchConfiguration {
    pub name: String,
    pub arn: String,
    pub image_id: String,
    pub instance_type: String,
    pub key_name: Option<String>,
    pub security_groups: Vec<String>,
    pub classic_link_vpc_id: Option<String>,
    pub classic_link_vpc_security_groups: Vec<String>,
    pub user_data: Option<String>,
    pub kernel_id: Option<String>,
    pub ramdisk_id: Option<String>,
    pub spot_price: Option<String>,
    pub iam_instance_profile: Option<String>,
    pub block_device_mappings: Vec<BlockDeviceMapping>,
    pub metadata_options: Option<InstanceMetadataOptions>,
    /// `InstanceMonitoring$Enabled`. The model documents the default as `true`
    /// (detailed monitoring).
    pub instance_monitoring_enabled: bool,
    pub ebs_optimized: bool,
    pub associate_public_ip: Option<bool>,
    pub placement_tenancy: Option<String>,
    pub created_time: SystemTime,
}

/// One instance attached to a group.
///
/// [`StoredInstance::instance_id`] is the id of an Amazon EC2 instance OF THE ACCOUNT,
/// launched through [`ec2_instances`] by the capacity change that placed it, so every
/// id a group reports resolves in the account's own Amazon EC2 state.
#[derive(Debug, Clone)]
pub struct StoredInstance {
    pub instance_id: String,
    pub availability_zone: String,
    pub lifecycle_state: String,
    pub health_status: String,
    pub launch_configuration_name: Option<String>,
    pub launch_template: Option<LaunchTemplateSpecification>,
    pub instance_type: Option<String>,
    pub image_id: Option<String>,
    pub protected_from_scale_in: bool,
    /// When this instance's transient lifecycle state comes due, if it is in one.
    pub transition_due: Option<SystemTime>,
}

/// A capacity change deferred until the group's cooldown has elapsed, because the
/// request asked for the cooldown to be honoured.
#[derive(Debug, Clone)]
pub struct PendingCapacity {
    pub desired_capacity: i32,
    pub due: SystemTime,
    pub cause: String,
}

/// A stored Auto Scaling group — the `AutoScalingGroup` resource, as created and
/// updated.
#[derive(Debug, Clone)]
pub struct StoredGroup {
    pub name: String,
    pub arn: String,
    /// The Region the group was created in, which is the Region the instances it
    /// launches run in.
    pub region: String,
    pub launch_configuration_name: Option<String>,
    pub launch_template: Option<LaunchTemplateSpecification>,
    pub mixed_instances_policy: Option<MixedInstancesPolicy>,
    pub min_size: i32,
    pub max_size: i32,
    pub desired_capacity: i32,
    pub default_cooldown: i32,
    pub availability_zones: Vec<String>,
    pub availability_zone_ids: Option<Vec<String>>,
    pub load_balancer_names: Option<Vec<String>>,
    pub target_group_arns: Option<Vec<String>>,
    pub health_check_type: String,
    pub health_check_grace_period: Option<i32>,
    pub placement_group: Option<String>,
    pub vpc_zone_identifier: Option<String>,
    pub termination_policies: Vec<String>,
    pub new_instances_protected_from_scale_in: bool,
    pub service_linked_role_arn: Option<String>,
    pub max_instance_lifetime: Option<i32>,
    pub capacity_rebalance: Option<bool>,
    pub context: Option<String>,
    pub desired_capacity_type: Option<String>,
    pub default_instance_warmup: Option<i32>,
    pub traffic_sources: Option<Vec<TrafficSourceIdentifier>>,
    pub instance_maintenance_policy: Option<InstanceMaintenancePolicy>,
    pub availability_zone_distribution: Option<AvailabilityZoneDistribution>,
    pub availability_zone_impairment_policy: Option<AvailabilityZoneImpairmentPolicy>,
    pub capacity_reservation_specification: Option<CapacityReservationSpecification>,
    pub instance_lifecycle_policy: Option<InstanceLifecyclePolicy>,
    pub operator: Option<Operator>,
    pub deletion_protection: Option<String>,
    pub tags: Vec<Tag>,
    pub status: Option<String>,
    pub created_time: SystemTime,
    pub instances: Vec<StoredInstance>,
    /// End of the group's cooldown after its last applied capacity change.
    pub cooldown_until: Option<SystemTime>,
    /// A capacity change waiting for [`Self::cooldown_until`].
    pub pending_capacity: Option<PendingCapacity>,
}

impl StoredGroup {
    /// The instances that count towards the group's capacity: everything that is
    /// not on its way out.
    pub fn active_instances(&self) -> impl Iterator<Item = &StoredInstance> {
        self.instances
            .iter()
            .filter(|i| i.lifecycle_state != LIFECYCLE_TERMINATING)
    }

    /// How many instances count towards the group's capacity.
    pub fn active_instance_count(&self) -> i32 {
        i32::try_from(self.active_instances().count()).unwrap_or(i32::MAX)
    }

    /// Whether the group is inside the cooldown that followed its last capacity
    /// change.
    pub fn in_cooldown(&self, now: SystemTime) -> bool {
        self.cooldown_until.is_some_and(|until| now < until)
    }

    /// How many more instances this group can be given before it reaches
    /// [`MAX_GROUP_CAPACITY`].
    ///
    /// Counts the instances still `Terminating`: they remain stored until the
    /// clock retires them, so they occupy the ceiling too. Without that, a caller
    /// alternating the capacity faster than the transition comes due could stack
    /// a fresh ceiling's worth of instances on top of the ones still leaving.
    pub fn launch_headroom(&self) -> i32 {
        MAX_GROUP_CAPACITY.saturating_sub(i32::try_from(self.instances.len()).unwrap_or(i32::MAX))
    }

    /// How many instances this group must launch to reach `desired_capacity`.
    pub fn launches_needed(&self, desired_capacity: i32) -> i32 {
        (desired_capacity - self.active_instance_count()).max(0)
    }

    /// The target groups attached to this group, as the group itself holds them:
    /// empty for a group that has none, so a caller never distinguishes "no
    /// attachments" from an absent member.
    pub fn attached_target_groups(&self) -> &[String] {
        self.target_group_arns.as_deref().unwrap_or_default()
    }
}

/// A stored scaling activity — the `Activity` resource a capacity change produces.
#[derive(Debug, Clone)]
pub struct StoredActivity {
    pub activity_id: String,
    pub auto_scaling_group_name: String,
    pub auto_scaling_group_arn: Option<String>,
    pub description: Option<String>,
    pub cause: String,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub status_code: String,
    pub progress: i32,
    pub details: Option<String>,
    pub auto_scaling_group_state: Option<String>,
    /// When this activity finishes, if it has not finished yet.
    pub completes_at: Option<SystemTime>,
}

/// A stored instance refresh — the `InstanceRefresh` resource `StartInstanceRefresh`
/// creates and `DescribeInstanceRefreshes` reports.
///
/// A refresh replaces the instances of ONE group, one at a time: the instances it has
/// still to replace are [`Self::pending_instance_ids`], and the clock brings the next
/// replacement due at [`Self::next_step_due`]
/// ([`AutoScalingState::advance_instance_refreshes`]). The progress a reader sees is
/// what those replacements have actually done.
#[derive(Debug, Clone)]
pub struct StoredInstanceRefresh {
    pub instance_refresh_id: String,
    pub auto_scaling_group_name: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    /// `RefreshStrategy`, as requested or [`DEFAULT_REFRESH_STRATEGY`].
    pub strategy: String,
    pub preferences: Option<RefreshPreferences>,
    pub desired_configuration: Option<DesiredConfiguration>,
    /// How many instances the refresh set out to replace, which is what its
    /// percentage complete is a percentage OF.
    pub instances_at_start: i32,
    /// The instances it has still to replace, in the order it replaces them. Once a
    /// rollback is under way this holds what the ROLLBACK has still to replace — the
    /// replacements the refresh had made, which the rollback replaces back.
    pub pending_instance_ids: Vec<String>,
    /// The instances the refresh LAUNCHED as replacements, in replacement order: the
    /// only instances of the group the refresh changed, and so exactly what a rollback
    /// has to replace back.
    pub replaced_instance_ids: Vec<String>,
    /// `PercentageComplete`.
    pub percentage_complete: i32,
    /// `InstancesToUpdate`.
    pub instances_to_update: i32,
    /// The rollback of this refresh, once one has been asked for: `RollbackDetails`.
    pub rollback: Option<StoredRollback>,
    /// When the next replacement — or the refresh's own completion — comes due;
    /// `None` once it has reached a terminal status.
    pub next_step_due: Option<SystemTime>,
}

/// The rollback of an instance refresh — the `RollbackDetails` a refresh reports once
/// one has been asked for: why it was started, when it started, and the progress the
/// refresh had made at that instant.
#[derive(Debug, Clone)]
pub struct StoredRollback {
    /// `RollbackReason`.
    pub reason: String,
    /// `RollbackStartTime`.
    pub start_time: SystemTime,
    /// `PercentageCompleteOnRollback`: "the value of PercentageComplete at the time the
    /// rollback started".
    pub percentage_complete_on_rollback: i32,
    /// `InstancesToUpdateOnRollback`: "the value of InstancesToUpdate at the time the
    /// rollback started".
    pub instances_to_update_on_rollback: i32,
}

/// Why a group's instance refresh could not be rolled back, for the operation to
/// answer the error class each situation belongs to.
#[derive(Debug)]
pub enum RollbackRefusal {
    /// The group has no instance refresh in progress, and no rollback either, so there
    /// is nothing for a rollback to act on.
    NoneInProgress,
    /// The refresh in progress cannot be reversed; the sentence says why.
    Irreversible(String),
}

impl StoredInstanceRefresh {
    /// Whether this refresh has not reached a terminal status, which is what makes it
    /// the group's ACTIVE refresh: a group may have only one at a time, and one whose
    /// cancellation has not completed still holds its group.
    pub fn is_active(&self) -> bool {
        self.is_in_progress() || self.status == REFRESH_CANCELLING || self.is_rolling_back()
    }

    /// Whether this refresh is BEING ROLLED BACK: the status that means its own
    /// replacements are being replaced back, which is a rollback a later call finds
    /// already running rather than one it starts.
    pub fn is_rolling_back(&self) -> bool {
        self.status == REFRESH_ROLLBACK_IN_PROGRESS
    }

    /// Whether a cancellation acts on this refresh: `CancelInstanceRefresh` "cancels an
    /// instance refresh OR ROLLBACK that is in progress"
    /// (`smithy_query(query="operation",operation="CancelInstanceRefresh")`), so the
    /// replacements a rollback is part way through are stopped by the same call that
    /// stops a refresh's own.
    pub fn is_cancellable(&self) -> bool {
        self.is_in_progress() || self.is_rolling_back()
    }

    /// Whether this refresh is IN PROGRESS: the statuses that mean it is still
    /// replacing instances, which is what a cancellation acts on.
    pub fn is_in_progress(&self) -> bool {
        self.status == REFRESH_PENDING || self.status == REFRESH_IN_PROGRESS
    }
}

/// Per-service state for com-amazonaws-autoscaling-autoscaling-2011-01-01. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAutoscalingAutoscaling20110101State {
    /// Launch configurations, keyed by `LaunchConfigurationName`.
    ///
    /// Empty for the life of the process: `CreateLaunchConfiguration` is refused
    /// with `UnsupportedOperation` because the account this emulator models cannot
    /// create a launch configuration at all. The collection stays here because
    /// `DescribeLaunchConfigurations`, `DeleteLaunchConfiguration` and a group's
    /// `LaunchConfigurationName` all answer FROM it rather than from a constant,
    /// so each of them reports what the store holds.
    pub launch_configurations: BTreeMap<String, StoredLaunchConfiguration>,
    /// Auto Scaling groups, keyed by `AutoScalingGroupName`.
    pub groups: BTreeMap<String, StoredGroup>,
    /// The scaling policies of those groups, keyed by the group they belong to and their
    /// name within it ([`PolicyKey`]), so the collection iterates group by group and, in
    /// a group, policy name by policy name. A group's policies are removed with the
    /// group: a policy only exists against a group that does.
    pub scaling_policies: BTreeMap<PolicyKey, StoredScalingPolicy>,
    /// Every scaling activity this service has recorded, oldest first.
    pub activities: Vec<StoredActivity>,
    /// Every instance refresh this service has recorded, oldest first.
    pub instance_refreshes: Vec<StoredInstanceRefresh>,
    /// Source of activity ids.
    pub next_activity_seq: u64,
    /// Source of the opaque id segment of a resource ARN.
    pub next_resource_seq: u64,
    /// The instance lifecycle events this store has not published to EventBridge yet,
    /// oldest first, each due at the instant the step it reports completes
    /// ([`lifecycle_events`]). At most [`lifecycle_events::MAX_PENDING_EVENTS`] entries,
    /// the oldest dropped first.
    pending_lifecycle_events: VecDeque<PendingLifecycleEvent>,
    /// Whether a clock pump ([`drive_lifecycle_clock`]) is already waiting on this store's
    /// due instants, so at most one runs per store.
    lifecycle_clock_pump_running: bool,
    /// The generation of the running pump's claim, so a pump releases its OWN claim and
    /// never a later pump's.
    lifecycle_clock_pump_generation: u64,
    /// The running pump's task handle, kept so the pump it replaces is aborted rather than
    /// merely forgotten.
    lifecycle_clock_pump: Option<::tokio::task::JoinHandle<()>>,
}

/// A shorter name for this service's state, for use inside the crate.
pub type AutoScalingState = ComAmazonawsAutoscalingAutoscaling20110101State;

/// Render `seq` in the 8-4-4-4-12 shape AWS resource ids use, so an id this
/// emulator mints has the form a client expects while staying deterministic
/// within a process (version nibble 4, variant nibble 8, as RFC 4122 lays them out).
fn uuid_shaped(seq: u64) -> String {
    format!("{:08x}-0000-4000-8000-{:012x}", seq & 0xffff_ffff, seq)
}

impl AutoScalingState {
    /// Mint the next activity id.
    pub fn mint_activity_id(&mut self) -> String {
        self.next_activity_seq += 1;
        uuid_shaped(self.next_activity_seq)
    }

    /// Mint an opaque resource id: the segment an Auto Scaling ARN carries, and the
    /// id an instance refresh is tracked by. ONE sequence, so no two resources of
    /// this service are ever minted the same id.
    pub fn mint_resource_id(&mut self) -> String {
        self.next_resource_seq += 1;
        uuid_shaped(self.next_resource_seq)
    }

    /// Remove every scaling policy of `group_name` and answer them, so the caller can
    /// retire the CloudWatch alarms they held once this store's lock is released.
    ///
    /// A policy only exists against a group that exists, so deleting the group deletes
    /// its policies: nothing a later put or read of the same name sees is left behind.
    pub fn take_group_policies(
        &mut self,
        group_name: &str,
    ) -> BTreeMap<PolicyKey, StoredScalingPolicy> {
        let keys: Vec<PolicyKey> = self
            .scaling_policies
            .keys()
            .filter(|key| key.auto_scaling_group_name == group_name)
            .cloned()
            .collect();
        keys.into_iter()
            .filter_map(|key| {
                self.scaling_policies
                    .remove(&key)
                    .map(|policy| (key, policy))
            })
            .collect()
    }

    /// Store one instance refresh under a freshly minted id and answer that id,
    /// keeping the log bounded.
    ///
    /// The ONE writer of [`Self::instance_refreshes`], so every refresh's id comes
    /// from one minter and [`MAX_RETAINED_INSTANCE_REFRESHES`] is applied in one
    /// place: `instance_refresh_id` on the way in is ignored.
    pub fn push_instance_refresh(&mut self, refresh: StoredInstanceRefresh) -> String {
        let instance_refresh_id = self.mint_resource_id();
        self.instance_refreshes.push(StoredInstanceRefresh {
            instance_refresh_id: instance_refresh_id.clone(),
            ..refresh
        });
        // Keep the log bounded: the oldest refresh is the first to go.
        if self.instance_refreshes.len() > MAX_RETAINED_INSTANCE_REFRESHES {
            let excess = self.instance_refreshes.len() - MAX_RETAINED_INSTANCE_REFRESHES;
            self.instance_refreshes.drain(..excess);
        }
        instance_refresh_id
    }

    /// The group's ACTIVE instance refresh, if it has one: the one still running.
    pub fn active_instance_refresh(&self, group_name: &str) -> Option<&StoredInstanceRefresh> {
        self.instance_refreshes
            .iter()
            .find(|refresh| refresh.auto_scaling_group_name == group_name && refresh.is_active())
    }

    /// Cancel the group's in-progress instance refresh at `now`, answering the id of
    /// the refresh the cancellation acted on, or `None` when the group has none in
    /// progress.
    ///
    /// A cancellation starts no further replacement and rolls none back, so the
    /// progress the refresh had already made stands: it moves to `Cancelling` and the
    /// clock brings it to `Cancelled` ([`INSTANCE_REFRESH_CANCELLATION`]). With
    /// `wait_for_transitioning_instances`, that is owed only once the group's in-flight
    /// launches and terminations have themselves settled; without it, the cancellation
    /// does not wait for them.
    pub fn cancel_instance_refresh(
        &mut self,
        group_name: &str,
        wait_for_transitioning_instances: bool,
        now: SystemTime,
    ) -> Option<String> {
        // The in-flight launches and terminations of the group: the instances whose
        // transient lifecycle state has not come due yet.
        let in_flight_settled = if wait_for_transitioning_instances {
            self.groups
                .get(group_name)
                .into_iter()
                .flat_map(|group| group.instances.iter())
                .filter_map(|instance| instance.transition_due)
                .chain([now])
                .max()
                .unwrap_or(now)
        } else {
            now
        };
        let refresh = self
            .instance_refreshes
            .iter_mut()
            .find(|refresh| {
                refresh.auto_scaling_group_name == group_name && refresh.is_cancellable()
            })?;
        refresh.status = REFRESH_CANCELLING.to_string();
        refresh.next_step_due = Some(in_flight_settled + INSTANCE_REFRESH_CANCELLATION);
        Some(refresh.instance_refresh_id.clone())
    }

    /// Start the rollback of the group's instance refresh at `now`, answering the id of
    /// the refresh the rollback acts on.
    ///
    /// A rollback "cancels an instance refresh that is in progress and rolls back any
    /// changes that it made": the replacements the refresh had made are themselves
    /// replaced, from the group's own launch source — the configuration it was using
    /// before the refresh started, since a refresh's desired configuration reaches the
    /// group only once that refresh has succeeded. The replacements are owed by the
    /// clock, one per [`INSTANCE_REFRESH_REPLACEMENT`], exactly as the refresh's own
    /// were ([`Self::advance_instance_refreshes`]).
    ///
    /// A rollback already under way is the one this call finds: it answers that
    /// refresh's id and restarts nothing, so asking twice replaces no instance twice.
    pub fn roll_back_instance_refresh<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        group_name: &str,
        now: SystemTime,
    ) -> Result<String, RollbackRefusal> {
        if let Some(refresh) = self
            .instance_refreshes
            .iter()
            .find(|refresh| refresh.auto_scaling_group_name == group_name && refresh.is_rolling_back())
        {
            return Ok(refresh.instance_refresh_id.clone());
        }
        let Some(index) = self.instance_refreshes.iter().position(|refresh| {
            refresh.auto_scaling_group_name == group_name && refresh.is_in_progress()
        }) else {
            return Err(RollbackRefusal::NoneInProgress);
        };
        if let Some(reason) = self.irreversible_reason(engine, group_name, index) {
            return Err(RollbackRefusal::Irreversible(reason));
        }

        // What the rollback has to replace back: the replacements the refresh made that
        // the group still holds. One the group has since lost is not replaced again —
        // the rollback restores the configuration, not a capacity change somebody else
        // made.
        let held: Vec<String> = self
            .groups
            .get(group_name)
            .into_iter()
            .flat_map(StoredGroup::active_instances)
            .map(|instance| instance.instance_id.clone())
            .collect();
        let Some(refresh) = self.instance_refreshes.get_mut(index) else {
            return Err(RollbackRefusal::NoneInProgress);
        };
        refresh.pending_instance_ids = refresh
            .replaced_instance_ids
            .iter()
            .filter(|instance_id| held.contains(instance_id))
            .cloned()
            .collect();
        refresh.rollback = Some(StoredRollback {
            reason: MANUAL_ROLLBACK_REASON.to_string(),
            start_time: now,
            percentage_complete_on_rollback: refresh.percentage_complete,
            instances_to_update_on_rollback: refresh.instances_to_update,
        });
        refresh.status = REFRESH_ROLLBACK_IN_PROGRESS.to_string();
        refresh.next_step_due = Some(now + INSTANCE_REFRESH_REPLACEMENT);
        Ok(refresh.instance_refresh_id.clone())
    }

    /// Why the refresh at `index` cannot be rolled back, when it cannot: the sentence
    /// the refusal carries, or `None` for a refresh a rollback can reverse.
    ///
    /// The situations are the ones the error class itself names: "a desired
    /// configuration was not found or an incompatible launch template (uses a Systems
    /// Manager parameter instead of an AMI ID) or launch template version (`$Latest` or
    /// `$Default`) is present on the Auto Scaling group"
    /// (`smithy_query(query="shape",shape="IrreversibleInstanceRefreshFault")`). Each is
    /// a configuration the pre-refresh state cannot be reconstructed from: without a
    /// desired configuration the refresh changed nothing to go back from, and a floating
    /// version or a parameter-resolved image pins no version to go back TO.
    fn irreversible_reason<Eng: Send + Sync + 'static>(
        &self,
        engine: &Eng,
        group_name: &str,
        index: usize,
    ) -> Option<String> {
        let refresh = self.instance_refreshes.get(index)?;
        if refresh.desired_configuration.is_none() {
            return Some(ROLLBACK_WITHOUT_DESIRED_CONFIGURATION_MESSAGE.to_string());
        }
        let group = self.groups.get(group_name)?;
        let specification = launch_template_of(
            group.launch_template.as_ref(),
            group.mixed_instances_policy.as_ref(),
        )?;
        let version = specification
            .version
            .as_deref()
            .unwrap_or(LAUNCH_TEMPLATE_VERSION_DEFAULT);
        if version == LAUNCH_TEMPLATE_VERSION_DEFAULT || version == LAUNCH_TEMPLATE_VERSION_LATEST {
            return Some(floating_launch_template_version_message(version));
        }
        // What the template's own version says the instances run, from the account's
        // Amazon EC2 store: an image named through a Systems Manager parameter is not an
        // AMI id, so the image the group was running before the refresh is not pinned
        // either.
        ec2_instances::resolves_image_through_ssm_parameter(engine, specification)
            .then(|| SSM_PARAMETER_IMAGE_ID_MESSAGE.to_string())
    }

    /// Append a scaling activity that finishes at `completes_at`, answering its id.
    #[allow(clippy::too_many_arguments)]
    pub fn record_activity(
        &mut self,
        group_name: &str,
        group_arn: &str,
        description: String,
        cause: String,
        group_state: &str,
        start_time: SystemTime,
        completes_at: Option<SystemTime>,
    ) -> String {
        let finished = completes_at.is_none();
        let status_code = if finished {
            ACTIVITY_SUCCESSFUL
        } else {
            ACTIVITY_IN_PROGRESS
        };
        self.push_activity(StoredActivity {
            activity_id: String::new(),
            auto_scaling_group_name: group_name.to_string(),
            auto_scaling_group_arn: Some(group_arn.to_string()),
            description: Some(description),
            cause,
            start_time,
            end_time: if finished { Some(start_time) } else { None },
            status_code: status_code.to_string(),
            progress: if finished { 100 } else { 50 },
            details: None,
            auto_scaling_group_state: Some(group_state.to_string()),
            completes_at,
        })
    }

    /// Append a scaling activity that FAILED: the capacity change asked for an
    /// instance Amazon EC2 did not place, so the group reports no instance for it and
    /// the activity it recorded says so instead of claiming a launch that never
    /// happened.
    pub fn record_failed_activity(
        &mut self,
        group_name: &str,
        group_arn: &str,
        description: String,
        cause: String,
        start_time: SystemTime,
    ) -> String {
        self.push_activity(StoredActivity {
            activity_id: String::new(),
            auto_scaling_group_name: group_name.to_string(),
            auto_scaling_group_arn: Some(group_arn.to_string()),
            description: Some(description),
            cause,
            start_time,
            // Nothing is owed on a launch that failed: it ended when it failed.
            end_time: Some(start_time),
            status_code: ACTIVITY_FAILED.to_string(),
            progress: 100,
            details: None,
            auto_scaling_group_state: Some(GROUP_STATE_IN_SERVICE.to_string()),
            completes_at: None,
        })
    }

    /// Store one activity under a freshly minted id, keeping the log bounded, and answer
    /// that id.
    ///
    /// The ONE writer of [`Self::activities`], so the id every activity carries comes
    /// from one minter and the retention bound is applied in one place however the
    /// activity ended: `activity_id` on the way in is ignored.
    fn push_activity(&mut self, activity: StoredActivity) -> String {
        let activity_id = self.mint_activity_id();
        self.activities.push(StoredActivity {
            activity_id: activity_id.clone(),
            ..activity
        });
        // Keep the activity log bounded: the oldest activity is the first to go.
        if self.activities.len() > MAX_RETAINED_ACTIVITIES {
            let excess = self.activities.len() - MAX_RETAINED_ACTIVITIES;
            self.activities.drain(..excess);
        }
        activity_id
    }

    /// Materialise everything the clock has already made due at `now`: deferred
    /// capacity changes whose cooldown has elapsed, the replacements a running
    /// instance refresh owes, instance lifecycle transitions, and activity
    /// completions.
    ///
    /// Every handler calls this first — including the reporting ones, which may
    /// write exactly this much and nothing else, because each of these changes was
    /// owed at `now` whether or not anybody called.
    ///
    /// `engine` is the shared engine the handler holds: a deferred capacity change
    /// launches and terminates the account's own Amazon EC2 instances when it comes
    /// due, exactly as the change a request applies straight away does.
    pub fn materialize_due<Eng: Send + Sync + 'static>(&mut self, engine: &Eng, now: SystemTime) {
        let due_changes: Vec<(String, i32, String, SystemTime)> = self
            .groups
            .values()
            .filter_map(|group| {
                group
                    .pending_capacity
                    .as_ref()
                    .filter(|pending| pending.due <= now)
                    .map(|pending| {
                        (
                            group.name.clone(),
                            // The group's bounds may have been narrowed while the
                            // change waited, and a group never holds a capacity
                            // outside its own bounds, so the deferred value is
                            // brought inside the ones in force when it is applied.
                            pending
                                .desired_capacity
                                .clamp(group.min_size, group.max_size),
                            pending.cause.clone(),
                            pending.due,
                        )
                    })
            })
            .collect();
        // Applied AT the instant it came due, not at the instant somebody looked:
        // the instances it launches and the cooldown it starts are anchored there,
        // so the passes below retire whatever that instant has itself made due and
        // the state a caller sees does not depend on when they read.
        for (name, desired_capacity, cause, due) in due_changes {
            self.apply_capacity(engine, &name, desired_capacity, &cause, due);
        }
        // Before the lifecycle pass below, so an instance a replacement launched at
        // an instant already past leaves its transient state in this same call.
        self.advance_instance_refreshes(engine, now);

        for group in self.groups.values_mut() {
            for instance in &mut group.instances {
                if instance.transition_due.is_some_and(|due| due <= now) {
                    instance.transition_due = None;
                    if instance.lifecycle_state == LIFECYCLE_PENDING {
                        instance.lifecycle_state = LIFECYCLE_IN_SERVICE.to_string();
                    }
                }
            }
            // An instance whose `Terminating` span has elapsed has left the group.
            group
                .instances
                .retain(|i| i.lifecycle_state != LIFECYCLE_TERMINATING || i.transition_due.is_some());
        }

        for activity in &mut self.activities {
            if activity.completes_at.is_some_and(|at| at <= now) {
                activity.end_time = activity.completes_at;
                activity.completes_at = None;
                activity.status_code = ACTIVITY_SUCCESSFUL.to_string();
                activity.progress = 100;
            }
        }
    }

    /// Set a group's desired capacity and move its instances to match: launch what
    /// is missing (`Pending`, in service once [`LIFECYCLE_TRANSITION`] is due) and
    /// scale in what is surplus (`Terminating`, gone once it is due). Each launch
    /// and each termination records its own scaling activity, and the group enters
    /// its cooldown.
    ///
    /// The instances are the account's OWN Amazon EC2 instances: the launch goes
    /// through [`ec2_instances::launch`], which mints them in the account's Amazon EC2
    /// store from the group's launch source, and the scale-in terminates those same
    /// instances there — so every id the group reports resolves in Amazon EC2. A unit
    /// of capacity Amazon EC2 could not place records a failed activity instead of an
    /// instance, since there is no instance to report.
    ///
    /// Does nothing when `group_name` names no group. The capacity is clamped to
    /// [`MAX_GROUP_CAPACITY`] here as well as refused by the handlers, so the
    /// allocation below is bounded however it is reached.
    pub fn apply_capacity<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        group_name: &str,
        desired_capacity: i32,
        cause: &str,
        now: SystemTime,
    ) {
        let desired_capacity = desired_capacity.clamp(0, MAX_GROUP_CAPACITY);
        let Some(group) = self.groups.get(group_name) else {
            return;
        };
        let arn = group.arn.clone();
        let zoneless = group.availability_zones.is_empty();
        let cooldown = group.default_cooldown.max(0);
        let previous = group.active_instance_count();
        let headroom = group.launch_headroom();

        let to_launch = if zoneless {
            // Without a zone to place an instance in, nothing can be launched; the
            // handlers refuse such a request before reaching here.
            0
        } else {
            // Bounded by the ceiling AND by what the group can still hold, so
            // neither one request nor a run of them can grow the stored instances
            // past MAX_GROUP_CAPACITY. The handlers refuse a change that does not
            // fit; this is the floor under them.
            (desired_capacity - previous)
                .clamp(0, MAX_GROUP_CAPACITY)
                .min(headroom)
        };
        let to_terminate = (previous - desired_capacity).max(0);
        let due = now + LIFECYCLE_TRANSITION;

        let wanted = usize::try_from(to_launch).unwrap_or(0);
        let (launched_ids, unplaced) = self.launch_into_group(
            engine,
            &GroupLaunch {
                group_name,
                count: wanted,
                due,
                // A capacity change launches from the group's own launch source.
                desired_configuration: None,
            },
        );

        let group = self
            .groups
            .get_mut(group_name)
            .expect("the group was present a moment ago");
        group.desired_capacity = desired_capacity;
        group.pending_capacity = None;
        group.cooldown_until = Some(now + Duration::from_secs(u64::from(cooldown.unsigned_abs())));

        let terminated_ids = self.scale_in_selection(group_name, to_terminate);
        self.terminate_group_instances(engine, group_name, &terminated_ids, due);

        for instance_id in launched_ids {
            self.record_instance_lifecycle_change(
                engine,
                &InstanceLifecycleChange {
                    kind: InstanceLifecycle::Launched,
                    group_name,
                    group_arn: &arn,
                    instance_id: &instance_id,
                    cause,
                    start_time: now,
                    due,
                },
            );
        }
        for instance_id in terminated_ids {
            self.record_instance_lifecycle_change(
                engine,
                &InstanceLifecycleChange {
                    kind: InstanceLifecycle::Terminated,
                    group_name,
                    group_arn: &arn,
                    instance_id: &instance_id,
                    cause,
                    start_time: now,
                    due,
                },
            );
        }
        for _ in 0..unplaced {
            self.record_failed_activity(
                group_name,
                &arn,
                "Launching a new EC2 instance failed: the account could not place it"
                    .to_string(),
                cause.to_string(),
                now,
            );
        }
    }

    /// Launch instances into the group through the account's own Amazon EC2 store and
    /// give the group its own view of each, answering the ids launched and how many
    /// of the ones asked for Amazon EC2 could not place.
    ///
    /// THE ONE launch path: a capacity change and an instance refresh's replacement
    /// both reach Amazon EC2 through here, so one group's instances are launched from
    /// one resolution of its launch source however the launch was asked for. A launch
    /// Amazon EC2 could not place gives the group no instance — an id it reported
    /// would resolve nowhere — and is counted for the caller to record.
    fn launch_into_group<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        request: &GroupLaunch<'_>,
    ) -> (Vec<String>, usize) {
        let Some(group) = self.groups.get(request.group_name) else {
            return (Vec::new(), request.count);
        };
        let region = group.region.clone();
        let zones = group.availability_zones.clone();
        let vpc_zone_identifier = group.vpc_zone_identifier.clone();
        let tags = group.tags.clone();
        let protected = group.new_instances_protected_from_scale_in;
        let placed = group.instances.len();
        let headroom = usize::try_from(group.launch_headroom()).unwrap_or(0);
        // What the instances run: the desired configuration of the instance refresh
        // that asked for them, when one did — its replacements run the new
        // configuration — and otherwise the group's own launch source.
        let (launch_configuration_name, launch_template, mixed_instances_policy) =
            match request.desired_configuration {
                Some(desired) => (
                    None,
                    desired.launch_template.clone(),
                    desired.mixed_instances_policy.clone(),
                ),
                None => (
                    group.launch_configuration_name.clone(),
                    group.launch_template.clone(),
                    group.mixed_instances_policy.clone(),
                ),
            };
        let launch_configuration = launch_configuration_name
            .as_ref()
            .and_then(|name| self.launch_configurations.get(name))
            .cloned();

        // Never more than the group can still hold, so neither one request nor a run
        // of them can grow the stored instances past MAX_GROUP_CAPACITY.
        let wanted = request.count.min(headroom);
        let launched = ec2_instances::launch(
            engine,
            &ec2_instances::LaunchRequest {
                region: &region,
                group_name: request.group_name,
                count: wanted,
                placed,
                zones: &zones,
                vpc_zone_identifier: vpc_zone_identifier.as_deref(),
                launch_configuration: launch_configuration.as_ref(),
                launch_template: launch_template.as_ref(),
                mixed_instances_policy: mixed_instances_policy.as_ref(),
                tags: &tags,
            },
        );
        let unplaced = request.count.saturating_sub(launched.len());
        let launched: Vec<StoredInstance> = launched
            .into_iter()
            .map(|instance| StoredInstance {
                instance_id: instance.instance_id,
                availability_zone: instance.availability_zone,
                lifecycle_state: LIFECYCLE_PENDING.to_string(),
                health_status: HEALTH_STATUS_HEALTHY.to_string(),
                launch_configuration_name: launch_configuration_name.clone(),
                launch_template: launch_template.clone(),
                // What Amazon EC2 launched it as, so the two services report one
                // instance the same way.
                instance_type: instance.instance_type,
                image_id: instance.image_id,
                protected_from_scale_in: protected,
                transition_due: Some(request.due),
            })
            .collect();
        let launched_ids: Vec<String> = launched
            .iter()
            .map(|instance| instance.instance_id.clone())
            .collect();
        if let Some(group) = self.groups.get_mut(request.group_name) {
            group.instances.extend(launched);
        }
        (launched_ids, unplaced)
    }

    /// The instances a scale-in of `count` takes out of the group.
    ///
    /// The most recently launched instances go first. The group's
    /// `TerminationPolicies` are persisted and reported as configured, but the
    /// `Default` policy's own inputs (Availability Zone rebalancing, billing hour,
    /// launch template age) are not modelled here, so selection is by launch order.
    /// An instance protected from scale-in is never chosen: that is what the
    /// protection means.
    fn scale_in_selection(&self, group_name: &str, count: i32) -> Vec<String> {
        let Some(group) = self.groups.get(group_name) else {
            return Vec::new();
        };
        let mut ids = Vec::new();
        let mut remaining = count;
        for instance in group.instances.iter().rev() {
            if remaining == 0 {
                break;
            }
            if instance.lifecycle_state == LIFECYCLE_TERMINATING || instance.protected_from_scale_in
            {
                continue;
            }
            ids.push(instance.instance_id.clone());
            remaining -= 1;
        }
        ids
    }

    /// Take the instances `instance_ids` names out of the group: each is reported
    /// `Terminating` until `due`, and the same Amazon EC2 instance is terminated
    /// where it lives, so the account sees it shut down rather than vanish from one
    /// service's view alone.
    ///
    /// An id the group does not hold, or one already on its way out, changes nothing.
    fn terminate_group_instances<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        group_name: &str,
        instance_ids: &[String],
        due: SystemTime,
    ) {
        if let Some(group) = self.groups.get_mut(group_name) {
            for instance in &mut group.instances {
                if instance_ids.contains(&instance.instance_id)
                    && instance.lifecycle_state != LIFECYCLE_TERMINATING
                {
                    instance.lifecycle_state = LIFECYCLE_TERMINATING.to_string();
                    instance.transition_due = Some(due);
                }
            }
        }
        ec2_instances::terminate(engine, instance_ids);
    }

    /// Materialise every instance-refresh replacement the clock has made due at
    /// `now`, and finish each refresh that has none left.
    ///
    /// A refresh replaces ONE instance per [`INSTANCE_REFRESH_REPLACEMENT`], so a
    /// refresh nobody has read for a while catches up over the steps it was owed,
    /// each anchored at the instant it came due rather than at the instant somebody
    /// looked. The loop therefore terminates: every pass either retires a pending
    /// replacement or brings a refresh to a terminal status, which clears its due
    /// instant.
    pub fn advance_instance_refreshes<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        now: SystemTime,
    ) {
        loop {
            let due: Vec<(usize, SystemTime)> = self
                .instance_refreshes
                .iter()
                .enumerate()
                .filter_map(|(index, refresh)| {
                    refresh
                        .next_step_due
                        .filter(|due| *due <= now)
                        .map(|due| (index, due))
                })
                .collect();
            if due.is_empty() {
                return;
            }
            for (index, due) in due {
                self.advance_one_instance_refresh(engine, index, due);
            }
        }
    }

    /// Retire the one step the refresh at `index` owes at `due`: complete its
    /// cancellation, replace its next instance — or, while it is being rolled back,
    /// replace one of its own replacements back — or, with none left, finish it.
    fn advance_one_instance_refresh<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        index: usize,
        due: SystemTime,
    ) {
        let Some(refresh) = self.instance_refreshes.get(index) else {
            return;
        };
        let group_name = refresh.auto_scaling_group_name.clone();
        let instance_refresh_id = refresh.instance_refresh_id.clone();
        let desired_configuration = refresh.desired_configuration.clone();
        let next_instance_id = refresh.pending_instance_ids.first().cloned();
        let cancelling = refresh.status == REFRESH_CANCELLING;
        let rolling_back = refresh.is_rolling_back();

        // A cancellation replaces nothing further and rolls nothing back, so the only
        // step a refresh being cancelled owes is the end of that cancellation — which
        // leaves the progress its replacements made, and the instances it never got to,
        // exactly as they stood.
        if cancelling {
            if let Some(refresh) = self.instance_refreshes.get_mut(index) {
                refresh.status = REFRESH_CANCELLED.to_string();
                refresh.end_time = Some(due);
                refresh.next_step_due = None;
            }
            return;
        }

        // A refresh whose group is gone has no instances left to replace and cannot
        // finish: the group it was started for no longer exists.
        if !self.groups.contains_key(&group_name) {
            if let Some(refresh) = self.instance_refreshes.get_mut(index) {
                refresh.status = if rolling_back {
                    REFRESH_ROLLBACK_FAILED.to_string()
                } else {
                    REFRESH_FAILED.to_string()
                };
                refresh.status_reason = Some(
                    "The Auto Scaling group was deleted while the instance refresh was in \
                     progress."
                        .to_string(),
                );
                refresh.end_time = Some(due);
                refresh.next_step_due = None;
            }
            return;
        }

        let Some(instance_id) = next_instance_id else {
            if rolling_back {
                // Every replacement the refresh had made has been replaced back, so the
                // rollback has restored the group to the configuration it was using
                // before the refresh started. "PercentageComplete does not include
                // instances that are replaced during a rollback. This value gradually
                // goes back down to zero during a rollback"
                // (smithy_query(query="doc",shape="InstanceRefresh",
                // member="PercentageComplete")), so it has reached zero here.
                if let Some(refresh) = self.instance_refreshes.get_mut(index) {
                    refresh.status = REFRESH_ROLLBACK_SUCCESSFUL.to_string();
                    refresh.percentage_complete = 0;
                    refresh.end_time = Some(due);
                    refresh.next_step_due = None;
                }
                return;
            }
            // Every instance the refresh set out to replace has been replaced, so the
            // refresh has succeeded — and the model documents that the group's own
            // settings then reflect the desired configuration it ran with.
            if let Some(desired) = desired_configuration {
                if let Some(group) = self.groups.get_mut(&group_name) {
                    if desired.launch_template.is_some() {
                        group.launch_template = desired.launch_template.clone();
                        group.launch_configuration_name = None;
                    }
                    if desired.mixed_instances_policy.is_some() {
                        group.mixed_instances_policy = desired.mixed_instances_policy.clone();
                        group.launch_configuration_name = None;
                    }
                }
            }
            if let Some(refresh) = self.instance_refreshes.get_mut(index) {
                refresh.status = REFRESH_SUCCESSFUL.to_string();
                refresh.percentage_complete = 100;
                refresh.instances_to_update = 0;
                refresh.end_time = Some(due);
                refresh.next_step_due = None;
            }
            return;
        };

        // A rollback launches from the group's OWN launch source — the configuration it
        // was using before the refresh started, since a desired configuration reaches
        // the group's settings only once the refresh has succeeded — while the refresh's
        // own replacements launch from the configuration it is moving the group to.
        let launched_instance_id = self.replace_group_instance(
            engine,
            &group_name,
            &instance_id,
            if rolling_back {
                None
            } else {
                desired_configuration.as_ref()
            },
            &if rolling_back {
                Self::instance_refresh_rollback_cause(&instance_refresh_id)
            } else {
                Self::instance_refresh_cause(&instance_refresh_id)
            },
            due,
        );

        let Some(refresh) = self.instance_refreshes.get_mut(index) else {
            return;
        };
        // A replacement Amazon EC2 could not place is a launch failure, and the model
        // documents that "an instance refresh might fail for several reasons, such as
        // EC2 launch failures": the instance stays pending, unreplaced, and the
        // refresh fails rather than reporting progress nothing made.
        let Some(launched_instance_id) = launched_instance_id else {
            if rolling_back {
                refresh.status = REFRESH_ROLLBACK_FAILED.to_string();
                refresh.status_reason = Some(
                    "The instance refresh rollback failed because a replacement instance could \
                     not be launched."
                        .to_string(),
                );
            } else {
                refresh.status = REFRESH_FAILED.to_string();
                refresh.status_reason = Some(
                    "The instance refresh failed because a replacement instance could not be \
                     launched."
                        .to_string(),
                );
            }
            refresh.end_time = Some(due);
            refresh.next_step_due = None;
            return;
        };
        if !refresh.pending_instance_ids.is_empty() {
            refresh.pending_instance_ids.remove(0);
        }
        let outstanding = i32::try_from(refresh.pending_instance_ids.len()).unwrap_or(i32::MAX);
        if rolling_back {
            // The instances the refresh never got to "don't need to be replaced as part
            // of the rollback" (smithy_query(query="doc",shape="InstanceRefresh",
            // member="InstancesToUpdate")), so what is left to update is what it was
            // when the rollback started; what comes down is the progress the refresh had
            // made, one replaced-back instance at a time.
            refresh.percentage_complete = if refresh.instances_at_start > 0 {
                (outstanding.saturating_mul(100) / refresh.instances_at_start).clamp(0, 100)
            } else {
                0
            };
        } else {
            refresh.replaced_instance_ids.push(launched_instance_id);
            refresh.status = REFRESH_IN_PROGRESS.to_string();
            refresh.instances_to_update = outstanding;
            let replaced = (refresh.instances_at_start - outstanding).max(0);
            refresh.percentage_complete = if refresh.instances_at_start > 0 {
                (replaced.saturating_mul(100) / refresh.instances_at_start).clamp(0, 100)
            } else {
                100
            };
        }
        refresh.next_step_due = Some(due + INSTANCE_REFRESH_REPLACEMENT);
    }

    /// Replace ONE instance of a group: launch its replacement from the configuration
    /// `desired_configuration` names (or the group's own launch source) and terminate
    /// the instance it replaces, recording the scaling activity each of those changes
    /// produces under `cause`.
    ///
    /// The instance being replaced is taken out only once its replacement exists, so
    /// a replacement Amazon EC2 could not place leaves the group's instances alone and
    /// records the failed launch instead. Answers the id of the replacement that was
    /// launched, which is what the caller reports progress from — and, for a refresh,
    /// what a later rollback has to replace back.
    fn replace_group_instance<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        group_name: &str,
        instance_id: &str,
        desired_configuration: Option<&DesiredConfiguration>,
        cause: &str,
        now: SystemTime,
    ) -> Option<String> {
        let arn = self.groups.get(group_name).map(|group| group.arn.clone())?;
        let due = now + LIFECYCLE_TRANSITION;
        let (launched_ids, unplaced) = self.launch_into_group(
            engine,
            &GroupLaunch {
                group_name,
                count: 1,
                due,
                desired_configuration,
            },
        );
        let replaced = if launched_ids.is_empty() {
            Vec::new()
        } else {
            vec![instance_id.to_string()]
        };
        self.terminate_group_instances(engine, group_name, &replaced, due);

        for launched_id in &launched_ids {
            self.record_instance_lifecycle_change(
                engine,
                &InstanceLifecycleChange {
                    kind: InstanceLifecycle::Launched,
                    group_name,
                    group_arn: &arn,
                    instance_id: launched_id,
                    cause,
                    start_time: now,
                    due,
                },
            );
        }
        for replaced_id in &replaced {
            self.record_instance_lifecycle_change(
                engine,
                &InstanceLifecycleChange {
                    kind: InstanceLifecycle::Terminated,
                    group_name,
                    group_arn: &arn,
                    instance_id: replaced_id,
                    cause,
                    start_time: now,
                    due,
                },
            );
        }
        for _ in 0..unplaced {
            self.record_failed_activity(
                group_name,
                &arn,
                "Launching a new EC2 instance failed: the account could not place it".to_string(),
                cause.to_string(),
                now,
            );
        }
        launched_ids.first().cloned()
    }

    /// The cause an Auto Scaling capacity change records: what asked for it, and
    /// the capacities it moved between.
    pub fn capacity_change_cause(reason: &str, from: i32, to: i32) -> String {
        format!("{reason}: changing the desired capacity from {from} to {to}.")
    }

    /// The cause the replacements of an instance refresh record: which refresh asked
    /// for them.
    pub fn instance_refresh_cause(instance_refresh_id: &str) -> String {
        format!(
            "an instance refresh is replacing the instances of the group: instance refresh \
             {instance_refresh_id}."
        )
    }

    /// The cause the replacements of an instance refresh ROLLBACK record: which
    /// refresh's rollback asked for them, so the activity log tells the replacements a
    /// refresh made apart from the ones that put them back.
    pub fn instance_refresh_rollback_cause(instance_refresh_id: &str) -> String {
        format!(
            "an instance refresh rollback is replacing the instances of the group: instance \
             refresh {instance_refresh_id}."
        )
    }
}

/// What a group is asked to launch: how many instances, when their transient state
/// comes due, and the configuration they run when it is not the group's own.
#[derive(Debug)]
struct GroupLaunch<'a> {
    /// The group the instances belong to.
    group_name: &'a str,
    /// How many instances to launch.
    count: usize,
    /// When the `Pending` state each launched instance is reported in comes due.
    due: SystemTime,
    /// The desired configuration of the instance refresh that asked for the launch,
    /// when one did: its replacements run the new configuration before the group's
    /// own settings are updated to it.
    desired_configuration: Option<&'a DesiredConfiguration>,
}

/// The values a `HealthCheckType` may name, from the model's own documentation
/// for `CreateAutoScalingGroup$HealthCheckType`: "A comma-separated value string
/// of one or more health check types. The valid values are EC2, EBS, ELB, and
/// VPC_LATTICE."
pub const HEALTH_CHECK_TYPES: &[&str] = &["EC2", "EBS", "ELB", "VPC_LATTICE"];

/// Whether `health_check_type` is a comma-separated list of [`HEALTH_CHECK_TYPES`].
pub fn is_valid_health_check_type(health_check_type: &str) -> bool {
    !health_check_type.is_empty()
        && health_check_type
            .split(',')
            .all(|part| HEALTH_CHECK_TYPES.contains(&part.trim()))
}

/// The policies a `TerminationPolicies` entry may name, from the model's own
/// documentation for `CreateAutoScalingGroup$TerminationPolicies`: "Valid values:
/// Default | AllocationStrategy | ClosestToNextInstanceHour | NewestInstance |
/// OldestInstance | OldestLaunchConfiguration | OldestLaunchTemplate |
/// arn:aws:lambda:region:account-id:function:my-function:my-alias".
pub const TERMINATION_POLICIES: &[&str] = &[
    "Default",
    "AllocationStrategy",
    "ClosestToNextInstanceHour",
    "NewestInstance",
    "OldestInstance",
    "OldestLaunchConfiguration",
    "OldestLaunchTemplate",
];

/// The prefix of the custom-policy form that documentation ends with: a Lambda
/// function ARN.
pub const TERMINATION_POLICY_LAMBDA_PREFIX: &str = "arn:aws:lambda:";

/// Whether `policy` is one of [`TERMINATION_POLICIES`] or a Lambda function ARN.
pub fn is_valid_termination_policy(policy: &str) -> bool {
    TERMINATION_POLICIES.contains(&policy) || policy.starts_with(TERMINATION_POLICY_LAMBDA_PREFIX)
}

// ---------------------------------------------------------------------------
// The reference-data catalogs
//
// Amazon EC2 Auto Scaling publishes a handful of fixed catalogs through
// zero-input describes (`DescribeScalingProcessTypes`,
// `DescribeTerminationPolicyTypes`, `DescribeAdjustmentTypes`,
// `DescribeAutoScalingNotificationTypes`, `DescribeLifecycleHookTypes`,
// `DescribeMetricCollectionTypes`). They are SERVICE data, not per-account
// state: the same answer for every caller, in every region, whatever the store
// holds — so they live here as constants and the handlers that report them take
// no lock and write nothing.
//
// Each catalog's doc comment names where its membership and ordering come
// from: normally the model member whose documentation publishes them, and
// where that documentation is behind the service, what it omits.
// ---------------------------------------------------------------------------

/// The processes `SuspendProcesses` and `ResumeProcesses` name, and the catalog
/// `DescribeScalingProcessTypes` reports.
///
/// Membership from the model's own documentation for `ProcessType$ProcessName`
/// ("One of the following processes: Launch, Terminate, AddToLoadBalancer,
/// AlarmNotification, AZRebalance, HealthCheck, InstanceRefresh,
/// ReplaceUnhealthy, ScheduledActions"). Held in ASCII order, which is the order
/// the service reports the catalog in.
pub const SCALING_PROCESS_TYPES: &[&str] = &[
    "AZRebalance",
    "AddToLoadBalancer",
    "AlarmNotification",
    "HealthCheck",
    "InstanceRefresh",
    "Launch",
    "ReplaceUnhealthy",
    "ScheduledActions",
    "Terminate",
];

/// The type name the custom termination policy is published under.
///
/// [`TERMINATION_POLICIES`] lists the policies a group may name literally and
/// [`TERMINATION_POLICY_LAMBDA_PREFIX`] accepts the ARN form beside them; the
/// catalog names that form by its TYPE rather than by any one function's ARN.
pub const TERMINATION_POLICY_TYPE_LAMBDA: &str = "Lambda";

/// The catalog `DescribeTerminationPolicyTypes` reports: every policy
/// [`TERMINATION_POLICIES`] accepts by name, plus
/// [`TERMINATION_POLICY_TYPE_LAMBDA`] for the Lambda-ARN form the same model
/// documentation ends with, in ASCII order.
pub const TERMINATION_POLICY_TYPES: &[&str] = &[
    "AllocationStrategy",
    "ClosestToNextInstanceHour",
    "Default",
    TERMINATION_POLICY_TYPE_LAMBDA,
    "NewestInstance",
    "OldestInstance",
    "OldestLaunchConfiguration",
    "OldestLaunchTemplate",
];

/// The catalog `DescribeAdjustmentTypes` reports, from the model's own
/// documentation for `AdjustmentType$AdjustmentType`: "The policy adjustment
/// type. The valid values are ChangeInCapacity, ExactCapacity, and
/// PercentChangeInCapacity." In ASCII order.
pub const ADJUSTMENT_TYPES: &[&str] = &[
    "ChangeInCapacity",
    "ExactCapacity",
    "PercentChangeInCapacity",
];

/// The catalog `DescribeLifecycleHookTypes` reports: the lifecycle transitions a
/// lifecycle hook may name, from the model's own documentation for
/// `PutLifecycleHookType$LifecycleTransition` ("To create a lifecycle hook for
/// scale-out events, specify autoscaling:EC2_INSTANCE_LAUNCHING. To create a
/// lifecycle hook for scale-in events, specify
/// autoscaling:EC2_INSTANCE_TERMINATING"). In ASCII order.
pub const LIFECYCLE_HOOK_TYPES: &[&str] = &[
    "autoscaling:EC2_INSTANCE_LAUNCHING",
    "autoscaling:EC2_INSTANCE_TERMINATING",
];

/// The catalog `DescribeAutoScalingNotificationTypes` reports: the event types a
/// group can notify about.
///
/// The model's own documentation for `NotificationConfiguration$NotificationType`
/// ("One of the following event notification types: …") enumerates the four
/// instance-lifecycle events and the test notification, so that prose is a LOWER
/// BOUND on what the service publishes rather than the catalog itself: the service
/// also notifies about the replacement of an instance's root volume, under
/// `autoscaling:EC2_ROOT_VOLUME_REPLACEMENT` and its `_ERROR` counterpart, which a
/// group may name in a notification configuration without the prose listing it. The
/// list target is an untyped string list
/// (`smithy_query(query="shape", shape="AutoScalingNotificationTypes")`), so the
/// membership is the service's own.
///
/// In ASCII order, which is the order the service publishes them in: the
/// instance-lifecycle events, then the root-volume ones, then the test
/// notification.
///
/// The lifecycle transitions of [`LIFECYCLE_HOOK_TYPES`] are a SEPARATE catalog,
/// published by `DescribeLifecycleHookTypes`, even though the model carries both
/// through the one `AutoScalingNotificationTypes` list shape (it is the target of
/// this operation's output member and of that one's alike): a transition is what a
/// lifecycle hook is created for, not an event a group notifies about, and the
/// model documents each list on its own member.
pub const AUTO_SCALING_NOTIFICATION_TYPES: &[&str] = &[
    "autoscaling:EC2_INSTANCE_LAUNCH",
    "autoscaling:EC2_INSTANCE_LAUNCH_ERROR",
    "autoscaling:EC2_INSTANCE_TERMINATE",
    "autoscaling:EC2_INSTANCE_TERMINATE_ERROR",
    "autoscaling:EC2_ROOT_VOLUME_REPLACEMENT",
    "autoscaling:EC2_ROOT_VOLUME_REPLACEMENT_ERROR",
    "autoscaling:TEST_NOTIFICATION",
];

/// The catalog `DescribeMetricCollectionTypes` reports as `Metrics`: the group
/// metrics `EnableMetricsCollection` and `DisableMetricsCollection` name.
///
/// The model's own documentation for `MetricCollectionType$Metric` ("One of the
/// following metrics: …") enumerates only the twenty metrics that predate the
/// retained-capacity series, so that prose is a LOWER BOUND on what the service
/// publishes rather than the catalog itself: the warm pool's own minimum
/// (`WarmPoolMinSize`) and the four retained-capacity series a group's instance
/// maintenance policy reports (`GroupTerminatingRetainedInstances`,
/// `GroupTerminatingRetainedCapacity`, `WarmPoolPendingRetainedCapacity`,
/// `WarmPoolTerminatingRetainedCapacity`) are enabled by name through
/// `EnableMetricsCollection` and published by this catalog without appearing in
/// it.
///
/// Grouped the way that documentation groups what it does enumerate — sizes,
/// then instance counts, then capacities, then the warm pool — with each metric
/// the prose omits beside the one it refines.
pub const METRIC_COLLECTION_TYPES: &[&str] = &[
    "GroupMinSize",
    "GroupMaxSize",
    "GroupDesiredCapacity",
    "GroupInServiceInstances",
    "GroupPendingInstances",
    "GroupStandbyInstances",
    "GroupTerminatingInstances",
    "GroupTerminatingRetainedInstances",
    "GroupTotalInstances",
    "GroupInServiceCapacity",
    "GroupPendingCapacity",
    "GroupStandbyCapacity",
    "GroupTerminatingCapacity",
    "GroupTerminatingRetainedCapacity",
    "GroupTotalCapacity",
    "WarmPoolMinSize",
    "WarmPoolDesiredCapacity",
    "WarmPoolWarmedCapacity",
    "WarmPoolPendingCapacity",
    "WarmPoolPendingRetainedCapacity",
    "WarmPoolTerminatingCapacity",
    "WarmPoolTerminatingRetainedCapacity",
    "WarmPoolTotalCapacity",
    "GroupAndWarmPoolDesiredCapacity",
    "GroupAndWarmPoolTotalCapacity",
];

/// Whether `metric` is one of [`METRIC_COLLECTION_TYPES`].
///
/// The metrics `EnableMetricsCollection` and `DisableMetricsCollection` accept
/// are the metrics this catalog publishes, so both sides check membership
/// through here and cannot drift apart. NOT YET IN FORCE on the wire: those two
/// operations are still scaffolded (each owned by its own issue), so nothing in
/// `src/ops/` validates a metric name today — this is the seam they are to use
/// when they are implemented.
pub fn is_valid_metric_collection_type(metric: &str) -> bool {
    METRIC_COLLECTION_TYPES.contains(&metric)
}

/// The catalog `DescribeMetricCollectionTypes` reports as `Granularities`, from
/// the model's own documentation for `MetricGranularityType$Granularity`: "The
/// granularity. The only valid value is 1Minute."
pub const METRIC_GRANULARITIES: &[&str] = &["1Minute"];

/// The units a `DesiredCapacityType` may name, from the model's own documentation
/// for `CreateAutoScalingGroup$DesiredCapacityType`: "Valid values: units | vcpu |
/// memory-mib".
pub const DESIRED_CAPACITY_TYPES: &[&str] = &["units", "vcpu", "memory-mib"];

/// Whether `name` satisfies the character rule the model documents for
/// `AutoScalingGroupName`: "The name can contain any ASCII character 33 to 126
/// including most punctuation characters, digits, and upper and lowercased
/// letters. You cannot use a colon (:) in the name."
pub fn is_valid_group_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| matches!(c, '!'..='~') && c != ':')
}

/// The smallest non-zero `MaxInstanceLifetime` the service accepts.
///
/// From the model's own documentation for
/// `CreateAutoScalingGroup$MaxInstanceLifetime`: "If specified, the value must be
/// either 0 or a number equal to or greater than 86,400 seconds (1 day)."
pub const MIN_MAX_INSTANCE_LIFETIME: i32 = 86_400;

/// Whether `seconds` satisfies that rule.
pub fn is_valid_max_instance_lifetime(seconds: i32) -> bool {
    seconds == 0 || seconds >= MIN_MAX_INSTANCE_LIFETIME
}

/// The message the service refuses a request whose maximum group size is below its
/// minimum with, shared by every operation that sets a group's bounds: the two bounds
/// in the order it reports them, and nothing else from the request.
pub fn max_below_min_message(max_size: i32, min_size: i32) -> String {
    format!("Max bound, {max_size}, must be greater than or equal to min bound, {min_size}")
}

/// The first of `sizes` past [`MAX_GROUP_CAPACITY`], if any.
///
/// Every operation that sets a group's sizes checks them through this, so one
/// request cannot store bounds whose capacity this process could not hold, and
/// the refusal can name the offending value.
pub fn size_past_ceiling(sizes: &[i32]) -> Option<i32> {
    sizes.iter().copied().find(|size| *size > MAX_GROUP_CAPACITY)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAutoscalingAutoscaling20110101State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAutoscalingAutoscaling20110101State>)
}

/// The region a request is signed for, which is the region the resources it
/// creates live in. Falls back to the emulator's default region when the request
/// carries no SigV4 credential scope (an unsigned call).
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an unsigned request's resources live in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The tracing target every audit record this service emits carries.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// Emit the structured audit record a state-changing operation owes: the
/// operation, the resource it changed, how the call ended, and which credential
/// was presented for it.
///
/// Only identifiers are recorded. The presented access key id names the caller;
/// the signature, the secret and the request body never appear here, and neither
/// does any other header. The subscriber stamps the time.
pub fn audit(parts: &::http::request::Parts, operation: &str, resource: &str, outcome: &str) {
    ::tracing::info!(
        target: AUDIT_TARGET,
        operation,
        resource,
        outcome,
        region = %request_region(parts),
        access_key_id = %::engine::extract_akid(&parts.headers),
        "Auto Scaling state change"
    );
}

/// Convert a stored instant to the wire timestamp type.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The ARN of an Auto Scaling group, in the form the service reports:
/// `arn:aws:autoscaling:{region}:{account}:autoScalingGroup:{id}:autoScalingGroupName/{name}`.
pub fn group_arn(region: &str, resource_id: &str, name: &str) -> String {
    ::engine::emulator_arn(
        "autoscaling",
        region,
        &format!("autoScalingGroup:{resource_id}:autoScalingGroupName/{name}"),
    )
}

/// The fixed text every refusal of a request naming a group this service does not hold
/// starts with. What follows it is the DETAIL the operation reports, which is that
/// operation's own.
const ABSENT_GROUP_PREFIX: &str = "AutoScalingGroup name not found - ";

/// The absent-group refusal whose detail names the group as the resource and the name
/// it was asked for, unquoted.
///
/// The name is the only part taken from the request; the rest of the sentence,
/// punctuation included, is fixed text.
pub fn absent_group_message(name: &str) -> String {
    format!("{ABSENT_GROUP_PREFIX}AutoScalingGroup {name} not found")
}

/// The message Amazon EC2 Auto Scaling refuses a legacy target-group detach that
/// names a target group the group does not hold with, which names those target
/// groups.
///
/// The refusal is for the request as a WHOLE: a call that mixes attached target
/// groups with one that is not attached detaches none of them, so the message says
/// what was wrong with the request rather than reporting a partial result.
pub fn unattached_target_groups_message(arns: &[String]) -> String {
    format!(
        "Trying to remove Target Groups that are not part of the group: {}",
        arns.join(", ")
    )
}

/// The message a call naming more target groups than
/// [`MAX_TARGET_GROUPS_PER_CALL`] is refused with, which quotes the bound the
/// caller has to come back under.
pub fn too_many_target_groups_message() -> String {
    format!(
        "Trying to update too many Target Groups at once. The limit is \
         {MAX_TARGET_GROUPS_PER_CALL}"
    )
}

/// The most instance IDs ONE `SetInstanceProtection` call may name.
///
/// From the model's own documentation for `SetInstanceProtection`: "If you exceed
/// your maximum limit of instance IDs, which is 50 per Auto Scaling group, the call
/// fails." The member carries no modelled length, so the bound is the service's own
/// and is checked by the handler, which answers the operation's modelled
/// `LimitExceededFault` past it.
pub const MAX_INSTANCE_IDS_PER_CALL: usize = 50;

/// The message a call naming more instance IDs than [`MAX_INSTANCE_IDS_PER_CALL`] is
/// refused with, which quotes the bound the caller has to come back under.
///
/// Same shape as [`too_many_target_groups_message`]: this service reports a per-call
/// bound by naming the resource the request named too many of and the limit itself.
pub fn too_many_instance_ids_message() -> String {
    format!("Trying to update too many Instances at once. The limit is {MAX_INSTANCE_IDS_PER_CALL}")
}

/// The message an instance-scoped update that names an instance the group does not
/// hold is refused with, which names those instances.
///
/// Same shape as [`unattached_target_groups_message`], the refusal this service
/// answers when a request names a resource of the same group-scoped kind that the
/// group does not hold: the refusal is for the request as a WHOLE, so a call that
/// mixes instances of the group with one that is not in it updates none of them, and
/// the message says what was wrong with the request rather than reporting a partial
/// result. The variable part is the instance ids the request named, in request order.
pub fn instances_not_in_group_message(instance_ids: &[String]) -> String {
    format!(
        "Trying to update Instances that are not part of the group: {}",
        instance_ids.join(", ")
    )
}

/// The absent-group refusal whose detail QUOTES the name it was asked for.
pub fn absent_group_message_quoting_name(name: &str) -> String {
    format!("{ABSENT_GROUP_PREFIX}AutoScalingGroup '{name}' not found")
}

/// The absent-group refusal whose detail spells the resource out as the service's own
/// prose name for it.
pub fn absent_group_message_in_prose(name: &str) -> String {
    format!("{ABSENT_GROUP_PREFIX}Auto Scaling group with name {name} not found")
}

/// The absent-group refusal whose detail field the operation leaves unfilled, so it
/// travels as the literal token `null`: the whole message is fixed text and no part of
/// the request appears in it.
pub const ABSENT_GROUP_MESSAGE_WITHOUT_DETAIL: &str = "AutoScalingGroup name not found - null";

/// The sentence a rollback of a refresh that carries no desired configuration is
/// refused with: the first of the situations the error class names, and the whole of
/// the message — the refusal is about the refresh's own configuration, so no part of
/// the request appears in it.
pub const ROLLBACK_WITHOUT_DESIRED_CONFIGURATION_MESSAGE: &str =
    "The instance refresh cannot be rolled back because there is no desired configuration \
     specified for the instance refresh.";

/// The sentence a rollback of a group whose launch template version floats is refused
/// with, naming the version the group uses: the pre-refresh configuration is not
/// pinned to a version a rollback could go back to.
pub fn floating_launch_template_version_message(version: &str) -> String {
    format!(
        "The instance refresh cannot be rolled back because the Auto Scaling group uses the \
         launch template's {version} version."
    )
}

/// The sentence a rollback of a group whose launch template resolves its image through
/// a Systems Manager parameter is refused with.
pub const SSM_PARAMETER_IMAGE_ID_MESSAGE: &str =
    "The instance refresh cannot be rolled back because the Auto Scaling group has a launch \
     template that uses a Systems Manager parameter instead of an AMI ID for the ImageId \
     property.";

/// The launch template specification a launch source names: the one it carries
/// directly, or the one its mixed instances policy carries.
pub fn launch_template_of<'a>(
    launch_template: Option<&'a LaunchTemplateSpecification>,
    mixed_instances_policy: Option<&'a MixedInstancesPolicy>,
) -> Option<&'a LaunchTemplateSpecification> {
    launch_template.or_else(|| {
        mixed_instances_policy
            .and_then(|policy| policy.launch_template.as_ref())
            .and_then(|template| template.launch_template_specification.as_ref())
    })
}

/// Whether two specifications name the same launch template and version, which is
/// what makes an instance launched from one of them already up to date with respect
/// to the other.
pub fn same_launch_template(
    left: Option<&LaunchTemplateSpecification>,
    right: Option<&LaunchTemplateSpecification>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            left.launch_template_id == right.launch_template_id
                && left.launch_template_name == right.launch_template_name
                && left.version == right.version
        }
        (None, None) => true,
        _ => false,
    }
}

/// Decode a `NextToken` into the offset it names, or `None` when the token is not
/// one this service issued.
///
/// The paginated describes in this service page over a name-ordered list, so an
/// issued token is the offset the next page starts at, behind a marker: anything
/// without the marker, and the offset 0 (the first page is reached with no token
/// at all, so no token for it is ever issued), is not a token this service gave
/// out.
pub fn decode_next_token(token: &str) -> Option<usize> {
    token
        .strip_prefix(NEXT_TOKEN_MARKER)?
        .parse::<usize>()
        .ok()
        .filter(|offset| *offset > 0)
}

/// The message a paginated read refuses an unusable `NextToken` with.
///
/// The REQUIREMENT here is that the refusal quotes the token the caller sent —
/// that is the only thing the caller can act on, since the token it is holding is
/// not one this service will resume from. The surrounding wording is an example,
/// not something the model or a recording pins down.
pub fn invalid_next_token_message(token: &str) -> String {
    format!("The token '{token}' was invalid")
}

/// The message `DescribePolicies` refuses an unusable `NextToken` with: the sentence that
/// operation's own recorded refusal states, which names the member by its role (a
/// pagination token) and quotes the token the caller sent.
///
/// Separate from [`invalid_next_token_message`] because the sentence is this
/// operation's: the one above is the example wording its callers already answer with,
/// and rewording them is not this operation's to do.
pub fn invalid_pagination_token_message(token: &str) -> String {
    format!("The pagination token '{token}' is invalid")
}

/// Encode the offset the next page starts at as a `NextToken`.
pub fn encode_next_token(offset: usize) -> String {
    format!("{NEXT_TOKEN_MARKER}{offset}")
}

/// The marker every `NextToken` this service issues carries, so a token it never
/// issued is refused rather than read as an offset.
const NEXT_TOKEN_MARKER: &str = "next-";

/// One page of a name-ordered list, plus the `NextToken` that follows it.
///
/// `max_records` is the request's `MaxRecords`; absent or out of range it falls
/// back to [`DEFAULT_MAX_RECORDS`], and it is capped at [`MAX_MAX_RECORDS`].
pub fn page<T: Clone>(items: &[T], offset: usize, max_records: Option<i32>) -> (Vec<T>, Option<String>) {
    let limit = page_limit(max_records);
    let start = offset.min(items.len());
    let end = start.saturating_add(limit).min(items.len());
    let next = (end < items.len()).then(|| encode_next_token(end));
    (items[start..end].to_vec(), next)
}

/// How many items one page holds, from the request's `MaxRecords`: absent or
/// non-positive it falls back to [`DEFAULT_MAX_RECORDS`], and it is capped at
/// [`MAX_MAX_RECORDS`].
///
/// Separate from [`page`] because a describe that pages over its own cursor
/// rather than over an offset still bounds its page the same way.
pub fn page_limit(max_records: Option<i32>) -> usize {
    max_records
        .filter(|value| *value > 0)
        .map_or(DEFAULT_MAX_RECORDS, |value| {
            (value as usize).min(MAX_MAX_RECORDS)
        })
}

/// A request's `MaxRecords` when it is outside the range the model documents
/// ("The default value is 50 and the maximum value is 100"), for an operation to
/// refuse: `None` when the request omits it or carries an acceptable value.
pub fn max_records_out_of_range(max_records: Option<i32>) -> Option<i32> {
    max_records.filter(|value| *value < 1 || usize::try_from(*value).unwrap_or(usize::MAX) > MAX_MAX_RECORDS)
}

/// The message a paginated read refuses a `MaxRecords` outside that range with: the
/// standard sentence the service states for a member value outside its bounds, naming
/// the member as the wire names it and the bound the value missed.
pub fn max_records_violation_message(max_records: i32) -> String {
    let constraint = if max_records < MIN_MAX_RECORDS {
        format!("Member must have value greater than or equal to {MIN_MAX_RECORDS}")
    } else {
        format!("Member must have value less than or equal to {MAX_MAX_RECORDS}")
    };
    format!(
        "1 validation error detected: Value '{max_records}' at 'maxRecords' failed to satisfy \
         constraint: {constraint}"
    )
}

/// The most names a describe may filter by, which the model documents on
/// `AutoScalingGroupNames`: "By default, you can only specify up to 50 names. You
/// can optionally increase this limit using the MaxRecords property."
pub fn max_filter_names(max_records: Option<i32>) -> usize {
    max_records
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(DEFAULT_MAX_RECORDS)
        .max(DEFAULT_MAX_RECORDS)
}

/// `MaxRecords` when the request omits it.
///
/// From the model's own documentation for `AutoScalingGroupNamesType$MaxRecords`:
/// "The default value is 50 and the maximum value is 100."
pub const DEFAULT_MAX_RECORDS: usize = 50;

/// The largest `MaxRecords` the service honours (same documentation).
pub const MAX_MAX_RECORDS: usize = 100;

/// The smallest `MaxRecords` a request may ask for: a page holds items, so it holds at
/// least one.
pub const MIN_MAX_RECORDS: i32 = 1;

/// `MaxRecords` when a read of the scaling activities omits it.
///
/// From the model's own documentation for `DescribeScalingActivitiesType$MaxRecords`:
/// "The default value is 100 and the maximum value is 100" — a larger default than the
/// name-ordered describes take, so that operation names it where it pages.
pub const DEFAULT_ACTIVITY_MAX_RECORDS: i32 = 100;

