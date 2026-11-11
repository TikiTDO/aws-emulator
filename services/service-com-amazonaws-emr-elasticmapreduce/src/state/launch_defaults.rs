// SPDX-License-Identifier: CC0-1.0

//! What Amazon EMR fills in for a cluster whose launch left it out: the EFFECTIVE
//! configuration a read reports, rather than the request's own words.
//!
//! One concern, split out of [`super`] so the defaults sit beside each other instead of
//! being spread over the projections that report them. A launch states only what the
//! caller cared about, and Amazon EMR describes the cluster it actually created: the
//! scale-down behaviour the release takes, node replacement, the application versions the
//! release ships, the placement groups the request's strategies really create, and — for
//! an instance fleet — the per-instance-type capacity, bid and Amazon EBS layout the
//! service supplies. Every value here is derived from the request and from this service's
//! own catalogs ([`APPLICATION_CATALOG`], [`INSTANCE_FAMILIES`]), so a read reports the
//! same effective configuration however often it is called and nothing is invented per
//! call.

use super::*;
use crate::ops::types::{
    InstanceFleetResizingSpecifications, SpotResizingSpecification, VolumeSpecification,
};

/// The `ScaleDownBehavior` a cluster takes when its launch names none.
///
/// `Cluster$ScaleDownBehavior` is an enum of `TERMINATE_AT_INSTANCE_HOUR` and
/// `TERMINATE_AT_TASK_COMPLETION`, and a cluster launched without one runs under the
/// task-completion behaviour — "Amazon EMR adds nodes to a deny list and drains tasks from
/// nodes before terminating the Amazon EC2 instances"
/// (`RunJobFlow$ScaleDownBehavior` doc). That member's doc sentence also claims
/// `TERMINATE_AT_INSTANCE_HOUR` as "the default for clusters created using" Amazon EMR
/// 5.1.0 and later, which the service's own answers contradict: a cluster launched on a
/// later release without the member is described with the task-completion behaviour, so
/// that is the one taken here.
///
/// The VALUE is the service's; the requirement is that the member is reported at all — a
/// described cluster states the behaviour it runs under
/// ([`effective_scale_down_behavior`]), so a client can tell a default from an unset
/// value.
pub const DEFAULT_SCALE_DOWN_BEHAVIOR: &str = "TERMINATE_AT_TASK_COMPLETION";

/// The scale-down behaviour a cluster launched with `requested` runs under.
pub fn effective_scale_down_behavior(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_SCALE_DOWN_BEHAVIOR.to_string())
}

/// Whether Amazon EMR replaces degraded core nodes of a cluster whose launch did not say.
///
/// "Indicates whether Amazon EMR should gracefully replace core nodes that have degraded
/// within the cluster" (`JobFlowInstancesConfig$UnhealthyNodeReplacement` doc). The member
/// states no default, and the service's own answers settle it: a cluster launched without
/// it is described with graceful replacement OFF, so the behaviour is one a launch opts
/// into — `JobFlowInstancesConfig$UnhealthyNodeReplacement` at the launch, or
/// `SetUnhealthyNodeReplacement` afterwards. Reporting it on for a cluster that never asked
/// would hand a client a setting the operator did not choose.
/// Whether Amazon EMR replaces degraded core nodes of a cluster whose launch did not say.
///
/// "Indicates whether Amazon EMR should gracefully replace core nodes that have degraded
/// within the cluster" (`JobFlowInstancesConfig$UnhealthyNodeReplacement` doc). The member
/// states no default, and the service's own answers settle it: a cluster launched without it
/// is described with graceful replacement ON, so it is a behaviour a launch opts OUT of —
/// with `JobFlowInstancesConfig$UnhealthyNodeReplacement` at the launch, or
/// `SetUnhealthyNodeReplacement` afterwards.
pub const DEFAULT_UNHEALTHY_NODE_REPLACEMENT: bool = true;

/// Whether the cluster a launch created runs with graceful replacement of its degraded core
/// nodes: what `DescribeCluster` reports for `Cluster$UnhealthyNodeReplacement`.
///
/// Two things decide it. A cluster with MORE THAN ONE primary node
/// ([`MIN_HIGHLY_AVAILABLE_PRIMARY_NODES`]) is not offered the behaviour at all — Amazon EMR
/// does not replace nodes of a highly available cluster that way, and describes such a
/// cluster with the setting off however it was launched — so the primary count comes first.
/// On every other cluster it is the launch's own value, or the service's default where the
/// launch named none ([`DEFAULT_UNHEALTHY_NODE_REPLACEMENT`]).
pub fn effective_unhealthy_node_replacement(primary_nodes: i64, requested: Option<bool>) -> bool {
    if primary_nodes >= MIN_HIGHLY_AVAILABLE_PRIMARY_NODES {
        return false;
    }
    requested.unwrap_or(DEFAULT_UNHEALTHY_NODE_REPLACEMENT)
}

