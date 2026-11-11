// SPDX-License-Identifier: CC0-1.0

//! The TRAINING CAPACITY slice of this service's state: the ML compute a `ResourceConfig`
//! asks for, what SageMaker will launch of it, and the sentences it refuses the rest with.
//!
//! A training job RUNS on the instances and the storage its `ResourceConfig` names, so the
//! two questions this module answers are asked before anything is stored:
//!
//! 1. **Does the configuration NAME ML compute to launch?** A job runs on an instance type
//!    with a count greater than zero ("The number of ML compute instances to use. For
//!    distributed training, provide a value greater than 1." —
//!    `ResourceConfig$InstanceCount`), or on the instance groups of a heterogeneous
//!    cluster (`ResourceConfig$InstanceGroups`), each of which names its own type and
//!    count. The modelled `@range` is wider — `InstanceCount` targets
//!    `TrainingInstanceCount` (`@range min 0`), a shape it shares with the read-only
//!    `SelectedInstanceCount` — and both members are modelled optional, so the boundary
//!    validator passes a configuration nothing can be launched on and this is where it is
//!    refused, as a 400 `ValidationException`.
//! 2. **Is the storage one SageMaker provisions?** The ML storage volume a job's instance
//!    is given is an Amazon EBS General Purpose SSD (gp2) volume
//!    (`ResourceConfig$VolumeSizeInGB`'s own doc names that volume type), and Service
//!    Quotas bounds the size of it per training job instance
//!    ([`TRAINING_VOLUME_QUOTA_NAME`]) where `OptionalVolumeSizeInGB` declares no maximum
//!    at all — so a size past that is the modelled `ResourceLimitExceeded`.
//! 3. **Will the account's quota provision the instances?** Service Quotas bounds how many
//!    instances of one ML instance type an account may run for training jobs
//!    ([`training_usage_quota_name`]), the same way it bounds the hosting ones
//!    ([`crate::state::endpoint_usage_quota_name`]) and the HyperPod ones
//!    ([`crate::state::cluster_usage_quota_name`]) — so a request past it is the modelled
//!    `ResourceLimitExceeded` too.
//!
//! Each quota refusal is [`crate::state::account_quota_exceeded`]'s sentence over that
//! quota's own entry, with the limit from this module, the utilisation from the account's
//! stored jobs and the delta from the request.
//!
//! All three readings are stated here once, over the `ResourceConfig` SHAPE, so every
//! operation that carries one judges the same configuration the same way rather than each
//! wording its own idea of a launchable cluster.

use super::*;
use crate::ops::types::ResourceConfig;

/// The fewest ML compute instances a training job can run on: a job that asks for none
/// launches nothing at all.
pub const TRAINING_INSTANCE_COUNT_MIN: i32 = 1;

/// How SageMaker states a `ResourceConfig` that names no ML compute to launch: a FIXED
/// sentence naming the pair a configuration has to carry, and nothing from the request.
///
/// The one sentence for the whole class — no instance type, no instance count, or a count
/// that is not greater than zero — because what the caller has to supply is the same in
/// each case.
pub const INSTANCE_TYPE_AND_COUNT_REQUIRED: &str =
    "InstanceType and InstanceCount greater than 0 must be specified";

/// The largest ML storage volume SageMaker provisions for one training job instance, in
/// GiB: the Amazon EBS General Purpose SSD (gp2) volume `ResourceConfig$VolumeSizeInGB`
/// names, at its published maximum size of 16 TiB.
///
/// `OptionalVolumeSizeInGB` declares no maximum, so the boundary validator passes a size
/// nothing can be provisioned at and this is the bound the service holds it to. A size of
/// ZERO stays admitted: the member's own doc says SageMaker selects the volume for an
/// instance type whose storage is fixed.
pub const TRAINING_VOLUME_SIZE_MAX_GB: i32 = 16_384;

