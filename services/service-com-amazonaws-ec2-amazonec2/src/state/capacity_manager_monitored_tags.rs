// SPDX-License-Identifier: CC0-1.0

//! The TAG KEYS EC2 Capacity Manager monitors for the account.
//!
//! "Activates or deactivates tag keys for monitoring by EC2 Capacity Manager. Activated tag
//! keys are included as dimensions in capacity metric data"
//! (`UpdateCapacityManagerMonitoredTagKeys`), and the report of them answers "the list of tag
//! keys being monitored by Capacity Manager, including their current status and metadata"
//! (`GetCapacityManagerMonitoredTagKeysResult$CapacityManagerTagKeys`). So the account holds
//! ONE set of monitored tag keys, written only by the update and read by every operation that
//! answers along them — which is why both live here rather than in either handler.
//!
//! A monitored key's STATUS is a fact of elapsed time rather than of a stored word:
//! activation and deactivation are moves the clock owes, so a record keeps the instants they
//! fall due and [`MonitoredTagKeyRecord::status_at`] derives the `activating` ->
//! `activated` and `deactivating` -> "no longer monitored" readings from them. A report
//! therefore writes nothing: two reads at one instant answer the same statuses, and a key
//! whose deactivation has fallen due is simply not among the monitored keys any more,
//! whether or not anybody asked in between. The update is the one path that prunes those
//! records out of the store ([`ComAmazonawsEc2Amazonec2State::update_monitored_tag_keys`]).
//!
//! The fourth member of the modelled status catalog, `suspended`
//! ([`crate::ops::types::CAPACITY_MANAGER_MONITORED_TAG_KEY_STATUS_VALUES`]), names
//! monitoring that Capacity Manager itself stopped. Nothing in this emulator suspends a key,
//! so no record is ever put into it — the same reading this crate gives every other state no
//! work here can reach.

use super::*;

use crate::ops::types::CapacityManagerMonitoredTagKey;

/// A tag key whose monitoring has been asked for and has not started yet:
/// `CapacityManagerMonitoredTagKeyStatus`'s `activating` member.
pub const MONITORED_TAG_KEY_STATUS_ACTIVATING: &str = "activating";

/// A tag key Capacity Manager is monitoring: that catalog's `activated` member, the status
/// an activation reaches one [`TRANSITION_DELAY`] after the call that asked for it.
pub const MONITORED_TAG_KEY_STATUS_ACTIVATED: &str = "activated";

/// A tag key whose monitoring is being stopped: that catalog's `deactivating` member, the
/// status a deactivation holds until it falls due, after which the key is monitored no more
/// and is reported by nothing.
pub const MONITORED_TAG_KEY_STATUS_DEACTIVATING: &str = "deactivating";

/// The most tag keys the account may have monitored at once.
///
/// CHOSEN: neither the model nor the operation publishes a quota, and a monitored key is
/// caller-chosen text, so without a ceiling repeated updates would grow this process's
/// memory without bound. Together with the per-key bound every tag key of this service is
/// held to ([`MAX_TAG_KEY_CHARS`]) it is also what bounds the caller-chosen BYTES this set
/// holds, which is why no byte budget is charged for one.
pub const MAX_MONITORED_TAG_KEYS: usize = 128;

/// The most tag keys one update may name in one of its two lists.
///
/// `ValueStringList` carries no modelled `@length`, so the handler is the only place the
/// bound can exist: a request may name every key the account could monitor at once and no
/// more, since naming more keys than that could never leave the account inside
/// [`MAX_MONITORED_TAG_KEYS`].
pub const MAX_MONITORED_TAG_KEYS_PER_REQUEST: usize = MAX_MONITORED_TAG_KEYS;

/// One tag key the account monitors: the two instants the clock owes its moves at.
///
/// No status WORD is stored — the word a reader sees is derived from these instants
/// ([`Self::status_at`]) — so reporting the monitored keys never writes, and a key cannot be
/// left in a status the clock has already carried it out of.
#[derive(Debug, Clone)]
pub struct MonitoredTagKeyRecord {
    /// When this key's activation falls due: the instant it stops being `activating` and
    /// becomes `activated`, one [`TRANSITION_DELAY`] after the update that asked for it.
    pub(super) activation_due: Instant,
    /// When this key's deactivation falls due, and `None` while no update has asked for one:
    /// the instant it stops being `deactivating` and is monitored no more.
    pub(super) deactivation_due: Option<Instant>,
}

