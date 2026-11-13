// SPDX-License-Identifier: CC0-1.0

//! The CORE-DEVICE half of this service's store: the fleet of Greengrass core devices,
//! the client devices associated with each of them for cloud discovery, and the pieces
//! the core-device operations share (paging, the surface's not-found refusal, and the
//! projection of a deployment onto the device it reaches).
//!
//! A core device is "an IoT thing that runs the IoT Greengrass Core software"
//! (`smithy_query(query="shape",shape="CoreDevice")`), and every fact this service
//! reports about one — its status, the software version, the platform, the architecture,
//! the runtime, the components it runs — is REPORTED BY THAT DEVICE: "IoT Greengrass
//! relies on individual devices to send status updates to the Amazon Web Services Cloud"
//! (`GetCoreDevice`'s own documentation). No operation this service's model binds carries
//! such a report, so a record here is written the way the device's own status update
//! writes it — through [`ComAmazonawsGreengrassv2Greengrassv2State::record_core_device`] —
//! and the modelled operations READ it: `GetCoreDevice` and `ListCoreDevices` report what
//! the device reported, `DeleteCoreDevice` "removes the core device from the list of core
//! devices" (its own documentation) and reports the modelled 404 for a thing name no
//! device reported under, and `ListInstalledComponents` reports the components the device
//! listed.
//!
//! The client-device ASSOCIATIONS are a store of their own, keyed by the core device's
//! thing name rather than held on the record above, because the association is a
//! discovery permission written by `BatchAssociateClientDeviceWithCoreDevice` for a thing
//! name — AWS records it, and `ListClientDevicesAssociatedWithCoreDevice` reports it,
//! whether or not a core device has ever reported itself under that name.
//!
//! Every collection here is bounded by an enforced COUNT: the fleet by
//! [`MAX_CORE_DEVICES`], one device's reported components by
//! [`MAX_INSTALLED_COMPONENTS_PER_CORE_DEVICE`], the association store by
//! [`MAX_CORE_DEVICES_WITH_ASSOCIATIONS`] names and each name's set by
//! [`MAX_CLIENT_DEVICES_PER_CORE_DEVICE`].
//!
//! What the listings here share with every other paginated reader of this service — the
//! page size and the cursor — lives in [`super::paging`].

use std::collections::BTreeSet;

use super::*;

/// The most core devices this emulator holds a status report for.
///
/// NOT an AWS quota: AWS's own ceiling on core devices per account is published as an
/// adjustable service quota, so this is the emulator's own, CHOSEN so that a sequence of
/// reports cannot grow the process without bound. A report past it is refused
/// ([`ComAmazonawsGreengrassv2Greengrassv2State::record_core_device`]) rather than
/// evicting a device a reader would then not find.
pub const MAX_CORE_DEVICES: usize = 10_000;

/// The most components one core device may report as installed.
///
/// NOT an AWS quota: CHOSEN, far above the component count of any real deployment, and
/// enforced so one device's report cannot grow the store without bound.
pub const MAX_INSTALLED_COMPONENTS_PER_CORE_DEVICE: usize = 1_000;

/// The most core device thing names the association store holds a set for.
///
/// `coreDeviceThingName` is a caller-chosen `@httpLabel`, so without this a caller that
/// varies the name per request would grow the store forever. NOT an AWS quota: CHOSEN.
pub const MAX_CORE_DEVICES_WITH_ASSOCIATIONS: usize = 10_000;

/// The most client devices one core device may have associated for cloud discovery.
///
/// NOT an AWS quota: CHOSEN. An entry past it is reported in the batch operation's own
/// per-entry error channel ([`AssociationRefusal`]), which is where that API reports an
/// entry it did not apply.
pub const MAX_CLIENT_DEVICES_PER_CORE_DEVICE: usize = 2_500;

/// The `CoreDeviceStatus` of a device whose Greengrass Core software and components all
/// run without issue (`smithy_query(query="shape",shape="CoreDeviceStatus")`: `HEALTHY`,
/// `UNHEALTHY`).
pub const CORE_DEVICE_STATUS_HEALTHY: &str = "HEALTHY";

