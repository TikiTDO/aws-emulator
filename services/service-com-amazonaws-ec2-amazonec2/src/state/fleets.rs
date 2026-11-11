// SPDX-License-Identifier: CC0-1.0

//! The EC2 fleet, Spot datafeed subscription, Mac modification task and IPAM internet-registry
//! stores.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// The `instant` member of [`FLEET_TYPE_VALUES`]: a fleet that places its requests once and
/// reports the instances it launched on the fleet itself.
pub const FLEET_TYPE_INSTANT: &str = "instant";

/// The `maintain` member of [`FLEET_TYPE_VALUES`], which is also what a `CreateFleet`
/// request that names no type gets: "Default: `maintain`" (`FleetData$Type`).
pub const FLEET_TYPE_MAINTAIN: &str = "maintain";

/// The `active` member of [`FLEET_STATE_CODE_VALUES`].
pub const FLEET_STATE_ACTIVE: &str = "active";

/// The `submitted` member of [`FLEET_STATE_CODE_VALUES`]: where an asynchronous fleet — one
/// of type `maintain` or `request`, which "places an asynchronous request for your desired
/// capacity" (`FleetData$Type`) — starts, before the clock owes it [`FLEET_STATE_ACTIVE`].
///
/// A fleet stored in this state is the one thing that makes [`FleetRecord::activate_at`]
/// fall due, so the two cannot disagree about which fleets still owe the move.
pub const FLEET_STATE_SUBMITTED: &str = "submitted";

/// The `fulfilled` member of [`FLEET_ACTIVITY_STATUS_VALUES`]: "`fulfilled` when the fleet
/// size meets or exceeds target capacity" (`FleetData$ActivityStatus`).
pub const FLEET_ACTIVITY_STATUS_FULFILLED: &str = "fulfilled";

/// The `pending_fulfillment` member of [`FLEET_ACTIVITY_STATUS_VALUES`]: the fleet is still
/// short of its target capacity, which is what a fleet that placed less capacity than it was
/// asked for reports.
pub const FLEET_ACTIVITY_STATUS_PENDING_FULFILLMENT: &str = "pending_fulfillment";

/// `ResourceType`'s `fleet` member — the resource an EC2 Fleet's own `TagSpecifications`
/// tag: "specify a resource type of `fleet` to tag the fleet"
/// (`CreateFleetRequest$TagSpecifications`).
pub const RESOURCE_TYPE_FLEET: &str = "fleet";

/// `ResourceType`'s `instance` member — the resource an EC2 Fleet's `instance` tag
/// specifications tag: "`instance` to tag the instances at launch"
/// (`CreateFleetRequest$TagSpecifications`).
pub const RESOURCE_TYPE_INSTANCE: &str = "instance";

/// The resource types an `instant` EC2 Fleet's `TagSpecifications` may name, from the
/// member's own documentation: "If the fleet type is `instant`, specify a resource type of
/// `fleet` to tag the fleet, `instance` to tag the instances at launch, `volume` to tag the
/// volumes at launch, or `network-interface` to tag the network interfaces at launch."
///
/// A fleet of any other type may name only [`RESOURCE_TYPE_FLEET`]: "if the fleet type is
/// `maintain` or `request` … you cannot specify a resource type of `instance`, `volume`, or
/// `network-interface`". Only the first two have a resource this slice stores: a launched
/// instance's root volume and primary network interface are derived from the instance
/// itself, so tags for them are carried by no resource here.
pub const INSTANT_FLEET_TAGGABLE_RESOURCES: &[&str] = &[
    RESOURCE_TYPE_FLEET,
    RESOURCE_TYPE_INSTANCE,
    "volume",
    RESOURCE_TYPE_NETWORK_INTERFACE,
];

/// The `healthy` member of the modelled `InstanceHealthStatus` catalog
/// ([`crate::ops::types::INSTANCE_HEALTH_STATUS_VALUES`]).
///
/// "If the status of either the instance status check or the system status check is
/// `impaired`, the health status of the instance is `unhealthy`. Otherwise, the health
/// status is `healthy`" (`ActiveInstance$InstanceHealth`). No operation of this emulator
/// impairs either status check — nothing stores a status check at all — so an instance it
/// reports is healthy; this is the ONE place that derivation lives, for status checks to be
/// read from once something models them.
pub const INSTANCE_HEALTH_HEALTHY: &str = "healthy";

/// One instance an EC2 Fleet has launched.
///
/// The instance ITSELF is a record of [`ComAmazonawsEc2Amazonec2State::instances`], named
/// here by id: a fleet keeps only what is true of the instance *as the fleet's* — the Spot
/// request it was placed through — so the type and the state a fleet reports for an
/// instance are always the ones the instance store holds.
#[derive(Debug, Clone)]
pub struct FleetInstanceRecord {
    /// The `InstanceId`, a key of [`ComAmazonawsEc2Amazonec2State::instances`].
    pub instance_id: String,
    /// "The ID of the Spot Instance request" (`ActiveInstance$SpotInstanceRequestId`) the
    /// instance was placed through, or `None` for an On-Demand instance, which is placed
    /// through no Spot request.
    pub spot_instance_request_id: Option<String>,
    /// Which pool of the fleet ([`FleetRecord::pools`]) placed this instance — an index into
    /// that list, so the launch template and overrides the instance was launched from are
    /// held once per pool rather than copied onto every instance of it. An index the fleet
    /// has no pool for reports no launch template, which is what a fleet stored with no
    /// pools at all has.
    pub pool: usize,
}

/// One stored EC2 Fleet: the modelled [`FleetData`](crate::ops::types::FleetData) every
/// operation that reports a fleet answers with, plus the instances it has launched.
#[derive(Debug, Clone)]
pub struct FleetRecord {
    /// The fleet as `DescribeFleets` reports it.
    pub fleet: crate::ops::types::FleetData,
    /// The instances the fleet has launched, in launch order.
    pub instances: Vec<FleetInstanceRecord>,
    /// The pools the fleet places its capacity in, in the order the request described them:
    /// one per launch template and override combination, named by
    /// [`FleetInstanceRecord::pool`].
    pub pools: Vec<FleetPoolView>,
    /// When the fleet's [`FLEET_STATE_SUBMITTED`] -> [`FLEET_STATE_ACTIVE`] move falls due,
    /// or `None` for a fleet that owes no move — one stored `active` already, and one whose
    /// move has been applied. The due instant is stored; nothing sleeps for it, and
    /// [`ComAmazonawsEc2Amazonec2State::materialize_due_fleets`] is the one place it is
    /// applied.
    pub activate_at: Option<Instant>,
}

impl FleetRecord {
    /// This fleet's `FleetId`.
    pub fn fleet_id(&self) -> &str {
        self.fleet.fleet_id.as_deref().unwrap_or_default()
    }

    /// This fleet's `FleetType` — one of [`FLEET_TYPE_VALUES`].
    pub fn fleet_type(&self) -> &str {
        self.fleet.r#type.as_deref().unwrap_or_default()
    }
}

/// What one stored EC2 Fleet is created with, for
/// [`ComAmazonawsEc2Amazonec2State::insert_fleet`].
///
/// One field per `FleetData` member a writer decides, with the instants as [`SystemTime`]s
/// rather than modelled timestamps because every writer computes them from the clock it
/// already holds; the store converts them once. The `FleetId` is NOT here: it is minted by
/// the store ([`ComAmazonawsEc2Amazonec2State::mint_fleet_id`]), never taken from a request.
#[derive(Debug, Clone)]
pub struct FleetSpec {
    /// `FleetData$Type` — one of [`FLEET_TYPE_VALUES`].
    pub fleet_type: String,
    /// `FleetData$FleetState` — one of [`FLEET_STATE_CODE_VALUES`].
    pub fleet_state: String,
    /// `FleetData$ActivityStatus` — one of [`FLEET_ACTIVITY_STATUS_VALUES`].
    pub activity_status: Option<String>,
    /// `FleetData$ClientToken`, the idempotency token the create carried.
    pub client_token: Option<String>,
    /// `FleetData$CreateTime`.
    pub create_time: SystemTime,
    /// `FleetData$LaunchTemplateConfigs`, "the launch template and overrides".
    pub launch_template_configs: Vec<crate::ops::types::FleetLaunchTemplateConfig>,
    /// `FleetData$TargetCapacitySpecification`, "the number of units to request".
    pub target_capacity_specification: Option<crate::ops::types::TargetCapacitySpecification>,
    /// `FleetData$FulfilledCapacity`.
    pub fulfilled_capacity: Option<f64>,
    /// `FleetData$FulfilledOnDemandCapacity`.
    pub fulfilled_on_demand_capacity: Option<f64>,
    /// `FleetData$TerminateInstancesWithExpiration`.
    pub terminate_instances_with_expiration: Option<bool>,
    /// `FleetData$ReplaceUnhealthyInstances`.
    pub replace_unhealthy_instances: Option<bool>,
    /// `FleetData$ExcessCapacityTerminationPolicy`.
    pub excess_capacity_termination_policy: Option<String>,
    /// `FleetData$ValidFrom`, when the request starts being fulfilled.
    pub valid_from: Option<SystemTime>,
    /// `FleetData$ValidUntil`, after which no new instance requests are placed.
    pub valid_until: Option<SystemTime>,
    /// `FleetData$Tags`, "the tags for an EC2 Fleet resource".
    pub tags: Vec<Tag>,
    /// `FleetData$Context` — "Reserved".
    pub context: Option<String>,
    /// `FleetData$SpotOptions`, "the configuration of Spot Instances in an EC2 Fleet", as the
    /// writer resolved it: every word out of its own modelled catalog.
    pub spot_options: Option<crate::ops::types::SpotOptions>,
    /// `FleetData$OnDemandOptions`, "the configuration of On-Demand Instances in an EC2
    /// Fleet", resolved the same way.
    pub on_demand_options: Option<crate::ops::types::OnDemandOptions>,
    /// The pools the fleet places its capacity in ([`FleetRecord::pools`]).
    pub pools: Vec<FleetPoolView>,
    /// The instances the fleet has launched, in launch order.
    pub instances: Vec<FleetInstanceRecord>,
}

/// The caller-chosen bytes one stored fleet holds: its client token, its context, its tags
/// and the launch template names, versions and overrides it keeps, the prices its option
/// structures carry, plus the id of each instance and Spot request it names and the launch
/// template its pools name.
///
/// The ONE definition of that charge, the way [`capacity_block_payload_bytes`] is for a
/// Capacity Block: a writer charges exactly it, on top of the fixed
/// [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs.
pub fn fleet_payload_bytes(
    fleet: &crate::ops::types::FleetData,
    instances: &[FleetInstanceRecord],
    pools: &[FleetPoolView],
) -> usize {
    let text = |value: Option<&String>| value.map_or(0, String::len);
    let template_bytes: usize = fleet
        .launch_template_configs
        .iter()
        .flatten()
        .map(|config| {
            let specification = config
                .launch_template_specification
                .as_ref()
                .map_or(0, |specification| {
                    text(specification.launch_template_id.as_ref())
                        .saturating_add(text(specification.launch_template_name.as_ref()))
                        .saturating_add(text(specification.version.as_ref()))
                });
            let overrides: usize = config
                .overrides
                .iter()
                .flatten()
                .map(|overridden| {
                    text(overridden.instance_type.as_ref())
                        .saturating_add(text(overridden.availability_zone.as_ref()))
                        .saturating_add(text(overridden.subnet_id.as_ref()))
                        .saturating_add(text(overridden.max_price.as_ref()))
                })
                .fold(0usize, usize::saturating_add);
            specification.saturating_add(overrides)
        })
        .fold(0usize, usize::saturating_add);
    let instance_bytes: usize = instances
        .iter()
        .map(|instance| {
            instance.instance_id.len().saturating_add(
                instance
                    .spot_instance_request_id
                    .as_ref()
                    .map_or(0, String::len),
            )
        })
        .fold(0usize, usize::saturating_add);
    text(fleet.client_token.as_ref())
        .saturating_add(text(fleet.context.as_ref()))
        .saturating_add(tag_list_bytes(fleet.tags.as_deref().unwrap_or_default()))
        .saturating_add(template_bytes)
        .saturating_add(instance_bytes)
        .saturating_add(
            fleet
                .spot_options
                .as_ref()
                .map_or(0, |options| text(options.max_total_price.as_ref())),
        )
        .saturating_add(
            fleet
                .on_demand_options
                .as_ref()
                .map_or(0, |options| text(options.max_total_price.as_ref())),
        )
        .saturating_add(
            pools
                .iter()
                .map(FleetPoolView::payload_bytes)
                .fold(0usize, usize::saturating_add),
        )
}

impl ComAmazonawsEc2Amazonec2State {
    /// A fresh, unused id of the form `<prefix><uuid>`
    /// ([`uuid_suffixed_id_is_well_formed`]).
    ///
    /// The store's own monotonic counter is rendered into the UUID's first and last groups,
    /// so the id is unique without being drawn from a request and is never the null UUID.
    /// The three middle groups are deliberately reserved zeros: nothing distinguishes two of
    /// these resources but the counter, and a reader comparing ids never needs them.
    ///
    /// The one mint of this id form, so an EC2 Fleet's `FleetId` and a Spot Fleet request's
    /// `SpotFleetRequestId` are issued the same way and draw on the same counter.
    pub fn mint_uuid_suffixed_id(&mut self, prefix: &str) -> String {
        self.id_counter = self.id_counter.saturating_add(1);
        let counter = self.id_counter;
        format!(
            "{prefix}{:08x}-0000-0000-0000-{:012x}",
            counter >> 48,
            counter & 0xffff_ffff_ffff
        )
    }

    /// A fresh, unused `FleetId`: the form [`fleet_id_is_well_formed`] reads — `fleet-` and a
    /// UUID — minted by [`Self::mint_uuid_suffixed_id`].
    pub fn mint_fleet_id(&mut self) -> String {
        self.mint_uuid_suffixed_id(FLEET_ID_PREFIX)
    }

