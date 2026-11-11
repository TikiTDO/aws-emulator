// SPDX-License-Identifier: CC0-1.0

//! What a PLACEMENT may override on the revision it runs.
//!
//! `RunTask` and `StartTask` carry a `TaskOverride`, whose members replace values the
//! STORED revision already fixed — "instead of the default value from the task
//! definition", as every `ContainerOverride` member's doc puts it. That makes every rule
//! here cross-field, keyed on state an earlier registration wrote, so none of them can be
//! expressed in the model:
//!
//! * a container override names the container it is for, and only a container the
//!   revision declares can receive one (`ContainerOverride$name`: "The name of the
//!   container that receives the override. This parameter is required if any override is
//!   specified.");
//! * the task role the placement names is the role the task assumes, in place of the one
//!   the revision declared ([`resolved_task_role_arn`]);
//! * the reservations an override replaces stay inside the TASK's own size —
//!   `ContainerDefinition$cpu`: "the only requirement is that the total amount of CPU
//!   reserved for all containers within a task be lower than the task-level `cpu` value";
//!   `ContainerDefinition$memory`: "The total amount of memory reserved for all
//!   containers within a task must be lower than the task `memory` value, if one is
//!   specified";
//! * a task-level `cpu`/`memory` override names a task size, so where the task runs on
//!   Fargate it names one of the published Fargate configurations
//!   ([`is_fargate_configuration`]);
//! * and the ephemeral storage it asks for stays in the range that member admits
//!   ([`ephemeral_storage_violation`]).
//!
//! Spelled ONCE for both placements, so an override one of them refuses cannot be run by
//! the other.

use super::*;
use crate::ops::types::ContainerOverride;

/// The noun `cpu` is counted in, for a refusal that has to state the unit.
const CPU_UNITS_NOUN: &str = "CPU units";

/// The noun `memory` is counted in.
const MEMORY_MIB_NOUN: &str = "MiB";

/// `InvalidParameterException$message` for a `containerOverrides` entry that carries an
/// override but names no container to apply it to. One fixed sentence, naming the member
/// and nothing of the request, worded as this service words every other missing
/// container member ([`container_member_required_message`]).
pub const CONTAINER_OVERRIDE_NAME_REQUIRED_MESSAGE: &str =
    "ContainerOverride.name should not be null or empty.";

/// `InvalidParameterException$message` for a `containerOverrides` entry naming a
/// container the revision does not declare. The sentence names THAT container, because it
/// is the one value the caller has to change and a task definition can carry several.
pub fn unknown_container_override_message(container_name: &str) -> String {
    format!(
        "Override for container named {container_name} is not a container in the \
         TaskDefinition."
    )
}

/// `InvalidParameterException$message` for a task-level `cpu` or `memory` override whose
/// value names no number of units at all. Amazon ECS names the member, the value AS THE
/// REQUEST SENT IT, and the unit the member counts in.
pub fn invalid_task_override_message(member: &str, value: &str, unit: &str) -> String {
    format!(
        "The specified {member} value '{value}' is invalid. Specify the number of {unit} \
         the task reserves and try again."
    )
}

/// `InvalidParameterException$message` for ONE container whose hard memory limit, after
/// the override, is above the whole task's. Amazon ECS names the member and the container
/// the override was asked for, and carries neither amount.
pub fn container_memory_above_task_message(container_name: &str) -> String {
    format!("'memory' setting for container '{container_name}' is greater than for task.")
}

/// `InvalidParameterException$message` for container reservations that add up past the
/// TASK's own size. Amazon ECS states the member, in the task definition's own terms, and
/// carries neither the total it reached nor the limit it crossed.
pub fn container_total_above_task_message(member: &str) -> String {
    format!("Sum of container '{member}' greater than task '{member}'.")
}

/// The role a `taskRoleArn` member names, or `None` when it names none: an absent member
/// supplies no role, and neither does one spelling nothing but whitespace.
pub fn declared_task_role_arn(task_role_arn: Option<&str>) -> Option<&str> {
    task_role_arn
        .map(str::trim)
        .filter(|arn| !arn.is_empty())
}

