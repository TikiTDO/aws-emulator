// SPDX-License-Identifier: CC0-1.0

//! The cross-field rules a REGISTRATION's container definitions have to meet before a
//! revision exists: the members whose values Amazon ECS fetches from another service on the
//! task's behalf and so need the task execution role, the log driver that only works beside
//! a log router, and a per-container resource limit whose two halves bound each other.
//!
//! Each rule is about the container list as a WHOLE or about one container's own members,
//! and none of them is expressible in the model — so each is checked here, once, for every
//! operation that registers container definitions ([`crate::ops::register_task_definition`]
//! and [`crate::ops::register_daemon_task_definition`]), which is why the checks read a
//! borrowed [`ContainerCredentials`] view rather than one of the two container shapes this
//! API declares. A registration that breaks one of them is refused BEFORE any revision is
//! allocated, so the family's revision counter does not advance for a request Amazon ECS
//! never persists.

use crate::ops::types::{
    ContainerDefinition, DaemonContainerDefinition, LogConfiguration, Ulimit,
};

/// The `LogConfiguration$logDriver` that sends a container's output to a LOG ROUTER running
/// beside it in the same task rather than shipping it anywhere itself, one of the
/// `LogDriver` enum values (`smithy_query(query="shape",shape="LogDriver")`).
pub const LOG_DRIVER_AWSFIRELENS: &str = "awsfirelens";

/// What one container definition says about the values Amazon ECS has to FETCH for it and
/// about its logging, read off whichever container shape the registration carries
/// (`ContainerDefinition` or `DaemonContainerDefinition`, whose members for these rules are
/// the same).
#[derive(Debug, Clone, Copy)]
pub struct ContainerCredentials<'a> {
    /// `ContainerDefinition$name` — the container a refusal names.
    pub name: &'a str,
    /// Whether the container takes any value Amazon ECS has to read from Secrets Manager or
    /// Systems Manager for it: `secrets`, or the `secretOptions` of its log configuration.
    pub takes_secrets: bool,
    /// Whether the container's image comes from a private registry Amazon ECS has to
    /// authenticate to: `repositoryCredentials`.
    pub takes_registry_credentials: bool,
    /// `LogConfiguration$logDriver`, when the container configures logging at all.
    pub log_driver: Option<&'a str>,
    /// Whether the container IS the task's log router: it carries a `firelensConfiguration`.
    pub is_log_router: bool,
    /// `ContainerDefinition$ulimits` — the resource limits the container sets.
    pub ulimits: &'a [Ulimit],
}

/// Whether a log configuration carries any `secretOptions` entry — a value the agent reads
/// for the container, exactly as a `secrets` entry is.
fn configures_log_secrets(logging: Option<&LogConfiguration>) -> bool {
    logging.is_some_and(|logging| {
        logging
            .secret_options
            .as_ref()
            .is_some_and(|options| !options.is_empty())
    })
}

impl<'a> ContainerCredentials<'a> {
    /// Read one `ContainerDefinition` of a task-definition registration.
    pub fn of_container(definition: &'a ContainerDefinition) -> Self {
        Self {
            name: definition.name.as_deref().unwrap_or_default(),
            takes_secrets: definition
                .secrets
                .as_ref()
                .is_some_and(|secrets| !secrets.is_empty())
                || configures_log_secrets(definition.log_configuration.as_ref()),
            takes_registry_credentials: definition.repository_credentials.is_some(),
            log_driver: definition
                .log_configuration
                .as_ref()
                .map(|logging| logging.log_driver.as_str()),
            is_log_router: definition.firelens_configuration.is_some(),
            ulimits: definition.ulimits.as_deref().unwrap_or_default(),
        }
    }

    /// Read one `DaemonContainerDefinition` of a daemon registration: the same members,
    /// declared on that resource's own container shape.
    pub fn of_daemon_container(definition: &'a DaemonContainerDefinition) -> Self {
        Self {
            name: definition.name.as_deref().unwrap_or_default(),
            takes_secrets: definition
                .secrets
                .as_ref()
                .is_some_and(|secrets| !secrets.is_empty())
                || configures_log_secrets(definition.log_configuration.as_ref()),
            takes_registry_credentials: definition.repository_credentials.is_some(),
            log_driver: definition
                .log_configuration
                .as_ref()
                .map(|logging| logging.log_driver.as_str()),
            is_log_router: definition.firelens_configuration.is_some(),
            ulimits: definition.ulimits.as_deref().unwrap_or_default(),
        }
    }
}