    /// Whether another EC2 Fleet fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`]: a writer asks this BEFORE storing
    /// anything and refuses the request with EC2's own quota code when it answers `false`.
    pub fn has_fleet_capacity(&self) -> bool {
        self.fleets.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store one EC2 Fleet and answer the `FleetId` it was given — or `None` when the store
    /// is at [`MAX_STORED_RECORDS_PER_STORE`], or the fleet's caller-chosen bytes would
    /// cross [`MAX_STORED_BYTES`], and NOTHING was stored.
    ///
    /// The one place a fleet enters this store, so every writer of one — and every operation
    /// that reports what this wrote — agrees on the id form, the stored shape and which
    /// members a fleet carries. Both ceilings are asked BEFORE anything is written, the way
    /// [`Self::insert_capacity_block`]'s caller does, so a refused write charges nothing and
    /// no path can store caller-chosen bytes uncharged.
    pub fn insert_fleet(&mut self, spec: FleetSpec) -> Option<String> {
        if !self.has_fleet_capacity() {
            return None;
        }
        let fleet_id = self.mint_fleet_id();
        let fleet = crate::ops::types::FleetData {
            activity_status: spec.activity_status,
            create_time: Some(system_time_to_date_time(spec.create_time)),
            fleet_id: Some(fleet_id.clone()),
            fleet_state: Some(spec.fleet_state),
            client_token: spec.client_token,
            excess_capacity_termination_policy: spec.excess_capacity_termination_policy,
            fulfilled_capacity: spec.fulfilled_capacity,
            fulfilled_on_demand_capacity: spec.fulfilled_on_demand_capacity,
            launch_template_configs: Some(spec.launch_template_configs),
            target_capacity_specification: spec.target_capacity_specification,
            terminate_instances_with_expiration: spec.terminate_instances_with_expiration,
            r#type: Some(spec.fleet_type),
            valid_from: spec.valid_from.map(system_time_to_date_time),
            valid_until: spec.valid_until.map(system_time_to_date_time),
            replace_unhealthy_instances: spec.replace_unhealthy_instances,
            // The option structures as the WRITER resolved them: every word comes out of its
            // own modelled catalog, so a fleet reports the configuration EC2 read rather
            // than the request's own text. The launch errors and the per-fleet instance
            // report are not stored members: both are derived from the instances the fleet
            // actually placed ([`fleet_instance_groups`]), so the create and every later
            // report of one fleet read the same instances. Reserved capacity is not a
            // capacity source this store places, so no fleet of it carries those options.
            spot_options: spec.spot_options,
            on_demand_options: spec.on_demand_options,
            reserved_capacity_options: None,
            tags: Some(spec.tags),
            errors: None,
            instances: None,
            context: spec.context,
        };
        let charge = STORED_RECORD_OVERHEAD_BYTES
            + fleet_payload_bytes(&fleet, &spec.instances, &spec.pools);
        // The byte ceiling is asked before the record is stored, not after: a fleet carries
        // caller-chosen text (its tags, its client token, its launch-template overrides), so
        // a write that would cross [`MAX_STORED_BYTES`] stores nothing at all and its caller
        // refuses the request.
        if self.would_exceed_stored_bytes(charge) {
            return None;
        }
        self.add_stored_bytes(charge);
        self.fleets.insert(
            fleet_id.clone(),
            FleetRecord {
                // A fleet stored as `submitted` owes the move to `active`, on the span every
                // clock-owed move of this crate takes; one stored in any other state owes
                // nothing. Derived HERE, from the state the fleet is stored in, so no writer
                // can store a `submitted` fleet that never becomes active.
                activate_at: (fleet.fleet_state.as_deref() == Some(FLEET_STATE_SUBMITTED))
                    .then(|| due_instant(TRANSITION_DELAY)),
                fleet,
                instances: spec.instances,
                pools: spec.pools,
            },
        );
        Some(fleet_id)
    }

    /// Apply the [`FLEET_STATE_SUBMITTED`] -> [`FLEET_STATE_ACTIVE`] move every EC2 Fleet
    /// that owed one by `now` has made.
    ///
    /// Reached only through [`Self::materialize_due_transitions`], so a reporting operation
    /// observes the move without having caused it: it was owed at this instant whether or
    /// not anybody called. "`maintain` - The EC2 Fleet places an asynchronous request for
    /// your desired capacity" (`FleetData$Type`), so a fleet of that kind is `submitted`
    /// while EC2 takes the request on and `active` once it has; the capacity it placed is a
    /// fact of the CALL that created it, which is why only the state word moves here.
    pub fn materialize_due_fleets(&mut self, now: Instant) {
        for record in self.fleets.values_mut() {
            if record.activate_at.is_none_or(|due_at| now < due_at) {
                continue;
            }
            // The move is applied once; nothing is owed on this fleet any more.
            record.activate_at = None;
            if record.fleet.fleet_state.as_deref() == Some(FLEET_STATE_SUBMITTED) {
                record.fleet.fleet_state = Some(FLEET_STATE_ACTIVE.to_string());
            }
        }
    }
}

/// The instances `launched` names that the account still HOLDS, as the modelled
/// [`ActiveInstance`](crate::ops::types::ActiveInstance) shape.
///
/// `launched` names each instance a fleet placed, with the Spot Instance request it was placed
/// through when it was placed through one. The ONE projection of an active instance, so an EC2
/// Fleet ([`fleet_active_instances`]) and a Spot Fleet request
/// ([`spot_fleet_active_instances`]) cannot report one instance two ways: an instance the
/// INSTANCE store holds is one of its fleet's from the moment the placement launched it until
/// it reaches `terminated`, reported with the type that store holds for it; an instance that
/// has reached `terminated`, and one the store no longer holds at all, belongs to no fleet.
///
/// That is the same set whose capacity the fleet counts as FULFILLED
/// ([`spot_fleet_fulfilled_capacity`]) and the same set a cancelled fleet waits for
/// ([`spot_fleet_has_running_instances`]), so a fleet cannot report capacity fulfilled by
/// instances it does not report, or wait for instances it says it has none of: an instance's
/// position in the `pending` -> `running` -> `stopped` lifecycle is reported by
/// `DescribeInstances`, and is not what puts it in or out of its fleet. `InstanceHealth` is
/// what this shape carries about an instance's condition — "if the status of either the
/// instance status check or the system status check is `impaired`, the health status of the
/// instance is `unhealthy`. Otherwise, the health status is `healthy`"
/// (`ActiveInstance$InstanceHealth`).
pub fn active_instances<'a>(
    store: &ComAmazonawsEc2Amazonec2State,
    launched: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> Vec<crate::ops::types::ActiveInstance> {
    launched
        .into_iter()
        .filter_map(|(instance_id, spot_instance_request_id)| {
            let instance = store.instances.get(instance_id)?;
            (instance.state != InstanceStateName::Terminated).then(|| {
                crate::ops::types::ActiveInstance {
                    instance_id: Some(instance.instance_id.clone()),
                    instance_type: Some(instance.instance_type.clone()),
                    spot_instance_request_id: spot_instance_request_id.map(str::to_string),
                    instance_health: Some(INSTANCE_HEALTH_HEALTHY.to_string()),
                }
            })
        })
        .collect()
}

/// The instances of `record`, as the modelled
/// [`ActiveInstance`](crate::ops::types::ActiveInstance) shape.
///
/// "Describes the running instances for the specified EC2 Fleet" (`DescribeFleetInstances`):
/// the instances the fleet is running for the account, which is every instance it placed that
/// the instance store still holds un-terminated ([`active_instances`]) — an instance it has
/// terminated, and one the store no longer holds at all, is not one of them.
///
/// Projected from the INSTANCE store rather than from anything the fleet record copies, so
/// the instance type a fleet reports for an instance is the one `DescribeInstances` reports
/// for it. The caller sees them in launch order; a report that pages puts them in id order
/// itself.
pub fn fleet_active_instances(
    store: &ComAmazonawsEc2Amazonec2State,
    record: &FleetRecord,
) -> Vec<crate::ops::types::ActiveInstance> {
    active_instances(
        store,
        record.instances.iter().map(|launched| {
            (
                launched.instance_id.as_str(),
                launched.spot_instance_request_id.as_deref(),
            )
        }),
    )
}

/// Refuse a request that named something that is not an EC2 Fleet id: HTTP 400
/// `InvalidFleetId.Malformed`, with the message naming the value the request sent.
pub fn invalid_fleet_id_malformed<E>(fleet_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_FLEET_ID_MALFORMED,
        400,
        format!("The fleet ID '{fleet_id}' is malformed"),
    )
}

/// Refuse a request that named a fleet id no EC2 Fleet of the account has: HTTP 400
/// `InvalidFleetId.NotFound`.
///
/// EC2 words this refusal as a BATCH — `The FleetId(s) <ids> do not exist.` — however many
/// ids the request named, the way it words the same refusal for a Spot Fleet request
/// (`The SpotFleetRequestId(s) <ids> do not exist.`). The ids are the ones the request sent.
pub fn invalid_fleet_id_not_found<E>(fleet_id: &str) -> Scaffolded<E> {
    fleet_ids_not_found(&[fleet_id])
}

/// The refusal above for the ids of ONE request, which is where the batch wording comes
/// from: an operation that resolves several fleet ids reports every one it could not find in
/// a single message.
pub fn fleet_ids_not_found<E>(fleet_ids: &[&str]) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_FLEET_ID_NOT_FOUND,
        400,
        format!("The FleetId(s) {} do not exist.", fleet_ids.join(", ")),
    )
}

/// Refuse `fleet_id` unless it is a well-formed EC2 Fleet id, BEFORE any lookup: the
/// question "is this a fleet id at all" is asked ahead of "does this account hold it", so
/// "not an id of this kind" is never reported as "no such fleet".
///
/// A handler calls this to refuse a malformed id WITHOUT taking the state lock;
/// [`require_known_fleet`] asks it again, so the ordering holds even for a caller that
/// goes straight to the lookup.
pub fn require_well_formed_fleet_id<E>(fleet_id: &str) -> Result<(), Scaffolded<E>> {
    if fleet_id_is_well_formed(fleet_id) {
        return Ok(());
    }
    Err(invalid_fleet_id_malformed(fleet_id))
}

/// The EC2 Fleet `fleet_id` names, refusing a malformed id first and an id this account
/// holds no fleet for second — the order [`require_well_formed_id`] establishes for every
/// resource of this module.
pub fn require_known_fleet<'a, E>(
    store: &'a ComAmazonawsEc2Amazonec2State,
    fleet_id: &str,
) -> Result<&'a FleetRecord, Scaffolded<E>> {
    require_well_formed_fleet_id(fleet_id)?;
    store
        .fleets
        .get(fleet_id)
        .ok_or_else(|| invalid_fleet_id_not_found(fleet_id))
}

/// Refuse a request this service does not serve for the resource it named: HTTP 400
/// `UnsupportedOperation`.
pub fn unsupported_operation<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(UNSUPPORTED_OPERATION, 400, message)
}

/// The code EC2 answers for a launch template override that says which instance type its pool
/// launches TWICE — once as an instance type and once as a set of instance requirements.
///
/// The `Exception` suffix is NOT this crate's usual ec2Query spelling
/// ([`INVALID_PARAMETER_COMBINATION`] is), and it is not a second spelling of that code by
/// accident: the recorded response to an override carrying both members answers
/// `InvalidParameterCombinationException` for this one refusal, so this is the code it is
/// answered with and nothing else uses it.
pub const INVALID_PARAMETER_COMBINATION_EXCEPTION: &str = "InvalidParameterCombinationException";

/// EC2's sentence for that override, which carries no variable part: it names the two members
/// and what to do about them, whichever fleet API the override was sent to.
pub const INSTANCE_REQUIREMENTS_WITH_INSTANCE_TYPE_MESSAGE: &str =
    "You can specify either InstanceRequirements or InstanceType, but not both. Remove either \
     InstanceRequirements or Instance Type, and try again.";

/// Refuse a launch template override that carries both an instance type and a set of instance
/// requirements: HTTP 400 [`INVALID_PARAMETER_COMBINATION_EXCEPTION`].
///
/// "If you specify `InstanceRequirements`, you can't specify `InstanceType`"
/// (`LaunchTemplateOverrides$InstanceRequirements`, and the same note on
/// `FleetLaunchTemplateOverridesRequest$InstanceRequirements`): the two members are two ways of
/// saying which instance type a pool launches, so an override carrying both describes its pool
/// twice. The ONE place that rule lives, so every fleet API that takes launch template
/// overrides refuses it with one code and one sentence.
pub fn instance_requirements_with_instance_type<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_PARAMETER_COMBINATION_EXCEPTION,
        400,
        INSTANCE_REQUIREMENTS_WITH_INSTANCE_TYPE_MESSAGE,
    )
}

// ---------------------------------------------------------------------------
// Allowed AMIs: the account's image-criteria setting in one Region
// ---------------------------------------------------------------------------
//
// See the module docs for why this is a SETTING keyed by Region rather than a resource,
// and why one record holds both the state and the criteria. The words below are members
// of the model's own enums, named here so the four operations that read or write the
// setting cannot spell one value two ways.

/// The Allowed AMIs state of an account that has not enabled the feature, and the one
/// `DisableAllowedImagesSettings` moves it to: the `AllowedImagesSettingsDisabledState`
/// enum's only member, `disabled` — "All AMIs are allowed"
/// (`GetAllowedImagesSettingsResult$State`). The generated catalog is
/// `crate::ops::types::ALLOWED_IMAGES_SETTINGS_DISABLED_STATE_VALUES`.
pub const ALLOWED_IMAGES_SETTINGS_DISABLED: &str = "disabled";

/// `AllowedImagesSettingsEnabledState`'s `enabled` member — "Only AMIs matching the image
/// criteria are discoverable and available for use". The generated catalog of the two
/// states an enable accepts is
/// `crate::ops::types::ALLOWED_IMAGES_SETTINGS_ENABLED_STATE_VALUES`.
pub const ALLOWED_IMAGES_SETTINGS_ENABLED: &str = "enabled";

