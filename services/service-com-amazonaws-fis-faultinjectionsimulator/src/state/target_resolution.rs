// SPDX-License-Identifier: CC0-1.0

//! What an experiment's TARGETS resolve to, and the resources they are resolved against.
//!
//! FIS documents a target as "a specific resource … or one or more resources that match
//! criteria that you specify, for example, resources that have specific tags", so a target
//! identifies its resources in one of two ways and this module serves both:
//!
//! * it NAMES them (`resourceArns`), and then the resources are the ones it named;
//! * it DESCRIBES them (`resourceTags`), and then the resources are the ones of the
//!   target's `resourceType` that CARRY those tags — which belong to another service, so
//!   they are read out of that service's own store in the SAME shared engine
//!   ([`TargetableResources::read`], the way the execution-role gate reads the account's
//!   IAM store in [`super::action_permissions`] and the stop conditions read the
//!   CloudWatch store in [`super::stop_conditions`]) and never guessed at.
//!
//! Either way the identified resources are then narrowed by the target's `filters` —
//! each one "the attribute path for the filter" and "the attribute values for the filter",
//! so a resource is admitted when the attribute its path names holds one of those values —
//! and scoped by its `selectionMode` ([`selection_count`]).
//!
//! The attributes a filter path may name are the members of the resource as the service
//! that OWNS it reports one, which is where each path below comes from: an EC2 instance's
//! are the members of the `Instance` a describe of it answers with (`State.Name`,
//! `Placement.AvailabilityZone` and the rest), an IAM role's are the members of `Role`.
//! A resource this emulator holds no record of has no attributes to read, which shows
//! nothing about it: a target that NAMED such a resource identifies it as it always did,
//! because a filter can only narrow what something can be read from — the same reading the
//! privileges gate takes of a role the account's IAM store does not hold.

use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

use super::*;

/// The target resource type whose resources are the account's EC2 subnets, as
/// `ListTargetResourceTypes` publishes it: "An Amazon EC2 subnet."
pub const RESOURCE_TYPE_EC2_SUBNET: &str = "aws:ec2:subnet";

/// The target resource type whose resources are the account's EC2 Spot Instances, as that
/// same listing publishes it: "An Amazon EC2 Spot Instance." — the instances of the
/// account whose lifecycle is the Spot one, which is a fact of the instance itself
/// ([`INSTANCE_LIFECYCLE_PATH`]).
pub const RESOURCE_TYPE_EC2_SPOT_INSTANCE: &str = "aws:ec2:spot-instance";

/// The attribute path an instance's lifecycle is read under: `Instance$InstanceLifecycle`,
/// "whether this is a Spot Instance or a Scheduled Instance".
const INSTANCE_LIFECYCLE_PATH: &str = "InstanceLifecycle";

/// The ARN vendor the EC2 resources of this module carry, and the resource type a subnet's
/// ARN names (`arn:aws:ec2:<region>:<account>:subnet/<id>`).
const EC2_ARN_VENDOR: &str = "ec2";

/// See [`EC2_ARN_VENDOR`].
const EC2_INSTANCE_ARN_RESOURCE: &str = "instance";

/// See [`EC2_ARN_VENDOR`].
const EC2_SUBNET_ARN_RESOURCE: &str = "subnet";

/// How many resources of ONE type a snapshot of the engine's stores holds.
///
/// One experiment's resolution is bounded by [`MAX_RESOLVED_TARGETS`], so a snapshot that
/// held more resources than that could never contribute one of them; bounding the READ at
/// the same number is what keeps the work a start pays for the stores of other services
/// fixed rather than growing with how many resources those stores happen to hold.
pub const MAX_TARGETABLE_RESOURCES: usize = MAX_RESOLVED_TARGETS;

/// How many STEPS ONE resolution spends, over every target of the experiment together: one
/// per candidate resource it examines, one per resource a target NAMES, and one per filter
/// value it compares.
///
/// None of the factors those steps multiply is capped by the model: a template's `targets`
/// map carries no ceiling, and neither a target's `filters` nor a filter's `values` does
/// (`smithy_query(query="shape", shape="ExperimentTemplateTargetFilterList", model_path="services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json")`),
/// so this budget is what bounds their PRODUCT rather than each factor alone. A resolution
/// that spends it identifies nothing further, which is the same "what cannot be finished
/// being read shows nothing" reading the rest of this service takes of a bounded walk. It is
/// comfortably above what any experiment resolves, since the resolution itself stops at
/// [`MAX_RESOLVED_TARGETS`].
pub const MAX_RESOLUTION_STEPS: usize = 10 * MAX_RESOLVED_TARGETS;