/// The published name of the quota [`TRAINING_VOLUME_SIZE_MAX_GB`] is the limit of, as a
/// refusal states it: the Service Quotas entry for the ML storage volume one training job
/// INSTANCE may be given, worded in the shape that service publishes its per-resource EBS
/// entries in.
pub const TRAINING_VOLUME_QUOTA_NAME: &str = "Size of EBS volume for a training job instance";

/// The unit the ML storage volume quota is counted in: what its limit, the account's
/// utilisation of it and a request's delta are numbers OF, as [`account_quota_exceeded`]
/// states them.
pub const VOLUME_SIZE_UNIT: &str = "GB";

/// How many instances of ONE ML instance type an account may run for its training jobs.
///
/// This emulator models neither Service Quotas nor an account's adjustments, so — as with
/// [`MODEL_QUOTA`] and the hosting bound [`DEFAULT_ENDPOINT_USAGE_QUOTA`] — this is the
/// emulator's OWN bound rather than a claim about any account's limit for a particular
/// type: far above any training job an emulator serves, and what stops one request asking
/// for an unbounded number of instances.
pub const DEFAULT_TRAINING_USAGE_QUOTA: i64 = 1_000;

/// The published name of the quota that bounds how many instances of ONE ML instance type
/// an account may run for training jobs, as a refusal states it.
///
/// Service Quotas names the entry `<instance type> for training job usage`, the same shape
/// as the endpoint-usage entry [`endpoint_usage_quota_name`] words, so the instance type is
/// part of the quota's NAME rather than prose around it.
#[must_use]
pub fn training_usage_quota_name(instance_type: &str) -> String {
    format!("{instance_type} for training job usage")
}

/// The instance type a `ResourceConfig` that names none is counted under, so a request's
/// demand is still measured against a quota entry.
///
/// A configuration that names no type at all is refused before any demand is counted
/// ([`resource_config_refusal`]); this label is what remains for one whose compute is
/// stated as instance groups, which carry their own types.
pub const UNNAMED_INSTANCE_TYPE: &str = "ml.unspecified";

/// Why the ML compute `config` names is not compute SageMaker launches, or `None` when it
/// is — the 400 `ValidationException` half of this module's reading.
#[must_use]
pub fn resource_config_refusal(config: &ResourceConfig) -> Option<String> {
    // A heterogeneous cluster states its compute in its groups, so a configuration
    // carrying groups is judged on THEM rather than on the members a single-type
    // configuration uses. Either way every count has to be greater than zero: a group
    // asking for no instances provisions nothing, exactly as a bare count of zero does.
    let groups: Vec<&crate::ops::types::InstanceGroup> =
        config.instance_groups.iter().flatten().collect();
    if !groups.is_empty() {
        let launchable = groups.iter().all(|group| {
            !group.instance_type.is_empty() && group.instance_count >= TRAINING_INSTANCE_COUNT_MIN
        });
        return (!launchable).then(|| INSTANCE_TYPE_AND_COUNT_REQUIRED.to_string());
    }
    let named = config
        .instance_type
        .as_deref()
        .is_some_and(|instance_type| !instance_type.is_empty());
    let counted = config
        .instance_count
        .is_some_and(|count| count >= TRAINING_INSTANCE_COUNT_MIN);
    (!named || !counted).then(|| INSTANCE_TYPE_AND_COUNT_REQUIRED.to_string())
}