/// The `InstalledComponentTopologyFilter` a request that names none is served under:
/// `ListInstalledComponents`'s `topologyFilter` documents "Default: `ROOT`", which lists
/// "only root components, which are components that you specify in a deployment".
pub const TOPOLOGY_FILTER_DEFAULT: &str = "ROOT";

/// The `InstalledComponentTopologyFilter` that lists every component installed on the
/// device, dependencies included.
pub const TOPOLOGY_FILTER_ALL: &str = "ALL";

/// The `EffectiveDeploymentExecutionStatus` of a deployment job that has reached the core
/// device but that the device has reported nothing about yet
/// (`smithy_query(query="shape",shape="EffectiveDeploymentExecutionStatus")`).
pub const EXECUTION_STATUS_QUEUED: &str = "QUEUED";

/// The `EffectiveDeploymentExecutionStatus` of a deployment whose rollout was cancelled,
/// so it reaches the device no further.
pub const EXECUTION_STATUS_CANCELED: &str = "CANCELED";

/// The `EffectiveDeploymentExecutionStatus` of a deployment whose job has finished.
pub const EXECUTION_STATUS_COMPLETED: &str = "COMPLETED";

/// What the core-device surface reports for a thing name no core device has reported
/// itself under, after the name: the refusal is one fixed phrase and the thing name the
/// request asked about.
pub const CORE_DEVICE_NOT_FOUND_SUFFIX: &str = "can not be found";

/// What a deployment that was created without a `deploymentName` is called where the name
/// is REQUIRED — `EffectiveDeployment$deploymentName`
/// (`smithy_query(query="shape",shape="EffectiveDeployment")`) — followed by the name of
/// the thing or thing group the deployment targets, which is how IoT Greengrass names a
/// deployment whose creator named none.
pub const UNNAMED_DEPLOYMENT_PREFIX: &str = "Deployment for";

/// The refusal every core-device-keyed operation answers a thing name this service holds
/// no core device for.
///
/// Stated once for the whole surface: `GetCoreDevice` and `DeleteCoreDevice` refuse an
/// unreported thing name identically, so the two cannot drift apart.
pub fn core_device_not_found(core_device_thing_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("{core_device_thing_name} {CORE_DEVICE_NOT_FOUND_SUFFIX}"),
        resource_id: core_device_thing_name.to_string(),
        resource_type: NamedResource::CoreDevice {
            core_device_thing_name: core_device_thing_name.to_string(),
        }
        .resource_type()
        .to_string(),
    }
}

/// The IoT thing name a deployment's `targetArn` names, or `None` when the target is a
/// thing GROUP rather than one thing.
///
/// `TargetARN`'s pattern is
/// `^arn:[^:]*:iot:[^:]*:[0-9]+:(thing|thinggroup)/.+$`
/// (`smithy_query(query="shape",shape="TargetARN")`), so the resource field is one
/// colon-free segment and a plain `:` split recovers it.
pub fn target_thing_name(target_arn: &str) -> Option<&str> {
    let fields: Vec<&str> = target_arn.split(':').collect();
    match fields.as_slice() {
        ["arn", _partition, "iot", _region, _account, resource] => resource
            .strip_prefix("thing/")
            .filter(|name| !name.is_empty()),
        _ => None,
    }
}

/// The name of the thing or thing group a deployment's `targetArn` names, which is what
/// an unnamed deployment is called after [`UNNAMED_DEPLOYMENT_PREFIX`].
pub fn target_resource_name(target_arn: &str) -> &str {
    target_arn
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(target_arn)
}

