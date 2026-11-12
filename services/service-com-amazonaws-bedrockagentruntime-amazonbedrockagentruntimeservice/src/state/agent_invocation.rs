// SPDX-License-Identifier: CC0-1.0

//! What an AGENT INVOCATION reads and writes: the agent alias a request is addressed at,
//! the text the response stream carries, and the summary the memory of that alias keeps.
//!
//! An agent and its aliases belong to the Bedrock Agent build-time plane, so existence is
//! read THERE ([`resolve`]) and never assumed from the request: an agent that plane does
//! not hold, or an alias it holds nothing for, is the modelled `ResourceNotFoundException`.
//! The same lookup answers the two facts an invocation needs about the agent it reached —
//! the foundation model the agent was created against, and whether the agent keeps a
//! session summary — so both come from the stored agent rather than from the call.
//!
//! NO LANGUAGE MODEL RUNS IN THIS EMULATOR, and nothing here pretends otherwise. The text
//! the response stream carries is the notice the Bedrock Runtime plane of this same
//! emulator answers every inference surface with (`EMULATED_COMPLETION_FLOOR`, the one
//! place that sentence is worded), extended with the facts of the invocation that was
//! really served: which agent alias answered and which foundation model the agent is bound
//! to. It is never the caller's own `inputText` handed back as if an agent had replied, and
//! never prose synthesised to look like orchestration.
//!
//! What IS written is the memory [`crate::state::agent_memory`] reports: this module is the
//! one place a session summary is recorded, which is what makes `GetAgentMemory` report a
//! session that was really invoked instead of one seeded by hand.

use std::time::SystemTime;

use ::aws_smithy_types::DateTime;

use ::service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state as agent_state;
use ::service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state as runtime_state;

use super::agent_memory::{self, SessionSummaryRecord};

/// Why the agent alias a request names cannot be invoked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvocationRefusal {
    /// The store the agents live in is not on this engine, so nothing can be said about
    /// the alias — answered with a modelled server error, never with a not-found.
    StoreUnavailable,
    /// The Bedrock Agent plane holds no agent of that identifier, or no alias of that
    /// identifier on it.
    NotFound,
}

/// The agent alias an invocation reached, as the build-time plane holds it.
#[derive(Debug, Clone)]
pub struct InvokedAlias {
    /// The agent the alias belongs to.
    pub agent_id: String,
    /// The alias that was invoked.
    pub agent_alias_id: String,
    /// The foundation model the agent was created against, when it names one. The agent's
    /// own, read from the stored record — never the request's.
    pub foundation_model: Option<String>,
    /// How long the agent retains conversational context, in days, when it is configured
    /// to keep a session summary at all. `None` for an agent that keeps none, which is
    /// what makes an invocation of it record nothing.
    pub session_summary_storage_days: Option<i32>,
}

/// The `MemoryType` an agent has to enable for an invocation to summarise its sessions:
/// the one value the runtime model's enum declares, which is also the one the build-time
/// plane's `MemoryConfiguration$enabledMemoryTypes` carries for it.
const SESSION_SUMMARY: &str = agent_memory::SESSION_SUMMARY_MEMORY_TYPE;

/// The agent alias `agent_alias_id` of `agent_id`, as the Bedrock Agent plane holds it.
///
/// The agent is read in the plane that OWNS it, so an agent this emulator never created is
/// not addressable here however well-formed its identifier is. An ALIAS of an agent is
/// created by that plane's `CreateAgentAlias`, which is still a stub, so the only alias an
/// agent holds today is the draft one every agent carries
/// ([`agent_memory::DRAFT_AGENT_ALIAS_ID`]) — the same rule the memory operations resolve
/// an alias by, stated there.
///
/// READ-ONLY for everything a later caller can observe: the one write on its path is that
/// plane materialising the status transitions its own clock had already made due, which is
/// owed whether or not anybody calls.
pub fn resolve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    agent_id: &str,
    agent_alias_id: &str,
) -> Result<InvokedAlias, InvocationRefusal> {
    let Some(agents) = agent_state::service_state(engine) else {
        return Err(InvocationRefusal::StoreUnavailable);
    };
    let mut store = agents.lock();
    store.materialise_due_transitions(SystemTime::now());
    if agent_alias_id != agent_memory::DRAFT_AGENT_ALIAS_ID {
        return Err(InvocationRefusal::NotFound);
    }
    let Some(agent) = store.agents.get(agent_id) else {
        return Err(InvocationRefusal::NotFound);
    };
    let session_summary_storage_days = agent
        .memory_configuration
        .as_ref()
        .filter(|memory| {
            memory
                .enabled_memory_types
                .iter()
                .any(|enabled| enabled == SESSION_SUMMARY)
        })
        .map(|memory| memory.storage_days);
    Ok(InvokedAlias {
        agent_id: agent_id.to_string(),
        agent_alias_id: agent_alias_id.to_string(),
        foundation_model: agent.foundation_model.clone(),
        session_summary_storage_days,
    })
}

