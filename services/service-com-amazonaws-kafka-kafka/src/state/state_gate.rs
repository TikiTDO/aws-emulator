// SPDX-License-Identifier: CC0-1.0

//! The state precondition a call that CHANGES a resource of this service applies,
//! and the one sentence Amazon MSK refuses such a call with when the resource is
//! not in a state that call is accepted from.
//!
//! The service answers that condition with ONE fixed sentence, which names the set
//! of states the call IS accepted from and nothing else — not the state the
//! resource happens to be in, and no value from the request. It is built here, from
//! the accepted-state set the handler gated on, so the gate a call applies and the
//! sentence it answers with cannot drift apart, and so the cluster family and the
//! configuration family say the same thing about the same condition.

use super::*;

/// The `ClusterState` values a call that CHANGES a cluster's configuration is
/// accepted from.
///
/// A cluster reconfigures one change at a time, so an update is only ever applied
/// to a cluster that is serving and settled: one still being provisioned, already
/// applying a change, rebooting its brokers or being deleted has no settled
/// configuration to update from. Named here so every update operation gates on one
/// set and reports the one refusal built from it.
pub const CLUSTER_STATES_A_CHANGE_IS_ACCEPTED_FROM: [&str; 1] = [CLUSTER_STATE_ACTIVE];

/// Whether a cluster in `state` accepts a call that changes its configuration.
pub fn cluster_accepts_change(state: &str) -> bool {
    CLUSTER_STATES_A_CHANGE_IS_ACCEPTED_FROM.contains(&state)
}

/// The refusal every such call answers for a cluster that is in some other state:
/// a `BadRequestException` (HTTP 400) carrying the fixed sentence over
/// [`CLUSTER_STATES_A_CHANGE_IS_ACCEPTED_FROM`] and naming the member the request
/// addressed the cluster by as the parameter at fault.
///
/// One envelope, so the state gate of one update operation cannot drift from
/// another's, and a caller can tell "wait for the cluster to settle" from a
/// reference the service cannot resolve at all
/// ([`RESOURCE_NOT_FOUND_MESSAGE`]).
pub fn cluster_not_in_accepted_state() -> BadRequestException {
    BadRequestException {
        invalid_parameter: Some(CLUSTER_ARN_PARAMETER.to_string()),
        message: Some(not_in_accepted_state_message(
            &CLUSTER_STATES_A_CHANGE_IS_ACCEPTED_FROM,
        )),
    }
}

/// The member a cluster-scoped refusal names as the parameter at fault, as the
/// model spells it on this service's cluster requests.
pub const CLUSTER_ARN_PARAMETER: &str = "ClusterArn";

/// The one sentence that refusal carries: fixed text around the set of states the
/// call is accepted from, in the spelling the state enum declares, and nothing
/// else — so a gate that admits more than one state says so, and no refusal
/// echoes a value back.
pub fn not_in_accepted_state_message(accepted: &[&str]) -> String {
    format!(
        "This operation is only valid for resources that are in one of the following states :[{}]",
        accepted.join(", ")
    )
}

/// The `TopicState` values a call that CHANGES a topic's configuration is accepted
/// from.
///
/// A topic takes one configuration change at a time: one still being created, one
/// whose partitions are still being reassigned by a change already accepted, and
/// one whose deletion the service has accepted are none of them a topic a further
/// change applies to. The same rule
/// [`CLUSTER_STATES_A_CHANGE_IS_ACCEPTED_FROM`] states for a cluster, over the
/// topic's own state machine.
pub const TOPIC_STATES_A_CHANGE_IS_ACCEPTED_FROM: [&str; 1] = [TOPIC_STATE_ACTIVE];

/// Whether a topic in `state` accepts a call that changes its configuration.
pub fn topic_accepts_change(state: &str) -> bool {
    TOPIC_STATES_A_CHANGE_IS_ACCEPTED_FROM.contains(&state)
}

/// The refusal such a call answers for a topic that is in some other state, in the
/// same envelope the cluster gate answers: the fixed sentence over
/// [`TOPIC_STATES_A_CHANGE_IS_ACCEPTED_FROM`], naming the member the request
/// addressed the topic by.
pub fn topic_not_in_accepted_state() -> BadRequestException {
    BadRequestException {
        invalid_parameter: Some(TOPIC_NAME_PARAMETER.to_string()),
        message: Some(not_in_accepted_state_message(
            &TOPIC_STATES_A_CHANGE_IS_ACCEPTED_FROM,
        )),
    }
}

/// The member a topic-scoped refusal names as the parameter at fault, as the model
/// spells it on this service's topic requests.
pub const TOPIC_NAME_PARAMETER: &str = "TopicName";
