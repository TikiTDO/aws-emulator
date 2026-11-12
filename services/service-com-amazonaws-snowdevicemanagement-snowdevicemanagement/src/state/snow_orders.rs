// SPDX-License-Identifier: CC0-1.0

//! The managed-device inventory as a projection of this account's Snow Family ORDERS.
//!
//! Snow Device Management models no device-creation operation, because a device does not
//! become manageable by being created here: it becomes manageable by being ORDERED. A
//! Snow Family job created through the Snowball API with `RemoteManagement` installed
//! ships a device that "will automatically be available when the device arrives at your
//! location" (`CreateJobRequest$RemoteManagement`), and `DeviceSummary$associatedWithJob`
//! / `ListDevicesInput$jobId` are "the ID of the job used to order the device" — the
//! order's own `JobId`.
//!
//! So the inventory is not minted here. It is read off the component that OWNS those
//! orders — the Snowball emulator's job store, reached through the one shared engine —
//! and materialised into this service's own records
//! ([`ComAmazonawsSnowdevicemanagementSnowdevicemanagementState::activate_ordered_devices`]),
//! which is what every device operation then reports from. The projection is a function
//! of the order store alone, so it answers the same whether a caller asked a minute ago
//! or never: materialising it is owed by the orders that were already placed, not by the
//! call that happens to observe it, exactly as settling the clock is.
//!
//! WHICH ORDERS SHIP A MANAGED DEVICE, and so what the projection covers:
//!
//! * the order installed remote management ([`REMOTELY_MANAGED`]) — with
//!   `NOT_INSTALLED`, "remote management will not be available on the device", and an
//!   order that named nothing installed none;
//! * the order has a device of its own: a main job has none ("Main jobs aren't associated
//!   with any Snowballs", `JobListEntry$IsMaster`), while each of its job parts and each
//!   node job of a cluster does;
//! * the order still HOLDS that device (`JobState::holds_device`): a cancelled order
//!   never had an appliance prepared for it, and a completed one has its device back at
//!   Amazon Web Services.
//!
//! The Region a projected device is managed in is the Region its order was placed in
//! (`StoredJob::order_region`), because this service reports "all devices on your Amazon
//! Web Services account that have Amazon Web Services Snow Device Management enabled in
//! the Amazon Web Services Region where the command is run".

use std::collections::{BTreeMap, HashSet};
use std::time::SystemTime;

use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice as snow_family_orders;
use snow_family_orders::state::StoredJob;

use super::ComAmazonawsSnowdevicemanagementSnowdevicemanagementState;

/// The `RemoteManagement` values that put Snow Device Management on the ordered device.
///
/// The member's three values are `INSTALLED_AUTOSTART`, `INSTALLED_ONLY` and
/// `NOT_INSTALLED` (`smithy_query(query="shape", shape="RemoteManagement",
/// model_path="services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice\
/// /smithy-model/com-amazonaws-snowball-awsiesnowballjobmanagementservice.json")`), and
/// the member is documented as "Allows you to securely operate and manage Snowcone
/// devices remotely from outside of your internal network. When set to
/// `INSTALLED_AUTOSTART`, remote management will automatically be available when the
/// device arrives at your location. Otherwise, you need to use the Snowball Edge client
/// to manage the device. When set to `NOT_INSTALLED`, remote management will not be
/// available on the device." So the two `INSTALLED_` values install it and
/// `NOT_INSTALLED` does not; an order that named no value installed nothing either.
pub const REMOTELY_MANAGED: &[&str] = &["INSTALLED_AUTOSTART", "INSTALLED_ONLY"];

/// One device an order ships, as the inventory records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderedDevice {
    /// The `JobId` of the order that ships it — `DeviceSummary$associatedWithJob`, "the
    /// ID of the job used to order the device", and what `ListDevices?jobId=` narrows on.
    pub job_id: String,
    /// The Region the order was placed in, which is the Region the device is managed in.
    pub region: String,
    /// The `SnowballType` the order was placed for, which is "the type of Amazon Web
    /// Services Snow Family device" `DescribeDevice` reports, and `None` for an order
    /// that named none, since this emulator reports no device type it was not told.
    pub device_type: Option<String>,
    /// When the order was placed, which is when its device became manageable. Read off
    /// the order rather than off the call that materialises the device, so every member
    /// of the record is a function of the orders alone.
    pub ordered_at: SystemTime,
}

