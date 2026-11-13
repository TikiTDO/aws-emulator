// SPDX-License-Identifier: CC0-1.0

//! Which accounts may create a NEW agent on this plane, and what records that they may.
//!
//! The agent surface of this service "is no longer open to new customers. … Existing
//! customers can continue to use the service as normal" — the sentence the model itself
//! carries on `CreateAgent` (`smithy_query(query="doc", shape="CreateAgent",
//! model_path="services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json")`).
//! So whether a new agent may be minted is a property of the ACCOUNT making the call, not
//! of the request it makes: an account that already used the agent surface keeps creating
//! agents, and an account with no usage of it is refused with `CreateAgent`'s own modelled
//! `AccessDeniedException` (403), whose message states that condition
//! ([`MAINTENANCE_MODE_MESSAGE`]).
//!
//! That usage history is not something any modelled operation of this plane writes — it is
//! what an account did before the surface closed to new customers — so it is a record of
//! the store: [`AgentsClassicUsage`], EMPTY by default, because an emulator's account has
//! used nothing until something says it has. The producer is
//! [`AgentsClassicUsage::record`], which this API does not model and which is NOT an AWS
//! operation: a host driving the existing-customer path is what calls it.
//!
//! Only the minting of a new agent turns on it. Every read, list, update and delete of the
//! agent surface, and every other resource family this plane holds (flows, knowledge
//! bases, data sources, prompts), is served whatever the account's history is — which is
//! what lets a caller tell this refusal from an authorization failure, since the same
//! account's `ListAgents` answers normally.

use std::collections::BTreeSet;

/// The sentence a `CreateAgent` from an account with no prior usage of the agent surface
/// is refused with.
///
/// Its two middle sentences are the service's own fixed text, recorded verbatim from the
/// refusal; the message carries no part taken from the request, because the condition is
/// the account's and not the request's.
pub const MAINTENANCE_MODE_MESSAGE: &str = "Amazon Bedrock Agents is in Maintenance Mode. \
     New agent creation is not available for accounts without prior service usage. \
     For new agent development, use Amazon Bedrock AgentCore.";

/// The accounts this store holds prior usage of the agent surface for: an account in here
/// may create a new agent, an account outside it may not.
///
/// A set, not a flag: the condition is per account, so one account's history says nothing
/// about another's.
#[derive(Debug, Default)]
pub struct AgentsClassicUsage {
    accounts: BTreeSet<String>,
}

impl AgentsClassicUsage {
    /// Whether `account_id` has prior usage of this plane's agent surface, which is what
    /// decides whether it may create a new agent.
    #[must_use]
    pub fn holds(&self, account_id: &str) -> bool {
        self.accounts.contains(account_id)
    }

    /// Record that `account_id` used the agent surface before it closed to new customers,
    /// which is what lets that account create agents. Answers whether this call added it.
    ///
    /// This is the producer this API does not model; it is NOT an AWS operation.
    pub fn record(&mut self, account_id: &str) -> bool {
        self.accounts.insert(account_id.to_string())
    }
}

/// The account a request to this plane is made by: every ARN this plane mints carries
/// `engine::EMULATOR_ACCOUNT_ID` (`AgentRecord::agent_arn`, minted through
/// `engine::emulator_arn`), so that is the account whose usage history decides a create.
#[must_use]
pub fn request_account_id() -> &'static str {
    ::engine::EMULATOR_ACCOUNT_ID
}