/// Why the capacity `config` asks for is past an account quota, or `None` when every quota
/// it draws on has room — the modelled `ResourceLimitExceeded` half of this module's
/// reading, worded by [`account_quota_exceeded`] over the quota's own entry.
///
/// `instances_in_use` answers how many instances of one type the account is already
/// running, and `volume_in_use` the largest ML storage volume its jobs already hold, so
/// each quota is weighed against the account's own utilisation of it.
pub fn resource_config_quota_refusal(
    config: &ResourceConfig,
    instances_in_use: impl Fn(&str) -> i64,
    volume_in_use: i64,
) -> Option<String> {
    // The storage first: it is a property of ONE instance, so the quota it draws on is the
    // per-instance entry rather than a per-type one.
    if let Some(size) = config.volume_size_in_gb {
        if size > TRAINING_VOLUME_SIZE_MAX_GB {
            return Some(account_quota_exceeded(
                TRAINING_VOLUME_QUOTA_NAME,
                VOLUME_SIZE_UNIT,
                i64::from(TRAINING_VOLUME_SIZE_MAX_GB),
                volume_in_use,
                i64::from(size),
            ));
        }
    }
    // Then the instances, per type: one type past its quota refuses the whole request, and
    // the one reported is the first in type order, so the same request is always refused
    // the same way — the reading the hosting quota takes
    // ([`ComAmazonawsSagemakerSagemakerState::hosting_quota_exceeded`]).
    training_instance_demand(config)
        .iter()
        .find_map(|(instance_type, delta)| {
            let held = instances_in_use(instance_type);
            (held + delta > DEFAULT_TRAINING_USAGE_QUOTA).then(|| {
                account_quota_exceeded(
                    &training_usage_quota_name(instance_type),
                    INSTANCES_UNIT,
                    DEFAULT_TRAINING_USAGE_QUOTA,
                    held,
                    *delta,
                )
            })
        })
}

/// How many instances of each ML instance type `config` asks for: one entry per type, the
/// counts of the configuration itself and of its instance groups added up.
///
/// A heterogeneous cluster's groups each name their own type, so a request naming one type
/// twice demands the sum rather than the larger of the two.
#[must_use]
pub fn training_instance_demand(config: &ResourceConfig) -> BTreeMap<String, i64> {
    let mut demand: BTreeMap<String, i64> = BTreeMap::new();
    if let Some(count) = config.instance_count {
        let instance_type = config
            .instance_type
            .clone()
            .unwrap_or_else(|| UNNAMED_INSTANCE_TYPE.to_string());
        *demand.entry(instance_type).or_default() += i64::from(count);
    }
    for group in config.instance_groups.iter().flatten() {
        *demand.entry(group.instance_type.clone()).or_default() += i64::from(group.instance_count);
    }
    demand
}

impl ComAmazonawsSagemakerSagemakerState {
    /// How many instances of `instance_type` the account's training jobs are already
    /// using — the utilisation its training quota is measured against.
    ///
    /// Only a job that is still `InProgress` holds instances: a job the clock has settled
    /// into a terminal status has released its cluster, so it counts for nothing, exactly
    /// as a deleted endpoint does for the hosting quota.
    pub fn training_instances_in_use(&self, instance_type: &str) -> i64 {
        self.training_jobs
            .values()
            .filter(|job| job.status == TRAINING_STATUS_IN_PROGRESS)
            .filter_map(|job| job.request.resource_config.as_ref())
            .filter_map(|config| training_instance_demand(config).remove(instance_type))
            .sum()
    }

    /// The largest ML storage volume the account's training jobs already hold — the
    /// utilisation the per-instance volume quota is measured against.
    ///
    /// The entry bounds ONE instance's volume, so what an account is using of it is the
    /// largest volume any of its running jobs was given rather than a sum; a job the clock
    /// has settled into a terminal status has released its storage and counts for nothing.
    pub fn training_volume_in_use(&self) -> i64 {
        self.training_jobs
            .values()
            .filter(|job| job.status == TRAINING_STATUS_IN_PROGRESS)
            .filter_map(|job| job.request.resource_config.as_ref())
            .filter_map(|config| config.volume_size_in_gb)
            .map(i64::from)
            .max()
            .unwrap_or_default()
    }