/// What this account's Snow Family orders say the inventory holds, read in one pass over
/// the order store.
#[derive(Debug, Default, Clone)]
pub struct OrderedInventory {
    /// The devices the orders still hold, in the order the orders were placed, so the
    /// inventory's own ids follow order-creation order.
    pub devices: Vec<OrderedDevice>,
    /// The `JobId`s the order store holds that ship NO manageable device right now —
    /// never ordered with remote management, a main job, or an order that no longer
    /// holds its device. A device the inventory carries for one of them is a device this
    /// account can no longer manage.
    pub without_device: HashSet<String>,
}

/// Whether `job` is an order that ships a device this account manages remotely, per the
/// three readings in this module's own documentation.
fn ships_managed_device(job: &StoredJob) -> bool {
    job.remote_management
        .as_deref()
        .is_some_and(|installed| REMOTELY_MANAGED.contains(&installed))
        && !job.is_master()
        && job.job_state.holds_device()
}

/// This account's Snow Family orders as the inventory reads them, or `None` when the
/// order store cannot be reached at all (so no order can be claimed either way).
///
/// The order store's lock is taken for this read alone and released before the inventory
/// is written, and the orders service never reads THIS store, so the two are never held
/// in a cycle.
pub fn ordered_inventory<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<OrderedInventory> {
    let orders = snow_family_orders::state::service_state(engine)?;
    let orders = orders.lock();
    let mut inventory = OrderedInventory::default();
    for job_id in &orders.job_order {
        let Some(job) = orders.jobs.get(job_id) else {
            continue;
        };
        if ships_managed_device(job) {
            inventory.devices.push(OrderedDevice {
                job_id: job.job_id.clone(),
                region: job.order_region.clone(),
                device_type: job.snowball_type.clone(),
                ordered_at: job.creation_date,
            });
        } else {
            inventory.without_device.insert(job.job_id.clone());
        }
    }
    Some(inventory)
}

impl ComAmazonawsSnowdevicemanagementSnowdevicemanagementState {
    /// Bring the inventory up to what this account's Snow Family orders ship: every
    /// order that ships a managed device has one device record here, and a device whose
    /// order no longer ships one is gone.
    ///
    /// Called by every operation before it reads a device, so a device is manageable from
    /// the moment its order exists — the only path a client has, since this API models no
    /// device-creation operation. It is NOT a mutation of this service's own resources:
    /// the result is a function of the order store, so what a later caller sees is the
    /// same whether or not an earlier one ran this.
    ///
    /// A device the orders say nothing about — one a suite seeded through
    /// [`Self::activate_devices`], or any device at all when the orders service is not
    /// mounted — is left exactly as it stands: the order store answers for the orders it
    /// holds and for nothing else.
    ///
    /// The inventory this can grow to is bounded by the order store itself, which holds a
    /// bounded number of jobs (`MAX_STORED_JOBS`) and refuses a create past it, so no
    /// number of calls here grows the process further than the orders already did.
    ///
    /// Nothing here reads the clock: every member of a projected device, the instant it
    /// became manageable included, comes from the order. The task and execution records
    /// of a device whose order stopped holding it are left alone — this API models no
    /// operation that deletes one, and the work a device did is the task's history, not
    /// the device's.
    pub fn activate_ordered_devices<Eng: Send + Sync + 'static>(&mut self, engine: &Eng) {
        let Some(inventory) = ordered_inventory(engine) else {
            return;
        };
        self.devices
            .retain(|_, device| !inventory.without_device.contains(&device.associated_with_job));
        let held: HashSet<&str> = self
            .devices
            .values()
            .map(|device| device.associated_with_job.as_str())
            .collect();
        let missing: Vec<OrderedDevice> = inventory
            .devices
            .into_iter()
            .filter(|ordered| !held.contains(ordered.job_id.as_str()))
            .collect();
        for ordered in missing {
            // A device that has just been ordered runs nothing: an Amazon EC2 instance
            // reaches a Snow device through the device's own EC2-compatible endpoint, and
            // neither this API nor the order models a launch, so its instance cache is
            // empty until something reports one.
            self.activate_device(
                &ordered.region,
                ordered.ordered_at,
                ordered.job_id,
                ordered.device_type,
                BTreeMap::new(),
            );
        }
    }
}