/// `AllowedImagesSettingsEnabledState`'s `audit-mode` member — "All AMIs are allowed, but
/// the ImageAllowed field is set to true if the AMI would be allowed with the current list
/// of criteria if allowed AMIs was enabled" (see [`ALLOWED_IMAGES_SETTINGS_ENABLED`] for
/// the catalog).
pub const ALLOWED_IMAGES_SETTINGS_AUDIT_MODE: &str = "audit-mode";

// The entity that manages the setting is `ManagedBy`'s `account` member for Allowed AMIs
// exactly as it is for snapshot block public access — "the Allowed AMIs settings is managed
// by the account" (`GetAllowedImagesSettingsResult$ManagedBy`), the account being the only
// writer of either setting this emulator has — so this crate spells it once, as
// [`MANAGED_BY_ACCOUNT`] above, and `GetAllowedImagesSettings` reports that constant.

/// The most `ImageCriterion` entries one Allowed AMIs setting may carry.
///
/// `ImageCriterionRequest`'s own doc: "The `ImageCriteria` can include up to: 10
/// `ImageCriterion`". The model declares no `@length` on `ImageCriterionRequestList`, so
/// the handler is the only place the bound can be enforced.
pub const MAX_IMAGE_CRITERIA: usize = 10;

/// The most `ImageProviders` values one `ImageCriterion` may carry: "Maximum: 200 values"
/// (`ImageCriterionRequest$ImageProviders`).
pub const MAX_IMAGE_PROVIDERS_PER_CRITERION: usize = 200;

/// The most `ImageNames` values one `ImageCriterion` may carry: "Maximum: 50 values"
/// (`ImageCriterionRequest$ImageNames`).
pub const MAX_IMAGE_NAMES_PER_CRITERION: usize = 50;

/// The most `MarketplaceProductCodes` values one `ImageCriterion` may carry: "Maximum: 50
/// values" (`ImageCriterionRequest$MarketplaceProductCodes`).
pub const MAX_MARKETPLACE_PRODUCT_CODES_PER_CRITERION: usize = 50;

/// The most `ImageWatermarks` entries one `ImageCriterion` may carry: "50 values for
/// `ImageWatermarks`" (`ImageCriterionRequest`'s own doc).
pub const MAX_IMAGE_WATERMARKS_PER_CRITERION: usize = 50;

/// The image-provider words `ImageProviders` accepts beside an account id and
/// [`IMAGE_PROVIDER_NONE`]: "`amazon`: Allow AMIs created by Amazon or verified providers.
/// `aws-marketplace`: Allow AMIs created by verified providers in the Amazon Web Services
/// Marketplace. `aws-backup-vault`: Allow AMIs created by Amazon Web Services Backup"
/// (`ImageCriterionRequest$ImageProviders`).
///
/// The same three aliases `DescribeImages`'s `Owners` accepts ([`IMAGE_OWNER_ALIASES`]),
/// which is why that catalog is the one read here rather than a second copy.
pub const IMAGE_PROVIDER_ALIASES: &[&str] = &IMAGE_OWNER_ALIASES;

/// The `ImageProviders` word that stands for the account itself: "`none`: Allow AMIs
/// created by your own account only. When `none` is specified, no other values can be
/// specified" (`ImageCriterionRequest$ImageProviders`).
pub const IMAGE_PROVIDER_NONE: &str = "none";

/// The fewest characters an `ImageNames` value may carry: "Length: 1–128 characters"
/// (`ImageCriterionRequest$ImageNames`).
pub const MIN_IMAGE_NAME_CRITERION_CHARS: usize = 1;

/// The most characters an `ImageNames` value may carry: the ceiling of that same
/// documented "Length: 1–128 characters".
///
/// Enforced by this crate rather than relied on from the model: the `@length` the model
/// carries sits on `ImageNameCriteriaRequestList`'s member, and a handler must not depend on
/// where the request walk happens to apply it — without a per-STRING ceiling every stored
/// name would be caller-sized.
pub const MAX_IMAGE_NAME_CRITERION_CHARS: usize = 128;

/// The fewest characters an `ImageNames` value carrying a `?` wildcard may have: "With
/// `?`, the minimum is 3 characters" (`ImageCriterionRequest$ImageNames`).
pub const MIN_IMAGE_NAME_CRITERION_CHARS_WITH_WILDCARD: usize = 3;

/// The special characters an `ImageNames` value may carry beside letters, numbers and
/// spaces: "Special characters: `( ) [ ] . / - ' @ _ * ?`"
/// (`ImageCriterionRequest$ImageNames`).
pub const IMAGE_NAME_CRITERION_SPECIAL_CHARACTERS: &str = "()[]./-'@_*?";

/// The fewest characters a `MarketplaceProductCodes` value may carry: "Length: 1-25
/// characters" (`ImageCriterionRequest$MarketplaceProductCodes`).
pub const MIN_MARKETPLACE_PRODUCT_CODE_CHARS: usize = 1;

/// The most characters a `MarketplaceProductCodes` value may carry (see
/// [`MIN_MARKETPLACE_PRODUCT_CODE_CHARS`]); the model declares no `@length` for it.
pub const MAX_MARKETPLACE_PRODUCT_CODE_CHARS: usize = 25;

/// The most characters one `ImageWatermarkFilter` pattern — a `WatermarkKey` or a
/// `SourceImageRegion` — may carry.
///
/// CHOSEN, not an AWS-documented length: the model bounds neither, and both are
/// caller-chosen strings this store keeps. A watermark key is an `accountId:name` and a
/// source image region is a Region name, with `*` and `?` wildcards allowed in either, so
/// this is far above anything either form needs while keeping one stored setting's size
/// finite.
pub const MAX_IMAGE_WATERMARK_PATTERN_CHARS: usize = 128;

/// The most Regions this emulator holds an Allowed AMIs setting for.
///
/// [`MAX_STORED_RECORDS_PER_STORE`] is the crate's ceiling for a store nothing reclaims, and
/// this store is bounded by it like every other: a setting is keyed by the Region of the
/// request's own SigV4 credential scope, so without a ceiling a caller could write a setting
/// for unboundedly many Region names. The bytes each setting holds are charged against
/// [`MAX_STORED_BYTES`] on top of that ([`image_criteria_payload_bytes`]), so neither the
/// number of settings nor their size is unbounded.
pub const MAX_ALLOWED_IMAGES_SETTINGS_REGIONS: usize = MAX_STORED_RECORDS_PER_STORE;

/// The account's Allowed AMIs setting in ONE Region: the state the feature is in, and the
/// criteria that are evaluated when it is applied.
///
/// Held in [`ComAmazonawsEc2Amazonec2State::allowed_images_settings`] under the Region's
/// name. The criteria are the modelled [`ImageCriterion`] shape `GetAllowedImagesSettings`
/// reports, so a replace converts and validates the request shapes once rather than at
/// every read.
#[derive(Debug, Clone)]
pub struct AllowedImagesSettings {
    /// "The current state of the Allowed AMIs setting at the account level in the specified
    /// Amazon Web Services Region" (`GetAllowedImagesSettingsResult$State`): one of
    /// [`ALLOWED_IMAGES_SETTINGS_DISABLED`], [`ALLOWED_IMAGES_SETTINGS_ENABLED`] or
    /// [`ALLOWED_IMAGES_SETTINGS_AUDIT_MODE`].
    pub state: String,
    /// "The list of criteria for images that are discoverable and usable in the account in
    /// the specified Amazon Web Services Region"
    /// (`GetAllowedImagesSettingsResult$ImageCriteria`), at most
    /// [`MAX_IMAGE_CRITERIA`] of them.
    pub image_criteria: Vec<ImageCriterion>,
}

/// The setting of a Region nobody has configured: `disabled` — "All AMIs are allowed" — with
/// no criteria, which is what an account that has never enabled Allowed AMIs has.
impl Default for AllowedImagesSettings {
    fn default() -> Self {
        Self {
            state: ALLOWED_IMAGES_SETTINGS_DISABLED.to_string(),
            image_criteria: Vec::new(),
        }
    }
}

/// Refuse a request that named a NAT gateway id the account holds no NAT gateway for:
/// HTTP 400 [`NAT_GATEWAY_NOT_FOUND`].
pub fn nat_gateway_not_found<E>(nat_gateway_id: &str) -> Scaffolded<E> {
    id_not_found(NAT_GATEWAY_NOT_FOUND, "natgateway ID", nat_gateway_id)
}

/// The NAT gateway `nat_gateway_id` names, refusing a malformed id before an unknown one —
/// the order the service asks the two questions in, so "not a NAT gateway id" is never
/// reported as "no such NAT gateway".
///
/// The ONE place every operation of the NAT gateway family resolves its gateway, so they
/// cannot answer an id two ways.
pub fn require_known_nat_gateway<'store, E>(
    store: &'store ComAmazonawsEc2Amazonec2State,
    nat_gateway_id: &str,
) -> Result<&'store NatGateway, Scaffolded<E>> {
    require_well_formed_id(NAT_GATEWAY_ID_PREFIX, nat_gateway_id, NAT_GATEWAY_MALFORMED)?;
    store
        .nat_gateways
        .get(nat_gateway_id)
        .ok_or_else(|| nat_gateway_not_found(nat_gateway_id))
}

/// The caller-chosen bytes one stored NAT gateway holds: its tags. Everything else on the
/// record — the ids, the state, the addresses, the connectivity — is derived by this
/// module or bounded by the model, so it is fixed-size.
pub fn nat_gateway_payload_bytes(gateway: &NatGateway) -> usize {
    tag_list_bytes(gateway.tags.as_deref().unwrap_or_default())
}

impl ComAmazonawsEc2Amazonec2State {
    /// Store a NAT gateway under the `NatGatewayId` it already carries, charging the
    /// caller-chosen bytes it holds against the store's byte budget so no path can store
    /// them uncharged, and answer that id.
    ///
    /// The same contract as [`Self::insert_address`]: the record IS the modelled shape
    /// every operation that reports this gateway answers with, so a member a writer sets
    /// round-trips without a field-by-field copy.
    pub fn insert_nat_gateway(&mut self, gateway: NatGateway) -> String {
        let nat_gateway_id = gateway.nat_gateway_id.clone().unwrap_or_default();
        self.add_stored_bytes(nat_gateway_payload_bytes(&gateway));
        self.nat_gateways.insert(nat_gateway_id.clone(), gateway);
        nat_gateway_id
    }

    /// How many Elastic IP addresses the NAT gateway `nat_gateway_id` holds right now,
    /// counting only those of the Availability Zone `zone` when one is named: the addresses
    /// on it that name an allocation, which is what the quota
    /// [`MAX_EIPS_PER_PUBLIC_NAT_GATEWAY`] counts. A private address of the gateway names
    /// no allocation and is not one of them.
    ///
    /// A zone is named for a REGIONAL gateway, which "uses a separate EIP in each AZ to
    /// handle outbound NAT traffic from that AZ"
    /// (`AssociateNatGatewayAddressRequest$AvailabilityZone`): the published default is "up
    /// to 2 Elastic IP addresses per public NAT gateway", and the NAT that holds an
    /// address is the one of its zone, so a gateway serving several zones is not held to
    /// the addresses of one. A single-zone gateway has only that one zone, so its count is
    /// every address it holds.
    pub fn nat_gateway_eip_count(&self, nat_gateway_id: &str, zone: Option<&str>) -> usize {
        self.nat_gateways
            .get(nat_gateway_id)
            .and_then(|gateway| gateway.nat_gateway_addresses.as_ref())
            .map(|addresses| {
                addresses
                    .iter()
                    .filter(|address| {
                        address.allocation_id.is_some()
                            && zone.is_none_or(|zone| {
                                address.availability_zone.as_deref() == Some(zone)
                            })
                    })
                    .count()
            })
            .unwrap_or_default()
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// The Allowed AMIs setting of `region` as it stands.
    ///
    /// A READ: a Region with no stored setting answers [`AllowedImagesSettings::default`]
    /// without storing it, so `GetAllowedImagesSettings` cannot change what a later caller
    /// sees.
    pub fn allowed_images_settings_in(&self, region: &str) -> Cow<'_, AllowedImagesSettings> {
        match self.allowed_images_settings.get(region) {
            Some(settings) => Cow::Borrowed(settings),
            None => Cow::Owned(AllowedImagesSettings::default()),
        }
    }

    /// Move the state of `region`'s Allowed AMIs setting to `state`, leaving its criteria as
    /// they stand — what `EnableAllowedImagesSettings` and `DisableAllowedImagesSettings`
    /// do.
    ///
    /// `Err` means NOTHING was stored and the writer must refuse the request; see
    /// [`AllowedImagesSettingsFull`].
    pub fn set_allowed_images_settings_state(
        &mut self,
        region: &str,
        state: &str,
    ) -> Result<(), AllowedImagesSettingsFull> {
        self.allowed_images_settings_entry(region)?.state = state.to_string();
        Ok(())
    }

    /// Replace the image criteria of `region`'s Allowed AMIs setting, leaving its state as it
    /// stands — what `ReplaceImageCriteriaInAllowedImagesSettings` does.
    ///
    /// The bytes the new criteria hold are charged against [`MAX_STORED_BYTES`] and the ones
    /// the criteria they replace held are given back, so a store that is replaced over and
    /// over does not drift, and a replace that would cross the budget is refused with
    /// NOTHING stored ([`AllowedImagesSettingsFull::Bytes`]).
    pub fn set_allowed_images_criteria(
        &mut self,
        region: &str,
        criteria: Vec<ImageCriterion>,
    ) -> Result<(), AllowedImagesSettingsFull> {
        let requested = image_criteria_payload_bytes(&criteria);
        let released = self
            .allowed_images_settings
            .get(region)
            .map_or(0, |settings| {
                image_criteria_payload_bytes(&settings.image_criteria)
            });
        // Only the GROWTH has to fit: a replace gives the bytes of the criteria it replaces
        // back, so the charge is the difference.
        if self.would_exceed_stored_bytes(requested.saturating_sub(released)) {
            return Err(AllowedImagesSettingsFull::Bytes { requested });
        }
        self.allowed_images_settings_entry(region)?.image_criteria = criteria;
        self.release_stored_bytes(released);
        self.add_stored_bytes(requested);
        Ok(())
    }