/// `ClientException$message` for a registration that asks Amazon ECS to read a container's
/// `secrets` (or its log configuration's `secretOptions`) without naming the role it would
/// read them under.
///
/// `TaskDefinition$executionRoleArn` is "the task execution role that grants the Amazon ECS
/// container agent permission to make Amazon Web Services API calls on your behalf": the
/// agent reads every such value under THAT role, so a revision that references one and names
/// no role references a call nothing could sign. ONE fixed sentence, naming the member that
/// is missing and none of the request's values.
pub const SECRETS_EXECUTION_ROLE_REQUIRED_MESSAGE: &str =
    "When you are specifying container secrets, you must also specify a value for \
     'executionRoleArn'.";

/// `ClientException$message` for the same rule read on a container whose IMAGE comes from a
/// private registry: `RepositoryCredentials$credentialsParameter` names the secret holding
/// that registry's credentials, which the agent reads under the task execution role, so a
/// revision that names the secret and no role cannot be pulled.
pub const PRIVATE_REGISTRY_EXECUTION_ROLE_REQUIRED_MESSAGE: &str =
    "Private repository authentication requires that the task definition specify a value \
     for executionRoleArn.";

/// `ClientException$message` for a task definition in which a container sends its output to
/// a log router no container of it provides.
///
/// `LogConfiguration$logDriver` [`LOG_DRIVER_AWSFIRELENS`] delivers the container's output to
/// the task's log router, and a task's log router is the container that carries a
/// `firelensConfiguration` ("the FireLens configuration for the container ... used to specify
/// and configure a log router for container logs"). With no such container in the list, the
/// driver has nothing to deliver to, so the revision could emit no log at all.
pub const FIRELENS_LOG_ROUTER_REQUIRED_MESSAGE: &str =
    "When awsfirelens log driver is specified in log configuration, a firelens \
     configuration object must be configured in one of the containers.";

/// `ClientException$message` for a `ulimit` whose soft limit is above its hard limit.
///
/// `Ulimit$softLimit` is "the soft limit for the ulimit type" and `Ulimit$hardLimit` "the
/// hard limit", the ceiling the soft limit is raised toward: a soft limit above it names a
/// reservation the limit can never admit. The sentence names the `Ulimit$name` the pair
/// belongs to — the one value a caller needs to find it among the container's limits — and
/// carries neither limit, so a pair that is EQUAL is accepted.
pub fn ulimit_soft_above_hard_message(ulimit_name: &str) -> String {
    format!("Soft limit exceeds hard limit for ulimit {ulimit_name}")
}

