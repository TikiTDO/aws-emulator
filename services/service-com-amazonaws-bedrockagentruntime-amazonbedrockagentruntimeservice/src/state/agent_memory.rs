// SPDX-License-Identifier: CC0-1.0

//! The MEMORY of an agent alias: what `GetAgentMemory` reports and what
//! `DeleteAgentMemory` removes, and how both resolve the alias they are addressed at.
//!
//! An agent's memory is kept PER ALIAS and per `memoryId`: the summaries of the sessions
//! that invoked that alias under that memory id. The two operations that address it have
//! to agree on three things, so all three live here rather than in either handler:
//!
//! * WHICH alias a request names. An agent and its aliases belong to the Bedrock Agent
//!   build-time plane, so existence is read THERE ([`alias_of_agent`]) and never assumed
//!   from the request: an agent that plane does not hold, or an alias it holds nothing
//!   for, is the modelled `ResourceNotFoundException`.
//! * WHAT the memory holds. The summaries live in this service's own store
//!   ([`crate::state::ComAmazonawsBedrockagentruntimeAmazonbedrockagentruntimeserviceState::agent_memories`]),
//!   keyed by the alias and the memory id, because the runtime plane is what records a
//!   session into an agent's memory. A key the store holds nothing for is an alias that
//!   has held no session, which is an EMPTY memory and not a missing resource — that is
//!   what AWS answers for an agent nobody has talked to yet.
//! * HOW the refusal is worded, so the two operations refuse the same way.
//!
//! What WRITES a summary is the agent invocation surface (`InvokeAgent`), which this
//! emulator does not serve yet — it is one of the operations left modelled-but-unwired in
//! `src/ops/SKIPPED_STUBS.md`. Nothing here invents a summary in its place: a memory is
//! reported exactly as the store holds it, so an alias whose sessions were never
//! summarised reports no memory rather than a fabricated one.

use std::time::SystemTime;

use ::aws_smithy_types::DateTime;

use ::service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state as agent_state;

/// The one value the model's `MemoryType` enum declares
/// (`smithy_query(query="shape",shape="MemoryType")` -> SESSION_SUMMARY): the kind of
/// memory this plane keeps, and the only kind either operation addresses.
pub const SESSION_SUMMARY_MEMORY_TYPE: &str = "SESSION_SUMMARY";

/// The alias every agent carries without one being created: the DRAFT (test) alias, which
/// Amazon Bedrock gives each agent so the draft version can be invoked before an alias is
/// published. Spelled exactly as the build-time plane's own alias patterns spell it
/// (`agent_state::flow_aliases::TEST_ALIAS_ID` for the flow plane's equivalent).
pub const DRAFT_AGENT_ALIAS_ID: &str = "TSTALIASID";

/// One session the memory of an alias holds: the summary of that session, as the modelled
/// `MemorySessionSummary` reports it.
#[derive(Debug, Clone)]
pub struct SessionSummaryRecord {
    /// The memory the summary is stored in.
    pub memory_id: String,
    /// The session that was summarised.
    pub session_id: String,
    pub session_start_time: DateTime,
    /// When the memory duration for the session ends.
    pub session_expiry_time: DateTime,
    pub summary_text: String,
}

/// The store key one memory lives under: the alias it belongs to and the memory id the
/// sessions were recorded against. The alias is part of the key because memory is kept per
/// alias — two aliases of one agent do not share a memory.
pub type MemoryKey = (String, String, String);

/// The key the memory `memory_id` of `agent_alias_id` on `agent_id` lives under.
pub fn memory_key(agent_id: &str, agent_alias_id: &str, memory_id: &str) -> MemoryKey {
    (
        agent_id.to_string(),
        agent_alias_id.to_string(),
        memory_id.to_string(),
    )
}

/// Why the alias a request names cannot be addressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasRefusal {
    /// The store the agents live in is not on this engine, so nothing can be said about
    /// the alias — answered with a modelled server error, never with a not-found.
    StoreUnavailable,
    /// The Bedrock Agent plane holds no agent of that identifier, or no alias of that
    /// identifier on it.
    NotFound,
}

/// Whether the Bedrock Agent plane holds the alias `agent_alias_id` of `agent_id`.
///
/// The agent is read in the plane that OWNS it, so an agent this emulator never created is
/// not addressable here however well-formed its identifier is. An ALIAS of an agent is
/// created by that plane's `CreateAgentAlias`, which is still a stub, so the only alias an
/// agent holds today is the draft one every agent carries ([`DRAFT_AGENT_ALIAS_ID`]) —
/// when the alias store lands, this is the one place that has to consult it.
///
/// READ-ONLY for everything a later caller can observe: the one write on its path is that
/// plane materialising the status transitions its own clock had already made due, which is
/// owed whether or not anybody calls.
pub fn alias_of_agent<Eng: Send + Sync + 'static>(
    engine: &Eng,
    agent_id: &str,
    agent_alias_id: &str,
) -> Result<(), AliasRefusal> {
    let Some(agents) = agent_state::service_state(engine) else {
        return Err(AliasRefusal::StoreUnavailable);
    };
    let mut store = agents.lock();
    store.materialise_due_transitions(SystemTime::now());
    if !store.agents.contains_key(agent_id) || agent_alias_id != DRAFT_AGENT_ALIAS_ID {
        return Err(AliasRefusal::NotFound);
    }
    Ok(())
}

/// The `ResourceNotFoundException` message both memory operations answer when the alias the
/// request names is one the Bedrock Agent plane holds nothing for.
///
/// Fixed text for this error class, as AWS words it: the refusal names neither the agent
/// nor the alias, because either half of the pair may be the missing one and the service
/// does not disclose which.
pub fn alias_not_found_message() -> String {
    AGENT_OR_ALIAS_NOT_FOUND.to_string()
}

/// The sentence [`alias_not_found_message`] answers with, as AWS words it for this error
/// class.
pub const AGENT_OR_ALIAS_NOT_FOUND: &str = "Agent or Alias not found.";

/// The ARN of the agent alias a request addresses, which is the RESOURCE an authorization
/// decision about a memory operation is made against: an identity-based policy for
/// `bedrock:GetAgentMemory` or `bedrock:DeleteAgentMemory` names the alias as
/// `agent-alias/<agentId>/<agentAliasId>`.
///
/// Built without a store lookup, so the decision is made before — and independently of —
/// whether the agent or the alias exists.
pub fn alias_resource_arn(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    agent_id: &str,
    agent_alias_id: &str,
) -> String {
    ::engine::emulator_arn(
        crate::state::ARN_NAMESPACE,
        &crate::state::request_region(ctx),
        &format!("agent-alias/{agent_id}/{agent_alias_id}"),
    )
}