    /// The Allowed AMIs setting of `region` to write to, started at
    /// [`AllowedImagesSettings::default`] when the Region has none yet and charged
    /// [`STORED_RECORD_OVERHEAD_BYTES`] for the record it adds, the way every other store's
    /// writer charges for one.
    ///
    /// `Err` once the store holds settings for [`MAX_ALLOWED_IMAGES_SETTINGS_REGIONS`] other
    /// Regions: a writer refuses rather than growing past the ceiling. Private, so the two
    /// public writers above are the only way a setting is written and neither can skip the
    /// ceiling or the charge.
    pub(super) fn allowed_images_settings_entry(
        &mut self,
        region: &str,
    ) -> Result<&mut AllowedImagesSettings, AllowedImagesSettingsFull> {
        if !self.allowed_images_settings.contains_key(region) {
            if self.allowed_images_settings.len() >= MAX_ALLOWED_IMAGES_SETTINGS_REGIONS {
                return Err(AllowedImagesSettingsFull::Regions);
            }
            self.add_stored_bytes(STORED_RECORD_OVERHEAD_BYTES);
        }
        Ok(self
            .allowed_images_settings
            .entry(region.to_string())
            .or_default())
    }
}

/// Why a write to an Allowed AMIs setting was refused, with NOTHING stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowedImagesSettingsFull {
    /// The store already holds the settings of
    /// [`MAX_ALLOWED_IMAGES_SETTINGS_REGIONS`] other Regions.
    Regions,
    /// The criteria would take the store past [`MAX_STORED_BYTES`]; `requested` is what they
    /// would have cost.
    Bytes {
        /// The bytes the refused criteria would have held.
        requested: usize,
    },
}

/// The caller-chosen bytes one Allowed AMIs criteria list holds: its image providers, image
/// names, Marketplace product codes and watermark patterns. The day conditions are two
/// integers bounded by the model's own range, so they are fixed-size.
///
/// The ONE definition of that charge, the way [`vpc_endpoint_connection_payload_bytes`] is
/// for a connection: a writer charges exactly it, and a replace gives back exactly what the
/// criteria it replaces cost.
pub fn image_criteria_payload_bytes(criteria: &[ImageCriterion]) -> usize {
    let text_bytes = |values: Option<&Vec<String>>| -> usize {
        values.into_iter().flatten().map(String::len).sum()
    };
    criteria
        .iter()
        .map(|criterion| {
            let watermark_bytes: usize = criterion
                .image_watermarks
                .iter()
                .flatten()
                .map(|watermark| {
                    watermark.watermark_key.as_deref().unwrap_or_default().len()
                        + watermark
                            .source_image_region
                            .as_deref()
                            .unwrap_or_default()
                            .len()
                })
                .sum();
            text_bytes(criterion.image_providers.as_ref())
                .saturating_add(text_bytes(criterion.image_names.as_ref()))
                .saturating_add(text_bytes(criterion.marketplace_product_codes.as_ref()))
                .saturating_add(watermark_bytes)
        })
        .fold(0usize, usize::saturating_add)
}

/// Refuse a write to an Allowed AMIs setting this emulator has no room for: HTTP 400, EC2's
/// `InvalidParameterValue`.
///
/// A bad request rather than a server error: nothing is wrong with the emulator, the Region
/// and the criteria are values of the request, and a retry of the same request would be
/// refused the same way.
pub fn allowed_images_settings_full<E>(region: &str, full: AllowedImagesSettingsFull) -> Scaffolded<E> {
    match full {
        AllowedImagesSettingsFull::Regions => invalid_parameter_value(format!(
            "This emulator holds the Allowed AMIs settings of \
             {MAX_ALLOWED_IMAGES_SETTINGS_REGIONS} Regions, which is as many as it stores, so \
             it cannot store the settings of '{region}' too"
        )),
        AllowedImagesSettingsFull::Bytes { requested } => invalid_parameter_value(format!(
            "The image criteria carry {requested} bytes, which this emulator has no room for: \
             it stores at most {MAX_STORED_BYTES} bytes of caller-supplied data"
        )),
    }
}

// ---------------------------------------------------------------------------
// The Spot Instance data feed: one subscription per account
// ---------------------------------------------------------------------------
//
// See the module docs for why the account's data feed is ONE stored record rather than a
// map, and why its absence is a refusal rather than a default.

/// "`Active`" — the `DatafeedSubscriptionState` member a data feed that is delivering sits
/// in (`smithy_query(query="shape",shape="DatafeedSubscriptionState")`), and the one a
/// create leaves it in: nothing has to be provisioned for a feed, so it is active as soon
/// as it exists. The whole catalog is the generated
/// [`crate::ops::types::DATAFEED_SUBSCRIPTION_STATE_VALUES`].
pub const DATAFEED_SUBSCRIPTION_STATE_ACTIVE: &str = "Active";

/// "`Inactive`" — the catalog's other member: the state AWS puts a feed into when delivery
/// itself has failed, which is the case `SpotDatafeedSubscription$Fault` reports ("the fault
/// codes for the Spot Instance request, if any").
///
/// No operation of this API moves a feed into it — a caller cannot ask for an inactive feed
/// and nothing inside this emulator writes to an S3 bucket to fail at — so this emulator
/// never stores it. It is named here so the word exists once, beside the member a create
/// does store.
pub const DATAFEED_SUBSCRIPTION_STATE_INACTIVE: &str = "Inactive";

/// The code EC2 answers a request about a Spot data feed the account does not have:
/// `InvalidSpotDatafeed.NotFound` — "you have no data feed for Spot Instances" (AWS's EC2
/// error-code reference entry).
pub const INVALID_SPOT_DATAFEED_NOT_FOUND: &str = "InvalidSpotDatafeed.NotFound";

/// Refuse a request about a Spot Instance data feed the account holds none of, the way EC2
/// does: HTTP 400 [`INVALID_SPOT_DATAFEED_NOT_FOUND`].
///
/// The message takes no argument because the condition takes none: the feed is a singleton
/// with no id, so there is nothing of the request to name in it, and the sentence is the
/// fixed one the real service answers for this condition.
pub fn invalid_spot_datafeed_not_found<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_SPOT_DATAFEED_NOT_FOUND,
        400,
        "Spot datafeed subscription does not exist.",
    )
}

/// The most BYTES a data feed `Prefix` may carry.
///
/// "The prefix for the data feed file names" (`CreateSpotDatafeedSubscriptionRequest$Prefix`)
/// is the leading part of the S3 object keys the feed writes, and "the name for a key is a
/// sequence of Unicode characters whose UTF-8 encoding is at most 1,024 bytes long" (Amazon
/// S3 User Guide, "Creating object key names"), so a prefix whose encoding is longer than a
/// whole key cannot name one. BYTES and not characters, because that is the unit the rule is
/// stated in: a 1,024-character prefix of multi-byte characters is up to four times a whole
/// key. The model puts no `@length` on the member, so this is the only place it can be
/// enforced — and it is also what bounds the one stored record.
pub const MAX_SPOT_DATAFEED_PREFIX_BYTES: usize = 1024;

/// The caller-chosen bytes the one stored data feed subscription holds, plus the fixed
/// overhead the record itself costs.
///
/// Only the bucket and the prefix are the caller's: the owner id and the state come from
/// this module. The ONE definition of that charge, so the charge a write takes and the one a
/// delete gives back cannot drift.
pub fn spot_datafeed_payload_bytes(bucket: &str, prefix: Option<&str>) -> usize {
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(bucket.len())
        .saturating_add(prefix.map_or(0, str::len))
}

impl ComAmazonawsEc2Amazonec2State {
    /// The account's Spot Instance data feed subscription right now, and `None` when it has
    /// none.
    ///
    /// A pure read: a data feed has no clock-owed move — [`DATAFEED_SUBSCRIPTION_STATE_ACTIVE`]
    /// is where a create leaves it and no operation of this API moves it anywhere — so
    /// nothing about it changes with time.
    pub fn spot_datafeed_subscription(&self) -> Option<SpotDatafeedSubscription> {
        self.spot_datafeed_subscription.clone()
    }

    /// Store the account's data feed over whatever it held before, and answer it as it was
    /// stored.
    ///
    /// `Err` means the store had no room for it and NOTHING was stored; it carries the
    /// GROWTH that would not fit, which is what [`spot_datafeed_store_full`] states. "You can
    /// create one data feed per Amazon Web Services account", so this REPLACES rather than
    /// adding: the bytes the record it replaces held are given back and the new ones charged,
    /// so an account that writes its feed over and over does not drift.
    pub fn set_spot_datafeed_subscription(
        &mut self,
        bucket: String,
        prefix: Option<String>,
    ) -> Result<SpotDatafeedSubscription, usize> {
        let requested = spot_datafeed_payload_bytes(&bucket, prefix.as_deref());
        let released = self.spot_datafeed_charge();
        // Only the GROWTH has to fit: the record this one replaces gives its bytes back.
        let growth = requested.saturating_sub(released);
        if self.would_exceed_stored_bytes(growth) {
            return Err(growth);
        }
        let subscription = SpotDatafeedSubscription {
            bucket: Some(bucket),
            prefix,
            // "The Amazon Web Services account ID of the account"
            // (`SpotDatafeedSubscription$OwnerId`) — this emulator's one account.
            owner_id: Some(::engine::emulator_account_id().to_string()),
            state: Some(DATAFEED_SUBSCRIPTION_STATE_ACTIVE.to_string()),
            // "The fault codes for the Spot Instance request, IF ANY"
            // (`SpotDatafeedSubscription$Fault`): a feed that was just stored has had nothing
            // go wrong with it, so there is no fault to report.
            fault: None,
        };
        self.release_stored_bytes(released);
        self.add_stored_bytes(requested);
        self.spot_datafeed_subscription = Some(subscription.clone());
        Ok(subscription)
    }

    /// Drop the account's data feed, giving its bytes back, and answer whether it had one.
    ///
    /// What `DeleteSpotDatafeedSubscription` calls. `false` means the account had no feed and
    /// nothing changed — which that delete answers the same success as a removal, because the
    /// feed is a singleton with no id and the postcondition already holds. It lives here so
    /// that the delete removes the record and releases the charge the create took in ONE
    /// place, rather than reaching into the field itself.
    pub fn remove_spot_datafeed_subscription(&mut self) -> bool {
        let released = self.spot_datafeed_charge();
        let had_one = self.spot_datafeed_subscription.take().is_some();
        self.release_stored_bytes(released);
        had_one
    }

    /// The bytes the data feed now stored is charged, or 0 when there is none.
    ///
    /// Private, so the charge is only ever computed from the record that is actually stored:
    /// every writer above gives back exactly this.
    pub(super) fn spot_datafeed_charge(&self) -> usize {
        self.spot_datafeed_subscription
            .as_ref()
            .map_or(0, |stored| {
                spot_datafeed_payload_bytes(
                    stored.bucket.as_deref().unwrap_or_default(),
                    stored.prefix.as_deref(),
                )
            })
    }
}

/// Refuse a data feed this emulator has no room for: HTTP 400, EC2's
/// `InvalidParameterValue`.
///
/// A bad request rather than a server error, for the reason
/// [`allowed_images_settings_full`] gives: the bucket and the prefix are values of the
/// request, and a retry of the same request would be refused the same way.
///
/// `requested` is what [`ComAmazonawsEc2Amazonec2State::set_spot_datafeed_subscription`]
/// needed room for — the GROWTH over the record it would have replaced, which is what that
/// writer gates on, not the whole record's charge — so the message states the quantity that
/// was actually refused.
pub fn spot_datafeed_store_full<E>(requested: usize) -> Scaffolded<E> {
    invalid_parameter_value(format!(
        "The data feed subscription needs {requested} more bytes, which this emulator has no \
         room for: it stores at most {MAX_STORED_BYTES} bytes of caller-supplied data"
    ))
}

// =============================================================================
// Future-dated Capacity Reservations
// =============================================================================

// A future-dated reservation is a record of the Capacity Reservation store above, not a
// resource of its own, so the id prefix, the stored shape, the ceilings and the
// `Invalid<Resource>Id.NotFound` refusal are the ones that section already declares. What
// is only a future-dated reservation's lives here: the two states only it reaches, the
// commitment it must carry, how far ahead it may start, the delivery method AWS supports
// and the instance families it may reserve. The Region a reservation belongs to is here
// too, because a future-dated create and the report of one ask the same question of it.

/// "`assessing` - (*Future-dated Capacity Reservations*) Amazon EC2 is assessing your request
/// for a future-dated Capacity Reservation" (that same member): where a future-dated
/// reservation sits until the request is approved.
pub const CAPACITY_RESERVATION_STATE_ASSESSING: &str = "assessing";

/// "`scheduled` - (*Future-dated Capacity Reservations*) The future-dated Capacity
/// Reservation request was approved and the Capacity Reservation is scheduled for delivery on
/// the requested start date" (that same member).
pub const CAPACITY_RESERVATION_STATE_SCHEDULED: &str = "scheduled";

/// The `targeted` member of the modelled `InstanceMatchCriteria` enum — the one a
/// future-dated Capacity Reservation must carry: "If you are requesting a future-dated
/// Capacity Reservation, you must specify `targeted`"
/// (`CreateCapacityReservationRequest$InstanceMatchCriteria`). The other member, `open`, is
/// spelled by [`DEFAULT_INSTANCE_MATCH_CRITERIA`], which is the default of that same member.
pub const INSTANCE_MATCH_CRITERIA_TARGETED: &str = "targeted";

/// The `incremental` member of the modelled `CapacityReservationDeliveryPreference` enum —
/// the only one a create may ask for: "The only supported value is `incremental`"
/// (`CreateCapacityReservationRequest$DeliveryPreference`).
pub const CAPACITY_RESERVATION_DELIVERY_PREFERENCE_INCREMENTAL: &str = "incremental";

/// The soonest a future-dated Capacity Reservation may start: "You can request a future-dated
/// Capacity Reservation between 5 and 120 days in advance"
/// (`CreateCapacityReservationRequest$StartDate`).
pub const MIN_FUTURE_CAPACITY_RESERVATION_LEAD_DAYS: u32 = 5;

