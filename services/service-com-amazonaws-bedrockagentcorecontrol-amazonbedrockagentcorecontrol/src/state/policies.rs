// SPDX-License-Identifier: CC0-1.0

//! The AgentCore policy plane's state: one [`PolicyEngineRecord`] per
//! `policyEngineId`, each holding the policies created in it
//! ([`PolicyRecord`]), the ARNs both carry, the `CREATING` -> `ACTIVE`
//! transition the clock owes each, and the refusals a missing one is reported
//! with.
//!
//! The policies live inside the engine record rather than in a map of their own,
//! because a policy is addressed only THROUGH the engine that evaluates it
//! (`/policy-engines/{policyEngineId}/policies/{policyId}`) and is gone when the
//! engine is — the same reason a gateway target lives inside its gateway.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CreatePolicyEngineInput, CreatePolicyEngineOutput, CreatePolicyInput, CreatePolicyOutput,
    DeletePolicyOutput, GetPolicyEngineOutput, GetPolicyOutput, PolicyDefinition,
    ResourceNotFoundException,
};

use super::{sorted_map, timestamp};

/// The ARN resource type a policy engine carries (`PolicyEngineArn`'s
/// `…:policy-engine/{policyEngineId}`), and the one a policy nests under it
/// (`PolicyArn`'s `…:policy-engine/{policyEngineId}/policy/{policyId}`).
const ARN_TYPE_POLICY_ENGINE: &str = "policy-engine";
const ARN_TYPE_POLICY: &str = "policy";

/// `PolicyEngineStatus` / `PolicyStatus` members this service drives. Both
/// enums' full value list is `CREATING | ACTIVE | UPDATING | DELETING |
/// CREATE_FAILED | UPDATE_FAILED | DELETE_FAILED` (`smithy_query
/// shape=PolicyEngineStatus,PolicyStatus`).
pub const POLICY_STATUS_CREATING: &str = "CREATING";
pub const POLICY_STATUS_ACTIVE: &str = "ACTIVE";
pub const POLICY_STATUS_DELETING: &str = "DELETING";

/// How long a freshly created policy engine or policy stays `CREATING` before
/// the clock owes it `ACTIVE`, and how long a policy whose deletion was accepted
/// stays readable in `DELETING` before the clock owes its removal.
///
/// The creates and the delete are all asynchronous — "This is an asynchronous
/// operation. Use the GetPolicy operation to poll the status field to track
/// completion" (`smithy_query doc shape=CreatePolicy,DeletePolicy`) — and AWS
/// documents no duration for either transition, so the span is chosen: long
/// enough that a caller polling straight after the call still observes
/// `CREATING` or `DELETING`, short enough to keep a client script quick.
pub const POLICY_SETTLE_DELAY: Duration = Duration::from_secs(3);

/// How many policy engines this emulator will hold, and how many policies one
/// engine will hold.
///
/// `CreatePolicyEngine` and `CreatePolicy` both model
/// `ServiceQuotaExceededException` (HTTP 402) for exceeding the real service's
/// quota. Neither number is a fidelity claim about a published quota: each is
/// the emulator's own ceiling, set generously so no realistic workflow meets it,
/// so that a caller looping on create cannot grow this process's memory without
/// bound.
pub const MAX_POLICY_ENGINES: usize = 1_000;
pub const MAX_POLICIES_PER_POLICY_ENGINE: usize = 1_000;

/// The modelled not-found error for a policy engine this account does not hold:
/// the id the request's path carried, in the sentence this plane words it with
/// ("Policy engine with ID <policyEngineId> not found").
///
/// The text around the id is fixed; the id is the one the request named, so
/// every operation that resolves an engine reports a missing one identically.
pub fn policy_engine_not_found(policy_engine_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Policy engine with ID {policy_engine_id} not found"
        )),
    }
}

/// The modelled not-found error for a policy an engine of this account does not
/// hold, worded as [`policy_engine_not_found`] words a missing engine: the
/// resource and the id the request carried for it.
pub fn policy_not_found(policy_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Policy with ID {policy_id} not found")),
    }
}

/// `arn:aws:bedrock-agentcore:{region}:{account}:policy-engine/{policyEngineId}`
/// — `PolicyEngineArn`'s pattern.
pub fn policy_engine_arn(region: &str, policy_engine_id: &str) -> String {
    ::engine::emulator_arn(
        super::ARN_SERVICE,
        region,
        &format!("{ARN_TYPE_POLICY_ENGINE}/{policy_engine_id}"),
    )
}