/// The Amazon Linux release a cluster runs, out of what its launch asked for.
///
/// "The Amazon Linux release specified in a cluster launch RunJobFlow request. If no Amazon
/// Linux release was specified, the default Amazon Linux release is shown in the response."
/// (`Cluster$OSReleaseLabel` doc.) So a described cluster always states the Amazon Linux
/// release it is running: the one the launch named, or the DEFAULT one for the generation
/// the cluster's Amazon EMR release is built on — the newest release this emulator's catalog
/// offers for that generation ([`os_releases_for`], the same catalog
/// `DescribeReleaseLabel` publishes under `AvailableOSReleases`).
///
/// A launch that named no Amazon EMR release at all names no generation to resolve, so such
/// a cluster states no Amazon Linux release rather than one invented for it.
pub fn effective_os_release_label(
    release_label: Option<&str>,
    requested: Option<String>,
) -> Option<String> {
    if let Some(named) = requested {
        return Some(named);
    }
    let (major, _, _) = release_label.and_then(catalog_release)?;
    os_releases_for(major)
        .iter()
        .next_back()
        .map(|&label| label.to_string())
}

/// The applications a cluster launched on `release_label` installs, out of the ones its
/// launch asked for.
///
/// "A case-insensitive list of applications for Amazon EMR to install and configure when
/// launching the cluster" (`RunJobFlow$Applications` doc) names applications, not
/// versions: "the version of the open-source application packages installed on the
/// cluster" is the RELEASE's (`RunJobFlow$ReleaseLabel` doc), so the service resolves each
/// requested name against the release and reports `Application$Version` as the version
/// that release ships — the same version `DescribeReleaseLabel` publishes for it, read off
/// the one catalog ([`APPLICATION_CATALOG`]). The name is reported in the single spelling
/// the catalog publishes, since the request's is matched without regard to case.
///
/// A name no release of the catalog bundles, and a request that named no release label at
/// all, resolve to no version: there is nothing installed to state one for, so the
/// application is reported exactly as it was asked for.
pub fn installed_applications(
    release_label: Option<&str>,
    requested: Vec<Application>,
) -> Vec<Application> {
    let release = release_label.and_then(catalog_release);
    requested
        .into_iter()
        .map(|application| {
            let Some(catalogued) = application
                .name
                .as_deref()
                .and_then(catalog_application)
            else {
                return application;
            };
            let installed = release
                .and_then(|(major, minor, _)| catalogued.version_at(major, minor))
                .map(str::to_string);
            let mut application = application;
            application.name = Some(catalogued.name.to_string());
            // A version the request stated itself is kept: nothing else in the request
            // says what was installed.
            application.version = application.version.or(installed);
            application
        })
        .collect()
}

/// The catalog's application `name` names, compared without regard to case.
fn catalog_application(name: &str) -> Option<&'static CatalogApplication> {
    APPLICATION_CATALOG
        .iter()
        .find(|application| application.name.eq_ignore_ascii_case(name))
}

/// `com.amazonaws.emr#PlacementGroupStrategy`'s "no placement group" value.
pub const PLACEMENT_GROUP_STRATEGY_NONE: &str = "NONE";

/// The placement groups a launch's `PlacementGroupConfigs` really creates.
///
/// "The configuration specifies the placement strategy that can be applied to instance
/// roles during cluster creation" (`PlacementGroupConfig` doc): an entry asking for the
/// `NONE` strategy asks for no Amazon EC2 placement group, so the launch creates none for
/// that role and the cluster reports none — a described cluster states the placement
/// groups that EXIST, not the entries the request listed.
pub fn created_placement_groups(requested: Vec<PlacementGroupConfig>) -> Vec<PlacementGroupConfig> {
    requested
        .into_iter()
        .filter(|config| {
            config.placement_strategy.as_deref() != Some(PLACEMENT_GROUP_STRATEGY_NONE)
        })
        .collect()
}

/// A configuration tree as a read serves it.
///
/// "A set of properties specified within a configuration classification"
/// (`Configuration$Properties` doc, a `StringMap`): a classification carries a property
/// set, empty for one that sets no property, so every served configuration — at any depth
/// of the tree — reports the map rather than leaving it out, and a client walking the tree
/// has no missing map to special-case. Everything else is reported exactly as it was
/// supplied.
pub fn served_configurations(configurations: &[Configuration]) -> Vec<Configuration> {
    served_configurations_within(configurations, MAX_SERVED_CONFIGURATION_DEPTH)
}

