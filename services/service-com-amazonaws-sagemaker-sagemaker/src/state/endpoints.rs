// SPDX-License-Identifier: CC0-1.0

//! The ENDPOINT slice of this service's state: the endpoint configurations, the
//! endpoints created from them, the inference components hosted on them, and the
//! hosting-capacity reading a create answers before it stores anything.
//!
//! Moved out of [`super`] unchanged, so the parent module stays inside the project's
//! file-length limit and this resource family reads as one unit. Every name is
//! re-exported from [`super`], so a handler keeps naming `crate::state::…`.

use super::*;

/// An endpoint configuration created by `CreateEndpointConfig`, as
/// `DescribeEndpointConfig` replays it and `CreateEndpoint` reads it.
#[derive(Debug, Clone)]
pub struct EndpointConfig {
    pub name: String,
    pub arn: String,
    pub production_variants: Vec<ProductionVariant>,
    pub shadow_production_variants: Option<Vec<ProductionVariant>>,
    pub data_capture_config: Option<DataCaptureConfig>,
    pub async_inference_config: Option<AsyncInferenceConfig>,
    pub explainer_config: Option<ExplainerConfig>,
    pub metrics_config: Option<MetricsConfig>,
    pub kms_key_id: Option<String>,
    pub execution_role_arn: Option<String>,
    pub vpc_config: Option<VpcConfig>,
    pub enable_network_isolation: Option<bool>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
}

/// An endpoint created by `CreateEndpoint`, as `DescribeEndpoint` replays it.
///
/// `in_service_at` is the instant the clock owes the `Creating` -> `InService`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
///
/// `instance_usage` is how many instances of each ML instance type this endpoint holds,
/// snapshotted from the configuration it was created from (like its variant summaries,
/// and for the same reason: a later configuration does not retroactively change what is
/// deployed). It is what the per-instance-type hosting quota counts against
/// ([`ComAmazonawsSagemakerSagemakerState::endpoint_instances_in_use`]), so deleting the
/// endpoint releases exactly what creating it took.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub name: String,
    pub arn: String,
    pub config_name: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub production_variants: Vec<ProductionVariantSummary>,
    pub shadow_production_variants: Vec<ProductionVariantSummary>,
    pub data_capture_config: Option<DataCaptureConfig>,
    pub async_inference_config: Option<AsyncInferenceConfig>,
    pub explainer_config: Option<ExplainerConfig>,
    pub metrics_config: Option<MetricsConfig>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub in_service_at: SystemTime,
    pub instance_usage: BTreeMap<String, i64>,
}

/// The unit the per-instance-type hosting quotas are counted in: what their limit, the
/// account's utilisation of them and a request's delta are numbers OF, as
/// [`account_quota_exceeded`] states them.
pub const INSTANCES_UNIT: &str = "Instances";

/// The published name of the quota that bounds how many instances of ONE ML instance type
/// an account may run behind its endpoints, as a refusal states it.
///
/// Service Quotas names the entry `<instance type> for endpoint usage` (it is listed under
/// that name for this service, beside the notebook-instance and training entries of the
/// same shape), so the instance type is part of the quota's NAME rather than prose around
/// it: a client reads which type it has no room for straight off the message.
pub fn endpoint_usage_quota_name(instance_type: &str) -> String {
    format!("{instance_type} for endpoint usage")
}

/// The instance FAMILIES whose published default per-type instance quota is zero: the
/// newest accelerated generations (the `p4`/`p5`/`p6` GPU families and the Trainium
/// ones), which an account has to be granted capacity for before it can run anything on
/// them.
///
/// Keyed on the FAMILY — the middle segment of an `ml.<family>.<size>` instance type — so
/// a size AWS adds to one of these families needs no edit here, and so the rule is one
/// about generations rather than a list of every type. Every other family's default is a
/// non-zero number of instances ([`DEFAULT_ENDPOINT_USAGE_QUOTA`] for hosting,
/// [`crate::state::DEFAULT_CLUSTER_USAGE_QUOTA`] for a HyperPod cluster). Read through
/// [`has_zero_default_instance_quota`], which both quotas share.
const ZERO_DEFAULT_HOSTING_FAMILIES: [&str; 11] = [
    "p4d",
    "p4de",
    "p5",
    "p5e",
    "p5en",
    "p6-b200",
    "p6-b300",
    "p6e-gb200",
    "trn1",
    "trn1n",
    "trn2",
];

/// How many instances of ONE type an account may host, for a type whose published default
/// is not zero.
///
/// This emulator models neither Service Quotas nor an account's adjustments, so — as with
/// [`MODEL_QUOTA`] — this is the emulator's OWN bound rather than a claim about any
/// account's limit for a particular type: far above any deployment an emulator serves, and
/// what stops one request asking for an unbounded number of instances.
pub const DEFAULT_ENDPOINT_USAGE_QUOTA: i64 = 1_000;

/// Whether AWS publishes a ZERO default quota for the instances of `instance_type`'s
/// family, so an account has to be granted capacity before it can run any.
///
/// Keyed on the FAMILY — the middle segment of an `ml.<family>.<size>` instance type —
/// stated once here because the same families carry the zero default for EVERY per-type
/// instance quota this service publishes: the endpoint-usage one
/// ([`endpoint_usage_quota`]) and the HyperPod cluster-usage one
/// ([`crate::state::cluster_usage_quota`]).
#[must_use]
pub fn has_zero_default_instance_quota(instance_type: &str) -> bool {
    // `ml.<family>.<size>`: the family is the second segment, and a value that is not in
    // that form names no family of the zero-default generations.
    let family = instance_type.split('.').nth(1).unwrap_or_default();
    ZERO_DEFAULT_HOSTING_FAMILIES.contains(&family)
}

