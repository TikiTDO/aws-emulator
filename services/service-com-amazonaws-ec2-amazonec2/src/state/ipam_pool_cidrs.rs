// SPDX-License-Identifier: CC0-1.0

//! The CIDRs provisioned to an IPAM pool: the store `ProvisionIpamPoolCidr` writes,
//! `GetIpamPoolCidrs` reports and `DeprovisionIpamPoolCidr` takes back.
//!
//! "A CIDR provisioned to an IPAM pool" ([`IpamPoolCidr`]) is a resource of its own — it
//! carries an id, a state and the netmask length it was provisioned with — so it is held in
//! a store of its own, keyed by that id, rather than inside the pool's modelled shape, which
//! has no member for it. The pool it belongs to is held beside the view for the same reason.
//!
//! This is also the store "you cannot delete an IPAM pool if there are allocations in it or
//! CIDRs provisioned to it" (`DeleteIpamPool`) is asked of: every reader of "what is
//! provisioned to this pool" goes through
//! [`ComAmazonawsEc2Amazonec2State::ipam_pool_provisioned_cidrs`], which answers these
//! records beside the BYOIP ranges `MoveByoipCidrToIpam` put in the pool, so one pool cannot
//! be judged empty by one path and occupied by another.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// The id prefix an IPAM pool CIDR is minted with, which is also how its own ARN-free id
/// reads on the wire (`IpamPoolCidr$IpamPoolCidrId`).
pub const IPAM_POOL_CIDR_ID_PREFIX: &str = "ipam-pool-cidr";

/// A CIDR whose provisioning has been asked for and has not settled yet: the
/// `IpamPoolCidrState` enum's `pending-provision` member
/// (`smithy_query(query="shape",shape="IpamPoolCidrState")`), which is what
/// `ProvisionIpamPoolCidr` answers with.
pub const IPAM_POOL_CIDR_STATE_PENDING_PROVISION: &str = "pending-provision";

/// A CIDR whose provisioning has settled: the `IpamPoolCidrState` enum's `provisioned`
/// member, which the clock owes an [`IPAM_POOL_CIDR_STATE_PENDING_PROVISION`] CIDR one
/// [`TRANSITION_DELAY`] after the call that provisioned it.
pub const IPAM_POOL_CIDR_STATE_PROVISIONED: &str = "provisioned";

/// A CIDR whose deprovisioning has been asked for and has not settled yet: the
/// `IpamPoolCidrState` enum's `pending-deprovision` member, which
/// `DeprovisionIpamPoolCidr` answers with.
pub const IPAM_POOL_CIDR_STATE_PENDING_DEPROVISION: &str = "pending-deprovision";

/// A CIDR whose deprovisioning has settled: the `IpamPoolCidrState` enum's `deprovisioned`
/// member. The record is REMOVED at that same instant
/// ([`ComAmazonawsEc2Amazonec2State::materialize_due_ipam_pool_cidr_moves`]) — the range is
/// no longer provisioned to the pool, so the pool reports it no more — which is why this
/// state is never one a reader sees stored.
pub const IPAM_POOL_CIDR_STATE_DEPROVISIONED: &str = "deprovisioned";

/// One CIDR provisioned to one IPAM pool: the modelled [`IpamPoolCidr`] every operation that
/// reports one answers with, the pool it belongs to, and the state move the clock still owes
/// it.
///
/// The pool id and the move are held beside the view rather than on it for the reason the
/// module docs give: the modelled shape carries neither.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct IpamPoolCidrRecord {
    /// The `IpamPoolId` of the pool this CIDR is provisioned to.
    pub ipam_pool_id: String,
    /// The CIDR as every reader of it reports it.
    pub view: IpamPoolCidr,
    /// The state the clock owes this CIDR, if it owes one — `provisioned` after a
    /// provision, `deprovisioned` after a deprovision.
    pub pending: Option<PendingStateMove>,
}

/// Whether a CIDR in `state` still holds space in its pool — whether it is one of the
/// "CIDRs provisioned to it" a `DeleteIpamPool` is refused for.
///
/// A range whose deprovisioning the caller has ALREADY asked for is not: the caller has
/// taken it back, and "to deprovision pool CIDRs, see DeprovisionIpamPoolCidr"
/// (`DeleteIpamPool`) is exactly the step that asking performs, so the pool no longer counts
/// it against a delete. Everything else does.
pub fn ipam_pool_cidr_holds_space(state: Option<&str>) -> bool {
    !matches!(
        state,
        Some(IPAM_POOL_CIDR_STATE_PENDING_DEPROVISION) | Some(IPAM_POOL_CIDR_STATE_DEPROVISIONED)
    )
}