/// How deep a served configuration tree is nested at most.
///
/// CHOSEN, not documented: `ConfigurationList` carries no depth bound, and the service's
/// own ceiling on a stored tree counts its ENTRIES rather than its levels
/// ([`MAX_INSTANCE_GROUP_CONFIGURATIONS`]), which a tree this deep already exceeds. The
/// walk is bounded by construction, so a record this emulator should never have persisted
/// cannot amplify a read: a configuration at this depth is served with its own
/// classification and properties, and the children below it are not reported.
const MAX_SERVED_CONFIGURATION_DEPTH: usize = 100;

/// [`served_configurations`], with `depth` levels of nesting left to serve.
fn served_configurations_within(
    configurations: &[Configuration],
    depth: usize,
) -> Vec<Configuration> {
    configurations
        .iter()
        .map(|configuration| Configuration {
            classification: configuration.classification.clone(),
            configurations: depth.checked_sub(1).and_then(|remaining| {
                configuration
                    .configurations
                    .as_deref()
                    .map(|children| served_configurations_within(children, remaining))
            }),
            properties: Some(configuration.properties.clone().unwrap_or_default()),
        })
        .collect()
}

/// The `WeightedCapacity` an instance type of a fleet provides when its configuration
/// names none.
///
/// "The number of units that a provisioned instance of this type provides toward
/// fulfilling the target capacities defined in InstanceFleetConfig. This value is 1 for a
/// master instance fleet, and must be 1 or greater for core and task instance fleets.
/// Defaults to 1 if not specified." (`InstanceTypeConfig$WeightedCapacity` doc.)
pub const DEFAULT_WEIGHTED_CAPACITY: i32 = 1;

/// The `BidPriceAsPercentageOfOnDemandPrice` a fleet's instance type bids when its
/// configuration names neither a bid price nor a percentage.
///
/// "If neither BidPrice nor BidPriceAsPercentageOfOnDemandPrice is provided,
/// BidPriceAsPercentageOfOnDemandPrice defaults to 100%."
/// (`InstanceTypeConfig$BidPrice` doc.)
pub const DEFAULT_BID_PRICE_PERCENTAGE_OF_ON_DEMAND: f64 = 100.0;

/// The percentage of the On-Demand price an instance type configured with `bid_price` and
/// `requested` percentage bids — `None` for one that named a bid price in USD, which is
/// the bid it made.
pub fn effective_bid_price_percentage(
    bid_price: Option<&str>,
    requested: Option<f64>,
) -> Option<f64> {
    requested.or_else(|| {
        bid_price
            .is_none()
            .then_some(DEFAULT_BID_PRICE_PERCENTAGE_OF_ON_DEMAND)
    })
}

/// Amazon EMR's default Spot resize timeout, in minutes.
///
/// "Spot resize timeout in minutes. If Spot Instances are not provisioned within this
/// time, the resize workflow will stop provisioning of Spot instances. Minimum value is 5
/// minutes and maximum value is 10,080 minutes (7 days)."
/// (`SpotResizingSpecification$TimeoutDurationMinutes` doc.) The timeout governs every
/// resize of a fleet that can be resized, so such a fleet runs under one whether or not
/// its configuration named it, and the service's own value is what it reports.
pub const DEFAULT_SPOT_RESIZE_TIMEOUT_MINUTES: i32 = 60;

/// The resize specifications a fleet of `instance_fleet_type` configured with `requested`
/// runs under: the ones it was given, with the Spot resize timeout the service supplies
/// where the configuration left it out.
///
/// A resize specification governs the resizes of the fleet, and the fleet that runs the
/// cluster's primary nodes is never resized — "the master node cannot be scaled after
/// initial configuration" (`ComputeLimits` doc), which is also why
/// `ModifyInstanceGroups` refuses to scale the primary group — so for that fleet there is
/// no resize policy to supply, and it reports only what its configuration named.
pub fn effective_resize_specifications(
    instance_fleet_type: &str,
    requested: Option<&InstanceFleetResizingSpecifications>,
) -> Option<InstanceFleetResizingSpecifications> {
    if instance_fleet_type == instance_group_type::MASTER {
        return requested.cloned();
    }
    let mut specifications = requested.cloned().unwrap_or_default();
    let spot = specifications.spot_resize_specification.take();
    let allocation_strategy = spot
        .as_ref()
        .and_then(|spot| spot.allocation_strategy.clone());
    let timeout_duration_minutes = spot
        .and_then(|spot| spot.timeout_duration_minutes)
        .unwrap_or(DEFAULT_SPOT_RESIZE_TIMEOUT_MINUTES);
    specifications.spot_resize_specification = Some(SpotResizingSpecification {
        timeout_duration_minutes: Some(timeout_duration_minutes),
        allocation_strategy,
    });
    Some(specifications)
}