/// One resource a target can identify, as the service that OWNS it holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetableResource {
    /// The resource's ARN — what a resolved entry reports under
    /// [`TARGET_INFORMATION_ARN_KEY`], and what a target that NAMES its resources names.
    pub arn: String,
    /// The tags the resource carries, as its owning service holds them: the map a target's
    /// `resourceTags` is matched against.
    pub tags: BTreeMap<String, String>,
    /// The attributes a `filters` entry's `path` can name, by that path — the members of
    /// the resource as its owning service reports one (see the module docs). An attribute
    /// the resource does not hold is absent rather than empty.
    pub attributes: BTreeMap<String, String>,
}

impl TargetableResource {
    /// Whether this resource carries every tag of `wanted` — the condition a target's
    /// `resourceTags` states.
    ///
    /// Every pair has to match, key and value both: `resourceTags` is "the tags for the
    /// target resources", so a resource missing one of them is not one of the resources the
    /// target describes. A target naming no tag at all is matched by every resource, which
    /// is why the caller only asks this of a target that named some.
    fn carries_tags(&self, wanted: &HashMap<String, String>) -> bool {
        wanted
            .iter()
            .all(|(key, value)| self.tags.get(key) == Some(value))
    }

    /// Whether every filter of `filters` admits this resource.
    ///
    /// A filter names an attribute by `path` and the values that attribute may hold, so the
    /// resource is admitted when its attribute at that path holds one of them. An attribute
    /// this emulator does not hold for the resource — a path no member of the owning
    /// service's report names — admits it for no value, and neither does a filter carrying
    /// no value at all: both narrow to nothing rather than to everything.
    ///
    /// `steps` is what is left of the whole resolution's work budget
    /// ([`MAX_RESOLUTION_STEPS`]): every value compared spends one of it, because how many
    /// filters a target carries and how many values each one lists are both unbounded by the
    /// model. A comparison there is no budget left for admits nothing, so no template can
    /// make a resolution pay more than that budget however long those lists are.
    fn admitted_by(&self, filters: &[ExperimentTargetFilter], steps: &mut usize) -> bool {
        filters.iter().all(|filter| {
            let Some(attribute) = filter
                .path
                .as_deref()
                .and_then(|path| self.attributes.get(path))
            else {
                return false;
            };
            for value in filter.values.iter().flatten() {
                if *steps == 0 {
                    return false;
                }
                *steps -= 1;
                if value == attribute {
                    return true;
                }
            }
            false
        })
    }
}

/// The resources of OTHER services an experiment's targets are resolved against, as one
/// snapshot.
///
/// Built with each owning store's guard held and holding none afterwards, so a start
/// resolves its targets with those stores free — and against ONE snapshot, so two targets
/// of one experiment cannot disagree about a resource.
#[derive(Debug, Clone, Default)]
pub struct TargetableResources {
    /// The resources of each target resource type this emulator can read, in ARN order —
    /// the stable total order the resolution of a tag-identified target reports them in.
    by_type: BTreeMap<&'static str, Vec<TargetableResource>>,
}

impl TargetableResources {
    /// The snapshot in which NO resource exists: what an engine carrying none of the
    /// owning stores shows, and what a resolution of targets that name every resource of
    /// theirs is taken against.
    pub fn none() -> Self {
        Self::default()
    }

