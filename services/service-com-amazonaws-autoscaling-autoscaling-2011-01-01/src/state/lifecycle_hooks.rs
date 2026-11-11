// SPDX-License-Identifier: CC0-1.0

//! The lifecycle hooks of an Auto Scaling group: the group's own named collection,
//! and the rules every writer of it shares.
//!
//! A lifecycle hook is a NAMED CHILD of one Auto Scaling group, so it is stored on
//! [`StoredGroup`] itself ([`StoredGroup::lifecycle_hooks`]): one collection, keyed
//! by the hook's name, that `PutLifecycleHook` upserts into, `DescribeLifecycleHooks`
//! projects from, `DeleteLifecycleHook` removes from, and the group's own delete takes
//! with the group. Nothing in this crate holds a second copy.
//!
//! # The upsert
//!
//! `PutLifecycleHook` "creates OR updates a lifecycle hook"
//! (`smithy_query(query="operation",operation="PutLifecycleHook")`), and the model
//! documents the update as PER MEMBER: `LifecycleTransition` and `RoleARN` are
//! "Required for new lifecycle hooks, but optional when updating existing hooks", and
//! an empty `NotificationTargetARN` "overrides the current ARN". So a request carries
//! three states for each member, which [`LifecycleHookPut`] keeps apart: absent (keep
//! what is stored, or take the documented default on create), present (overwrite), and
//! the empty notification target (clear the stored one). The documented defaults are
//! applied ON CREATE only, so an update that names one member cannot silently reset
//! another to its default.

use std::collections::BTreeMap;

use super::StoredGroup;

/// The most lifecycle hooks one Auto Scaling group may hold.
///
/// From the model's own documentation for `PutLifecycleHook`: "If you exceed your
/// maximum limit of lifecycle hooks, which by default is 50 per Auto Scaling group,
/// the call fails." The member carries no modelled bound, so the handler checks it and
/// answers the operation's modelled `LimitExceededFault` past it.
pub const MAX_LIFECYCLE_HOOKS_PER_GROUP: usize = 50;

/// `HeartbeatTimeout` when a request that CREATES a hook omits it.
///
/// From the model's own documentation for `PutLifecycleHookType$HeartbeatTimeout`:
/// "The default value is 3600 seconds (1 hour)."
pub const DEFAULT_HEARTBEAT_TIMEOUT: i32 = 3600;

/// The smallest `HeartbeatTimeout` the service accepts, from the same documentation:
/// "The range is from 30 to 7200 seconds."
pub const MIN_HEARTBEAT_TIMEOUT: i32 = 30;

/// The largest `HeartbeatTimeout` the service accepts (same documentation).
pub const MAX_HEARTBEAT_TIMEOUT: i32 = 7200;

/// The ceiling on a hook's `GlobalTimeout`.
///
/// From the model's own documentation for `LifecycleHook$GlobalTimeout`: "The maximum
/// time, in seconds, that an instance can remain in a wait state. The maximum is 172800
/// seconds (48 hours) or 100 times HeartbeatTimeout, whichever is smaller."
pub const MAX_GLOBAL_TIMEOUT: i32 = 172_800;

/// How many heartbeat timeouts a `GlobalTimeout` spans before [`MAX_GLOBAL_TIMEOUT`]
/// caps it (same documentation: "100 times HeartbeatTimeout").
pub const GLOBAL_TIMEOUT_HEARTBEATS: i32 = 100;

/// `DefaultResult` when a request that CREATES a hook omits it.
///
/// From the model's own documentation for `PutLifecycleHookType$DefaultResult`: "The
/// default value is ABANDON."
pub const DEFAULT_LIFECYCLE_ACTION_RESULT: &str = "ABANDON";

/// The results a lifecycle hook may default to, from the same documentation: "Valid
/// values: CONTINUE | ABANDON".
pub const LIFECYCLE_ACTION_RESULTS: &[&str] = &["CONTINUE", DEFAULT_LIFECYCLE_ACTION_RESULT];

/// One lifecycle hook of a group, as it is stored.
///
/// The name is the key of [`StoredGroup::lifecycle_hooks`] rather than a field, so a
/// hook cannot be stored under a name that disagrees with its own.
#[derive(Debug, Clone)]
pub struct StoredLifecycleHook {
    /// `LifecycleTransition`: the transition the hook acts on. Required to create a
    /// hook, so every stored hook has one.
    pub lifecycle_transition: String,
    /// `NotificationTargetARN`, absent for a hook with no notification target — which
    /// is what an empty value in a request leaves behind.
    pub notification_target_arn: Option<String>,
    /// `RoleARN`.
    pub role_arn: Option<String>,
    /// `NotificationMetadata`.
    pub notification_metadata: Option<String>,
    /// `HeartbeatTimeout`, as requested or [`DEFAULT_HEARTBEAT_TIMEOUT`].
    pub heartbeat_timeout: i32,
    /// `DefaultResult`, as requested or [`DEFAULT_LIFECYCLE_ACTION_RESULT`].
    pub default_result: String,
}

