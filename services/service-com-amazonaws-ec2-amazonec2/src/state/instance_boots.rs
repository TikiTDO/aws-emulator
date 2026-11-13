// SPDX-License-Identifier: CC0-1.0

//! The BOOTS of one instance's guest: every time that instance's operating system started.
//!
//! An instance's console is a device of its GUEST, and a guest writes to it once per boot:
//! "the instance console output displays the exact console output that would normally be
//! displayed on a physical monitor attached to a computer" (`GetConsoleOutput`). So the
//! console of an instance that has booted twice carries what BOTH boots wrote — the buffer
//! is appended to, never replaced — and each boot carries its own identity, which is how a
//! client tells a guest that restarted from one that never did.
//!
//! A boot is owed to the CLOCK, not to a call: an instance's operating system starts when
//! the instance starts running (its launch, and each time a stopped instance is started
//! again), and again when a queued reboot takes effect — `RebootInstances` "only queues a
//! request to reboot the specified instances", so the restart itself happens a moment
//! later, whether or not anybody calls again. [`InstanceRecord::boot_due`](
//! super::InstanceRecord::boot_due) is when the next one falls due and
//! [`ComAmazonawsEc2Amazonec2State::boot_due_instance_guests`] is what materialises it, on
//! the same clock-owed-transition arrangement as every other move of this crate
//! ([`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] calls it).
//!
//! A reboot leaves everything ELSE about the instance where it was: the instance keeps its
//! id, its launch time and its `running` state, because a reboot restarts the operating
//! system rather than the instance — which is precisely what makes the new boot's identity
//! the only observable difference.

use std::time::SystemTime;

use super::{iso8601_utc, ComAmazonawsEc2Amazonec2State, InstanceRecord, InstanceStateName};

/// One boot of one instance's guest.
///
/// Not a modelled shape: no EC2 operation reports a boot on its own. What a client reads it
/// through is the instance's CONSOLE ([`instance_boot_lines`]), which is where the boot
/// identity of a guest that restarted becomes observable.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct InstanceBoot {
    /// Which boot of this guest it is, counted from 1 at the instance's launch.
    pub number: u32,
    /// This boot's own identity: a fresh UUID per boot, minted by
    /// [`ComAmazonawsEc2Amazonec2State::mint_uuid_suffixed_id`] so it is drawn from the
    /// store's own counter and never from a request.
    pub boot_id: String,
    /// When the guest came up. STORED rather than re-derived, so two reads of one console
    /// answer the same bytes.
    pub booted_at: SystemTime,
}

/// How many of one guest's boots the console keeps.
///
/// A console is a fixed-size buffer — AWS serves the most recent output of one, not every
/// byte an instance ever wrote — so an instance that is rebooted without end keeps the most
/// recent boots and drops the oldest, which is what bounds the bytes one instance's boot
/// history can hold in this process.
pub const MAX_RETAINED_BOOTS: usize = 32;

/// The console label the boot NUMBER is reported under. See
/// [`super::CONSOLE_LABEL_WIDTH`] for the column the values line up at.
pub const CONSOLE_LABEL_BOOT: &str = "boot";

/// The console label this boot's own identity is reported under.
pub const CONSOLE_LABEL_BOOT_ID: &str = "boot-id";

/// The console label the instant the guest came up is reported under.
pub const CONSOLE_LABEL_BOOT_TIME: &str = "boot-time";

