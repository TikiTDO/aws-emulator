// SPDX-License-Identifier: CC0-1.0

//! What a cluster has COST so far, in the unit Amazon EMR reports it in.
//!
//! One concern of this service's state, split out of [`super`]: `NormalizedInstanceHours` is
//! "an approximation of the cost of the cluster, represented in m1.small/hours. This value
//! is incremented one time for every hour an m1.small instance runs. Larger instances are
//! weighted more" (`Cluster$NormalizedInstanceHours` doc), so it is a function of three
//! things the cluster already holds — which instances it was created with, how much each of
//! them weighs, and how long the cluster has been up — and of nothing else. It is DERIVED
//! at every read rather than stored, so two reads of a cluster that has not run any longer
//! report the same number and nothing is incremented because somebody looked.
//!
//! The weight of one instance is its instance type's normalization factor
//! ([`NORMALIZATION_FACTORS`]), and the hours are counted from the instant the cluster was
//! created to the instant it ended (or to now, for one that is still up), rounded UP to a
//! whole hour: the unit is an instance-hour, and an instance that has run for part of one
//! has that hour counted.

use super::*;

/// How long one normalized instance hour is.
const ONE_HOUR: Duration = Duration::from_secs(60 * 60);

/// The normalization factor of each Amazon EC2 instance SIZE, which is what "larger
/// instances are weighted more" means: one hour of an instance of that size counts as this
/// many m1.small hours.
///
/// CATALOG DATA, from the Amazon EMR Management Guide's normalized-instance-hours table
/// (the same guide this service's instance catalog is refreshed from). The table is in
/// instance SIZE alone: every family of one size carries the same factor.
const NORMALIZATION_FACTORS: &[(&str, i64)] = &[
    ("small", 1),
    ("medium", 2),
    ("large", 4),
    ("xlarge", 8),
    ("2xlarge", 16),
    ("3xlarge", 24),
    ("4xlarge", 32),
    ("6xlarge", 48),
    ("8xlarge", 64),
    ("9xlarge", 72),
    ("10xlarge", 80),
    ("12xlarge", 96),
    ("16xlarge", 128),
    ("18xlarge", 144),
    ("24xlarge", 192),
    ("32xlarge", 256),
];

/// How many m1.small hours one hour of `instance_type` counts as.
///
/// The factor follows the SIZE of the type ([`NORMALIZATION_FACTORS`]). A size the table
/// does not list is weighted by the ratio the table itself is built on — twice the size's
/// vCPU count, which every listed size above `medium` is — and a type whose size this
/// service's catalog knows nothing about weighs nothing, because there is no published
/// factor to report one with.
#[must_use]
pub fn normalization_factor(instance_type: &str) -> i64 {
    let Some((_, size)) = instance_type.split_once('.') else {
        return 0;
    };
    if let Some((_, factor)) = NORMALIZATION_FACTORS
        .iter()
        .find(|(listed, _)| *listed == size)
    {
        return *factor;
    }
    vcpu_for_size(size).map_or(0, |vcpu| i64::from(vcpu).saturating_mul(2))
}

/// How many m1.small hours ONE hour of this cluster's instances counts as: the weight of
/// every instance it was created with.
///
/// A cluster states its instances either as instance GROUPS — each naming its instance type
/// and how many of them it runs — or as instance FLEETS, where the capacity is stated in the
/// fleet's own weighted units and each instance type says how many of those units one of its
/// instances provides (`InstanceTypeConfig$WeightedCapacity`, "the number of units that a
/// provisioned instance of this type provides toward fulfilling the target capacities",
/// defaulting to one). So a fleet's instance count follows from its target capacity and that
/// weight, which is how many instances the fleet has to run to fulfil it.
#[must_use]
pub fn hourly_normalized_units(cluster: &ClusterRecord) -> i64 {
    let in_groups = cluster.groups.iter().fold(0i64, |total, group| {
        let instances = i64::from(group.config.instance_count.max(0));
        total.saturating_add(
            instances.saturating_mul(normalization_factor(&group.config.instance_type)),
        )
    });
    cluster.fleets.iter().fold(in_groups, |total, fleet| {
        total.saturating_add(fleet_normalized_units(fleet))
    })
}

/// See [`hourly_normalized_units`]: what one hour of a fleet's instances weighs.
fn fleet_normalized_units(fleet: &InstanceFleetRecord) -> i64 {
    let capacity = i64::from(fleet.config.target_on_demand_capacity.unwrap_or(0).max(0))
        .saturating_add(i64::from(fleet.config.target_spot_capacity.unwrap_or(0).max(0)));
    if capacity == 0 {
        return 0;
    }
    // The fleet provisions instances of the types it was configured with; the first is what
    // its capacity is counted in here, since this service does not hold which types the
    // fleet really provisioned and the member it feeds is "an approximation of the cost".
    let Some(specification) = fleet
        .config
        .instance_type_configs
        .as_deref()
        .unwrap_or_default()
        .first()
    else {
        return 0;
    };
    let weight = i64::from(
        specification
            .weighted_capacity
            .unwrap_or(DEFAULT_WEIGHTED_CAPACITY)
            .max(1),
    );
    let instances = capacity
        .saturating_add(weight.saturating_sub(1))
        .saturating_div(weight);
    instances.saturating_mul(normalization_factor(&specification.instance_type))
}

/// The `NormalizedInstanceHours` this cluster has accrued by `now`.
///
/// The hours are the ones the cluster has been up for — from its creation to its end, or to
/// `now` while it is still up — rounded UP to a whole hour, because the unit counts an hour
/// an instance ran and an instance that has run for part of one has run in it. They are
/// multiplied by what one hour of the cluster's instances weighs
/// ([`hourly_normalized_units`]).
///
/// Both halves are read off the record, so the value is stable across reads of a cluster
/// that has stopped and grows only as the clock does for one that is still running.
#[must_use]
pub fn normalized_instance_hours(cluster: &ClusterRecord, now: SystemTime) -> i32 {
    let until = cluster.status.end_date_time.unwrap_or(now);
    let elapsed = until
        .duration_since(cluster.status.creation_date_time)
        .unwrap_or_default();
    if elapsed.is_zero() {
        return 0;
    }
    let hours = i64::try_from(
        elapsed
            .as_secs()
            // A part of a second is part of the hour it falls in, so it counts: the cluster
            // has had its instances since the instant it was created.
            .saturating_add(u64::from(elapsed.subsec_nanos() > 0))
            .div_ceil(ONE_HOUR.as_secs()),
    )
    .unwrap_or(i64::MAX);
    let accrued = hourly_normalized_units(cluster).saturating_mul(hours);
    i32::try_from(accrued).unwrap_or(i32::MAX)
}