/// The `EffectiveDeploymentExecutionStatus` of one deployment on one core device.
///
/// A deployment is applied to a device by the AWS IoT job the deployment reported, so the
/// status of that job on the device is what this reports: a job whose rollout was
/// cancelled reaches the device no further ([`EXECUTION_STATUS_CANCELED`]), a finished one
/// is [`EXECUTION_STATUS_COMPLETED`], and one that is still rolling out is
/// [`EXECUTION_STATUS_QUEUED`] — the status of a deployment job that has reached the
/// device, which has reported nothing about it. `job_status` is `None` once the job is no
/// longer in the jobs plane (deleted there), and then the deployment's own recorded status
/// answers instead, so the two cannot disagree.
pub fn effective_execution_status(
    job_status: Option<&str>,
    deployment_status: &str,
) -> &'static str {
    let reported = job_status.unwrap_or(deployment_status);
    if reported == ::service_common::iot_jobs::JOB_STATUS_CANCELED
        || reported == DEPLOYMENT_STATUS_CANCELED
    {
        EXECUTION_STATUS_CANCELED
    } else if reported == ::service_common::iot_jobs::JOB_STATUS_COMPLETED {
        EXECUTION_STATUS_COMPLETED
    } else {
        EXECUTION_STATUS_QUEUED
    }
}

/// What a deployment is called where `EffectiveDeployment$deploymentName` requires a name:
/// the name its creator gave it, or — for a deployment created without one —
/// [`UNNAMED_DEPLOYMENT_PREFIX`] and the thing or thing group it targets.
pub fn effective_deployment_name(deployment: &DeploymentEntry) -> String {
    deployment.deployment_name.clone().unwrap_or_else(|| {
        format!(
            "{UNNAMED_DEPLOYMENT_PREFIX} {}",
            target_resource_name(&deployment.target_arn)
        )
    })
}

/// One component a core device reported as installed, with everything
/// `ListInstalledComponents` reports about it.
#[derive(Debug, Clone)]
pub struct InstalledComponentRecord {
    pub component_name: String,
    pub component_version: String,
    /// An `InstalledComponentLifecycleState` value, as the device reported it.
    pub lifecycle_state: String,
    pub lifecycle_state_details: Option<String>,
    /// Whether the device runs this component because a deployment named it (a ROOT
    /// component) rather than because another component depends on it.
    pub is_root: bool,
    /// When the component's lifecycle state last changed, as the device reported it.
    pub last_status_change: SystemTime,
    /// When the device last reported this component.
    pub last_reported: SystemTime,
    pub last_installation_source: Option<String>,
    pub lifecycle_status_codes: Vec<String>,
}

impl InstalledComponentRecord {
    /// A component a device reports as running, with the timestamps of that report.
    #[must_use]
    pub fn reported(
        component_name: &str,
        component_version: &str,
        lifecycle_state: &str,
        is_root: bool,
        at: SystemTime,
    ) -> Self {
        Self {
            component_name: component_name.to_string(),
            component_version: component_version.to_string(),
            lifecycle_state: lifecycle_state.to_string(),
            lifecycle_state_details: None,
            is_root,
            last_status_change: at,
            last_reported: at,
            last_installation_source: None,
            lifecycle_status_codes: Vec::new(),
        }
    }
}

/// One Greengrass core device, as the device's own status update describes it.
///
/// The members AWS reports only once a device has reported them (`coreVersion`,
/// `platform`, `architecture`, `runtime`) are optional here for that reason: a device
/// that reported no platform has none reported back, rather than one being invented for
/// it.
#[derive(Debug, Clone)]
pub struct CoreDeviceEntry {
    /// The device's thing name, which is also its key in the fleet.
    pub core_device_thing_name: String,
    /// A `CoreDeviceStatus` value, as the device's last status update reported it.
    pub status: String,
    /// When that status update arrived — `lastStatusUpdateTimestamp`.
    pub last_status_update: SystemTime,
    /// The version of the IoT Greengrass Core software the device runs.
    pub core_version: Option<String>,
    pub platform: Option<String>,
    pub architecture: Option<String>,
    pub runtime: Option<String>,
    /// The thing groups whose deployments IoT Greengrass applies to this device. They are
    /// what `ListCoreDevices`'s `thingGroupArn` filter selects on — "the list includes
    /// only core devices that have successfully deployed a deployment that targets the
    /// thing group" — and what makes a group-targeted deployment effective on it.
    pub thing_group_arns: BTreeSet<String>,
    /// The components the device reported as installed, keyed by component name: a device
    /// runs one version of a component at a time, so the name is the key, and the key
    /// order is the order the listing pages in.
    pub installed_components: BTreeMap<String, InstalledComponentRecord>,
    /// The tag set written on this device's ARN through the tagging route.
    pub tags: BTreeMap<String, String>,
}