impl StoredLifecycleHook {
    /// `GlobalTimeout`: a value DERIVED from the hook's own heartbeat timeout rather
    /// than one a caller supplies — "172800 seconds (48 hours) or 100 times
    /// HeartbeatTimeout, whichever is smaller".
    pub fn global_timeout(&self) -> i32 {
        self.heartbeat_timeout
            .saturating_mul(GLOBAL_TIMEOUT_HEARTBEATS)
            .min(MAX_GLOBAL_TIMEOUT)
    }
}

/// The hook members ONE request carries, each `Some` only where the request named it.
///
/// This is what keeps an omitted member apart from a supplied one, which is the whole
/// of the update rule: see this module's own documentation.
#[derive(Debug, Clone, Default)]
pub struct LifecycleHookPut {
    pub lifecycle_transition: Option<String>,
    pub notification_target_arn: Option<String>,
    pub role_arn: Option<String>,
    pub notification_metadata: Option<String>,
    pub heartbeat_timeout: Option<i32>,
    pub default_result: Option<String>,
}

/// Why a lifecycle hook could not be stored, for the operation to answer the error
/// class each situation belongs to.
#[derive(Debug, PartialEq, Eq)]
pub enum LifecycleHookRefusal {
    /// The request creates a hook but names no transition for it to act on.
    TransitionMissing,
    /// The transition the request names is not one a hook can be created for; carries
    /// what it named.
    UnknownTransition(String),
    /// The default result the request names is not one a hook can take; carries what it
    /// named.
    UnknownResult(String),
    /// The heartbeat timeout the request names is outside the documented range; carries
    /// what it named.
    HeartbeatOutOfRange(i32),
    /// The group already holds [`MAX_LIFECYCLE_HOOKS_PER_GROUP`] hooks and this request
    /// would create one more.
    LimitExceeded,
}

impl LifecycleHookRefusal {
    /// What the caller is told, for the hook the request named.
    ///
    /// ONE spelling per situation for every writer of a hook — the operation that
    /// upserts one and the group create that carries specifications — so two of them
    /// cannot refuse the same request differently. The hook's name is the only part
    /// taken from the request where the sentence does not quote the offending value.
    pub fn message(&self, hook_name: &str) -> String {
        match self {
            Self::TransitionMissing => transition_missing_message(hook_name),
            Self::UnknownTransition(transition) => unknown_transition_message(transition),
            Self::UnknownResult(result) => unknown_result_message(result),
            Self::HeartbeatOutOfRange(timeout) => heartbeat_timeout_violation_message(*timeout),
            Self::LimitExceeded => too_many_hooks_message(hook_name),
        }
    }
}

impl StoredGroup {
    /// The hook the group holds under `name`, if it holds one.
    pub fn lifecycle_hook(&self, name: &str) -> Option<&StoredLifecycleHook> {
        self.lifecycle_hooks.get(name)
    }

    /// Create or update the hook `name` of this group, per the update rule in this
    /// module's own documentation.
    pub fn put_lifecycle_hook(
        &mut self,
        name: &str,
        put: &LifecycleHookPut,
    ) -> Result<(), LifecycleHookRefusal> {
        put_lifecycle_hook(&mut self.lifecycle_hooks, name, put)
    }

    /// Remove the hook `name` from this group, answering whether it held one.
    pub fn delete_lifecycle_hook(&mut self, name: &str) -> bool {
        self.lifecycle_hooks.remove(name).is_some()
    }
}

/// The lifecycle hooks of ONE group, keyed by `LifecycleHookName`.
pub type LifecycleHooks = BTreeMap<String, StoredLifecycleHook>;