    /// The resources the account's stores in this engine hold right now, for the Region
    /// `region` the request was signed for.
    ///
    /// REGIONAL resources are scoped to that Region, because a resource created through one
    /// Region's endpoint is not one an experiment started in another resolves; the account's
    /// IAM roles are not, because IAM is a global service and its roles belong to the
    /// account rather than to a Region.
    ///
    /// `None` from a store accessor means the engine carries no such store, which shows
    /// nothing about any resource of it: nothing is resolved for the types it owns rather
    /// than a resource being invented for them.
    pub fn read<Eng: Send + Sync + 'static>(engine: &Eng, region: &str) -> Self {
        let mut by_type: BTreeMap<&'static str, Vec<TargetableResource>> = BTreeMap::new();
        if let Some(state) = ec2::service_state(engine) {
            let store = state.lock();
            let instances = ec2_instances(&store, region);
            // A Spot Instance IS one of the account's instances — the one whose lifecycle
            // says so — so the two types are resolved from one read of that store rather
            // than from two notions of what an instance is.
            by_type.insert(
                RESOURCE_TYPE_EC2_SPOT_INSTANCE,
                instances
                    .iter()
                    .filter(|resource| {
                        resource.attributes.get(INSTANCE_LIFECYCLE_PATH).map(String::as_str)
                            == Some(ec2::INSTANCE_LIFECYCLE_SPOT)
                    })
                    .cloned()
                    .collect(),
            );
            by_type.insert(RESOURCE_TYPE_EC2_INSTANCE, instances);
            by_type.insert(RESOURCE_TYPE_EC2_SUBNET, ec2_subnets(&store, region));
        }
        if let Some(state) = iam_state(engine) {
            let store = state.lock();
            by_type.insert(RESOURCE_TYPE_IAM_ROLE, iam_roles(&store));
        }
        Self { by_type }
    }

    /// The resources of the target resource type `resource_type` this snapshot holds, in
    /// ARN order — empty for a type whose owning store this emulator does not hold.
    pub fn of_type(&self, resource_type: &str) -> &[TargetableResource] {
        self.by_type
            .get(resource_type)
            .map_or(&[], |resources| resources.as_slice())
    }

    /// The resource of type `resource_type` the ARN `arn` names, or `None` when this
    /// snapshot holds no record of it — which is what a target that NAMES a resource of
    /// another account, or of a service this emulator does not hold, resolves against.
    ///
    /// The resources of one type are held in ARN order ([`in_arn_order`]), so the lookup is
    /// a binary search: a target that names resources costs the same however many resources
    /// the owning store holds.
    fn by_arn(&self, resource_type: &str, arn: &str) -> Option<&TargetableResource> {
        let resources = self.of_type(resource_type);
        let found = resources
            .binary_search_by(|resource| resource.arn.as_str().cmp(arn))
            .ok()?;
        resources.get(found)
    }
}

/// The account's EC2 instances in `region`, as resources a target can identify.
///
/// The ARN is the one EC2 reports for an instance, built through the shared
/// [`::engine::emulator_arn`] every other ARN of this emulator goes through, so an instance
/// a tag-identified target resolves to is named exactly as a target that NAMED it would
/// have named it — which is what makes an action's effect reach it
/// ([`super::action_execution`]).
///
/// The attributes are read off the instance as `DescribeInstances` reports it (EC2's own
/// `instance_view`), so a filter path names the same member a client reading that describe
/// would name.
fn ec2_instances(
    store: &ec2::ComAmazonawsEc2Amazonec2State,
    region: &str,
) -> Vec<TargetableResource> {
    let mut resources = Vec::new();
    // The store keys its instances by id in a map that iterates in NO fixed order, so the
    // walk goes over the ids in order: which instances a store larger than the budget
    // contributes is then the same at every read, and an id's order is its ARN's.
    let mut instance_ids: Vec<&String> = store.instances.keys().collect();
    instance_ids.sort();
    for instance_id in instance_ids {
        if resources.len() >= MAX_TARGETABLE_RESOURCES {
            break;
        }
        let Some(record) = store.instances.get(instance_id) else {
            continue;
        };
        // An instance's Region is its Availability Zone's, read through EC2's own reading
        // of a zone name so the two services cannot place one instance in two Regions.
        if ec2::region_of_zone_name(&record.placement_az) != region {
            continue;
        }
        let view = store.instance_view(record);
        resources.push(TargetableResource {
            arn: ::engine::emulator_arn(
                EC2_ARN_VENDOR,
                region,
                &format!("{EC2_INSTANCE_ARN_RESOURCE}/{}", record.instance_id),
            ),
            tags: record.tags.clone(),
            attributes: attributes([
                ("InstanceId", view.instance_id.clone()),
                ("ImageId", view.image_id.clone()),
                ("InstanceType", view.instance_type.clone()),
                ("Architecture", view.architecture.clone()),
                ("KeyName", view.key_name.clone()),
                ("SubnetId", view.subnet_id.clone()),
                ("VpcId", view.vpc_id.clone()),
                (INSTANCE_LIFECYCLE_PATH, view.instance_lifecycle.clone()),
                ("PrivateIpAddress", view.private_ip_address.clone()),
                ("PublicIpAddress", view.public_ip_address.clone()),
                (
                    "State.Name",
                    view.state.as_ref().and_then(|state| state.name.clone()),
                ),
                (
                    "State.Code",
                    view.state
                        .as_ref()
                        .and_then(|state| state.code)
                        .map(|code| code.to_string()),
                ),
                (
                    "Placement.AvailabilityZone",
                    view.placement
                        .as_ref()
                        .and_then(|placement| placement.availability_zone.clone()),
                ),
                (
                    "Placement.Tenancy",
                    view.placement
                        .as_ref()
                        .and_then(|placement| placement.tenancy.clone()),
                ),
            ]),
        });
    }
    in_arn_order(resources)
}