impl CoreDeviceEntry {
    /// A core device as a status update reports one: the thing name, the status it
    /// reported and the instant it reported.
    #[must_use]
    pub fn reported(core_device_thing_name: &str, status: &str, at: SystemTime) -> Self {
        Self {
            core_device_thing_name: core_device_thing_name.to_string(),
            status: status.to_string(),
            last_status_update: at,
            core_version: None,
            platform: None,
            architecture: None,
            runtime: None,
            thing_group_arns: BTreeSet::new(),
            installed_components: BTreeMap::new(),
            tags: BTreeMap::new(),
        }
    }

    /// The components this device reported, under `topology_filter`: `ROOT` reports only
    /// the components a deployment named, `ALL` reports every one the device installed.
    pub fn components_under(
        &self,
        topology_filter: &str,
    ) -> impl Iterator<Item = &InstalledComponentRecord> {
        let root_only = topology_filter != TOPOLOGY_FILTER_ALL;
        self.installed_components
            .values()
            .filter(move |component| !root_only || component.is_root)
    }
}

/// Why one entry of a client-device batch was not applied.
///
/// A batch operation reports a failed entry in its `errorEntries` list rather than
/// failing the request, so each variant is the per-entry error ONE entry carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssociationRefusal {
    /// This core device already holds [`MAX_CLIENT_DEVICES_PER_CORE_DEVICE`] client
    /// devices.
    CoreDeviceFull,
    /// The association store already holds a set for
    /// [`MAX_CORE_DEVICES_WITH_ASSOCIATIONS`] core device thing names.
    StoreFull,
}

impl AssociationRefusal {
    /// The `code` the failed entry carries: the error class a quota breach belongs to.
    #[must_use]
    pub fn code(&self) -> &'static str {
        "ServiceQuotaExceededException"
    }
}

impl std::fmt::Display for AssociationRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CoreDeviceFull => write!(
                formatter,
                "the core device already has the maximum of \
                 {MAX_CLIENT_DEVICES_PER_CORE_DEVICE} associated client devices"
            ),
            Self::StoreFull => write!(
                formatter,
                "client devices are already associated with the maximum of \
                 {MAX_CORE_DEVICES_WITH_ASSOCIATIONS} core devices"
            ),
        }
    }
}

impl ComAmazonawsGreengrassv2Greengrassv2State {
    /// The core device `core_device_thing_name` names, or `None` when no device has
    /// reported itself under that name.
    #[must_use]
    pub fn core_device(&self, core_device_thing_name: &str) -> Option<&CoreDeviceEntry> {
        self.core_devices.get(core_device_thing_name)
    }

    /// The core device `core_device_thing_name` names, for a writer.
    pub fn core_device_mut(
        &mut self,
        core_device_thing_name: &str,
    ) -> Option<&mut CoreDeviceEntry> {
        self.core_devices.get_mut(core_device_thing_name)
    }

    /// Record a core device's status update, reporting whether it was stored.
    ///
    /// A report REPLACES whatever the device reported before — it is that device's
    /// current state, not an addition to a history — and keeps the tag set already
    /// written on the device, which belongs to the resource rather than to the report.
    /// A report for a thing name the fleet does not already hold is refused once the
    /// fleet is at [`MAX_CORE_DEVICES`], so the store is bounded by a count a caller
    /// cannot raise.
    pub fn record_core_device(&mut self, mut entry: CoreDeviceEntry) -> bool {
        if entry.installed_components.len() > MAX_INSTALLED_COMPONENTS_PER_CORE_DEVICE {
            return false;
        }
        match self.core_devices.get_mut(&entry.core_device_thing_name) {
            Some(stored) => {
                entry.tags = std::mem::take(&mut stored.tags);
                *stored = entry;
                true
            }
            None => {
                if self.core_devices.len() >= MAX_CORE_DEVICES {
                    return false;
                }
                self.core_devices
                    .insert(entry.core_device_thing_name.clone(), entry);
                true
            }
        }
    }