/// The account's quota, in instances, for hosting `instance_type` behind an endpoint.
///
/// Zero for a family AWS publishes a zero default for ([`ZERO_DEFAULT_HOSTING_FAMILIES`]),
/// so an endpoint on one is refused until the account is granted capacity;
/// [`DEFAULT_ENDPOINT_USAGE_QUOTA`] for every other type.
pub fn endpoint_usage_quota(instance_type: &str) -> i64 {
    if has_zero_default_instance_quota(instance_type) {
        return 0;
    }
    DEFAULT_ENDPOINT_USAGE_QUOTA
}

/// How many instances of each ML instance type `variants` ask for: one entry per type, the
/// instance counts of the variants naming it added up.
///
/// A variant that names an instance type runs at least one instance of it, so a variant
/// with no `InitialInstanceCount` counts as one; a variant that names no single type at all
/// — a serverless one, or one that lists a pool of alternatives for the service to choose
/// from — holds no instances of any one type and contributes nothing.
pub fn instance_demand<'variants>(
    variants: impl IntoIterator<Item = &'variants ProductionVariant>,
) -> BTreeMap<String, i64> {
    let mut demand: BTreeMap<String, i64> = BTreeMap::new();
    for variant in variants {
        let Some(instance_type) = variant.instance_type.as_deref() else {
            continue;
        };
        let count = i64::from(variant.initial_instance_count.unwrap_or(1)).max(1);
        *demand.entry(instance_type.to_string()).or_default() += count;
    }
    demand
}

/// Materialise the `Creating` -> `InService` transition the clock already owes a
/// resource that carries one.
///
/// This is the one write a reporting operation may make: the transition was due at
/// `in_service_at` whether or not anybody called, so the observer makes no difference to
/// the result. `last_modified_time` moves to the instant the transition was due, not to
/// "now", so the value is stable across later reads.
///
/// Answers whether the transition happened on this call, so the caller can record it.
pub(super) fn settle_creating(
    status: &mut String,
    last_modified_time: &mut SystemTime,
    in_service_at: SystemTime,
    now: SystemTime,
) -> bool {
    if status == STATUS_CREATING && now >= in_service_at {
        *status = STATUS_IN_SERVICE.to_string();
        *last_modified_time = in_service_at;
        return true;
    }
    false
}

impl Endpoint {
    /// Materialise the `Creating` -> `InService` transition the clock already owes; see
    /// [`settle_creating`].
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        settle_creating(
            &mut self.status,
            &mut self.last_modified_time,
            self.in_service_at,
            now,
        )
    }
}

/// How long after `CreateInferenceComponent` the clock owes that component the
/// `Creating` -> `InService` transition, on the same terms as
/// [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that describes the component straight after
/// creating it still observes `Creating` with no copies running, and one that waits
/// observes `InService` with the copies it asked for. Compressed, never zero.
///
/// The span is measured from the later of the create and the instant the hosting
/// endpoint goes `InService`, because a component's copies run on that endpoint's
/// capacity and cannot be placed before it is serving.
pub const INFERENCE_COMPONENT_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// See [`MODEL_QUOTA`]: the same bound on the inference-component store, which
/// `CreateInferenceComponent` answers `ResourceLimitExceeded` past and
/// `DeleteInferenceComponent` releases a slot of.
pub const INFERENCE_COMPONENT_QUOTA: usize = 1_000;

/// An inference component created by `CreateInferenceComponent`, as
/// `DescribeInferenceComponent` replays it.
///
/// The endpoint it is hosted on is snapshotted by name AND by ARN, so the component
/// keeps reporting the endpoint it was deployed to even after that endpoint is deleted.
/// Exactly one of `specification` / `specifications` is ever set: the model doc for
/// `Specifications` says a caller uses "either this parameter or the singular
/// Specification parameter, but not both".
///
/// `in_service_at` is the instant the clock owes the `Creating` -> `InService`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
#[derive(Debug, Clone)]
pub struct InferenceComponent {
    pub name: String,
    pub arn: String,
    pub endpoint_name: String,
    pub endpoint_arn: String,
    pub variant_name: Option<String>,
    pub specification: Option<crate::ops::types::InferenceComponentSpecification>,
    pub specifications: Option<Vec<crate::ops::types::InferenceComponentSpecification>>,
    pub runtime_config: Option<crate::ops::types::InferenceComponentRuntimeConfig>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub in_service_at: SystemTime,
}

impl InferenceComponent {
    /// Materialise the `Creating` -> `InService` transition the clock already owes; see
    /// [`settle_creating`].
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        settle_creating(
            &mut self.status,
            &mut self.last_modified_time,
            self.in_service_at,
            now,
        )
    }

    /// The specifications this component deploys, whether the caller sent the singular
    /// `Specification` or the `Specifications` list: one slice both readers walk.
    ///
    /// A stored record carries at most ONE of the two members —
    /// `CreateInferenceComponent` refuses a request that sends both — so the
    /// both-populated pair is not a case this has to choose between.
    pub fn specifications(&self) -> &[crate::ops::types::InferenceComponentSpecification] {
        match (&self.specification, &self.specifications) {
            (Some(single), None) => ::std::slice::from_ref(single),
            (None, Some(list)) => list.as_slice(),
            _ => &[],
        }
    }
}