/// Create or update the hook `name` in `hooks`, per the update rule in this module's
/// own documentation.
///
/// Takes the COLLECTION rather than the group, so a group's hooks can be assembled
/// before the group itself exists — which is what lets `CreateAutoScalingGroup` take
/// its lifecycle hook specifications before it allocates or stores anything.
///
/// Every rule is checked BEFORE the first member is written, so a refused request
/// leaves the hook — and the rest of the collection — exactly as it was.
pub fn put_lifecycle_hook(
    hooks: &mut LifecycleHooks,
    name: &str,
    put: &LifecycleHookPut,
) -> Result<(), LifecycleHookRefusal> {
    if let Some(transition) = put.lifecycle_transition.as_deref() {
        if !super::LIFECYCLE_HOOK_TYPES.contains(&transition) {
            return Err(LifecycleHookRefusal::UnknownTransition(
                transition.to_string(),
            ));
        }
    }
    if let Some(result) = put.default_result.as_deref() {
        if !LIFECYCLE_ACTION_RESULTS.contains(&result) {
            return Err(LifecycleHookRefusal::UnknownResult(result.to_string()));
        }
    }
    if let Some(timeout) = put.heartbeat_timeout {
        if !(MIN_HEARTBEAT_TIMEOUT..=MAX_HEARTBEAT_TIMEOUT).contains(&timeout) {
            return Err(LifecycleHookRefusal::HeartbeatOutOfRange(timeout));
        }
    }

    match hooks.get_mut(name) {
        // An UPDATE: every member the request omits keeps the value the hook already
        // carries, and the empty notification target clears the stored one.
        Some(stored) => {
            if let Some(transition) = put.lifecycle_transition.clone() {
                stored.lifecycle_transition = transition;
            }
            if let Some(target) = put.notification_target_arn.as_deref() {
                stored.notification_target_arn = (!target.is_empty()).then(|| target.to_string());
            }
            if let Some(role) = put.role_arn.clone() {
                stored.role_arn = Some(role);
            }
            if let Some(metadata) = put.notification_metadata.clone() {
                stored.notification_metadata = Some(metadata);
            }
            if let Some(timeout) = put.heartbeat_timeout {
                stored.heartbeat_timeout = timeout;
            }
            if let Some(result) = put.default_result.clone() {
                stored.default_result = result;
            }
            Ok(())
        }
        // A CREATE: the transition is required, the group's own limit applies, and the
        // members the request omits take their documented defaults.
        None => {
            let Some(transition) = put.lifecycle_transition.clone() else {
                return Err(LifecycleHookRefusal::TransitionMissing);
            };
            if hooks.len() >= MAX_LIFECYCLE_HOOKS_PER_GROUP {
                return Err(LifecycleHookRefusal::LimitExceeded);
            }
            hooks.insert(
                name.to_string(),
                StoredLifecycleHook {
                    lifecycle_transition: transition,
                    notification_target_arn: put
                        .notification_target_arn
                        .as_deref()
                        .filter(|target| !target.is_empty())
                        .map(str::to_string),
                    role_arn: put.role_arn.clone(),
                    notification_metadata: put.notification_metadata.clone(),
                    heartbeat_timeout: put.heartbeat_timeout.unwrap_or(DEFAULT_HEARTBEAT_TIMEOUT),
                    default_result: put
                        .default_result
                        .clone()
                        .unwrap_or_else(|| DEFAULT_LIFECYCLE_ACTION_RESULT.to_string()),
                },
            );
            Ok(())
        }
    }
}

/// A catalog as a refusal states it, through the one spelling this service states every
/// admissible value set in ([`super::stated_value_set`]).
fn stated_catalog(values: &[&str]) -> String {
    super::stated_value_set(values.iter().copied())
}

/// The message a request naming a transition no lifecycle hook can be created for is
/// refused with, in the shape this service refuses every value outside one of its own
/// catalogs: the member as the model names it, the value from the request, and the
/// catalog the caller has to choose from.
pub fn unknown_transition_message(transition: &str) -> String {
    format!(
        "The LifecycleTransition value {transition} is not valid: it must be one of {}",
        stated_catalog(super::LIFECYCLE_HOOK_TYPES)
    )
}

/// The message a request naming a default result outside the documented pair is refused
/// with, in the same shape.
pub fn unknown_result_message(result: &str) -> String {
    format!(
        "The DefaultResult value {result} is not valid: it must be one of {}",
        stated_catalog(LIFECYCLE_ACTION_RESULTS)
    )
}

/// The message a request whose `HeartbeatTimeout` is outside the documented range is
/// refused with: the standard sentence this service states for a member value outside
/// its bounds, naming the member as the wire names it and the bound the value missed.
pub fn heartbeat_timeout_violation_message(timeout: i32) -> String {
    let constraint = if timeout < MIN_HEARTBEAT_TIMEOUT {
        format!("Member must have value greater than or equal to {MIN_HEARTBEAT_TIMEOUT}")
    } else {
        format!("Member must have value less than or equal to {MAX_HEARTBEAT_TIMEOUT}")
    };
    super::constraint_violation_message(timeout, "heartbeatTimeout", &constraint)
}

/// The message a request that would create a hook past
/// [`MAX_LIFECYCLE_HOOKS_PER_GROUP`] is refused with, which quotes the limit the caller
/// has to come back under — the shape this service reports every per-group ceiling in.
pub fn too_many_hooks_message(name: &str) -> String {
    format!(
        "The lifecycle hook {name} exceeds the limit of {MAX_LIFECYCLE_HOOKS_PER_GROUP} lifecycle \
         hooks per Auto Scaling group"
    )
}

/// The message a request that creates a hook without naming a transition is refused
/// with: the model documents `LifecycleTransition` as "Required for new lifecycle hooks,
/// but optional when updating existing hooks", so the refusal says which of the two the
/// request is.
pub fn transition_missing_message(name: &str) -> String {
    format!(
        "The LifecycleTransition is required to create the lifecycle hook {name}, and may be \
         omitted only when updating one that already exists"
    )
}