/// The caller-chosen bytes one stored pool CIDR holds: the range itself, which is the only
/// text the caller chose.
///
/// The fixed [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs is charged on top by
/// the writer, exactly as [`ipam_policy_payload_bytes`] is charged for a policy.
pub fn ipam_pool_cidr_payload_bytes(view: &IpamPoolCidr) -> usize {
    STORED_RECORD_OVERHEAD_BYTES + view.cidr.as_deref().map_or(0, str::len)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another IPAM pool CIDR fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]) — the same contract as
    /// [`Self::has_ipam_pool_capacity`]: a writer asks first and refuses when it answers
    /// `false`, so repeated provisions cannot grow memory without bound.
    pub fn has_ipam_pool_cidr_capacity(&self) -> bool {
        self.ipam_pool_cidrs.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// The CIDRs provisioned to the pool `pool_id` names, as the records the store holds,
    /// in the one order every reader of them reports: the order of their ids.
    pub fn ipam_pool_cidr_records(&self, pool_id: &str) -> Vec<&IpamPoolCidrRecord> {
        self.ipam_pool_cidrs
            .values()
            .filter(|record| record.ipam_pool_id == pool_id)
            .collect()
    }

    /// The record the pool `pool_id` holds for the range `cidr`, if it holds one.
    ///
    /// One pool holds at most one record per range — a second provision of the same range
    /// would say nothing the first does not — which is what makes this the lookup
    /// `DeprovisionIpamPoolCidr` resolves its `Cidr` through.
    pub fn ipam_pool_cidr(&self, pool_id: &str, cidr: &str) -> Option<&IpamPoolCidrRecord> {
        self.ipam_pool_cidrs.values().find(|record| {
            record.ipam_pool_id == pool_id && record.view.cidr.as_deref() == Some(cidr)
        })
    }

    /// Provision `cidr` to the pool `pool_id` names and answer the CIDR as the store then
    /// holds it — `None` when the store is at its ceiling and NOTHING was stored.
    ///
    /// The ONE way a pool CIDR is stored, so the members the SERVICE owns cannot be built two
    /// ways: the id and the `pending-provision` state it starts in are minted here, the way
    /// [`Self::insert_creating_ipam_pool`] mints a pool's, and the netmask length is the
    /// range's own rather than a caller's claim about it. The clock owes the CIDR
    /// `provisioned` one [`TRANSITION_DELAY`] later, which is RECORDED rather than slept for,
    /// so a caller acting straight after the call still sees the transient state.
    pub fn provision_ipam_pool_cidr(
        &mut self,
        pool_id: &str,
        cidr: &str,
        netmask_length: Option<i32>,
    ) -> Option<IpamPoolCidr> {
        let view = IpamPoolCidr {
            cidr: Some(cidr.to_string()),
            state: Some(IPAM_POOL_CIDR_STATE_PENDING_PROVISION.to_string()),
            // "Details related to why an IPAM pool CIDR failed to be provisioned"
            // (`IpamPoolCidr$FailureReason`): nothing has failed, so there is nothing to
            // report about it.
            failure_reason: None,
            ipam_pool_cidr_id: None,
            netmask_length,
        };
        let charge = ipam_pool_cidr_payload_bytes(&view);
        if !self.has_ipam_pool_cidr_capacity() || self.would_exceed_stored_bytes(charge) {
            return None;
        }
        let cidr_id = self.next_id(IPAM_POOL_CIDR_ID_PREFIX);
        let mut view = view;
        view.ipam_pool_cidr_id = Some(cidr_id.clone());
        self.add_stored_bytes(charge);
        self.ipam_pool_cidrs.insert(
            cidr_id,
            IpamPoolCidrRecord {
                ipam_pool_id: pool_id.to_string(),
                view: view.clone(),
                pending: Some(PendingStateMove::starting_now(
                    IPAM_POOL_CIDR_STATE_PROVISIONED,
                )),
            },
        );
        Some(view)
    }

    /// Start deprovisioning the range `cidr` of the pool `pool_id` names, and answer the CIDR
    /// as the store then holds it — `None` when the pool holds no such range.
    ///
    /// "Deprovision a CIDR provisioned from an IPAM pool" is work AWS reports as under way:
    /// the `IpamPoolCidrState` catalog carries both `pending-deprovision` and
    /// `deprovisioned`, so the record is left in the TRANSIENT state with the clock owing it
    /// the terminal one — the same arrangement, on the same clock, as the provision above.
    /// Reaching the terminal state REMOVES the record
    /// ([`Self::materialize_due_ipam_pool_cidr_moves`]), which is what gives the bytes it
    /// held back to the store's budget.
    ///
    /// A BYOIP address range IPAM manages is provisioned to the pool too
    /// (`ByoipCidr$IpamPoolId`), and it is the pool's record of that range: it stops being
    /// managed by this pool at once, the way `MoveByoipCidrToIpam` put it in at once, and the
    /// range itself is left exactly as it was — it is once again a range the account holds
    /// outside IPAM.
    pub fn deprovision_ipam_pool_cidr(
        &mut self,
        pool_id: &str,
        cidr: &str,
    ) -> Option<IpamPoolCidr> {
        let held = self
            .ipam_pool_cidr(pool_id, cidr)
            .and_then(|record| record.view.ipam_pool_cidr_id.clone());
        if let Some(cidr_id) = held {
            let record = self.ipam_pool_cidrs.get_mut(&cidr_id)?;
            record.view.state = Some(IPAM_POOL_CIDR_STATE_PENDING_DEPROVISION.to_string());
            record.pending = Some(PendingStateMove::starting_now(
                IPAM_POOL_CIDR_STATE_DEPROVISIONED,
            ));
            return Some(record.view.clone());
        }
        let byoip = self.byoip_cidrs.get_mut(cidr)?;
        if byoip.byoip_cidr.ipam_pool_id.as_deref() != Some(pool_id) {
            return None;
        }
        byoip.byoip_cidr.ipam_pool_id = None;
        byoip.byoip_cidr.pool_id = None;
        Some(IpamPoolCidr {
            cidr: Some(cidr.to_string()),
            state: Some(IPAM_POOL_CIDR_STATE_DEPROVISIONED.to_string()),
            failure_reason: None,
            // A BYOIP range is held by its range rather than by an id of this store, so
            // there is no `IpamPoolCidrId` to report for it.
            ipam_pool_cidr_id: None,
            netmask_length: cidr_block_prefix_length(cidr).and_then(|length| i32::try_from(length).ok()),
        })
    }

    /// Remove every CIDR record of the pool `pool_id` names, giving the bytes they held back
    /// to the store's budget.
    ///
    /// A record names its pool for as long as it is stored, so a pool that LEAVES the store
    /// takes its records with it rather than leaving rows naming a pool nothing resolves.
    pub(super) fn remove_ipam_pool_cidrs(&mut self, pool_id: &str) {
        let doomed: Vec<String> = self
            .ipam_pool_cidrs
            .iter()
            .filter(|(_, record)| record.ipam_pool_id == pool_id)
            .map(|(cidr_id, _)| cidr_id.clone())
            .collect();
        for cidr_id in doomed {
            if let Some(record) = self.ipam_pool_cidrs.remove(&cidr_id) {
                self.release_stored_bytes(ipam_pool_cidr_payload_bytes(&record.view));
            }
        }
    }

    /// Apply the state moves the clock owes the stored pool CIDRs, as of `now`.
    ///
    /// A `pending-provision` range is `provisioned` once its own instant has passed, and a
    /// `pending-deprovision` one has left the pool at its instant — both owed whether or not
    /// anybody called, which is why they are applied from
    /// [`Self::materialize_due_transitions`], its one caller, before anything reads the
    /// store.
    pub(super) fn materialize_due_ipam_pool_cidr_moves(&mut self, now: Instant) {
        let mut deprovisioned: Vec<String> = Vec::new();
        for (cidr_id, record) in &mut self.ipam_pool_cidrs {
            let Some(due) = record
                .pending
                .as_ref()
                .and_then(|pending| pending.due_by(now))
            else {
                continue;
            };
            if due == IPAM_POOL_CIDR_STATE_DEPROVISIONED {
                deprovisioned.push(cidr_id.clone());
                continue;
            }
            record.view.state = Some(due.to_string());
            record.pending = None;
        }
        for cidr_id in deprovisioned {
            if let Some(record) = self.ipam_pool_cidrs.remove(&cidr_id) {
                self.release_stored_bytes(ipam_pool_cidr_payload_bytes(&record.view));
            }
        }
    }
}
