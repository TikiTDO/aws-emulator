// SPDX-License-Identifier: CC0-1.0

//! The CLASSIC load balancer a `loadBalancers` entry attaches a service to, resolved
//! against the Classic Elastic Load Balancing service's own records.
//!
//! A `loadBalancers` entry names ONE Elastic Load Balancing resource, and which kind it
//! names is decided by the member it fills: `LoadBalancer$targetGroupArn` names an Elastic
//! Load Balancing v2 target group, and `LoadBalancer$loadBalancerName` is "The name of the
//! load balancer to associate with the Amazon ECS service or task set", a CLASSIC load
//! balancer — "If you are using an Application Load Balancer or a Network Load Balancer the
//! load balancer name parameter should be omitted."
//!
//! Two requirements follow from the `loadBalancers` doc, and neither can be expressed in the
//! model because neither is a value the request carries:
//!
//! * "For Classic Load Balancers, this object must contain the load balancer name ... When a
//!   task from this service is placed on a container instance, the container instance is
//!   registered with the load balancer that's specified here" — a name the account holds no
//!   load balancer for registers the instance with nothing, so it is refused. The load
//!   balancers of the account are the Classic Elastic Load Balancing service's OWN records,
//!   so they are read out of that service's store through the one shared engine rather than
//!   copied here, the same cross-service read the v2 target group a sibling entry names is
//!   resolved through ([`target_group_target_types`]).
//! * "Services with tasks that use the `awsvpc` network mode (for example, those with the
//!   Fargate launch type) only support Application Load Balancers and Network Load
//!   Balancers. Classic Load Balancers aren't supported" — such a task is reached at the
//!   elastic network interface of its own, and a Classic load balancer registers the
//!   container INSTANCE, so the pairing has no network path and is refused. It is the
//!   Classic counterpart of the target-type pairing [`required_target_type`] states.

use super::*;

/// Every CLASSIC load balancer a `loadBalancers` list names, in request order and without
/// repeats: each entry's `loadBalancerName`, which is the member an attachment to that kind
/// of load balancer is made through.
///
/// The counterpart of [`requested_target_group_arns`] for the other half of the member pair,
/// and bounded the same way: the list is caller-supplied, so the duplicate check is a
/// hash-set lookup and the walk is linear in a list a handler has already bounded by
/// [`MAX_LOAD_BALANCERS`].
pub fn requested_load_balancer_names(load_balancers: &[LoadBalancer]) -> Vec<String> {
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    for entry in load_balancers {
        if let Some(name) = entry.load_balancer_name.as_deref() {
            if seen.insert(name.to_string()) {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// Which of `names` the account holds a CLASSIC load balancer for in `region`, read from the
/// Classic Elastic Load Balancing service's OWN store through the one shared engine — so a
/// load balancer created over that API is seen here with no propagation step and no second
/// copy of it kept in this service's state.
///
/// The lookup is made in the Region the request was signed for: a Classic load balancer is a
/// REGIONAL resource, one name naming at most one of them within a Region, so a name that
/// names one of another Region names none of this one.
///
/// `None` when that store is unreachable (the engine is not the shared in-memory one), which
/// a handler answers with the same modelled server error it uses for its own store being
/// unreachable: whether the account holds the load balancer has to be grounded in what that
/// service answered, never assumed.
///
/// LOCK ORDER: this takes the Classic Elastic Load Balancing store's lock, holds no other
/// while it does, and hands back owned values rather than a guard — so it is called with NO
/// guard of this service's store held. Keeping the cross-service read one-directional and
/// guard-free is what makes a lock cycle between the two stores unreachable, the same
/// contract [`target_group_target_types`] has.
pub fn held_classic_load_balancer_names<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    names: &[String],
) -> Option<Vec<String>> {
    let state =
        ::service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::state::service_state(
            engine,
        )?;
    let store = state.lock();
    let held = store.load_balancers_in(region);
    Some(
        names
            .iter()
            .filter(|name| held.is_some_and(|held| held.contains_key(name.as_str())))
            .cloned()
            .collect(),
    )
}

/// The refusal a `loadBalancers` list a request NAMES earns for the CLASSIC load balancers
/// in it, or `None` when every name it carries attaches to one the service's tasks could be
/// registered with.
///
/// `held` is what [`held_classic_load_balancer_names`] read for
/// [`requested_load_balancer_names`] of the same list: a name it does not hold names no load
/// balancer of the account at all, and a name it does is checked against the revision's
/// network mode by [`classic_load_balancer_mode_refusal`]. A service with no resolved task
/// definition (a deployment controller that takes its tasks from a task set) has no network
/// mode to check against, so only the existence of the load balancer is.
///
/// The two halves are reported in that order, as the v2 pairing reports them: a name that
/// resolves to nothing is reported as that, whatever mode the revision declares.
pub fn classic_load_balancer_refusal(
    load_balancers: &[LoadBalancer],
    network_mode: Option<&str>,
    held: &[String],
) -> Option<String> {
    for name in requested_load_balancer_names(load_balancers) {
        if !held.contains(&name) {
            return Some(format!(
                "The load balancer with loadBalancerName {name} does not exist"
            ));
        }
    }
    network_mode.and_then(|mode| classic_load_balancer_mode_refusal(load_balancers, mode))
}

/// The network-mode half of [`classic_load_balancer_refusal`]: the refusal the Classic load
/// balancers a `loadBalancers` list names earn against a revision whose `networkMode` is
/// `network_mode`. `None` for every mode that reaches a task at the container instance
/// running it, and for a list that attaches no Classic load balancer at all.
///
/// Only [`NETWORK_MODE_AWSVPC`] is refused: a task of that mode is associated with an
/// elastic network interface of its own rather than with an Amazon EC2 instance, which is
/// exactly what a Classic load balancer registers, so the attachment could never reach the
/// task. The refusal names the load balancer the entry carried and the mode the resolved
/// revision declares, both read off the request and this store.
pub fn classic_load_balancer_mode_refusal(
    load_balancers: &[LoadBalancer],
    network_mode: &str,
) -> Option<String> {
    if network_mode != NETWORK_MODE_AWSVPC {
        return None;
    }
    requested_load_balancer_names(load_balancers)
        .into_iter()
        .next()
        .map(|name| {
            format!(
                "The provided load balancer {name} is a Classic Load Balancer, which is \
                 incompatible with the {network_mode} network mode specified in the task \
                 definition."
            )
        })
}