/// `…:policy-engine/{policyEngineId}/policy/{policyId}` — `PolicyArn`'s pattern,
/// which nests the policy under the engine that evaluates it.
pub fn policy_arn(region: &str, policy_engine_id: &str, policy_id: &str) -> String {
    ::engine::emulator_arn(
        super::ARN_SERVICE,
        region,
        &format!("{ARN_TYPE_POLICY_ENGINE}/{policy_engine_id}/{ARN_TYPE_POLICY}/{policy_id}"),
    )
}

/// One policy engine: what `CreatePolicyEngine` stored, the policies created in
/// it, and the transition to `ACTIVE` the clock owes it.
#[derive(Debug, Clone)]
pub struct PolicyEngineRecord {
    pub policy_engine_id: String,
    pub policy_engine_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub encryption_key_arn: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The status until [`settles_at`](Self::settles_at) passes.
    pub transient_status: String,
    /// The status from [`settles_at`](Self::settles_at) onwards.
    pub settled_status: String,
    /// The instant the transient status is owed its transition. Stored, never
    /// slept on: a read materialises the transition the clock already made due.
    pub settles_at: SystemTime,
    /// Every policy created in this engine, keyed by `policyId`. The key order
    /// is the deterministic list order a policy listing pages over.
    pub policies: BTreeMap<String, PolicyRecord>,
}

impl PolicyEngineRecord {
    /// The engine a create stores for the request `requested` in `region`, under
    /// the id that create minted: `CREATING`, with the transition to `ACTIVE`
    /// owed by the clock rather than by the call.
    pub fn created(
        region: &str,
        policy_engine_id: String,
        requested: CreatePolicyEngineInput,
        now: SystemTime,
    ) -> Self {
        Self {
            policy_engine_arn: policy_engine_arn(region, &policy_engine_id),
            policy_engine_id,
            name: requested.name,
            description: requested.description,
            encryption_key_arn: requested.encryption_key_arn,
            tags: sorted_map(requested.tags),
            created_at: now,
            updated_at: now,
            transient_status: POLICY_STATUS_CREATING.to_string(),
            settled_status: POLICY_STATUS_ACTIVE.to_string(),
            settles_at: now + POLICY_SETTLE_DELAY,
            policies: BTreeMap::new(),
        }
    }

    /// The status this engine reports at `now`: the transient one until the
    /// transition the clock owes comes due, the settled one afterwards.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// This engine as `CreatePolicyEngine`'s response envelope.
    pub fn create_output(&self, now: SystemTime) -> CreatePolicyEngineOutput {
        CreatePolicyEngineOutput {
            policy_engine_id: self.policy_engine_id.clone(),
            name: self.name.clone(),
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
            policy_engine_arn: self.policy_engine_arn.clone(),
            status: self.status_at(now).to_string(),
            encryption_key_arn: self.encryption_key_arn.clone(),
            description: self.description.clone(),
            // "Additional information about the policy status. This provides
            // details about any failures or the current state": a create this
            // emulator accepts has failed in no way, so there is nothing to
            // report beside the status itself.
            status_reasons: Vec::new(),
        }
    }

    /// This engine as `GetPolicyEngine`'s response envelope — the same members
    /// [`Self::create_output`] renders, read off the same record.
    pub fn get_output(&self, now: SystemTime) -> GetPolicyEngineOutput {
        let created = self.create_output(now);
        GetPolicyEngineOutput {
            policy_engine_id: created.policy_engine_id,
            name: created.name,
            created_at: created.created_at,
            updated_at: created.updated_at,
            policy_engine_arn: created.policy_engine_arn,
            status: created.status,
            encryption_key_arn: created.encryption_key_arn,
            description: created.description,
            status_reasons: created.status_reasons,
        }
    }
}

/// One policy created in a policy engine: the definition and modes the caller
/// supplied, and the transition to `ACTIVE` the clock owes it.
#[derive(Debug, Clone)]
pub struct PolicyRecord {
    pub policy_id: String,
    pub policy_arn: String,
    pub policy_engine_id: String,
    pub name: String,
    pub definition: PolicyDefinition,
    pub description: Option<String>,
    /// As the request carried it, with the model's own `ACTIVE` default already
    /// applied by the boundary walk.
    pub enforcement_mode: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The status until [`settles_at`](Self::settles_at) passes.
    pub transient_status: String,
    /// The status from [`settles_at`](Self::settles_at) onwards.
    pub settled_status: String,
    /// The instant the transient status is owed its transition. Stored, never
    /// slept on: a read materialises the transition the clock already made due.
    pub settles_at: SystemTime,
    /// The instant an accepted deletion removes this record, once `DeletePolicy`
    /// has accepted one; `None` for a policy nobody has deleted. Stored, never
    /// slept on: the removal is the clock's, materialised by
    /// [`expire_deleted_policies`](crate::state::ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState::expire_deleted_policies)
    /// on the next call that reads the policy plane.
    pub removal_due_at: Option<SystemTime>,
}