/// The furthest ahead one may start, from that same sentence.
pub const MAX_FUTURE_CAPACITY_RESERVATION_LEAD_DAYS: u32 = 120;

/// One day, as the lead-time bounds above are published in days.
pub const ONE_DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// The instance families a future-dated Capacity Reservation may reserve: "You can request
/// future-dated Capacity Reservations for instance types in the C, M, R, I, T, and G instance
/// families only" (`CreateCapacityReservationRequest$InstanceType`).
///
/// A family is the LETTERS an instance type starts with, before its generation digit
/// (`m5.xlarge` -> `m`, `r6g.medium` -> `r`, `inf1.xlarge` -> `inf`), so the whole alphabetic
/// prefix is what a type is matched by and a family outside this set — `inf`, `trn`, `im` —
/// is not one of these six because it merely starts with one of their letters. Lowercase, as
/// every member of the modelled `InstanceType` catalog is
/// ([`family_letters_of_instance_type`] folds case before matching).
pub const FUTURE_CAPACITY_RESERVATION_INSTANCE_FAMILIES: [&str; 6] =
    ["c", "m", "r", "i", "t", "g"];

/// The instance family `instance_type` belongs to, WITHOUT its generation: the letters the
/// family starts with — `m5.xlarge` -> `m`, `im4gn.large` -> `im`, `inf1.xlarge` -> `inf`.
///
/// [`instance_family_of`] is the other reading of a family in this crate, the one
/// `HostProperties$InstanceFamily` publishes (`m5` of `m5.large`), and this is built on top
/// of it so the two cannot disagree about where a family ends. A rule published per family
/// letter ([`FUTURE_CAPACITY_RESERVATION_INSTANCE_FAMILIES`]) is matched against the whole
/// alphabetic prefix rather than a single character, which would admit any family merely
/// starting with the same letter.
pub fn family_letters_of_instance_type(instance_type: &str) -> String {
    instance_family_of(instance_type)
        .chars()
        .take_while(|character| character.is_ascii_alphabetic())
        .flat_map(char::to_lowercase)
        .collect()
}

/// What a caller committed to when it requested a future-dated Capacity Reservation, for
/// [`CapacityReservationSpec::commitment`].
///
/// Both members are decided by the handler — the duration is a value of the request, held to
/// [`MIN_CAPACITY_RESERVATION_COMMITMENT_SECONDS`]..=[`MAX_CAPACITY_RESERVATION_COMMITMENT_SECONDS`]
/// before it gets here, and the instant it expires is computed from the reservation's start
/// date with checked arithmetic — so the store does no clock arithmetic of its own on a
/// caller-supplied number.
#[derive(Debug, Clone, Copy)]
pub struct CapacityReservationCommitment {
    /// "The commitment duration, in seconds, for the future-dated Capacity Reservation"
    /// (`CapacityReservationCommitmentInfo$CommitmentDuration`).
    pub duration_seconds: i64,
    /// "The date and time at which the commitment duration expires"
    /// (`$CommitmentEndDate`): the start date plus that duration.
    pub end_date: SystemTime,
}

/// The fewest seconds a commitment may run: the minimum of the modelled `@range` on
/// `CapacityReservationCommitmentDuration`
/// (`smithy_query(query="operation",operation="CreateCapacityReservation")` reports
/// `range 1..200000000`).
///
/// Enforced by the handler rather than relied on from the model: the generated input member
/// is a plain `Option<i64>`, and a handler must not depend on where the request walk happens
/// to apply a range — without it a negative duration would reach the store.
pub const MIN_CAPACITY_RESERVATION_COMMITMENT_SECONDS: i64 = 1;

/// The most seconds a commitment may run, from the maximum of that same modelled `@range`.
pub const MAX_CAPACITY_RESERVATION_COMMITMENT_SECONDS: i64 = 200_000_000;

/// Is this Capacity Reservation one of the Region whose Availability Zones are `zones`?
///
/// "The results describe only the Capacity Reservations in the Amazon Web Services Region
/// that you're currently using" (`DescribeCapacityReservations`): a reservation reserves
/// capacity in ONE Availability Zone, so the Region it belongs to is the Region that zone is
/// in. `zones` is that Region's [`availability_zones_in_region`], passed in so a scan over
/// the store derives it once rather than per candidate.
pub fn capacity_reservation_is_in_region(
    reservation: &CapacityReservation,
    zones: &[AvailabilityZoneRecord],
) -> bool {
    reservation.availability_zone.as_deref().is_some_and(|zone| {
        zones
            .iter()
            .any(|in_region| in_region.zone_name.as_str() == zone)
    })
}

/// Refuse a Capacity Reservation id the request named that this store holds no reservation
/// for IN THAT REGION.
///
/// Region-scoped on purpose, where [`require_known_capacity_reservation`] resolves an id
/// whatever Region it was created in: a reservation of another Region is not one this
/// request can name, so it is reported not found rather than resolved across Regions. A
/// value outside the id form is refused as malformed first, exactly as that reader orders
/// the two.
pub fn require_capacity_reservation_in_region<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    zones: &[AvailabilityZoneRecord],
    capacity_reservation_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    require_well_formed_capacity_reservation_id(capacity_reservation_id)?;
    if store
        .capacity_reservations
        .get(capacity_reservation_id)
        .is_some_and(|record| capacity_reservation_is_in_region(&record.view, zones))
    {
        return Ok(());
    }
    Err(invalid_capacity_reservation_id_not_found(
        capacity_reservation_id,
    ))
}

// =============================================================================
// Root volume replacement tasks
// =============================================================================
//
// See the module docs for why a task is stored as the modelled shape plus the two moves
// the clock owes it. The words below are members of the model's own enums and the id form
// EC2 issues, named here so `CreateReplaceRootVolumeTask` and
// `DescribeReplaceRootVolumeTasks` cannot spell one value two ways.

/// The id prefix EC2 gives a root volume replacement task: `replacevol-<hex>`, the same
/// `<prefix>-<hex>` form every other EC2 resource id takes, so
/// [`ComAmazonawsEc2Amazonec2State::next_id`] mints them and
/// [`prefixed_id_is_well_formed`] reads them.
pub const REPLACE_ROOT_VOLUME_TASK_ID_PREFIX: &str = "replacevol";

/// The one `ResourceType` a `CreateReplaceRootVolumeTask` tag specification may name: "The
/// tags to apply to the root volume replacement task"
/// (`CreateReplaceRootVolumeTaskRequest$TagSpecifications`), spelled as the modelled
/// `ResourceType` enum spells it (`smithy_query(query="shape",shape="ResourceType")`).
pub const REPLACE_ROOT_VOLUME_TASK_TAGGABLE_RESOURCE: &str = "replace-root-volume-task";

/// `ReplaceRootVolumeTaskState`'s `pending` member — "the replacement volume is being
/// created" (`ReplaceRootVolumeTask$TaskState`), which is the state a task is created in.
/// The generated catalog is
/// `crate::ops::types::REPLACE_ROOT_VOLUME_TASK_STATE_VALUES`.
pub const REPLACE_ROOT_VOLUME_TASK_STATE_PENDING: &str = "pending";

/// `ReplaceRootVolumeTaskState`'s `in-progress` member — "the original volume is being
/// detached and the replacement volume is being attached".
pub const REPLACE_ROOT_VOLUME_TASK_STATE_IN_PROGRESS: &str = "in-progress";

/// `ReplaceRootVolumeTaskState`'s `succeeded` member — "the replacement volume has been
/// successfully attached to the instance and the instance is available": the terminal state
/// a replacement that runs to completion reaches.
pub const REPLACE_ROOT_VOLUME_TASK_STATE_SUCCEEDED: &str = "succeeded";

/// The slowest volume initialization rate a request may ask for: "Valid range: 100 - 300
/// MiB/s" (`CreateReplaceRootVolumeTaskRequest$VolumeInitializationRate`). The model
/// declares no `@range` for the member, so a handler is the only place the published bound
/// can be enforced.
pub const MIN_VOLUME_INITIALIZATION_RATE: i64 = 100;

/// The fastest volume initialization rate a request may ask for — the ceiling of that same
/// documented "Valid range: 100 - 300 MiB/s".
pub const MAX_VOLUME_INITIALIZATION_RATE: i64 = 300;

/// The largest page `DescribeReplaceRootVolumeTasks` serves, and the page it serves when
/// the request names no size.
///
/// The top of the modelled `MaxResults` range, `range 1..50`
/// (`smithy_query(query="operation",operation="DescribeReplaceRootVolumeTasks")`), so a
/// request that names no page size is answered with the largest page the operation can be
/// ASKED for and the `NextToken` path behaves the same either way.
pub const MAX_REPLACE_ROOT_VOLUME_TASKS_PAGE_SIZE: usize = 50;

/// EC2's code for something that is not a root volume replacement task id at all.
///
/// EC2's model declares no error shape (see [`crate::scaffolded::ServiceError`]), and the
/// service answers `Invalid<Resource>Id.Malformed` BEFORE it looks an id up —
/// [`INVALID_LAUNCH_TEMPLATE_ID_MALFORMED`] is the same code for a launch template — so
/// that is the code this resource's id gets.
pub const INVALID_REPLACE_ROOT_VOLUME_TASK_ID_MALFORMED: &str =
    "InvalidReplaceRootVolumeTaskId.Malformed";

/// EC2's code for a root volume replacement task id that names no task of the account,
/// named the way [`INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND`] is.
pub const INVALID_REPLACE_ROOT_VOLUME_TASK_ID_NOT_FOUND: &str =
    "InvalidReplaceRootVolumeTaskId.NotFound";

/// One stored root volume replacement task: the modelled [`ReplaceRootVolumeTask`] every
/// operation that reports one answers with, plus the two moves the clock owes it.
///
/// `CreateReplaceRootVolumeTask` answers a task in the non-terminal `pending` state, and
/// the task walks `pending` -> `in-progress` -> `succeeded` on its own whether or not
/// anybody calls again, so each move's due instant is recorded here on the monotonic clock
/// and [`Self::view_at`] derives the state from it. `TaskState` is therefore NOT stored on
/// the view: a reader that observed one state could otherwise disagree with the clock.
#[derive(Debug, Clone)]
pub struct ReplaceRootVolumeTaskRecord {
    /// The task as the model shapes it, with `TaskState` and `CompleteTime` left for
    /// [`Self::view_at`] to fill.
    pub view: ReplaceRootVolumeTask,
    /// The instant from which this task's `pending` -> `in-progress` move is due.
    /// Monotonic ([`Instant`]), because it is only ever compared against the clock and
    /// never reported, so a wall-clock adjustment cannot move a task backwards.
    pub in_progress_at: Instant,
    /// The instant from which this task's `in-progress` -> `succeeded` move is due.
    pub succeeded_at: Instant,
    /// "The time the task completed" (`ReplaceRootVolumeTask$CompleteTime`), rendered from
    /// the wall clock at the instant [`Self::succeeded_at`] falls due — reported only once
    /// the task has reached that state, because until then there is no instant at which it
    /// completed.
    pub complete_time: String,
}

impl ReplaceRootVolumeTaskRecord {
    /// A task starting now: `pending`, with its `in-progress` move due one
    /// [`TRANSITION_DELAY`] from now and its `succeeded` move one delay after that.
    ///
    /// `started_at` is the wall-clock instant the task was created, which is what
    /// `StartTime` reports and what `CompleteTime` is derived from, so the two timestamps
    /// come from one reading of the clock.
    pub fn starting_now(view: ReplaceRootVolumeTask, started_at: SystemTime) -> Self {
        let now = Instant::now();
        Self {
            view,
            in_progress_at: now + TRANSITION_DELAY,
            succeeded_at: now + TRANSITION_DELAY * 2,
            complete_time: iso8601_utc(started_at + TRANSITION_DELAY * 2),
        }
    }

    /// The task's `ReplaceRootVolumeTaskState` at `now`.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if now < self.in_progress_at {
            REPLACE_ROOT_VOLUME_TASK_STATE_PENDING
        } else if now < self.succeeded_at {
            REPLACE_ROOT_VOLUME_TASK_STATE_IN_PROGRESS
        } else {
            REPLACE_ROOT_VOLUME_TASK_STATE_SUCCEEDED
        }
    }

    /// This task as of `now`: the stored view with the state the clock has reached, and
    /// the completion time only once it has completed. A read observes the moves the clock
    /// already owed without writing anything.
    pub fn view_at(&self, now: Instant) -> ReplaceRootVolumeTask {
        let task_state = self.state_at(now);
        ReplaceRootVolumeTask {
            task_state: Some(task_state.to_string()),
            complete_time: (task_state == REPLACE_ROOT_VOLUME_TASK_STATE_SUCCEEDED)
                .then(|| self.complete_time.clone()),
            ..self.view.clone()
        }
    }
}

/// The bytes one stored root volume replacement task holds for the process's lifetime: the
/// tags the request chose, the `ClientToken` the token map keeps for it, and the fixed
/// overhead of the record itself ([`STORED_RECORD_OVERHEAD_BYTES`]).
///
/// Everything else on the record — the task's own id, the instance, image, snapshot and
/// volume ids it names, the timestamps and the state — is this module's own or bounded by
/// the id forms the model publishes, so it is fixed-size. The ONE definition of that
/// charge, so a writer cannot store bytes the store's budget never saw.
pub fn replace_root_volume_task_payload_bytes(
    view: &ReplaceRootVolumeTask,
    client_token: Option<&str>,
) -> usize {
    tag_list_bytes(view.tags.as_deref().unwrap_or_default())
        .saturating_add(client_token.map_or(0, str::len))
        .saturating_add(STORED_RECORD_OVERHEAD_BYTES)
}