    /// Why this account cannot run the capacity `config` asks for, or `None` when every
    /// quota it draws on has room: [`resource_config_quota_refusal`] over this account's
    /// own utilisation.
    pub fn training_quota_refusal(&self, config: &ResourceConfig) -> Option<String> {
        resource_config_quota_refusal(
            config,
            |instance_type| self.training_instances_in_use(instance_type),
            self.training_volume_in_use(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        resource_config_quota_refusal, resource_config_refusal, training_instance_demand,
        training_usage_quota_name, DEFAULT_TRAINING_USAGE_QUOTA, TRAINING_VOLUME_QUOTA_NAME,
        TRAINING_VOLUME_SIZE_MAX_GB, UNNAMED_INSTANCE_TYPE, VOLUME_SIZE_UNIT,
    };
    use crate::ops::types::{InstanceGroup, ResourceConfig};
    use crate::state::{account_quota_exceeded, INSTANCES_UNIT};

    /// A configuration of `count` instances of `instance_type` with a `size` GiB volume.
    fn config(instance_type: Option<&str>, count: Option<i32>, size: Option<i32>) -> ResourceConfig {
        ResourceConfig {
            instance_type: instance_type.map(str::to_string),
            instance_count: count,
            volume_size_in_gb: size,
            ..ResourceConfig::default()
        }
    }

    /// One instance group asking for `count` instances of `instance_type`.
    fn group(instance_type: &str, count: i32) -> InstanceGroup {
        InstanceGroup {
            instance_type: instance_type.to_string(),
            instance_count: count,
            instance_group_name: format!("group-{count}"),
        }
    }

    /// RULE: a configuration names launchable compute exactly when it carries an instance
    /// type with a count greater than zero, or instance groups that each do.
    #[test]
    fn launchable_compute_is_a_type_and_a_count_greater_than_zero() {
        // (case, configuration, launchable)
        let rows: Vec<(&str, ResourceConfig, bool)> = vec![
            (
                "one instance of a named type",
                config(Some("ml.m5.large"), Some(1), Some(30)),
                true,
            ),
            (
                "a distributed job",
                config(Some("ml.m5.large"), Some(8), Some(30)),
                true,
            ),
            (
                "no volume size stated at all",
                config(Some("ml.m5.large"), Some(2), None),
                true,
            ),
            (
                "a count of zero",
                config(Some("ml.m5.large"), Some(0), Some(30)),
                false,
            ),
            (
                "no count at all",
                config(Some("ml.m5.large"), None, Some(30)),
                false,
            ),
            (
                "a count with no instance type",
                config(None, Some(2), Some(30)),
                false,
            ),
            (
                "an empty instance type",
                config(Some(""), Some(2), Some(30)),
                false,
            ),
            ("neither member", config(None, None, Some(30)), false),
            (
                "a heterogeneous cluster of two populated groups",
                ResourceConfig {
                    instance_groups: Some(vec![group("ml.m5.large", 2), group("ml.c5.xlarge", 3)]),
                    ..ResourceConfig::default()
                },
                true,
            ),
            (
                "a heterogeneous cluster whose second group asks for nothing",
                ResourceConfig {
                    instance_groups: Some(vec![group("ml.m5.large", 2), group("ml.c5.xlarge", 0)]),
                    ..ResourceConfig::default()
                },
                false,
            ),
            (
                "a group with no instance type",
                ResourceConfig {
                    instance_groups: Some(vec![group("", 2)]),
                    ..ResourceConfig::default()
                },
                false,
            ),
        ];

        for (case, held, launchable) in rows {
            let refusal = resource_config_refusal(&held);
            assert!(
                refusal.is_none() == launchable,
                "case {case:?}: expected launchable={launchable}, got {refusal:?}"
            );
        }
    }

    /// RULE: a volume past the published per-instance quota, and instances past the
    /// account's quota for their type, are each refused; everything inside both is
    /// admitted.
    #[test]
    fn the_quotas_bound_the_volume_and_the_instances_of_each_type() {
        // (case, configuration, instances of ml.m5.large already held, refused)
        let rows: Vec<(&str, ResourceConfig, i64, bool)> = vec![
            (
                "a volume size of zero, which SageMaker selects for itself",
                config(Some("ml.m5.large"), Some(1), Some(0)),
                0,
                false,
            ),
            (
                "a volume at the published quota",
                config(Some("ml.m5.large"), Some(1), Some(TRAINING_VOLUME_SIZE_MAX_GB)),
                0,
                false,
            ),
            (
                "a volume one past the published quota",
                config(
                    Some("ml.m5.large"),
                    Some(1),
                    Some(TRAINING_VOLUME_SIZE_MAX_GB + 1),
                ),
                0,
                true,
            ),
            (
                "instances at the quota",
                config(Some("ml.m5.large"), Some(1_000), Some(30)),
                0,
                false,
            ),
            (
                "instances one past the quota",
                config(Some("ml.m5.large"), Some(1_001), Some(30)),
                0,
                true,
            ),
            (
                "instances that fit only because another type holds the rest",
                config(Some("ml.m5.large"), Some(500), Some(30)),
                500,
                false,
            ),
            (
                "instances past the quota once the account's own are counted",
                config(Some("ml.m5.large"), Some(501), Some(30)),
                500,
                true,
            ),
            (
                "a group past the quota for its own type",
                ResourceConfig {
                    instance_groups: Some(vec![group("ml.c5.xlarge", 1_001)]),
                    ..ResourceConfig::default()
                },
                0,
                true,
            ),
        ];

        for (case, held, in_use, refused) in rows {
            let refusal = resource_config_quota_refusal(
                &held,
                |instance_type| {
                    if instance_type == "ml.m5.large" {
                        in_use
                    } else {
                        0
                    }
                },
                0,
            );
            assert!(
                refusal.is_some() == refused,
                "case {case:?}: expected refused={refused}, got {refusal:?}"
            );
        }
    }

    /// RULE: each quota refusal is the account-quota sentence over that quota's own entry,
    /// stating its limit, the account's utilisation of it and this request's delta.
    #[test]
    fn a_quota_refusal_states_the_entry_the_limit_the_utilisation_and_the_delta() {
        let size = TRAINING_VOLUME_SIZE_MAX_GB + 1;
        let held = 30;
        let refusal = resource_config_quota_refusal(
            &config(Some("ml.m5.large"), Some(1), Some(size)),
            |_| 0,
            held,
        );
        assert_eq!(
            refusal.as_deref(),
            Some(
                account_quota_exceeded(
                    TRAINING_VOLUME_QUOTA_NAME,
                    VOLUME_SIZE_UNIT,
                    i64::from(TRAINING_VOLUME_SIZE_MAX_GB),
                    held,
                    i64::from(size),
                )
                .as_str()
            ),
            "case \"a volume past its quota\": the refusal's message, got {refusal:?}"
        );

        let refusal = resource_config_quota_refusal(
            &config(Some("ml.m5.large"), Some(600), Some(30)),
            |_| 500,
            0,
        );
        assert_eq!(
            refusal.as_deref(),
            Some(
                account_quota_exceeded(
                    &training_usage_quota_name("ml.m5.large"),
                    INSTANCES_UNIT,
                    DEFAULT_TRAINING_USAGE_QUOTA,
                    500,
                    600,
                )
                .as_str()
            ),
            "case \"instances past their quota\": the refusal's message, got {refusal:?}"
        );
    }

    /// RULE: the demand is counted per instance type, groups included, and a
    /// configuration naming no type is counted under its own label.
    #[test]
    fn the_demand_is_summed_per_instance_type() {
        let mut held = config(Some("ml.m5.large"), Some(2), Some(30));
        held.instance_groups = Some(vec![group("ml.m5.large", 3), group("ml.c5.xlarge", 4)]);
        let demand = training_instance_demand(&held);
        assert_eq!(
            demand.get("ml.m5.large").copied(),
            Some(5),
            "case \"a type named twice\": the counts add up, got {demand:?}"
        );
        assert_eq!(
            demand.get("ml.c5.xlarge").copied(),
            Some(4),
            "case \"a group's own type\": got {demand:?}"
        );

        let unnamed = training_instance_demand(&ResourceConfig {
            instance_count: Some(2),
            ..ResourceConfig::default()
        });
        assert_eq!(
            unnamed.get(UNNAMED_INSTANCE_TYPE).copied(),
            Some(2),
            "case \"no instance type\": the demand is counted under the unnamed label, got \
             {unnamed:?}"
        );
    }
}