/// The FIRST of a registration's container-definition cross-field failures, stated as the
/// refusal's message — or `None` when the list breaks none of the rules.
///
/// The rules are checked in a FIXED order so the refusal a list earns never depends on
/// anything but its values: per container, in the order the request lists them, the values
/// Amazon ECS would have to fetch for it and then its resource limits; then the one rule
/// about the list as a whole, the log router its drivers deliver to.
pub fn container_credential_violation(
    containers: &[ContainerCredentials<'_>],
    execution_role_arn: Option<&str>,
) -> Option<String> {
    let has_execution_role = execution_role_arn
        .map(str::trim)
        .is_some_and(|arn| !arn.is_empty());
    for container in containers {
        if !has_execution_role {
            for (asks, message) in [
                (container.takes_secrets, SECRETS_EXECUTION_ROLE_REQUIRED_MESSAGE),
                (
                    container.takes_registry_credentials,
                    PRIVATE_REGISTRY_EXECUTION_ROLE_REQUIRED_MESSAGE,
                ),
            ] {
                if asks {
                    return Some(message.to_string());
                }
            }
        }
        for ulimit in container.ulimits {
            if ulimit.soft_limit > ulimit.hard_limit {
                return Some(ulimit_soft_above_hard_message(&ulimit.name));
            }
        }
    }
    // The log router is a property of the LIST: any container may provide it, which is what
    // "configured in one of the containers" states.
    let routed = containers
        .iter()
        .any(|container| container.log_driver == Some(LOG_DRIVER_AWSFIRELENS));
    let has_router = containers.iter().any(|container| container.is_log_router);
    if routed && !has_router {
        return Some(FIRELENS_LOG_ROUTER_REQUIRED_MESSAGE.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::LOG_DRIVER_AWSLOGS;

    /// A container that asks for nothing: the base every row below changes one member of.
    fn plain(name: &str) -> ContainerCredentials<'_> {
        ContainerCredentials {
            name,
            takes_secrets: false,
            takes_registry_credentials: false,
            log_driver: None,
            is_log_router: false,
            ulimits: &[],
        }
    }

    /// How one row of the table below makes its container ask Amazon ECS to fetch a value.
    type AsksForAFetchedValue = fn(&mut ContainerCredentials<'_>);

    /// RULE: a value Amazon ECS has to fetch for a container needs the task execution role,
    /// whichever member asks for it.
    #[test]
    fn a_fetched_value_needs_the_execution_role() {
        // (case, the member that asks, the sentence the refusal carries)
        let rows: &[(&str, AsksForAFetchedValue, &str)] = &[
            (
                "secrets",
                |container| container.takes_secrets = true,
                SECRETS_EXECUTION_ROLE_REQUIRED_MESSAGE,
            ),
            (
                "repositoryCredentials",
                |container| container.takes_registry_credentials = true,
                PRIVATE_REGISTRY_EXECUTION_ROLE_REQUIRED_MESSAGE,
            ),
        ];
        for (case, asks, message) in rows {
            let mut container = plain("app");
            asks(&mut container);
            let refused = container_credential_violation(&[container], None);
            assert_eq!(
                refused.as_deref(),
                Some(*message),
                "case {case:?}: expected the execution-role refusal, got {refused:?}"
            );
            let with_role = container_credential_violation(
                &[container],
                Some("arn:aws:iam::000000000000:role/execution"),
            );
            assert!(
                with_role.is_none(),
                "case {case:?}: a named role satisfies the requirement, got {with_role:?}"
            );
        }
    }

    /// RULE: the `awsfirelens` driver needs a log router in the same task definition, and any
    /// container of the list may be it.
    #[test]
    fn the_firelens_driver_needs_a_router_in_the_list() {
        let mut routed = plain("app");
        routed.log_driver = Some(LOG_DRIVER_AWSFIRELENS);
        let alone = container_credential_violation(&[routed], None);
        assert_eq!(
            alone.as_deref(),
            Some(FIRELENS_LOG_ROUTER_REQUIRED_MESSAGE),
            "case \"no router\": expected the log-router refusal, got {alone:?}"
        );
        let mut router = plain("log-router");
        router.is_log_router = true;
        let beside = container_credential_violation(&[routed, router], None);
        assert!(
            beside.is_none(),
            "case \"a router beside it\": expected the list to be accepted, got {beside:?}"
        );
        let mut other_driver = plain("app");
        other_driver.log_driver = Some(LOG_DRIVER_AWSLOGS);
        let unrouted = container_credential_violation(&[other_driver], None);
        assert!(
            unrouted.is_none(),
            "case \"another driver\": the rule is about the awsfirelens driver alone, got \
             {unrouted:?}"
        );
    }

    /// RULE: a ulimit's soft limit may not exceed its hard limit; an equal pair is a
    /// reservation the limit admits.
    #[test]
    fn a_soft_limit_may_not_exceed_its_hard_limit() {
        // (case, softLimit, hardLimit, refused)
        let rows: &[(&str, i32, i32, bool)] = &[
            ("soft above hard", 8192, 4096, true),
            ("soft equal to hard", 4096, 4096, false),
            ("soft below hard", 1024, 4096, false),
            ("both at zero", 0, 0, false),
            ("soft above a negative hard", 1, -1, true),
        ];
        for (case, soft_limit, hard_limit, refused) in rows {
            let ulimits = [Ulimit {
                name: "nofile".to_string(),
                soft_limit: *soft_limit,
                hard_limit: *hard_limit,
            }];
            let mut container = plain("app");
            container.ulimits = &ulimits;
            let answer = container_credential_violation(&[container], None);
            assert_eq!(
                answer.is_some(),
                *refused,
                "case {case:?}: expected refused == {refused}, got {answer:?}"
            );
            if *refused {
                assert_eq!(
                    answer.as_deref(),
                    Some(ulimit_soft_above_hard_message("nofile").as_str()),
                    "case {case:?}: expected the ulimit refusal to name the limit, got \
                     {answer:?}"
                );
            }
        }
    }
}