/// The IAM role the task a placement launches assumes: the one its `overrides` name, and
/// the one the STORED revision declares when the overrides name none.
///
/// `TaskOverride$taskRoleArn` is "The Amazon Resource Name (ARN) of the role that
/// containers in this task can assume. All containers in this task are granted the
/// permissions that are specified in this role" — an OVERRIDE, so it stands in place of
/// the value the revision fixed ("instead of the default value from the task definition",
/// as every other override member's doc puts it) and the revision's own `taskRoleArn` is
/// the DEFAULT it replaces. So every rule keyed on the task's role is decided on this
/// resolved value, and a placement that supplies the role itself needs nothing of the
/// revision.
///
/// Spelled ONCE for both placements that carry a `TaskOverride`, so `RunTask` and
/// `StartTask` resolve the same request to the same role.
pub fn resolved_task_role_arn<'a>(
    overrides: Option<&'a TaskOverride>,
    definition: &'a TaskDefinitionRecord,
) -> Option<&'a str> {
    declared_task_role_arn(
        overrides.and_then(|overrides| overrides.task_role_arn.as_deref()),
    )
    .or_else(|| declared_task_role_arn(definition.task_role_arn.as_deref()))
}

/// The IAM role Amazon ECS itself acts as for the task a placement launches — the one its
/// `overrides` name, and the one the STORED revision declares when the overrides name
/// none.
///
/// `TaskOverride$executionRoleArn` is "The Amazon Resource Name (ARN) of the task execution
/// role override for the task", so like the task role it stands in place of the value the
/// revision fixed (`TaskDefinition$executionRoleArn`: "the task execution role that grants
/// the Amazon ECS container agent permission to make Amazon Web Services API calls on your
/// behalf"). It is the principal the image pull of this task is made as
/// ([`evaluate_image_pulls`]).
///
/// Spelled ONCE for both placements that carry a `TaskOverride`, exactly as
/// [`resolved_task_role_arn`] is, so `RunTask` and `StartTask` resolve the same request to
/// the same role.
pub fn resolved_execution_role_arn<'a>(
    overrides: Option<&'a TaskOverride>,
    definition: &'a TaskDefinitionRecord,
) -> Option<&'a str> {
    declared_task_role_arn(
        overrides.and_then(|overrides| overrides.execution_role_arn.as_deref()),
    )
    .or_else(|| declared_task_role_arn(definition.execution_role_arn.as_deref()))
}

/// The override a `containerOverrides` list carries for ONE container of the revision, or
/// `None` when the list names it nowhere.
fn override_for<'a>(
    overrides: &'a [ContainerOverride],
    container_name: Option<&str>,
) -> Option<&'a ContainerOverride> {
    let container_name = container_name?;
    overrides
        .iter()
        .find(|entry| entry.name.as_deref() == Some(container_name))
}

/// Whether the task this placement creates holds a FARGATE task size, which is what
/// decides whether the size an override names has to be one of the published Fargate
/// configurations.
///
/// The launch type the request named, or the capacity providers its strategy did
/// ([`places_on_fargate`]) — and otherwise the revision's own registration:
/// `requiresCompatibilities` is "The task launch type that Amazon ECS validates the task
/// definition against", so a revision registered for Fargate was stored with a Fargate
/// task size ([`is_fargate_configuration`] is what the registration refused anything else
/// by) and an override of it names one too, whichever placement runs it.
pub fn runs_fargate_task_size(
    definition: &TaskDefinitionRecord,
    launch_type: Option<&str>,
    strategy: &[CapacityProviderStrategyItem],
) -> bool {
    places_on_fargate(launch_type, strategy)
        || definition
            .requires_compatibilities
            .iter()
            .any(|compatibility| compatibility == LAUNCH_TYPE_FARGATE)
}