/// The account's EC2 subnets in `region`, as resources a target can identify — read the
/// same way [`ec2_instances`] reads an instance, off the `Subnet` a describe of it reports.
fn ec2_subnets(
    store: &ec2::ComAmazonawsEc2Amazonec2State,
    region: &str,
) -> Vec<TargetableResource> {
    let mut resources = Vec::new();
    for (subnet_id, subnet) in &store.subnets {
        if resources.len() >= MAX_TARGETABLE_RESOURCES {
            break;
        }
        // A subnet belongs to ONE Availability Zone, so its Region is that zone's.
        let zone = subnet.availability_zone.as_deref().unwrap_or_default();
        if ec2::region_of_zone_name(zone) != region {
            continue;
        }
        resources.push(TargetableResource {
            // `Subnet$SubnetArn` is the ARN EC2 itself reports for the subnet; a record
            // holding none is named the way every other EC2 ARN here is built.
            arn: subnet.subnet_arn.clone().unwrap_or_else(|| {
                ::engine::emulator_arn(
                    EC2_ARN_VENDOR,
                    region,
                    &format!("{EC2_SUBNET_ARN_RESOURCE}/{subnet_id}"),
                )
            }),
            tags: ec2_tags(subnet.tags.as_deref()),
            attributes: attributes([
                ("SubnetId", subnet.subnet_id.clone()),
                ("State", subnet.state.clone()),
                ("VpcId", subnet.vpc_id.clone()),
                ("CidrBlock", subnet.cidr_block.clone()),
                ("AvailabilityZone", subnet.availability_zone.clone()),
                ("AvailabilityZoneId", subnet.availability_zone_id.clone()),
            ]),
        });
    }
    in_arn_order(resources)
}

/// The account's IAM roles, as resources a target can identify.
///
/// The ARN is the role's own (`Role$Arn`) — the record `CreateRole` wrote — so a
/// tag-identified role is named exactly as a target that NAMED it would have named it,
/// which is what makes the API faults of a running experiment reach it
/// ([`super::api_fault_injection`]). The attributes are the members of that same `Role`.
fn iam_roles(store: &IamState) -> Vec<TargetableResource> {
    let mut resources = Vec::new();
    for role in store.roles.values() {
        if resources.len() >= MAX_TARGETABLE_RESOURCES {
            break;
        }
        resources.push(TargetableResource {
            arn: role.arn.clone(),
            tags: role
                .tags
                .iter()
                .flatten()
                .map(|tag| (tag.key.clone(), tag.value.clone()))
                .collect(),
            attributes: attributes([
                ("Arn", Some(role.arn.clone())),
                ("RoleName", Some(role.role_name.clone())),
                ("RoleId", Some(role.role_id.clone())),
                ("Path", Some(role.path.clone())),
                ("Description", role.description.clone()),
                (
                    "MaxSessionDuration",
                    role.max_session_duration
                        .map(|duration| duration.to_string()),
                ),
            ]),
        });
    }
    in_arn_order(resources)
}