impl MonitoredTagKeyRecord {
    /// A key whose monitoring was asked for at `now`.
    pub(super) fn activating_at(now: Instant) -> Self {
        Self {
            activation_due: now + TRANSITION_DELAY,
            deactivation_due: None,
        }
    }

    /// The `CapacityManagerMonitoredTagKeyStatus` this key holds as of `now`, or `None` once
    /// its deactivation has fallen due — a key Capacity Manager no longer monitors, which
    /// the report of the monitored keys leaves out rather than reporting in a status that
    /// says it is still being worked on.
    pub fn status_at(&self, now: Instant) -> Option<&'static str> {
        match self.deactivation_due {
            Some(due) if now >= due => None,
            Some(_) => Some(MONITORED_TAG_KEY_STATUS_DEACTIVATING),
            None if now >= self.activation_due => Some(MONITORED_TAG_KEY_STATUS_ACTIVATED),
            None => Some(MONITORED_TAG_KEY_STATUS_ACTIVATING),
        }
    }
}

/// The modelled shape one monitored key is reported as, as of `now`, or `None` for a key
/// whose deactivation has fallen due.
///
/// The ONE projection of a stored key, so the update's own answer and the report of the
/// monitored keys cannot describe one key two ways.
pub fn monitored_tag_key_view(
    tag_key: &str,
    record: &MonitoredTagKeyRecord,
    now: Instant,
) -> Option<CapacityManagerMonitoredTagKey> {
    Some(CapacityManagerMonitoredTagKey {
        tag_key: Some(tag_key.to_string()),
        status: Some(record.status_at(now)?.to_string()),
        // "A message providing additional details about the current status of the monitored
        // tag key" (`StatusMessage`): an activation here carries no detail beyond the status
        // itself — nothing can fail or stall one — so the member is left off rather than
        // filled with a guess.
        status_message: None,
        // "Indicates whether this tag key is provided by Capacity Manager by default, rather
        // than being user-activated" (`CapacityManagerProvided`): every key in this set is
        // here because an update activated it, so none of them is one Capacity Manager
        // provided.
        capacity_manager_provided: Some(false),
        // "The earliest timestamp from which tag data is available for queries"
        // (`EarliestDatapointTimestamp`) — a bound on the DATAPOINTS Capacity Manager holds
        // for the key, and nothing in this emulator ingests one yet
        // (`GetCapacityManagerMetricData` is still a scaffold), which is the same reading
        // `GetCapacityManagerAttributes` gives the account's own datapoint bounds.
        earliest_datapoint_timestamp: None,
    })
}

impl ComAmazonawsEc2Amazonec2State {
    /// Every tag key the account monitors as of `now`, in tag-key order.
    ///
    /// REPORTING ONLY: each entry is derived from the stored record and the clock, so two
    /// reads at one instant answer the same thing and a read changes nothing a later caller
    /// sees. Tag-key order is the store's own, which is the stable order a report of them
    /// pages over.
    pub fn monitored_tag_keys_at(&self, now: Instant) -> Vec<CapacityManagerMonitoredTagKey> {
        self.capacity_manager_monitored_tag_keys
            .iter()
            .filter_map(|(tag_key, record)| monitored_tag_key_view(tag_key, record, now))
            .collect()
    }