/// Refuse a request that named a root volume replacement task id this service cannot
/// resolve: a malformed one before an unknown one, the order EC2 asks the two questions in
/// — the contract [`require_known_volume`] keeps for volumes.
pub fn require_known_replace_root_volume_task<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    task_id: &str,
) -> Result<(), Scaffolded<E>> {
    require_well_formed_id(
        REPLACE_ROOT_VOLUME_TASK_ID_PREFIX,
        task_id,
        INVALID_REPLACE_ROOT_VOLUME_TASK_ID_MALFORMED,
    )?;
    if !store.replace_root_volume_tasks.contains_key(task_id) {
        return Err(id_not_found(
            INVALID_REPLACE_ROOT_VOLUME_TASK_ID_NOT_FOUND,
            "replace root volume task id",
            task_id,
        ));
    }
    Ok(())
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another root volume replacement task fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`]: a writer asks this BEFORE storing
    /// anything and refuses the request when it answers `false`.
    pub fn has_replace_root_volume_task_capacity(&self) -> bool {
        self.replace_root_volume_tasks.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store `record` under a freshly minted `ReplaceRootVolumeTaskId`, charge the bytes it
    /// and `client_token` retain against the store's byte budget, and answer that id.
    ///
    /// The id is minted HERE rather than by the writer, so a task never shares an id with
    /// another resource of this store, and the record is written with that id on it so no
    /// reader has to learn the key twice. A create still asks
    /// [`Self::has_replace_root_volume_task_capacity`] and
    /// [`Self::would_exceed_stored_bytes`] first so it can refuse instead of overshooting.
    pub fn insert_replace_root_volume_task(
        &mut self,
        mut record: ReplaceRootVolumeTaskRecord,
        client_token: Option<&str>,
    ) -> String {
        let task_id = self.next_id(REPLACE_ROOT_VOLUME_TASK_ID_PREFIX);
        record.view.replace_root_volume_task_id = Some(task_id.clone());
        self.add_stored_bytes(replace_root_volume_task_payload_bytes(
            &record.view,
            client_token,
        ));
        if let Some(token) = client_token {
            self.replace_root_volume_task_client_tokens
                .insert(token.to_string(), task_id.clone());
        }
        self.replace_root_volume_tasks.insert(task_id.clone(), record);
        task_id
    }
}

// =============================================================================
// EC2 Mac modification tasks: the SIP modification and the volume ownership
// delegation task an EC2 Mac instance can be given
// =============================================================================

/// The id prefix this service mints a Mac modification task id with.
///
/// CHOSEN: `MacModificationTaskId` is a bare string in the model
/// (`smithy_query(query="shape",shape="MacModificationTaskId")` publishes no `@pattern`),
/// so there is no prefix to read off it; this one is spelled for the Mac modification task
/// it names, in the style of the other task prefixes this store mints. Every id comes from
/// [`ComAmazonawsEc2Amazonec2State::next_id`], which appends the seventeen lowercase
/// hexadecimal digits every long EC2 resource id carries, so each one is an id
/// [`prefixed_id_is_well_formed`] accepts.
pub const MAC_MODIFICATION_TASK_ID_PREFIX: &str = "macmdf";

/// The `ResourceType` a Mac modification task is tagged as: the `mac-modification-task`
/// member of the modelled `ResourceType` catalog (the generated catalog is
/// `crate::ops::types::RESOURCE_TYPE_VALUES`), which is the one resource type a create's
/// `TagSpecifications` may name.
pub const MAC_MODIFICATION_TASK_TAGGABLE_RESOURCE: &str = "mac-modification-task";

/// `MacModificationTaskState`'s `pending` member — the state a task is created in, before
/// the service has begun carrying it out. The generated catalog is
/// `crate::ops::types::MAC_MODIFICATION_TASK_STATE_VALUES`.
pub const MAC_MODIFICATION_TASK_STATE_PENDING: &str = "pending";

/// `MacModificationTaskState`'s `in-progress` member — the task is being carried out on the
/// instance.
pub const MAC_MODIFICATION_TASK_STATE_IN_PROGRESS: &str = "in-progress";

/// `MacModificationTaskState`'s `successful` member — the terminal state a task that runs
/// to completion reaches.
pub const MAC_MODIFICATION_TASK_STATE_SUCCESSFUL: &str = "successful";

/// `MacModificationTaskType`'s `sip-modification` member, the type of the task
/// `CreateMacSystemIntegrityProtectionModificationTask` starts. The generated catalog is
/// `crate::ops::types::MAC_MODIFICATION_TASK_TYPE_VALUES`.
pub const MAC_MODIFICATION_TASK_TYPE_SIP_MODIFICATION: &str = "sip-modification";

/// `MacModificationTaskType`'s `volume-ownership-delegation` member, the type of the task
/// `CreateDelegateMacVolumeOwnershipTask` starts.
pub const MAC_MODIFICATION_TASK_TYPE_VOLUME_OWNERSHIP_DELEGATION: &str =
    "volume-ownership-delegation";

/// The largest page `DescribeMacModificationTasks` serves, and the page it serves when the
/// request names no size: the top of the modelled `MaxResults` range, `range 1..500`
/// (`smithy_query(query="operation",operation="DescribeMacModificationTasks")`), so a
/// request that names no page size is answered with the largest page the operation can be
/// asked for and the `NextToken` path behaves the same either way.
pub const MAX_MAC_MODIFICATION_TASKS_PAGE_SIZE: usize = 500;

/// EC2's code for something that is not a Mac modification task id at all, named the way
/// [`INVALID_REPLACE_ROOT_VOLUME_TASK_ID_MALFORMED`] is: EC2 answers
/// `Invalid<Resource>Id.Malformed` BEFORE it looks an id up.
pub const INVALID_MAC_MODIFICATION_TASK_ID_MALFORMED: &str = "InvalidMacModificationTaskId.Malformed";

/// EC2's code for a Mac modification task id that names no task of the account.
pub const INVALID_MAC_MODIFICATION_TASK_ID_NOT_FOUND: &str = "InvalidMacModificationTaskId.NotFound";

/// One stored Mac modification task: the modelled [`MacModificationTask`] every operation
/// that reports one answers with, plus the two moves the clock owes it.
///
/// A create answers a task in the non-terminal `pending` state, and the task walks
/// `pending` -> `in-progress` -> `successful` on its own whether or not anybody calls again
/// (`smithy_query(query="shape",shape="MacModificationTaskState")`), so each move's due
/// instant is recorded here on the monotonic clock and [`Self::view_at`] derives the state
/// from it. `TaskState` is therefore NOT stored on the view: a reader that observed one
/// state could otherwise disagree with the clock.
#[derive(Debug, Clone)]
pub struct MacModificationTaskRecord {
    /// The task as the model shapes it, with `TaskState` left for [`Self::view_at`] to fill.
    pub view: MacModificationTask,
    /// The instant from which this task's `pending` -> `in-progress` move is due.
    /// Monotonic ([`Instant`]), because it is only ever compared against the clock and never
    /// reported, so a wall-clock adjustment cannot move a task backwards.
    pub in_progress_at: Instant,
    /// The instant from which this task's `in-progress` -> `successful` move is due.
    pub successful_at: Instant,
}

impl MacModificationTaskRecord {
    /// A task starting now: `pending`, with its `in-progress` move due one
    /// [`TRANSITION_DELAY`] from now and its `successful` move one delay after that.
    pub fn starting_now(view: MacModificationTask) -> Self {
        let now = Instant::now();
        Self {
            view,
            in_progress_at: now + TRANSITION_DELAY,
            successful_at: now + TRANSITION_DELAY * 2,
        }
    }

    /// The task's `MacModificationTaskState` at `now`.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if now < self.in_progress_at {
            MAC_MODIFICATION_TASK_STATE_PENDING
        } else if now < self.successful_at {
            MAC_MODIFICATION_TASK_STATE_IN_PROGRESS
        } else {
            MAC_MODIFICATION_TASK_STATE_SUCCESSFUL
        }
    }

    /// This task as of `now`: the stored view with the state the clock has reached. A read
    /// observes the moves the clock already owed without writing anything.
    pub fn view_at(&self, now: Instant) -> MacModificationTask {
        MacModificationTask {
            task_state: Some(self.state_at(now).to_string()),
            ..self.view.clone()
        }
    }
}

/// The bytes one stored Mac modification task holds for the process's lifetime: the tags the
/// request chose, the `ClientToken` the token map keeps for it, and the fixed overhead of the
/// record itself ([`STORED_RECORD_OVERHEAD_BYTES`]).
///
/// Everything else on the record — the task's own id, the instance it names, the SIP
/// configuration (eight members of a two-value catalog), the start time and the state — is
/// this module's own or bounded by the forms the model publishes, so it is fixed-size. The
/// ONE definition of that charge, so a writer cannot store bytes the store's budget never
/// saw.
pub fn mac_modification_task_payload_bytes(
    view: &MacModificationTask,
    client_token: Option<&str>,
) -> usize {
    tag_list_bytes(view.tags.as_deref().unwrap_or_default())
        .saturating_add(client_token.map_or(0, str::len))
        .saturating_add(STORED_RECORD_OVERHEAD_BYTES)
}

/// Refuse a request that named a Mac modification task id this service cannot resolve: a
/// malformed one before an unknown one, the order EC2 asks the two questions in — the
/// contract [`require_known_replace_root_volume_task`] keeps for root volume replacement
/// tasks.
pub fn require_known_mac_modification_task<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    task_id: &str,
) -> Result<(), Scaffolded<E>> {
    require_well_formed_id(
        MAC_MODIFICATION_TASK_ID_PREFIX,
        task_id,
        INVALID_MAC_MODIFICATION_TASK_ID_MALFORMED,
    )?;
    if !store.mac_modification_tasks.contains_key(task_id) {
        return Err(id_not_found(
            INVALID_MAC_MODIFICATION_TASK_ID_NOT_FOUND,
            "mac modification task id",
            task_id,
        ));
    }
    Ok(())
}

/// The stored instance a Mac modification task may be given to, or the refusal the request
/// gets instead.
///
/// Both tasks act on "an Amazon EC2 Mac instance" (`MacModificationTask$InstanceId`), so an
/// instance of a family that does not run macOS ([`is_mac_instance_family`]) is no instance
/// either task can be created for, and an instance on its way to `terminated` has no macOS
/// left to configure. The ONE reading of those three refusals, so the two creates cannot
/// disagree about which instance a task may name, and the resolved record is handed back so
/// a handler reads the instance once.
pub fn require_mac_instance<'store, E>(
    store: &'store ComAmazonawsEc2Amazonec2State,
    instance_id: &str,
) -> Result<&'store InstanceRecord, Scaffolded<E>> {
    // The Mac task validators word an instance id they cannot read their own way — "The
    // instance ID '<id>' is malformed", which is what the recorded refusal of
    // `CreateMacSystemIntegrityProtectionModificationTask` for the all-zero placeholder
    // carries — while the RULE behind that refusal is the crate's one reading of an instance
    // id ([`require_instance_record_worded`]).
    let instance = require_instance_record_worded(
        store,
        instance_id,
        Scaffolded::instance_id_is_malformed,
    )?;
    let instance_family = instance_family_of(&instance.instance_type);
    if !is_mac_instance_family(instance_family) {
        return Err(invalid_parameter_value(format!(
            "Invalid value '{instance_id}' for instanceId. The instance is a \
             '{instance_type}' instance, which is not an Amazon EC2 Mac instance",
            instance_type = instance.instance_type,
        )));
    }
    if instance.state.is_terminating() {
        return Err(Scaffolded::incorrect_instance_state(
            instance_id,
            "given a Mac modification task",
        ));
    }
    Ok(instance)
}

/// The members this service reads WITHOUT REGARD TO ASCII CASE, as `(the input shape that
/// carries the member, the modelled member name, the values it is read against)`.
///
/// EC2 holds a modelled enum to its value set, and this service's suites pin that: a value
/// outside the set, in any case, is refused. The table is deliberately a list of MEMBERS
/// rather than a rule about enums or about value sets — `[disabled, enabled]` is the value
/// set of a dozen other members of this model, each of which is judged exactly as declared.
///
/// `MacSystemIntegrityProtectionStatus` is in it because that is how this service's recorded
/// traffic has EC2 reading it: the corpus unit
/// `ec2.create-mac-sip-modification-task-rejected-unknown-instance` sends
/// `MacSystemIntegrityProtectionStatus=Disabled` against the modelled `[disabled, enabled]`
/// and is answered for its INSTANCE ID, so the status was read as the member it spells. A
/// member with no such recording keeps the strict reading.
///
/// `CreateApplicationStatusCheckRequest$Protocol` is in it for the same reason: the recorded
/// traffic has EC2 SERVING a `CreateApplicationStatusCheck` whose `Protocol` differs from the
/// declared `[http, https]` only in ASCII case, and answering the rest of the request — so
/// the protocol was read as the member it spells, where holding it to the declared spelling
/// refuses the create at the boundary before the handler runs.
///
/// Each row names the INPUT SHAPE as well as the member, because a bare member name does not
/// identify one member of this model: `Protocol` is also a member of
/// `CreateNetworkAclEntryRequest`, `ReplaceNetworkAclEntryRequest` (free-text `String`s, where
/// a rewrite would report back a value the client never sent),
/// `CreateNetworkInsightsPathRequest`, `CreateTransitGatewayConnectRequest` and
/// `ModifyApplicationStatusCheckRequest`. The shape is the one whose INPUT is being read, which
/// is what [`crate::service`] hands [`declared_enum_spelling`]; the member schema itself carries
/// only its own id and kind (`vendor/smithy-macros/src/lib.rs` emits `Schema::new_member(...)`
/// with the member's own id), so the enum behind it is not reachable from there and the value
/// set is named here as the generated constant of that member's own enum shape — this rule and
/// the framework's validating walk cannot disagree about what the model declares. The rule
/// fires only for a value that IS one of those declared values in some other case.
pub const CASE_INSENSITIVE_MEMBERS: &[(&str, &str, &[&str])] = &[
    (
        "CreateMacSystemIntegrityProtectionModificationTaskRequest",
        "MacSystemIntegrityProtectionStatus",
        crate::ops::types::MAC_SYSTEM_INTEGRITY_PROTECTION_SETTING_STATUS_VALUES,
    ),
    (
        "CreateApplicationStatusCheckRequest",
        "Protocol",
        crate::ops::types::NETWORK_PROTOCOL_ENUM_VALUES,
    ),
];

/// The declared spelling of `value` for the member `member_name` of the input shape
/// `input_shape`, when that member is one this service reads without regard to case and
/// `value` is one of its declared values in some other case.
///
/// `None` leaves the value exactly as the client sent it, for the framework's walk to accept
/// or refuse as it would have — which is what every member outside
/// [`CASE_INSENSITIVE_MEMBERS`], every member of another shape, and every value that is no
/// declared one in any case, gets. The member name is matched without regard to case too,
/// because the wire key a client sends is the member name with its first letter capitalised
/// and nothing should turn on that; the shape name is the model's own and is matched exactly.
pub fn declared_enum_spelling(
    input_shape: &str,
    member_name: &str,
    value: &str,
) -> Option<&'static str> {
    let (_, _, declared) = CASE_INSENSITIVE_MEMBERS.iter().find(|(shape, member, _)| {
        *shape == input_shape && member.eq_ignore_ascii_case(member_name)
    })?;
    if declared.contains(&value) {
        return None;
    }
    declared
        .iter()
        .find(|spelling| spelling.eq_ignore_ascii_case(value))
        .copied()
}

/// What one Mac modification task is created from: the facts its writer resolved, which
/// [`ComAmazonawsEc2Amazonec2State::start_mac_modification_task`] turns into the stored
/// record.
///
/// The caller's `MacCredentials` is NOT among them: the member is `@sensitive`
/// (`smithy_query(query="shape",shape="SensitiveMacCredentials")`), the modelled
/// `MacModificationTask` carries no member for it, and nothing a reader is shown is derived
/// from it — so it is never stored.
#[derive(Debug, Clone)]
pub struct MacModificationTaskSpec {
    /// "The ID of the Amazon EC2 Mac instance" (`MacModificationTask$InstanceId`), already
    /// resolved by [`require_mac_instance`].
    pub instance_id: String,
    /// "The type of task" (`MacModificationTask$TaskType`): one of
    /// [`MAC_MODIFICATION_TASK_TYPE_SIP_MODIFICATION`] and
    /// [`MAC_MODIFICATION_TASK_TYPE_VOLUME_OWNERSHIP_DELEGATION`].
    pub task_type: &'static str,
    /// "[SIP modification tasks only] Information about the SIP configuration"
    /// (`MacModificationTask$MacSystemIntegrityProtectionConfig`) — `None` for a task of any
    /// other type, which is what "SIP modification tasks only" means.
    pub sip_configuration: Option<MacSystemIntegrityProtectionConfiguration>,
    /// "The tags assigned to the task" (`MacModificationTask$Tags`), as the request's
    /// `TagSpecifications` named them.
    pub tags: Vec<Tag>,
    /// The request's idempotency token, if it carried one.
    pub client_token: Option<String>,
}

/// Does the stored task `view` report exactly the request `spec` describes?
///
/// Everything the modelled `MacModificationTask` carries OF THE REQUEST is compared — the
/// instance, the task type, the SIP configuration and the tags — so a `ClientToken` reuse
/// counts as a replay only when the request really is the one the token already named; the
/// members the SERVICE owns (the id, the start time and the state) are its own and are not
/// part of a request's identity. The modelled shapes derive no `PartialEq`, so the
/// comparison is written out member by member here, in the one place that needs it.
pub(super) fn mac_modification_task_reports(
    view: &MacModificationTask,
    spec: &MacModificationTaskSpec,
) -> bool {
    let settings = |configuration: Option<&MacSystemIntegrityProtectionConfiguration>| {
        configuration.map(|configuration| {
            [
                configuration.apple_internal.clone(),
                configuration.base_system.clone(),
                configuration.debugging_restrictions.clone(),
                configuration.d_trace_restrictions.clone(),
                configuration.filesystem_protections.clone(),
                configuration.kext_signing.clone(),
                configuration.nvram_protections.clone(),
                configuration.status.clone(),
            ]
        })
    };
    let pairs = |tags: &[Tag]| -> Vec<(Option<String>, Option<String>)> {
        tags.iter()
            .map(|tag| (tag.key.clone(), tag.value.clone()))
            .collect()
    };
    view.instance_id.as_deref() == Some(spec.instance_id.as_str())
        && view.task_type.as_deref() == Some(spec.task_type)
        && settings(view.mac_system_integrity_protection_config.as_ref())
            == settings(spec.sip_configuration.as_ref())
        && pairs(view.tags.as_deref().unwrap_or_default()) == pairs(&spec.tags)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another Mac modification task fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_replace_root_volume_task_capacity`].
    pub fn has_mac_modification_task_capacity(&self) -> bool {
        self.mac_modification_tasks.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Start the Mac modification task `spec` describes and answer it as a reader will see
    /// it, or refuse the request because this emulator cannot hold another task.
    ///
    /// The ONE way a Mac modification task is stored, so the two creates cannot write one
    /// resource two ways: the id is minted here, the start time is this service's own
    /// reading of the clock, and the state is left to the clock
    /// ([`MacModificationTaskRecord`]). A `ClientToken` already in the store names the task
    /// it started — "unique, case-sensitive identifier you provide to ensure the idempotency
    /// of the request" (`CreateMacSystemIntegrityProtectionModificationTaskRequest$ClientToken`)
    /// — so a replay reports that task and starts nothing, while a token reused by a
    /// different request is refused ([`idempotent_parameter_mismatch`]). Both are answered
    /// before the ceilings a fresh create has to clear, because neither stores anything.
    pub fn start_mac_modification_task<E>(
        &mut self,
        spec: MacModificationTaskSpec,
    ) -> Result<MacModificationTask, Scaffolded<E>> {
        if let Some(task_id) = spec
            .client_token
            .as_deref()
            .and_then(|token| self.mac_modification_task_client_tokens.get(token))
            .cloned()
        {
            // The token names the task it started, so a REPLAY reports that task. A
            // DIFFERENT request under the same token is not a replay — "the request uses the
            // same client token as a previous, but non-identical request" — so it is told so
            // rather than served, and the identity compared is EVERYTHING the stored task
            // reports of the request it was created from ([`mac_modification_task_reports`]).
            let started = self
                .mac_modification_tasks
                .get(&task_id)
                .map(|record| &record.view);
            let names_the_same_request =
                started.is_some_and(|view| mac_modification_task_reports(view, &spec));
            if !names_the_same_request {
                return Err(idempotent_parameter_mismatch());
            }
            return self.mac_modification_task_view(&task_id);
        }

        let view = MacModificationTask {
            // Minted at the insert below, so an id is never chosen twice.
            mac_modification_task_id: None,
            instance_id: Some(spec.instance_id),
            mac_system_integrity_protection_config: spec.sip_configuration,
            // "The date and time the task was created"
            // (`MacModificationTask$StartTime`): this service's own reading of the wall
            // clock, never a value a request carried.
            start_time: Some(system_time_to_date_time(SystemTime::now())),
            tags: Some(spec.tags),
            // Derived by the record from the clock, not stored on the view.
            task_state: None,
            task_type: Some(spec.task_type.to_string()),
        };
        let payload_bytes =
            mac_modification_task_payload_bytes(&view, spec.client_token.as_deref());
        if !self.has_mac_modification_task_capacity() {
            return Err(Scaffolded::request_limit_exceeded(format!(
                "This emulator holds {MAX_STORED_RECORDS_PER_STORE} Mac modification tasks, \
                 which is as many as it stores"
            )));
        }
        if self.would_exceed_stored_bytes(payload_bytes) {
            return Err(Scaffolded::request_limit_exceeded(format!(
                "This emulator holds {} of at most {MAX_STORED_BYTES} bytes of \
                 caller-supplied data, so it cannot store {payload_bytes} more",
                self.stored_bytes()
            )));
        }

        let task_id = self.next_id(MAC_MODIFICATION_TASK_ID_PREFIX);
        let mut record = MacModificationTaskRecord::starting_now(view);
        record.view.mac_modification_task_id = Some(task_id.clone());
        self.add_stored_bytes(payload_bytes);
        if let Some(token) = spec.client_token {
            self.mac_modification_task_client_tokens
                .insert(token, task_id.clone());
        }
        self.mac_modification_tasks.insert(task_id.clone(), record);
        self.mac_modification_task_view(&task_id)
    }

    /// The stored Mac modification task `task_id` names, as of now.
    ///
    /// A create answers the task by reading it back OUT of the store through here, so the
    /// caller sees what a later `DescribeMacModificationTasks` will see rather than the
    /// request object.
    pub(super) fn mac_modification_task_view<E>(
        &self,
        task_id: &str,
    ) -> Result<MacModificationTask, Scaffolded<E>> {
        self.mac_modification_tasks
            .get(task_id)
            .map(|record| record.view_at(Instant::now()))
            .ok_or_else(|| {
                Scaffolded::internal_error(format!(
                    "the Mac modification task '{task_id}' could not be read back"
                ))
            })
    }
}

// =============================================================================
// IPAM internet registry associations, and the routing policy registrations
// (Route Origin Authorizations) made under them
// =============================================================================

/// The id prefix this service mints an IPAM internet registry association with.
///
/// NOT `ipam-ira`, and the recorded wire corpus is why. AWS answers
/// `ipam-ira-0000000000000000f` — a seventeen-hexadecimal-digit suffix, the long EC2 resource
/// id form — with `InvalidIpamInternetRegistryAssociationId.Malformed` ("The
/// ipam-internet-registry-association ID '…' is malformed"), while the SAME suffix under the
/// prefixes AWS does issue is resolved and only then reported missing:
/// `ipam-pool-0000000000000000f` ("The ipam-pool ID '…' does not exist"),
/// `ipam-scope-0000000000000000f`, `ipam-0000000000000000f` and `tgw-rtb-0000000000000000f`
/// each come back as a NotFound. The suffix is therefore well formed and it is the PREFIX
/// that EC2 does not know: `ipam-ira` is not a resource-id prefix of this API, which is what
/// makes an id carrying it malformed rather than missing.
///
/// Which prefix AWS does issue for this resource is published nowhere the model carries
/// (`smithy_query(query="shape",shape="IpamInternetRegistryAssociationId")` is a plain
/// `String` with no `@pattern`), so this one is CHOSEN, spelled in the style of the IPAM
/// resource discovery association's own prefix: `ir` for the internet registry the
/// association is with, `-assoc` for the association itself.
/// [`ComAmazonawsEc2Amazonec2State::next_id`] appends the seventeen lowercase hexadecimal
/// digits every long EC2 id carries, so every id this store mints is one
/// [`ipam_internet_registry_association_id_is_well_formed`] accepts.
pub const IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_PREFIX: &str = "ipam-ir-assoc";

/// The `ResourceType` an internet registry association's own ARN names, which is also how
/// EC2's refusals spell this resource kind.
pub const RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION: &str =
    "ipam-internet-registry-association";

/// An association a create has just made: the `IpamInternetRegistryAssociationState`
/// catalog's `create-in-progress` member (the generated catalog is
/// `crate::ops::types::IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES`), the TRANSIENT state
/// a create answers with.
pub const IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_CREATE_IN_PROGRESS: &str =
    "create-in-progress";

/// An association whose creation has finished and whose delegated RPKI setup has not been
/// enabled yet: that catalog's `pending-enable` member, which the clock owes every
/// association a create has started on.
///
/// Enabling one is a request of its own (`EnableIpamInternetRegistryAssociation`, which takes
/// the parent handle and the parent BPKI trust anchor the internet registry answered the
/// child request with), so a created association RESTS here until that call is made rather
/// than moving on by itself.
pub const IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_PENDING_ENABLE: &str = "pending-enable";

/// The `StateMessage` an association carries while it is still being created — "a message
/// describing the current state of the internet registry association"
/// (`IpamInternetRegistryAssociation$StateMessage`), in this crate's own words rather than a
/// recording's.
pub const IPAM_INTERNET_REGISTRY_ASSOCIATION_CREATE_IN_PROGRESS_MESSAGE: &str =
    "The internet registry association is being created.";

/// The `StateMessage` an association carries once it is created and waiting to be enabled,
/// written by the clock that owed it
/// ([`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`]).
pub const IPAM_INTERNET_REGISTRY_ASSOCIATION_PENDING_ENABLE_MESSAGE: &str =
    "The internet registry association is waiting to be enabled.";

/// An association whose deletion has been asked for and is not finished: the
/// `IpamInternetRegistryAssociationState` catalog's `delete-in-progress` member (the generated
/// catalog is `crate::ops::types::IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES`), the
/// TRANSIENT state a delete answers with.
pub const IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_IN_PROGRESS: &str =
    "delete-in-progress";

/// An association whose deletion is finished: that catalog's `delete-complete` member, which
/// the clock owes every association a delete has started on.
pub const IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_COMPLETE: &str = "delete-complete";

/// The `StateMessage` an association carries while its deletion is under way — "a message
/// describing the current state of the internet registry association"
/// (`IpamInternetRegistryAssociation$StateMessage`), in this crate's own words rather than a
/// recording's.
pub const IPAM_INTERNET_REGISTRY_ASSOCIATION_DELETE_IN_PROGRESS_MESSAGE: &str =
    "The internet registry association is being deleted.";

/// The `StateMessage` an association carries once its deletion is finished, written by the
/// clock that owed it ([`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`]).
pub const IPAM_INTERNET_REGISTRY_ASSOCIATION_DELETE_COMPLETE_MESSAGE: &str =
    "The internet registry association has been deleted.";

/// The `StateMessage` an association sitting in `state` reports, for the states this service
/// MOVES one into, and `None` for a state it carries no message of its own for.
///
/// The ONE mapping of a state to its message, so the clock that materialises a move owed to
/// an association ([`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`]) and the
/// write that started that move cannot describe one state two ways.
pub fn ipam_internet_registry_association_state_message(state: &str) -> Option<&'static str> {
    match state {
        IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_CREATE_IN_PROGRESS => {
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_CREATE_IN_PROGRESS_MESSAGE)
        }
        IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_PENDING_ENABLE => {
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_PENDING_ENABLE_MESSAGE)
        }
        IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_IN_PROGRESS => {
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_DELETE_IN_PROGRESS_MESSAGE)
        }
        IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_COMPLETE => {
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_DELETE_COMPLETE_MESSAGE)
        }
        _ => None,
    }
}

/// The id prefix a routing policy registration delta is minted with.
///
/// "The unique identifier of the delta" (`IpamRoutingPolicyRegistrationDelta$DeltaId`) carries
/// no documented form — no `@pattern` and no example in the model — so the form is CHOSEN:
/// an EC2 resource id of the long form under a prefix naming the resource, like every other
/// id this store mints.
pub const IPAM_ROUTING_POLICY_REGISTRATION_DELTA_ID_PREFIX: &str = "ipam-rpr-delta";

/// A routing policy registration in service: the `IpamRoutingPolicyRegistrationState`
/// catalog's `create-complete` member (the generated catalog is
/// `crate::ops::types::IPAM_ROUTING_POLICY_REGISTRATION_STATE_VALUES`).
pub const IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE: &str = "create-complete";

/// A registration whose deletion has been asked for and is not finished: that catalog's
/// `delete-in-progress` member, the TRANSIENT state a delete answers with.
pub const IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_IN_PROGRESS: &str = "delete-in-progress";

/// A registration whose deletion is finished: that catalog's `delete-complete` member, which
/// the clock owes every registration a delete has started on.
pub const IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_COMPLETE: &str = "delete-complete";

/// A delta that has not been published yet: the `IpamRoutingPolicyRegistrationDeltaState`
/// catalog's `pending` member ("Valid values: `pending` | `published` | `failed`",
/// `IpamRoutingPolicyRegistrationDelta$State`).
pub const IPAM_ROUTING_POLICY_REGISTRATION_DELTA_STATE_PENDING: &str = "pending";

/// A delta that has been published: that catalog's `published` member, which the clock owes
/// every delta this store mints.
pub const IPAM_ROUTING_POLICY_REGISTRATION_DELTA_STATE_PUBLISHED: &str = "published";

/// The `StateMessage` a delta carries while it is
/// [`IPAM_ROUTING_POLICY_REGISTRATION_DELTA_STATE_PENDING`] — "a message describing the
/// current state" (`IpamRoutingPolicyRegistrationDelta$StateMessage`), in this crate's own
/// words rather than a recording's.
pub const IPAM_ROUTING_POLICY_REGISTRATION_DELTA_PENDING_MESSAGE: &str =
    "The routing policy registration delta has not been published yet.";

/// The `StateMessage` a delta carries once it is
/// [`IPAM_ROUTING_POLICY_REGISTRATION_DELTA_STATE_PUBLISHED`].
pub const IPAM_ROUTING_POLICY_REGISTRATION_DELTA_PUBLISHED_MESSAGE: &str =
    "The routing policy registration delta has been published.";

/// `InvalidIpamInternetRegistryAssociationId.Malformed` (HTTP 400): the request named
/// something that is not an IPAM internet registry association id at all, so there is
/// nothing to look up. The code AWS answers, as the recorded wire corpus behind
/// `issues/…-ec2-deleteipamroutingpolicyregistration-stub.md` shows.
pub const INVALID_IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_MALFORMED: &str =
    "InvalidIpamInternetRegistryAssociationId.Malformed";

/// `InvalidIpamInternetRegistryAssociationId.NotFound` (HTTP 400): the request named a
/// well-formed IPAM internet registry association id this account holds no association for.
///
/// The `.NotFound` counterpart of the `.Malformed` code above, which is how every EC2
/// resource spells the second of those two questions (`InvalidIpamPoolId.NotFound`,
/// `InvalidVpcEndpointId.NotFound`, …).
pub const INVALID_IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_NOT_FOUND: &str =
    "InvalidIpamInternetRegistryAssociationId.NotFound";

/// Whether `id` is a well-formed IPAM internet registry association id — one this service
/// could have ISSUED.
///
/// `<prefix>-<8 or 17 lowercase hex digits>`, read by the one generic id reader
/// ([`prefixed_id_is_well_formed`]) under this resource's own prefix
/// ([`IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_PREFIX`], whose doc carries the recorded
/// evidence for it). Anything else is `.Malformed` BEFORE any lookup, so a caller can tell
/// "not an id of this kind" from "no such association".
pub fn ipam_internet_registry_association_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_PREFIX, id)
}

/// Refuse something that is not an IPAM internet registry association id at all: HTTP 400
/// `InvalidIpamInternetRegistryAssociationId.Malformed`, worded the way AWS words a
/// malformed id of this resource and built from the request's own value.
pub fn invalid_ipam_internet_registry_association_id_malformed<E>(id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_MALFORMED,
        400,
        format!("The {RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION} ID '{id}' is malformed"),
    )
}

/// Refuse a well-formed IPAM internet registry association id this account holds no
/// association for: HTTP 400 `InvalidIpamInternetRegistryAssociationId.NotFound`, worded by
/// the one writer of that refusal ([`id_not_found`]).
pub fn invalid_ipam_internet_registry_association_id_not_found<E>(id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_NOT_FOUND,
        &format!("{RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION} ID"),
        id,
    )
}

/// Refuse an `IpamInternetRegistryAssociationId` this service cannot resolve: a malformed
/// one before an unknown one, the order [`require_well_formed_id`] documents.
///
/// The ONE place an association id is resolved, so every operation that takes one refuses
/// the same id the same way; the contract [`require_known_instance`] writes out for
/// instances.
pub fn require_known_ipam_internet_registry_association<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    id: &str,
) -> Result<(), Scaffolded<E>> {
    if !ipam_internet_registry_association_id_is_well_formed(id) {
        return Err(invalid_ipam_internet_registry_association_id_malformed(id));
    }
    if !store.ipam_internet_registry_associations.contains_key(id) {
        return Err(invalid_ipam_internet_registry_association_id_not_found(id));
    }
    Ok(())
}

/// Refuse a request that named a CIDR prefix the association holds no routing policy
/// registration for: HTTP 400 `InvalidCidr.NotFound`, which is EC2's code for an address
/// range a request named that there is nothing stored under
/// ([`invalid_cidr_not_found`] words the BYOIP path's own).
///
/// A registration is addressed by `IpamInternetRegistryAssociationId` + `Cidr` and carries
/// no id of its own, so the refusal names both: the caller can tell which of several
/// prefixes, under which association, was not found from this text alone.
pub fn ipam_routing_policy_registration_not_found<E>(
    association_id: &str,
    cidr: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CIDR_NOT_FOUND,
        400,
        format!(
            "The {RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION} '{association_id}' has no \
             routing policy registration for the CIDR ({cidr})."
        ),
    )
}

/// One routing policy registration: the modelled [`IpamRoutingPolicyRegistration`] every
/// operation that reports one answers with, plus the move the clock owes it.
///
/// The modelled shape itself rather than a parallel record, for the reason every other store
/// here holds one: a member a writer sets round-trips through
/// `GetIpamRoutingPolicyRegistrations` without a field-by-field copy that can silently drop
/// one.
#[derive(Debug, Clone)]
pub struct IpamRoutingPolicyRegistrationRecord {
    /// The registration as the model shapes it, carrying the state reached so far.
    pub view: IpamRoutingPolicyRegistration,
    /// The terminal state the clock owes this registration while it is still transient
    /// (`delete-in-progress` -> `delete-complete`); `None` for one that owes none.
    pub pending: Option<PendingStateMove>,
}

impl IpamRoutingPolicyRegistrationRecord {
    /// A registration sitting in `state` with the ASNs and prefix it authorizes, and nothing
    /// owed to it by the clock.
    pub fn in_state(view: IpamRoutingPolicyRegistration, state: &str) -> Self {
        Self {
            view: IpamRoutingPolicyRegistration {
                state: Some(state.to_string()),
                ..view
            },
            pending: None,
        }
    }

    /// The registration's current `IpamRoutingPolicyRegistrationState`.
    pub fn state(&self) -> &str {
        self.view.state.as_deref().unwrap_or_default()
    }

    /// Whether a deletion of this registration has already been asked for: it is either
    /// still `delete-in-progress` or already `delete-complete`, and in neither state is
    /// there an authorization left to withdraw.
    pub fn is_deleting(&self) -> bool {
        self.state() == IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_IN_PROGRESS
            || self.state() == IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_COMPLETE
    }
}

/// One IPAM internet registry association and the routing policy registrations made under
/// it.
///
/// The registrations are keyed by the CIDR prefix they authorize, in the canonical form
/// [`canonical_byoip_cidr`] reads, because a registration IS per (association, prefix): the
/// operations that act on one name it by `IpamInternetRegistryAssociationId` + `Cidr` and
/// carry no id of their own for it. The key order is the stable order a report of them pages
/// over.
#[derive(Debug, Clone)]
pub struct IpamInternetRegistryAssociationRecord {
    /// The association itself, as the modelled shape its own operations report.
    pub view: IpamInternetRegistryAssociation,
    /// The routing policy registrations under this association, by canonical CIDR prefix.
    pub registrations: BTreeMap<String, IpamRoutingPolicyRegistrationRecord>,
    /// The terminal state the clock owes this association while it is still transient
    /// (`delete-in-progress` -> `delete-complete`); `None` for one that owes none.
    pub pending: Option<PendingStateMove>,
}

impl IpamInternetRegistryAssociationRecord {
    /// The association's current `IpamInternetRegistryAssociationState`.
    pub fn state(&self) -> &str {
        self.view.state.as_deref().unwrap_or_default()
    }

    /// Whether a deletion of this association has already been asked for: it is either still
    /// `delete-in-progress` or already `delete-complete`, and in neither state is there an
    /// association left to delete.
    pub fn is_deleting(&self) -> bool {
        self.state() == IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_IN_PROGRESS
            || self.state() == IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_COMPLETE
    }

    /// The prefixes this association still holds a routing policy registration for, in the
    /// store's own order.
    ///
    /// A registration is REMOVED once its withdrawal is finished
    /// ([`IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_COMPLETE`]), so one a delete has only
    /// started on still counts: it is an authorization the internet registry has not dropped
    /// yet. The ONE definition of "what still depends on this association", so the operation
    /// that deletes one and any later reader cannot disagree about it.
    pub fn held_registration_cidrs(&self) -> Vec<&str> {
        self.registrations
            .iter()
            .filter(|(_, registration)| {
                registration.state() != IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_COMPLETE
            })
            .map(|(cidr, _)| cidr.as_str())
            .collect()
    }
}

/// The bytes one stored IPAM internet registry association holds: the tags assigned to it,
/// its description, its organization handle at the registry and the message about its state,
/// on top of the fixed [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs.
///
/// The ONE definition of that charge, the way [`ipam_policy_payload_bytes`] is for a policy:
/// a writer charges exactly it, so a write that swaps one stored message for another cannot
/// drift the store's budget.
pub fn ipam_internet_registry_association_payload_bytes(
    view: &IpamInternetRegistryAssociation,
) -> usize {
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(tag_list_bytes(view.tags.as_deref().unwrap_or_default()))
        .saturating_add(view.description.as_deref().map_or(0, str::len))
        .saturating_add(view.organization_handle.as_deref().map_or(0, str::len))
        .saturating_add(view.state_message.as_deref().map_or(0, str::len))
}

/// One routing policy registration delta: the modelled
/// [`IpamRoutingPolicyRegistrationDelta`] the write that minted it answered with, plus which
/// registration it changed and the publication the clock owes it.
///
/// "Contains information about a routing policy registration change"
/// (`IpamRoutingPolicyRegistrationDelta`): a delta is the record of ONE change, so it is kept
/// after the call that made it rather than being rebuilt by whoever asks next, and the
/// registration it belongs to is named on it so a report of the deltas of one registration
/// needs no scan of the registrations.
#[derive(Debug, Clone)]
pub struct IpamRoutingPolicyRegistrationDeltaRecord {
    /// The delta as the model shapes it, carrying the state reached so far.
    pub view: IpamRoutingPolicyRegistrationDelta,
    /// The association the changed registration belongs to.
    pub ipam_internet_registry_association_id: String,
    /// The canonical CIDR prefix of the changed registration.
    pub cidr: String,
    /// The move to `published` a delta owes while it is still `pending`; `None` once it has
    /// been applied.
    pub pending: Option<PendingStateMove>,
}

/// What a routing policy registration delta is minted from.
///
/// A struct rather than four positional arguments so no caller can swap the association id
/// and the prefix, which are both strings.
#[derive(Debug, Clone)]
pub struct IpamRoutingPolicyRegistrationDeltaSpec {
    /// The association whose registration changed.
    pub ipam_internet_registry_association_id: String,
    /// The canonical CIDR prefix of the registration that changed.
    pub cidr: String,
    /// "The JSON specification describing the changes applied in this delta"
    /// (`IpamRoutingPolicyRegistrationDelta$DeltaJson`), built by the operation that made
    /// the change from the registration it changed.
    pub delta_json: String,
}

/// The bytes one stored routing policy registration holds: the prefix it authorizes, the
/// ASNs authorized to originate it, its description and the id of its latest delta, on top
/// of the fixed [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs.
///
/// The ONE definition of that charge, the way [`byoip_cidr_payload_bytes`] is for an address
/// range: a writer charges exactly it.
pub fn ipam_routing_policy_registration_payload_bytes(
    view: &IpamRoutingPolicyRegistration,
) -> usize {
    let asn_bytes = view
        .asns
        .iter()
        .flatten()
        .map(String::len)
        .fold(0usize, usize::saturating_add);
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(view.cidr.as_deref().map_or(0, str::len))
        .saturating_add(view.description.as_deref().map_or(0, str::len))
        .saturating_add(view.latest_delta_id.as_deref().map_or(0, str::len))
        .saturating_add(asn_bytes)
}

/// The bytes one stored delta holds: its JSON specification and its state message, on top of
/// the same fixed overhead.
pub fn ipam_routing_policy_registration_delta_payload_bytes(
    view: &IpamRoutingPolicyRegistrationDelta,
) -> usize {
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(view.delta_json.as_deref().map_or(0, str::len))
        .saturating_add(view.state_message.as_deref().map_or(0, str::len))
}