    /// Remove a core device from the fleet, reporting whether it was there.
    ///
    /// The client devices associated with that thing name are NOT removed with it: an
    /// association is a record for the thing name, written and removed by the batch
    /// operations, and AWS keeps one for a name no core device has reported under.
    pub fn forget_core_device(&mut self, core_device_thing_name: &str) -> bool {
        self.core_devices.remove(core_device_thing_name).is_some()
    }

    /// The client devices associated with `core_device_thing_name` and the instant each
    /// was associated, or `None` when that name has none.
    #[must_use]
    pub fn associated_client_devices(
        &self,
        core_device_thing_name: &str,
    ) -> Option<&BTreeMap<String, SystemTime>> {
        self.client_device_associations.get(core_device_thing_name)
    }

    /// Associate one client device with one core device, within both bounds.
    ///
    /// Associating a client device that is already associated is not a failure and adds
    /// nothing: the association already says what the request asks for, so the instant it
    /// was first associated at stands.
    pub fn associate_client_device(
        &mut self,
        core_device_thing_name: &str,
        client_device_thing_name: &str,
        now: SystemTime,
    ) -> Result<(), AssociationRefusal> {
        let devices = match self
            .client_device_associations
            .get_mut(core_device_thing_name)
        {
            Some(devices) => devices,
            None => {
                if self.client_device_associations.len() >= MAX_CORE_DEVICES_WITH_ASSOCIATIONS {
                    return Err(AssociationRefusal::StoreFull);
                }
                self.client_device_associations
                    .entry(core_device_thing_name.to_string())
                    .or_default()
            }
        };
        if devices.contains_key(client_device_thing_name) {
            return Ok(());
        }
        if devices.len() >= MAX_CLIENT_DEVICES_PER_CORE_DEVICE {
            return Err(AssociationRefusal::CoreDeviceFull);
        }
        devices.insert(client_device_thing_name.to_string(), now);
        Ok(())
    }

    /// Remove one client device's association with one core device, reporting whether it
    /// was there. A thing name whose last association is removed is forgotten, so the
    /// store holds no empty sets.
    pub fn disassociate_client_device(
        &mut self,
        core_device_thing_name: &str,
        client_device_thing_name: &str,
    ) -> bool {
        let Some(devices) = self
            .client_device_associations
            .get_mut(core_device_thing_name)
        else {
            return false;
        };
        let removed = devices.remove(client_device_thing_name).is_some();
        if devices.is_empty() {
            self.client_device_associations.remove(core_device_thing_name);
        }
        removed
    }

    /// The deployments IoT Greengrass sends to the core device `core_device_thing_name`,
    /// in a stable order.
    ///
    /// A deployment is effective on a device when it targets THAT THING, or when it
    /// targets one of the thing groups whose deployments the device applies
    /// ([`CoreDeviceEntry::thing_group_arns`]) — "when you add a new core device to a
    /// group of core devices that has a deployment, IoT Greengrass deploys that group's
    /// deployment to the new device" (`CreateDeployment`'s own documentation). Nothing is
    /// stored per device: the answer is computed from the deployments this service holds,
    /// so a deployment created, cancelled or evicted is reflected here with no second
    /// copy to disagree with.
    #[must_use]
    pub fn deployments_effective_for(&self, core_device_thing_name: &str) -> Vec<&DeploymentEntry> {
        let groups = self
            .core_device(core_device_thing_name)
            .map(|device| &device.thing_group_arns);
        self.deployments
            .values()
            .filter(|deployment| {
                target_thing_name(&deployment.target_arn) == Some(core_device_thing_name)
                    || groups.is_some_and(|groups| groups.contains(&deployment.target_arn))
            })
            .collect()
    }
}
