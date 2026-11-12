// SPDX-License-Identifier: CC0-1.0

//! What a FLOW INVOCATION reads: the flow an alias routes to, and the path the request's
//! input really travels through that flow's own definition.
//!
//! A flow, its versions and its aliases belong to the Bedrock Agent build-time plane, so
//! every one of them is read THERE and never assumed from the request:
//!
//! * the `flowIdentifier` names a stored flow, in either form the modelled shape admits
//!   (a flow ARN or the bare id), through that plane's own reader;
//! * the `flowAliasIdentifier` names an alias STORED ON THAT FLOW — an alias of another
//!   flow is a resource the addressed flow does not hold;
//! * the alias's `routingConfiguration` names the VERSION the invocation runs, and the
//!   definition that version packages is the one that is walked: the DRAFT version is the
//!   flow record itself, a numbered version its own snapshot. So a flow whose draft has
//!   moved since the version was cut still runs what the version packages;
//! * a flow that is not PREPARED runs nothing: preparation is what turns a definition into
//!   something invocable, and its status is the build-time plane's.
//!
//! NO LANGUAGE MODEL, LAMBDA FUNCTION OR OTHER EXTERNAL WORKER RUNS IN THIS EMULATOR, so
//! the walk computes exactly the nodes it can and REFUSES the rest rather than inventing a
//! value for them: the request's document enters at the flow's input node and reaches the
//! output nodes that node is connected to. A path through any other node kind — a prompt, a
//! knowledge base, a Lambda function, a condition, an iterator — is refused with the
//! sentence [`FlowRefusal::Unservable`] carries, because an output synthesised for a node
//! nothing ran would be indistinguishable from one the flow really produced.
//!
//! READ-ONLY for everything a later caller can observe: the one write on its path is the
//! build-time plane materialising the status transitions its own clock had already made
//! due, which is owed whether or not anybody calls.

use std::time::SystemTime;

use ::aws_smithy_types::Document;

use ::service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::ops::types::FlowDefinition;
use ::service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state as agent_state;

/// Why a flow invocation cannot be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowRefusal {
    /// The store the flows live in is not on this engine, so nothing can be said about the
    /// flow — reported as a server error, never as a not-found.
    StoreUnavailable,
    /// The Bedrock Agent plane holds no flow of that identifier, no such alias on it, or
    /// no version the alias routes to.
    NotFound,
    /// The flow exists and the request is well formed, but this service cannot run it: the
    /// string is the sentence the refusal states, naming what about the flow or the request
    /// it could not compute.
    Unservable(String),
}

/// One value a flow invocation delivers: the output node it left the flow at, and the
/// document that reached it.
#[derive(Debug, Clone)]
pub struct FlowOutputValue {
    /// The name of the flow output node the value left the flow at.
    pub node_name: String,
    /// The document that reached that node.
    pub document: Document,
}

/// The `FlowNodeType` of a flow's input node, as the build-time model spells it
/// (`smithy_query(query="shape",shape="FlowNodeType")` -> Input, Output, KnowledgeBase,
/// Condition, Lex, Prompt, LambdaFunction, Storage, Agent, Retrieval, Iterator, Collector,
/// InlineCode, Loop, LoopInput, LoopController).
const INPUT_NODE: &str = "Input";

/// See [`INPUT_NODE`]: the type of a flow's output node.
const OUTPUT_NODE: &str = "Output";

/// The `FlowConnectionType` of a connection that carries a VALUE from one node to the next
/// (`smithy_query(query="shape",shape="FlowConnectionType")` -> Data, Conditional).
const DATA_CONNECTION: &str = "Data";

/// The `FlowStatus` a flow has to be in to be invoked: preparation is what turns a
/// definition into something invocable
/// (`smithy_query(query="shape",shape="FlowStatus")` -> Failed, Prepared, Preparing,
/// NotPrepared).
const PREPARED: &str = "Prepared";

/// Run `document` through the flow the alias routes to, entering at `input_node`.
///
/// The whole lookup happens under ONE hold of the build-time plane's store, so the
/// definition that is walked is the one the alias routed to at that instant.
pub fn run<Eng: Send + Sync + 'static>(
    engine: &Eng,
    flow_identifier: &str,
    flow_alias_identifier: &str,
    input_node: &str,
    document: &Document,
) -> Result<Vec<FlowOutputValue>, FlowRefusal> {
    let Some(agents) = agent_state::service_state(engine) else {
        return Err(FlowRefusal::StoreUnavailable);
    };
    let mut store = agents.lock();
    store.materialise_due_transitions(SystemTime::now());

    let Some(flow_id) = agent_state::flow_id_from_identifier(flow_identifier) else {
        return Err(FlowRefusal::NotFound);
    };
    let Some((aliased_flow, alias_id)) =
        agent_state::flow_alias_from_identifier(flow_alias_identifier)
    else {
        return Err(FlowRefusal::NotFound);
    };
    // An alias ARN carries the flow it belongs to: one naming another flow addresses a
    // resource the flow in the path does not hold.
    if aliased_flow.is_some_and(|named| named != flow_id) {
        return Err(FlowRefusal::NotFound);
    }
    let Some(flow) = store.flows.get(flow_id) else {
        return Err(FlowRefusal::NotFound);
    };
    let Some(alias) = store.flow_alias_of(flow_id, alias_id) else {
        return Err(FlowRefusal::NotFound);
    };
    let Some(routed) = alias
        .routing_configuration
        .first()
        .and_then(|item| item.flow_version.clone())
    else {
        return Err(FlowRefusal::NotFound);
    };

    // The definition the alias routes to, and the status of the thing that packages it.
    let (definition, status) = if routed == agent_state::DRAFT_VERSION {
        (flow.definition.as_ref(), flow.status.as_wire())
    } else {
        let Some(version) = routed
            .parse::<u32>()
            .ok()
            .and_then(|number| store.flow_versions.get(&(flow_id.to_string(), number)))
        else {
            return Err(FlowRefusal::NotFound);
        };
        (version.definition.as_ref(), version.status.as_wire())
    };
    if status != PREPARED {
        return Err(FlowRefusal::Unservable(format!(
            "the flow version {routed} this alias routes to is in status {status}: prepare \
             the flow before invoking it"
        )));
    }
    let Some(definition) = definition else {
        return Err(FlowRefusal::Unservable(format!(
            "the flow version {routed} this alias routes to carries no definition, so there \
             is no node for the input to enter at"
        )));
    };
    walk(definition, input_node, document)
}