/// The Amazon EBS block devices one instance of a fleet's instance type is attached.
///
/// An `InstanceTypeConfig` whose `EbsConfiguration` declares block devices gets exactly
/// the volumes it requested, one `EbsBlockDevice` per requested volume
/// ([`ebs_block_devices`]) — so a configuration that requested none of a declared device
/// keeps that answer. One that declares no block device at all gets the DEFAULT layout of
/// its instance type ([`default_ebs_volume_specifications`]), which is what the service
/// attaches so the instance has storage to run on. Both forms are named in the same
/// `/dev/sd…` sequence, so an instance type's devices read the same whoever chose the
/// volumes.
pub fn fleet_ebs_block_devices(config: &InstanceTypeConfig) -> Vec<EbsBlockDevice> {
    let declares_devices = config
        .ebs_configuration
        .as_ref()
        .and_then(|configuration| configuration.ebs_block_device_configs.as_deref())
        .is_some_and(|configs| !configs.is_empty());
    if declares_devices {
        return ebs_block_devices(config.ebs_configuration.as_ref());
    }
    default_ebs_volume_specifications(&config.instance_type)
        .into_iter()
        .enumerate()
        .map(|(index, volume_specification)| EbsBlockDevice {
            volume_specification: Some(volume_specification),
            device: Some(ebs_device_name(index)),
        })
        .collect()
}

/// The Amazon EBS volume type the default layout is made of.
///
/// CATALOG DATA, from the Amazon EMR Management Guide's default Amazon EBS storage table
/// (the same page the instance-type catalog is refreshed from): the volumes Amazon EMR
/// attaches by default are general-purpose SSD.
const DEFAULT_EBS_VOLUME_TYPE: &str = "gp2";

/// GiB of default Amazon EBS storage per vCPU of an Amazon EBS-only instance type.
///
/// CATALOG DATA, derived from that same table, whose rows are uniform in it: a `large`
/// (2 vCPUs) is attached 32 GiB, an `xlarge` (4) 64 GiB, a `2xlarge` (8) 128 GiB, a
/// `4xlarge` (16) 256 GiB, and so on up the ladder.
const DEFAULT_EBS_GIB_PER_VCPU: i32 = 16;

/// How many volumes that storage is split over at most: the table splits a size's default
/// storage over one volume for a `large`, two for an `xlarge` and four from `2xlarge` up.
const MAX_DEFAULT_EBS_VOLUMES: i32 = 4;

/// The default Amazon EBS volumes Amazon EMR attaches to each instance of
/// `instance_type`, in device order.
///
/// A cluster node needs storage, and an Amazon EBS-only instance type carries none of its
/// own, so Amazon EMR attaches a default layout to every instance of such a type that the
/// request did not configure volumes for. A type that carries instance-store disks
/// ([`InstanceFamilySpec::instance_store`]) already has its storage and is attached no
/// default volumes, and a type outside this service's catalog has no published layout to
/// state, so neither gets any.
pub fn default_ebs_volume_specifications(instance_type: &str) -> Vec<VolumeSpecification> {
    let Some((count, size_in_gb)) = default_ebs_layout(instance_type) else {
        return Vec::new();
    };
    (0..count)
        .map(|_| VolumeSpecification {
            volume_type: DEFAULT_EBS_VOLUME_TYPE.to_string(),
            iops: None,
            size_in_gb,
            throughput: None,
        })
        .collect()
}

/// `(volume count, GiB per volume)` of the default layout of `instance_type`, or `None`
/// for a type that is attached no default volumes.
fn default_ebs_layout(instance_type: &str) -> Option<(i32, i32)> {
    let (family_name, size) = instance_type.split_once('.')?;
    let family = INSTANCE_FAMILIES
        .iter()
        .find(|family| family.family == family_name)?;
    if family.instance_store.iter().any(|disks| disks.size == size) {
        return None;
    }
    let vcpu = vcpu_for_size(size)?;
    let count = (vcpu / 2).clamp(1, MAX_DEFAULT_EBS_VOLUMES);
    Some((count, vcpu.saturating_mul(DEFAULT_EBS_GIB_PER_VCPU) / count))
}