/// The refusal a placement's `overrides` earns, or `None` when every member of it is one
/// the stored revision can be run with.
///
/// `on_fargate` is whether the task this placement creates runs on Fargate — the launch
/// type it named, the capacity providers its strategy did, or the compatibility the
/// revision was registered for — which is what decides whether the task size it names has
/// to be one of the published Fargate configurations.
///
/// The members are examined in a FIXED order (the ephemeral storage, the task size, each
/// container override, then the totals), so the refusal a request earns never depends on
/// anything but its values.
pub fn task_override_violation(
    definition: &TaskDefinitionRecord,
    overrides: Option<&TaskOverride>,
    on_fargate: bool,
) -> Option<String> {
    let overrides = overrides?;

    // "The ephemeral storage setting override for the task" is the same structure the
    // registration carries, so it is held to the same range.
    if let Some(message) = ephemeral_storage_violation(overrides.ephemeral_storage.as_ref()) {
        return Some(message);
    }

    // The task size this placement runs with: what the override names, else what the
    // revision already fixed.
    let cpu = overrides.cpu.clone().or_else(|| definition.cpu.clone());
    let memory = overrides.memory.clone().or_else(|| definition.memory.clone());
    // A task size the override CHANGED is validated the way the registration's own is: a
    // Fargate task runs one of the published configurations, and an override that moved
    // the pair outside the catalog — including one that spells no number of units at all —
    // names a task Fargate cannot run.
    let overrides_task_size = overrides.cpu.is_some() || overrides.memory.is_some();
    if on_fargate && overrides_task_size {
        if let (Some(cpu), Some(memory)) = (cpu.as_deref(), memory.as_deref()) {
            if !is_fargate_configuration(
                cpu,
                memory,
                &requested_os_family(definition.runtime_platform.as_ref()),
            ) {
                return Some(FARGATE_TASK_SIZE_MESSAGE.to_string());
            }
        }
    }
    // Off Fargate there is no catalog to check the pair against, but each member still has
    // to name a number of the units it counts in: the task's reservation is what the
    // containers below are weighed against.
    if !on_fargate {
        if let Some(requested) = overrides.cpu.as_deref() {
            if cpu_units(requested).is_none() {
                return Some(invalid_task_override_message("cpu", requested, CPU_UNITS_NOUN));
            }
        }
        if let Some(requested) = overrides.memory.as_deref() {
            if memory_mib(requested).is_none() {
                return Some(invalid_task_override_message(
                    "memory",
                    requested,
                    MEMORY_MIB_NOUN,
                ));
            }
        }
    }

    let container_overrides = overrides
        .container_overrides
        .as_deref()
        .unwrap_or_default();
    let task_memory = memory.as_deref().and_then(memory_mib);
    for entry in container_overrides {
        let Some(container_name) = entry
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
        else {
            return Some(CONTAINER_OVERRIDE_NAME_REQUIRED_MESSAGE.to_string());
        };
        let Some(container) = definition
            .container_definitions
            .iter()
            .find(|container| container.name.as_deref() == Some(container_name))
        else {
            return Some(unknown_container_override_message(container_name));
        };
        // Each reservation the override REPLACES still has to name a value its own unit
        // admits, exactly as the registration's did — the overridden values are what the
        // container would run with.
        if let Some(message) = container_reservation_violation(
            container_name,
            entry.cpu.unwrap_or(container.cpu),
            entry.memory.or(container.memory),
            entry.memory_reservation.or(container.memory_reservation),
        ) {
            return Some(message);
        }
        // "The hard limit (in MiB) of memory to present to the container": one container
        // of a task cannot be presented more memory than the whole task reserves, so the
        // amount this override asks for is weighed against the task's own and the refusal
        // names the container it was asked for.
        let presented = entry.memory.or(container.memory).map(i64::from);
        if let (Some(limit), Some(presented)) = (task_memory, presented) {
            if presented > limit {
                return Some(container_memory_above_task_message(container_name));
            }
        }
    }

    // And the totals, with every override applied: the containers of a task share the
    // task's own reservation, so what they add up to is bounded by the task size above.
    for (member, limit, total) in [
        (
            "cpu",
            cpu.as_deref().and_then(cpu_units),
            definition
                .container_definitions
                .iter()
                .map(|container| {
                    i64::from(
                        override_for(container_overrides, container.name.as_deref())
                            .and_then(|entry| entry.cpu)
                            .unwrap_or(container.cpu),
                    )
                })
                .sum::<i64>(),
        ),
        (
            "memory",
            task_memory,
            definition
                .container_definitions
                .iter()
                .map(|container| {
                    i64::from(
                        override_for(container_overrides, container.name.as_deref())
                            .and_then(|entry| entry.memory)
                            .or(container.memory)
                            .unwrap_or_default(),
                    )
                })
                .sum::<i64>(),
        ),
    ] {
        // A revision that reserves nothing at the task level bounds nothing: "if one is
        // specified" is the member's own condition.
        if let Some(limit) = limit {
            if total > limit {
                return Some(container_total_above_task_message(member));
            }
        }
    }
    None
}