impl PolicyRecord {
    /// The policy a create stores in the engine `policy_engine_id`, under the id
    /// that create minted, from the request `requested`.
    ///
    /// `enforcementMode` is taken as the request carries it, with the model's own
    /// `ACTIVE` default already applied by the boundary walk, so the policy is
    /// stored under the mode it will be enforced in. The engine id is the one the
    /// lookup resolved rather than the raw path member, so the stored policy and
    /// its ARN name the engine that actually holds it.
    pub fn created(
        region: &str,
        policy_engine_id: String,
        policy_id: String,
        requested: CreatePolicyInput,
        now: SystemTime,
    ) -> Self {
        Self {
            policy_arn: policy_arn(region, &policy_engine_id, &policy_id),
            policy_id,
            policy_engine_id,
            name: requested.name,
            definition: requested.definition,
            description: requested.description,
            enforcement_mode: requested.enforcement_mode,
            created_at: now,
            updated_at: now,
            transient_status: POLICY_STATUS_CREATING.to_string(),
            settled_status: POLICY_STATUS_ACTIVE.to_string(),
            settles_at: now + POLICY_SETTLE_DELAY,
            removal_due_at: None,
        }
    }

    /// Whether a deletion has been accepted for this policy: it reports
    /// `DELETING` and the clock owes its removal.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Accept a deletion at `now`: the policy reports `DELETING` from this
    /// instant on and the clock owes its removal one settle span later.
    ///
    /// Idempotent, as `DeletePolicy` is (`@idempotent`): a repeat while the
    /// deletion is still under way leaves the record and the instant it is owed
    /// alone rather than re-arming the clock.
    pub fn accept_deletion(&mut self, now: SystemTime) {
        if self.is_deleting() {
            return;
        }
        self.transient_status = POLICY_STATUS_DELETING.to_string();
        self.settled_status = POLICY_STATUS_DELETING.to_string();
        self.settles_at = now;
        self.removal_due_at = Some(now + POLICY_SETTLE_DELAY);
        self.updated_at = now;
    }

    /// The status this policy reports at `now`, on the same clock-owed rule the
    /// engine that holds it follows.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// This policy as `CreatePolicy`'s response envelope.
    ///
    /// `CreatePolicyResponse` and `GetPolicyResponse` declare the SAME members,
    /// so both envelopes are rendered from the one stored record here rather
    /// than in either handler: a member the create reports and the read does not
    /// would be a difference no stored state justifies.
    pub fn create_output(&self, now: SystemTime) -> CreatePolicyOutput {
        CreatePolicyOutput {
            policy_id: self.policy_id.clone(),
            name: self.name.clone(),
            policy_engine_id: self.policy_engine_id.clone(),
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
            policy_arn: self.policy_arn.clone(),
            status: self.status_at(now).to_string(),
            enforcement_mode: self.enforcement_mode.clone(),
            definition: self.definition.clone(),
            description: self.description.clone(),
            // Nothing failed, so the status carries no reason of its own.
            status_reasons: Vec::new(),
        }
    }

    /// This policy as `DeletePolicy`'s response envelope — `DeletePolicyResponse`
    /// declares the same members as `CreatePolicyResponse`, so it too is
    /// rendered from the one stored record: what the deletion reports about the
    /// policy is the policy, with the status the accepted deletion left on it.
    pub fn delete_output(&self, now: SystemTime) -> DeletePolicyOutput {
        let created = self.create_output(now);
        DeletePolicyOutput {
            policy_id: created.policy_id,
            name: created.name,
            policy_engine_id: created.policy_engine_id,
            created_at: created.created_at,
            updated_at: created.updated_at,
            policy_arn: created.policy_arn,
            status: created.status,
            enforcement_mode: created.enforcement_mode,
            definition: created.definition,
            description: created.description,
            status_reasons: created.status_reasons,
        }
    }

    /// This policy as `GetPolicy`'s response envelope — the same members
    /// [`Self::create_output`] renders, read off the same record.
    pub fn get_output(&self, now: SystemTime) -> GetPolicyOutput {
        let created = self.create_output(now);
        GetPolicyOutput {
            policy_id: created.policy_id,
            name: created.name,
            policy_engine_id: created.policy_engine_id,
            created_at: created.created_at,
            updated_at: created.updated_at,
            policy_arn: created.policy_arn,
            status: created.status,
            enforcement_mode: created.enforcement_mode,
            definition: created.definition,
            description: created.description,
            status_reasons: created.status_reasons,
        }
    }
}