/// The `ResourceNotFoundException` message an agent invocation answers when the agent or
/// the alias the request names is one the Bedrock Agent plane holds nothing for.
///
/// Entirely fixed text for this error class, as AWS words it: the refusal names neither the
/// agent nor the alias, because either half of the pair may be the missing one and the
/// service does not disclose which.
pub fn alias_not_found_message() -> String {
    RESOURCE_NOT_FOUND.to_string()
}

/// The sentence [`alias_not_found_message`] answers with, as AWS words it for this error
/// class.
const RESOURCE_NOT_FOUND: &str = "Failed to retrieve resource because it doesn't exist. \
                                  Retry the request with a different resource identifier.";

/// The `MimeType` an agent response stream is reported under, which the modelled
/// `contentType` response header carries: the agent's answer travels as JSON-encoded
/// event-stream frames.
pub const RESPONSE_CONTENT_TYPE: &str = "application/json";

/// The text the single `chunk` of a served invocation carries: that no model ran, which
/// alias answered, and which foundation model the agent is bound to.
///
/// The caller's own `inputText` is NOT part of it: a prompt handed back would read as an
/// agent that had replied to it.
pub fn completion_notice(alias: &InvokedAlias) -> String {
    let model = alias
        .foundation_model
        .as_deref()
        .unwrap_or("no foundation model");
    format!(
        "{} Agent {} alias {} is bound to {model} and generated no response.",
        runtime_state::EMULATED_COMPLETION_FLOOR, alias.agent_id, alias.agent_alias_id
    )
}

/// The summary a session that ENDED is remembered by, in the words that say no model
/// summarised it.
fn summary_text(alias: &InvokedAlias, session_id: &str) -> String {
    format!(
        "{} The session {session_id} on agent {} alias {} was not summarised by a model.",
        runtime_state::EMULATED_COMPLETION_FLOOR, alias.agent_id, alias.agent_alias_id
    )
}

/// How many seconds one day of retention is, for turning the agent's `storageDays` into
/// the instant a summary's memory duration ends.
const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

/// The summary record an invocation that ENDED its session leaves in the memory of the
/// alias it reached, or `None` when the agent keeps no session summary.
///
/// `session_expiry_time` is the instant the memory duration for the session ends: the
/// session's start plus the agent's own `storageDays`, which the build-time model gives a
/// `default` of 30 ("The number of days the agent is configured to retain the
/// conversational context") — so the expiry is the stored agent's configuration and never
/// a span chosen here.
pub fn session_summary(
    alias: &InvokedAlias,
    memory_id: &str,
    session_id: &str,
    at: DateTime,
) -> Option<SessionSummaryRecord> {
    let storage_days = i64::from(alias.session_summary_storage_days?);
    Some(SessionSummaryRecord {
        memory_id: memory_id.to_string(),
        session_id: session_id.to_string(),
        session_start_time: at,
        session_expiry_time: DateTime::from_secs(
            at.secs().saturating_add(storage_days.saturating_mul(SECONDS_PER_DAY)),
        ),
        summary_text: summary_text(alias, session_id),
    })
}

/// The resource an identity-based policy for `bedrock:InvokeAgent` names: the agent alias
/// the call invokes, as `agent-alias/<agentId>/<agentAliasId>` — the same resource the
/// memory operations of this plane decide against ([`agent_memory::alias_resource_arn`]).
pub fn alias_resource_arn(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    agent_id: &str,
    agent_alias_id: &str,
) -> String {
    agent_memory::alias_resource_arn(ctx, agent_id, agent_alias_id)
}