impl ComAmazonawsEc2Amazonec2State {
    /// Record one boot of the guest of the instance `instance_id` names, as of `at`.
    ///
    /// The ONE writer of a boot, so every path a guest can start by — a launch, a start of a
    /// stopped instance, a queued reboot taking effect — leaves the same record behind. It
    /// APPENDS: the boots an instance has already had are what its console still carries, up
    /// to [`MAX_RETAINED_BOOTS`], past which the oldest is dropped the way a console buffer
    /// drops what no longer fits.
    ///
    /// An id this store holds no instance for records nothing, and nothing else about the
    /// instance moves — a boot restarts the operating system, so the instance keeps its id,
    /// its launch time and the state it was in.
    pub fn record_instance_boot(&mut self, instance_id: &str, at: SystemTime) {
        if !self.instances.contains_key(instance_id) {
            return;
        }
        // Minted before the instance is borrowed, since the mint draws on the store's own
        // counter; the id above is already known to resolve, so no counter is spent for an
        // instance that is not there.
        let boot_id = self.mint_uuid_suffixed_id("");
        let Some(instance) = self.instances.get_mut(instance_id) else {
            return;
        };
        let number = instance
            .boots
            .last()
            .map_or(0, |boot| boot.number)
            .saturating_add(1);
        instance.boots.push(InstanceBoot {
            number,
            boot_id,
            booted_at: at,
        });
        if instance.boots.len() > MAX_RETAINED_BOOTS {
            instance.boots.remove(0);
        }
        // The boot that was owed has happened: nothing is owed until something starts the
        // guest again.
        instance.boot_due = None;
    }

    /// Materialise every guest boot the clock already owes at `now`.
    ///
    /// The one write reading a console can cause, and it only makes a change that was owed
    /// at `now` whether or not anybody called: an instance whose queued reboot fell due has
    /// restarted since, and one that has just finished starting has booted. A boot owed by
    /// an instance that is not `running` yet waits for it to get there — a guest cannot come
    /// up while the instance is still `pending` — and nothing at all is owed by an instance
    /// that is stopped or gone.
    ///
    /// Each boot is stamped with the instant it FELL DUE rather than with this call, so the
    /// same history always renders the same console text.
    pub(super) fn boot_due_instance_guests(&mut self, now: SystemTime) {
        let due: Vec<(String, SystemTime)> = self
            .instances
            .values()
            .filter(|instance| instance.state == InstanceStateName::Running)
            .filter_map(|instance| {
                let at = instance.boot_due.filter(|due| now >= *due)?;
                Some((instance.instance_id.clone(), at))
            })
            .collect();
        for (instance_id, at) in due {
            self.record_instance_boot(&instance_id, at);
        }
    }

    /// Reboot the guest of the instance `instance_id` names, as of `at`, answering whether
    /// there was a guest to reboot.
    ///
    /// The form a reboot takes for a caller that has no request of its own to refuse — the
    /// FIS action `aws:ec2:reboot-instances`, which "reboot[s] the specified EC2 instances"
    /// as the service that OWNS them rather than as a client of them. It is the same write
    /// `RebootInstances` performs ([`Self::queue_instance_reboot`]), held to the same two
    /// readings that handler takes of the instance's state: "requests to reboot terminated
    /// instances are ignored", and an instance on its way up (`pending`) is one the clock is
    /// about to make `running`, so its guest is rebooted like any other.
    ///
    /// `false` is "there was nothing to reboot": this store holds no such instance, or the
    /// instance has no running operating system to restart. Nothing is written for it, so
    /// nothing claims a reboot that did not happen.
    pub fn reboot_instance_guest(&mut self, instance_id: &str, at: SystemTime) -> bool {
        let Some(instance) = self.instances.get(instance_id) else {
            return false;
        };
        if !matches!(
            instance.state,
            InstanceStateName::Running | InstanceStateName::Pending
        ) {
            return false;
        }
        self.queue_instance_reboot(instance_id, at);
        true
    }
}

/// The console lines one guest BOOT shows: which boot it is, its own identity and when it
/// came up, labelled on the same column as every other console line.
///
/// Appended to the instance's own facts by [`super::instance_console_lines`], one block per
/// boot in the order the guest had them, so a client reading the console of an instance that
/// restarted finds the earlier boot's lines unchanged and the new boot's after them.
pub fn instance_boot_lines(boot: &InstanceBoot) -> Vec<(String, String)> {
    vec![
        (CONSOLE_LABEL_BOOT.to_string(), boot.number.to_string()),
        (CONSOLE_LABEL_BOOT_ID.to_string(), boot.boot_id.clone()),
        (
            CONSOLE_LABEL_BOOT_TIME.to_string(),
            iso8601_utc(boot.booted_at),
        ),
    ]
}

/// The boots the instance `instance` has had, oldest first — what its console carries.
pub fn instance_boots(instance: &InstanceRecord) -> &[InstanceBoot] {
    &instance.boots
}