/// `tags` as a tag map, from the `Tag` list EC2 reports a resource's tags as.
fn ec2_tags(
    tags: Option<&[::service_com_amazonaws_ec2_amazonec2::ops::types::Tag]>,
) -> BTreeMap<String, String> {
    // `Tag$Key` and `Tag$Value` are both optional in EC2's model; a tag with no key is no
    // tag at all, and one with no value carries the empty value.
    tags.into_iter()
        .flatten()
        .filter_map(|tag| Some((tag.key.clone()?, tag.value.clone().unwrap_or_default())))
        .collect()
}

/// The attribute map a filter path is read out of: every entry whose value the resource
/// holds, keyed by the path that names it. An absent or empty member is left OUT, so a
/// filter on it narrows to nothing rather than matching the empty string.
fn attributes<const N: usize>(
    entries: [(&'static str, Option<String>); N],
) -> BTreeMap<String, String> {
    entries
        .into_iter()
        .filter_map(|(path, value)| {
            value
                .filter(|value| !value.is_empty())
                .map(|value| (path.to_string(), value))
        })
        .collect()
}

/// `resources` in ARN order — the stable total order a tag-identified target's resolution
/// is reported in, so two reads of one experiment report the same resolution and a store
/// whose map iterates in no fixed order cannot reorder it.
fn in_arn_order(mut resources: Vec<TargetableResource>) -> Vec<TargetableResource> {
    resources.sort_by(|left, right| left.arn.cmp(&right.arn));
    resources
}

/// What `targets` RESOLVE to: one `ResolvedTarget` per resource a target identifies,
/// scoped by that target's `selectionMode`.
///
/// `resources` is the snapshot of the engine's stores the tag-identified targets are
/// resolved against ([`TargetableResources::read`]).
///
/// The order is a stable total order — targets by name, and within one target the order
/// [`identified_resources`] reports its resources in — which is what
/// [`resolved_target_page_token`] indexes into.
///
/// The work is linear and bounded: de-duplication goes through a set, one target names at
/// most [`MAX_TARGET_RESOURCE_ARNS`] resources (the model's own ceiling, refused at the
/// boundary) and resolves each of them by a binary search, and every candidate resource
/// examined, resource named and filter value compared spends one step of
/// [`MAX_RESOLUTION_STEPS`] — which bounds the product of the factors the model leaves
/// unbounded. The resolution itself stops at [`MAX_RESOLVED_TARGETS`].
pub fn resolve_targets(
    targets: &HashMap<String, ExperimentTarget>,
    resources: &TargetableResources,
) -> Vec<ResolvedTarget> {
    let mut by_name: Vec<(&String, &ExperimentTarget)> = targets.iter().collect();
    by_name.sort_by_key(|(name, _)| *name);
    let mut resolved = Vec::new();
    let mut steps = MAX_RESOLUTION_STEPS;
    for (name, target) in by_name {
        // [`MAX_RESOLVED_TARGETS`] is the emulator's own ceiling on one experiment's
        // resolution: the work this function does, and the records it hands to the store,
        // are bounded by it rather than by how long a list a request carried.
        if resolved.len() >= MAX_RESOLVED_TARGETS {
            break;
        }
        let identified = identified_resources(target, resources, &mut steps);
        let keep = selection_count(target.selection_mode.as_deref(), identified.len())
            .min(MAX_RESOLVED_TARGETS - resolved.len());
        for arn in identified.into_iter().take(keep) {
            resolved.push(ResolvedTarget {
                resource_type: target.resource_type.clone(),
                target_name: Some(name.clone()),
                target_information: Some(HashMap::from([(
                    TARGET_INFORMATION_ARN_KEY.to_string(),
                    arn,
                )])),
            });
        }
    }
    resolved
}

/// The ARNs of the resources `target` IDENTIFIES, before its `selectionMode` scopes them.
///
/// The two ways a target identifies its resources, which a stored target carries exactly
/// one of ([`targets_refusal`] refuses a target naming both and a target naming neither):
///
/// * it NAMES them, and then they are the ARNs it named, in that order, each named resource
///   identified once however often the target lists it;
/// * it DESCRIBES them by `resourceTags`, and then they are the resources of the target's
///   `resourceType` this emulator holds that carry every one of those tags, in ARN order.
///
/// Either way the target's `filters` then narrow them: a resource is kept only when every
/// filter admits it ([`TargetableResource::admitted_by`]), and a NAMED resource this
/// emulator holds no record of is kept, because nothing can be read about it to narrow it
/// by.
///
/// `steps` is what is left of the whole resolution's work budget
/// ([`MAX_RESOLUTION_STEPS`]): every candidate resource examined, every resource named and
/// every filter value compared spends one of it, and a target reaching here with none left
/// identifies nothing.
fn identified_resources(
    target: &ExperimentTarget,
    resources: &TargetableResources,
    steps: &mut usize,
) -> Vec<String> {
    let resource_type = target.resource_type.as_deref().unwrap_or_default();
    let filters = target.filters.as_deref().unwrap_or_default();
    // Membership goes through a SET, so de-duplicating the resources costs time linear in
    // how many there are rather than quadratic.
    let mut seen: HashSet<String> = HashSet::new();
    let mut identified: Vec<String> = Vec::new();
    match target
        .resource_tags
        .as_ref()
        .filter(|tags| !tags.is_empty())
    {
        Some(wanted) => {
            for resource in resources.of_type(resource_type) {
                if *steps == 0 {
                    break;
                }
                *steps -= 1;
                if !resource.carries_tags(wanted) || !resource.admitted_by(filters, steps) {
                    continue;
                }
                if seen.insert(resource.arn.clone()) {
                    identified.push(resource.arn.clone());
                }
            }
        }
        None => {
            for arn in target.resource_arns.iter().flatten() {
                if *steps == 0 {
                    break;
                }
                *steps -= 1;
                if resources
                    .by_arn(resource_type, arn)
                    .is_some_and(|resource| !resource.admitted_by(filters, steps))
                {
                    continue;
                }
                if seen.insert(arn.clone()) {
                    identified.push(arn.clone());
                }
            }
        }
    }
    identified
}

/// How many of the `identified` resources a target's `selectionMode` keeps.
///
/// FIS documents three forms: `ALL` ("Run the action on all identified targets. This is
/// the default"), `COUNT(n)` ("the specified number of targets") and `PERCENT(n)` ("the
/// specified percentage of targets"). A count or a percentage asking for more than the
/// target identified keeps what it identified.
///
/// One choice this emulator makes, where FIS documents no rule: a percentage that does
/// not reach a whole resource is taken DOWN (`PERCENT(25)` of two resources keeps none,
/// of four keeps one). FIS picks the kept resources "at random"; this emulator keeps the
/// first ones in the order the resolution identified them, which satisfies the documented
/// count and makes a resolution reproducible.
///
/// A target that names NO `selectionMode`, and one whose mode is in none of the three
/// documented forms, keep every identified resource — the documented default. A stored
/// target is never in that second case: [`targets_refusal`] refuses a mode outside the
/// documented forms before a template carrying it is persisted, and it decides that by
/// asking [`selection_mode_count`], so the rule and the resolution read one definition of
/// the three forms.
pub fn selection_count(selection_mode: Option<&str>, identified: usize) -> usize {
    selection_mode
        .and_then(|mode| selection_mode_count(mode, identified))
        .unwrap_or(identified)
}

/// How many of the `identified` resources the `selectionMode` `mode` keeps, or `None` when
/// `mode` is in NONE of the three forms FIS documents for it.
///
/// The one definition of those forms: `ALL`, `COUNT(n)` and `PERCENT(n)`, matched
/// case-sensitively because the documented forms are upper-case. Both the rule that admits
/// a target ([`targets_refusal`]) and the resolution that scopes one
/// ([`selection_count`]) read it, so a mode a created template carries is one the
/// resolution understands.
pub fn selection_mode_count(mode: &str, identified: usize) -> Option<usize> {
    let mode = mode.trim();
    if mode == SELECTION_MODE_ALL {
        return Some(identified);
    }
    if let Some(count) = selection_argument(mode, SELECTION_MODE_COUNT_PREFIX) {
        return Some(identified.min(count));
    }
    if let Some(percent) = selection_argument(mode, SELECTION_MODE_PERCENT_PREFIX) {
        return Some(identified.saturating_mul(percent.min(100)) / 100);
    }
    None
}

/// The `n` of a `selectionMode` of the form `<prefix>n)`, or `None` when `mode` is not of
/// that form. The documented forms are upper-case, so the match is case-sensitive.
fn selection_argument(mode: &str, prefix: &str) -> Option<usize> {
    mode.strip_prefix(prefix)?.strip_suffix(')')?.trim().parse().ok()
}