/// The values the request's document reaches the flow's output nodes with.
///
/// The walk is one hop by construction: the input node's `Data` connections are followed,
/// and a target that is anything but an output node is refused. See the module docs for
/// why nothing further is computed.
fn walk(
    definition: &FlowDefinition,
    input_node: &str,
    document: &Document,
) -> Result<Vec<FlowOutputValue>, FlowRefusal> {
    let nodes = definition.nodes.as_deref().unwrap_or_default();
    let node_type = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .map(|node| node.r#type.clone())
    };
    let Some(entry_type) = node_type(input_node) else {
        return Err(FlowRefusal::Unservable(format!(
            "the flow holds no node named {input_node}, so the input it was sent to does \
             not exist in this flow"
        )));
    };
    if entry_type != INPUT_NODE {
        return Err(FlowRefusal::Unservable(format!(
            "the node {input_node} is of type {entry_type}, and an invocation enters a flow \
             at a node of type {INPUT_NODE}"
        )));
    }

    let mut reached: Vec<FlowOutputValue> = Vec::new();
    for connection in definition.connections.as_deref().unwrap_or_default() {
        if connection.source != input_node || connection.r#type != DATA_CONNECTION {
            continue;
        }
        let Some(target_type) = node_type(&connection.target) else {
            return Err(FlowRefusal::Unservable(format!(
                "the connection {} leaves {input_node} for the node {}, which the flow does \
                 not hold",
                connection.name, connection.target
            )));
        };
        if target_type != OUTPUT_NODE {
            return Err(FlowRefusal::Unservable(unservable_node(
                &connection.target,
                &target_type,
            )));
        }
        reached.push(FlowOutputValue {
            node_name: connection.target.clone(),
            document: document.clone(),
        });
    }
    if reached.is_empty() {
        return Err(FlowRefusal::Unservable(format!(
            "the flow's input node {input_node} is connected to no node of type \
             {OUTPUT_NODE}, so the input this invocation carries reaches no output"
        )));
    }
    // One order for one flow: an invocation of the same flow always delivers its outputs
    // the same way round.
    reached.sort_by(|left, right| left.node_name.cmp(&right.node_name));
    Ok(reached)
}

/// The sentence a node this service cannot run earns: what the node is, and that nothing
/// in this process performs the work its kind does.
fn unservable_node(name: &str, node_type: &str) -> String {
    format!(
        "the node {name} is of type {node_type}, whose work — a model, a function or \
         another service answering — nothing in this emulator performs, so no value can be \
         computed for it; a flow this service runs sends its input node straight to its \
         output nodes"
    )
}

/// The message a flow invocation delivers inside its stream when the flow, the alias or
/// the version the alias routes to is one the Bedrock Agent plane does not hold.
///
/// Fixed text for this error class: the refusal names neither the flow nor the alias,
/// because any one of them may be the missing resource and the service does not disclose
/// which.
pub fn not_found_message() -> String {
    FLOW_RESOURCE_NOT_FOUND.to_string()
}

/// The sentence [`not_found_message`] answers with, as AWS words a missing resource on this
/// plane (the same sentence its agent invocation surface states).
const FLOW_RESOURCE_NOT_FOUND: &str = "Failed to retrieve resource because it doesn't \
                                       exist. Retry the request with a different resource \
                                       identifier.";

/// The resource an identity-based policy for `bedrock:InvokeFlow` names: the flow alias the
/// call invokes.
///
/// An identifier that is already an ARN IS that resource; a bare pair names the alias of
/// that flow in the region the caller signed for. Built without a store lookup, so a
/// decision made with it is made before — and independently of — whether the flow exists.
pub fn alias_resource_arn(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    flow_identifier: &str,
    flow_alias_identifier: &str,
) -> String {
    if flow_alias_identifier.starts_with("arn:") {
        return flow_alias_identifier.to_string();
    }
    let flow = agent_state::flow_id_from_identifier(flow_identifier).unwrap_or(flow_identifier);
    ::engine::emulator_arn(
        crate::state::ARN_NAMESPACE,
        &crate::state::request_region(ctx),
        &format!("flow/{flow}/alias/{flow_alias_identifier}"),
    )
}