    /// The status `tag_key` holds as of `now`, or `None` when the account does not monitor
    /// it — the one reading of "is this key monitored", so no operation can disagree with
    /// the report of the monitored keys about one key.
    pub fn monitored_tag_key_status(&self, tag_key: &str, now: Instant) -> Option<&'static str> {
        self.capacity_manager_monitored_tag_keys
            .get(tag_key)
            .and_then(|record| record.status_at(now))
    }

    /// How many tag keys the account monitors as of `now` — the count
    /// [`MAX_MONITORED_TAG_KEYS`] bounds, which counts only the keys a report answers.
    pub fn monitored_tag_key_count(&self, now: Instant) -> usize {
        self.capacity_manager_monitored_tag_keys
            .values()
            .filter(|record| record.status_at(now).is_some())
            .count()
    }

    /// Would the account still be inside [`MAX_MONITORED_TAG_KEYS`] after starting to
    /// monitor `added` further keys as of `now`?
    ///
    /// The ONE reading of that ceiling, which the update asks BEFORE it writes, so the
    /// refusal and the bound cannot drift apart. `added` counts the DISTINCT keys the account
    /// does not monitor yet: a key it already monitors, and a key named twice in one request,
    /// cost nothing, because the set keeps one record per key.
    pub fn has_monitored_tag_key_capacity_for(&self, added: usize, now: Instant) -> bool {
        self.monitored_tag_key_count(now).saturating_add(added) <= MAX_MONITORED_TAG_KEYS
    }

    /// Activate `activate` and deactivate `deactivate` for monitoring, and answer the keys
    /// the update AFFECTED as they now stand — "the list of tag keys affected by the update,
    /// including their current status and metadata"
    /// (`UpdateCapacityManagerMonitoredTagKeysResult$CapacityManagerTagKeys`).
    ///
    /// The ONE writer of the monitored set, so what a later report answers is always what
    /// this left here. The readings it applies, each one documented by the operation:
    ///
    /// * a key the account does not monitor starts `activating` and reaches `activated` one
    ///   [`TRANSITION_DELAY`] later, so a caller reading straight after the update still
    ///   sees the transient status;
    /// * a key already monitored is left exactly where it is, with any deactivation it was
    ///   part-way through CANCELLED — the account asked for that key to be monitored, which
    ///   it already is, so a repeat activation neither restarts a completed activation nor
    ///   stops the key being monitored;
    /// * a key named for deactivation starts `deactivating` and is monitored no more once
    ///   that falls due, while a key the account does not monitor is not affected at all and
    ///   so is not among the answered keys.
    ///
    /// Idempotent in both directions, which is what makes a repeated request — the same
    /// `ClientToken` presented twice — leave the account exactly where the first one did.
    ///
    /// The ceiling on how many keys the account may monitor
    /// ([`MAX_MONITORED_TAG_KEYS`]) is asked BEFORE this is called, through
    /// [`Self::has_monitored_tag_key_capacity_for`]: an update either applies whole or is
    /// refused whole, so the refusal belongs to the operation rather than to a writer that
    /// could only answer it by storing part of a request.
    pub fn update_monitored_tag_keys(
        &mut self,
        activate: &[String],
        deactivate: &[String],
        now: Instant,
    ) -> Vec<CapacityManagerMonitoredTagKey> {
        // The records the clock has already carried out of monitoring go first: they are no
        // longer reported, so keeping them would let a set the account cannot see fill the
        // ceiling below. This is the write path, so pruning here changes nothing any read
        // would have answered.
        self.capacity_manager_monitored_tag_keys
            .retain(|_, record| record.status_at(now).is_some());

        for tag_key in activate {
            match self
                .capacity_manager_monitored_tag_keys
                .get_mut(tag_key.as_str())
            {
                Some(record) => record.deactivation_due = None,
                None => {
                    self.capacity_manager_monitored_tag_keys
                        .insert(tag_key.clone(), MonitoredTagKeyRecord::activating_at(now));
                }
            }
        }
        for tag_key in deactivate {
            if let Some(record) = self
                .capacity_manager_monitored_tag_keys
                .get_mut(tag_key.as_str())
            {
                record.deactivation_due = Some(now + TRANSITION_DELAY);
            }
        }
        // Which keys the update affected: those it activated, and those it deactivated that
        // the account was monitoring. Answered OUT of the store, so the caller is shown what
        // a later report will show rather than what the request asked for.
        let affected: ::std::collections::BTreeSet<&String> = activate
            .iter()
            .chain(deactivate)
            .filter(|tag_key| {
                self.capacity_manager_monitored_tag_keys
                    .contains_key(tag_key.as_str())
            })
            .collect();
        affected
            .into_iter()
            .filter_map(|tag_key| {
                let record = self
                    .capacity_manager_monitored_tag_keys
                    .get(tag_key.as_str())?;
                monitored_tag_key_view(tag_key, record, now)
            })
            .collect()
    }
}
