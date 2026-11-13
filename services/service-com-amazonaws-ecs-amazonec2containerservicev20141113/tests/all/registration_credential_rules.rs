// SPDX-License-Identifier: CC0-1.0

//! The cross-field rules a REGISTRATION's container definitions have to meet, driven end to
//! end over the shared wire harness in [`crate::ecs_wire`].
//!
//! THE RULES UNDER TEST:
//!
//! * A container that asks Amazon ECS to FETCH a value for it needs the task execution role
//!   the agent would fetch it under (`TaskDefinition$executionRoleArn`: "the task execution
//!   role that grants the Amazon ECS container agent permission to make Amazon Web Services
//!   API calls on your behalf"): a `secrets` entry, a log configuration's `secretOptions`
//!   entry, or the private-registry credential a `repositoryCredentials` names. A
//!   registration that asks for one and names no role is refused HTTP 400 `ClientException`,
//!   whatever launch type it declares.
//! * `LogConfiguration$logDriver` `awsfirelens` delivers a container's output to the task's
//!   LOG ROUTER, and a log router is a container carrying a `firelensConfiguration`: a
//!   definition that uses the driver and holds no such container is refused.
//! * A `ulimit` whose `softLimit` is above its `hardLimit` names a reservation the limit can
//!   never admit, and is refused.
//! * Every one of these is judged BEFORE the family's revision counter moves, so a refused
//!   registration consumes no revision: the next accepted one is `:1`.
//!
//! The tables span what the rules generalise over: WHICH member asks for a fetched value,
//! whether the role is named, the launch type and network mode the revision declares, which
//! container of the list carries the router and which carries the driver, where the offending
//! `ulimit` sits in its list, and — for the rule shared with the daemon registration — which
//! of the two registering operations the request is sent to.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, refusal, service, string, ACCOUNT};

/// The image every container below starts from: a reference naming a registry this emulator
/// answers nothing for, so the image fetch decides nothing here.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// A task execution role of this account, as `TaskDefinition$executionRoleArn` carries one.
fn execution_role() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/ecsTaskExecutionRole")
}

/// A Secrets Manager secret a `secrets` or `secretOptions` entry reads a value from
/// (`Secret$valueFrom`: "the full ARN of the Secrets Manager secret").
fn secret_arn() -> String {
    format!("arn:aws:secretsmanager:us-west-2:{ACCOUNT}:secret:db-AbCdEf")
}

/// The sentence a registration whose container references a secret without a task execution
/// role is refused with.
const SECRETS_NEED_THE_ROLE: &str =
    "When you are specifying container secrets, you must also specify a value for \
     'executionRoleArn'.";

/// The sentence a registration whose container pulls from a private registry without a task
/// execution role is refused with.
const PRIVATE_REGISTRY_NEEDS_THE_ROLE: &str =
    "Private repository authentication requires that the task definition specify a value \
     for executionRoleArn.";

/// The sentence a registration that uses the FireLens log driver with no log router in the
/// definition is refused with.
const FIRELENS_NEEDS_A_ROUTER: &str =
    "When awsfirelens log driver is specified in log configuration, a firelens \
     configuration object must be configured in one of the containers.";

/// The sentence a `ulimit` whose soft limit is above its hard limit is refused with: it
/// names the `Ulimit$name` the pair belongs to and neither limit.
fn soft_above_hard(ulimit_name: &str) -> String {
    format!("Soft limit exceeds hard limit for ulimit {ulimit_name}")
}

/// The modelled error class every one of these refusals carries
/// (`smithy_query(query="operation",operation="RegisterTaskDefinition")` -> errors:
/// `ClientException` 400).
const CLIENT_EXCEPTION: &str = "ClientException";

/// One container definition: a name, an image and its own memory, plus whatever the row
/// declares on top.
fn container(name: &str, declared: Value) -> Value {
    let mut definition = json!({ "name": name, "image": IMAGE, "memory": 128 });
    for (member, value) in declared.as_object().into_iter().flatten() {
        definition[member.as_str()] = value.clone();
    }
    definition
}

/// One row of the table below: a registration request and whether it is served.
struct Row {
    case: &'static str,
    /// The operation the row is sent to — the plain or the daemon registration.
    operation: &'static str,
    /// What the request carries beyond its `family`.
    request: Value,
    /// The message the refusal carries, or `None` for a registration that is served.
    refused_with: Option<String>,
}

/// The `family` the row's registration uses, distinct per row so each row's revision
/// counter is its own.
fn family(index: usize) -> String {
    format!("credential-rules-{index}")
}

#[tokio::test]
async fn a_registration_is_judged_against_the_values_its_containers_ask_amazon_ecs_to_fetch() {
    // The rows differ along the member that asks for a fetched value, whether the role is
    // named, the launch type and network mode, the container of the list the asking member
    // sits in, and which of the two registering operations carries the request.
    let rows: Vec<Row> = vec![
        Row {
            case: "a Fargate container with secrets and no execution role",
            operation: "RegisterTaskDefinition",
            request: json!({
                "cpu": "2048",
                "memory": "4096",
                "networkMode": "awsvpc",
                "requiresCompatibilities": ["FARGATE"],
                "containerDefinitions": [container(
                    "app",
                    json!({ "secrets": [{ "name": "DB", "valueFrom": secret_arn() }] }),
                )],
            }),
            refused_with: Some(SECRETS_NEED_THE_ROLE.to_string()),
        },
        Row {
            case: "an EC2 container with secrets and no execution role",
            operation: "RegisterTaskDefinition",
            request: json!({
                "networkMode": "bridge",
                "containerDefinitions": [container(
                    "app",
                    json!({ "secrets": [{ "name": "DB", "valueFrom": secret_arn() }] }),
                )],
            }),
            refused_with: Some(SECRETS_NEED_THE_ROLE.to_string()),
        },
        Row {
            case: "the SECOND container asks for the secret",
            operation: "RegisterTaskDefinition",
            request: json!({
                "networkMode": "bridge",
                "containerDefinitions": [
                    container("sidecar", json!({})),
                    container(
                        "app",
                        json!({ "secrets": [{ "name": "DB", "valueFrom": secret_arn() }] }),
                    ),
                ],
            }),
            refused_with: Some(SECRETS_NEED_THE_ROLE.to_string()),
        },
        Row {
            case: "a log configuration's secretOptions with no execution role",
            operation: "RegisterTaskDefinition",
            request: json!({
                "networkMode": "bridge",
                "containerDefinitions": [container(
                    "app",
                    json!({
                        "logConfiguration": {
                            "logDriver": "splunk",
                            "secretOptions": [{ "name": "token", "valueFrom": secret_arn() }],
                        },
                    }),
                )],
            }),
            refused_with: Some(SECRETS_NEED_THE_ROLE.to_string()),
        },
        Row {
            case: "secrets WITH an execution role",
            operation: "RegisterTaskDefinition",
            request: json!({
                "networkMode": "bridge",
                "executionRoleArn": execution_role(),
                "containerDefinitions": [container(
                    "app",
                    json!({ "secrets": [{ "name": "DB", "valueFrom": secret_arn() }] }),
                )],
            }),
            refused_with: None,
        },
        Row {
            case: "private registry credentials with no execution role",
            operation: "RegisterTaskDefinition",
            request: json!({
                "networkMode": "bridge",
                "containerDefinitions": [container(
                    "app",
                    json!({
                        "repositoryCredentials": { "credentialsParameter": secret_arn() },
                    }),
                )],
            }),
            refused_with: Some(PRIVATE_REGISTRY_NEEDS_THE_ROLE.to_string()),
        },
        Row {
            case: "private registry credentials WITH an execution role",
            operation: "RegisterTaskDefinition",
            request: json!({
                "networkMode": "bridge",
                "executionRoleArn": execution_role(),
                "containerDefinitions": [container(
                    "app",
                    json!({
                        "repositoryCredentials": { "credentialsParameter": secret_arn() },
                    }),
                )],
            }),
            refused_with: None,
        },
        Row {
            case: "the daemon registration asks for a secret with no execution role",
            operation: "RegisterDaemonTaskDefinition",
            request: json!({
                "containerDefinitions": [container(
                    "app",
                    json!({ "secrets": [{ "name": "DB", "valueFrom": secret_arn() }] }),
                )],
            }),
            refused_with: Some(SECRETS_NEED_THE_ROLE.to_string()),
        },
        Row {
            case: "the daemon registration names the execution role",
            operation: "RegisterDaemonTaskDefinition",
            request: json!({
                "executionRoleArn": execution_role(),
                "containerDefinitions": [container(
                    "app",
                    json!({ "secrets": [{ "name": "DB", "valueFrom": secret_arn() }] }),
                )],
            }),
            refused_with: None,
        },
        Row {
            case: "the awsfirelens driver with no log router",
            operation: "RegisterTaskDefinition",
            request: json!({
                "cpu": "2048",
                "memory": "4096",
                "networkMode": "awsvpc",
                "requiresCompatibilities": ["FARGATE"],
                "executionRoleArn": execution_role(),
                "containerDefinitions": [container(
                    "app",
                    json!({ "logConfiguration": { "logDriver": "awsfirelens" } }),
                )],
            }),
            refused_with: Some(FIRELENS_NEEDS_A_ROUTER.to_string()),
        },
        Row {
            case: "the awsfirelens driver beside a router that FOLLOWS it",
            operation: "RegisterTaskDefinition",
            request: json!({
                "networkMode": "bridge",
                "containerDefinitions": [
                    container(
                        "app",
                        json!({ "logConfiguration": { "logDriver": "awsfirelens" } }),
                    ),
                    container(
                        "log-router",
                        json!({ "firelensConfiguration": { "type": "fluentbit" } }),
                    ),
                ],
            }),
            refused_with: None,
        },
        Row {
            case: "the awsfirelens driver beside a router that PRECEDES it",
            operation: "RegisterTaskDefinition",
            request: json!({
                "networkMode": "bridge",
                "containerDefinitions": [
                    container(
                        "log-router",
                        json!({ "firelensConfiguration": { "type": "fluentd" } }),
                    ),
                    container(
                        "app",
                        json!({ "logConfiguration": { "logDriver": "awsfirelens" } }),
                    ),
                ],
            }),
            refused_with: None,
        },
        Row {
            case: "another log driver needs no router",
            operation: "RegisterTaskDefinition",
            request: json!({
                "networkMode": "bridge",
                "containerDefinitions": [container(
                    "app",
                    json!({ "logConfiguration": { "logDriver": "json-file" } }),
                )],
            }),
            refused_with: None,
        },
        Row {
            case: "a soft limit above its hard limit",
            operation: "RegisterTaskDefinition",
            request: json!({
                "networkMode": "bridge",
                "containerDefinitions": [container(
                    "app",
                    json!({
                        "ulimits": [
                            { "name": "nofile", "softLimit": 8192, "hardLimit": 4096 },
                        ],
                    }),
                )],
            }),
            refused_with: Some(soft_above_hard("nofile")),
        },
        Row {
            case: "the SECOND ulimit of the list is the offending one",
            operation: "RegisterTaskDefinition",
            request: json!({
                "networkMode": "bridge",
                "containerDefinitions": [container(
                    "app",
                    json!({
                        "ulimits": [
                            { "name": "nofile", "softLimit": 1024, "hardLimit": 4096 },
                            { "name": "nproc", "softLimit": 64, "hardLimit": 32 },
                        ],
                    }),
                )],
            }),
            refused_with: Some(soft_above_hard("nproc")),
        },
        Row {
            case: "a soft limit EQUAL to its hard limit",
            operation: "RegisterTaskDefinition",
            request: json!({
                "networkMode": "bridge",
                "containerDefinitions": [container(
                    "app",
                    json!({
                        "ulimits": [
                            { "name": "nofile", "softLimit": 4096, "hardLimit": 4096 },
                        ],
                    }),
                )],
            }),
            refused_with: None,
        },
        Row {
            case: "a soft limit below its hard limit",
            operation: "RegisterTaskDefinition",
            request: json!({
                "networkMode": "bridge",
                "containerDefinitions": [container(
                    "app",
                    json!({
                        "ulimits": [
                            { "name": "memlock", "softLimit": 1, "hardLimit": 2 },
                        ],
                    }),
                )],
            }),
            refused_with: None,
        },
        Row {
            case: "a daemon container's soft limit above its hard limit",
            operation: "RegisterDaemonTaskDefinition",
            request: json!({
                "containerDefinitions": [container(
                    "app",
                    json!({
                        "ulimits": [
                            { "name": "nofile", "softLimit": 8192, "hardLimit": 4096 },
                        ],
                    }),
                )],
            }),
            refused_with: Some(soft_above_hard("nofile")),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // A fresh engine per row, so one row's revisions cannot leak into another.
        let router = service();
        let case = row.case;
        let family = family(index);
        let mut request = row.request.clone();
        request["family"] = json!(family);

        match &row.refused_with {
            None => {
                let answered = ok(&router, row.operation, request, case).await;
                let revision = registered_revision(&answered, row.operation, case);
                assert_eq!(
                    revision, 1,
                    "case {case:?}: the first accepted registration of a family is revision \
                     1, got {revision}"
                );
            }
            Some(message) => {
                let (code, reported) = refusal(&router, row.operation, request, case).await;
                assert_eq!(
                    (code.as_str(), reported.as_str()),
                    (CLIENT_EXCEPTION, message.as_str()),
                    "case {case:?}: expected a {CLIENT_EXCEPTION} stating the rule, got \
                     {code:?} with {reported:?}"
                );
                // The refusal consumed no revision of the family: the next accepted
                // registration of it is still revision 1.
                let accepted = ok(
                    &router,
                    row.operation,
                    json!({
                        "family": family,
                        "containerDefinitions": [container("app", json!({}))],
                    }),
                    case,
                )
                .await;
                let revision = registered_revision(&accepted, row.operation, case);
                assert_eq!(
                    revision, 1,
                    "case {case:?}: a refused registration must consume no revision, got \
                     {revision} for the next accepted one"
                );
            }
        }
    }
}

/// The revision an accepted registration minted, read back out of the response of whichever
/// registering operation the row used.
fn registered_revision(answered: &Value, operation: &str, case: &str) -> i64 {
    if operation == "RegisterTaskDefinition" {
        return answered["taskDefinition"]["revision"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a revision, got {answered}"));
    }
    // The daemon registration answers the ARN alone, whose last segment is the revision.
    let arn = string(answered, "daemonTaskDefinitionArn", case);
    arn.rsplit(':')
        .next()
        .and_then(|revision| revision.parse().ok())
        .unwrap_or_else(|| panic!("case {case:?}: expected a revision in {arn}"))
}

#[tokio::test]
async fn a_refused_registration_stores_no_revision_a_later_read_can_see() {
    let router = service();
    let case = "a secret with no execution role";
    let family = "credential-rules-store";

    let (code, _) = refusal(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [container(
                "app",
                json!({ "secrets": [{ "name": "DB", "valueFrom": secret_arn() }] }),
            )],
        }),
        case,
    )
    .await;
    assert_eq!(
        code, CLIENT_EXCEPTION,
        "case {case:?}: expected a {CLIENT_EXCEPTION}, got {code:?}"
    );

    // Nothing was stored: the account holds no revision of that family, and no family of
    // that name at all.
    let revisions = ok(
        &router,
        "ListTaskDefinitions",
        json!({ "familyPrefix": family }),
        case,
    )
    .await;
    assert!(
        list(&revisions, "taskDefinitionArns", case).is_empty(),
        "case {case:?}: a refused registration stores no revision, got {revisions}"
    );
    let families = ok(
        &router,
        "ListTaskDefinitionFamilies",
        json!({ "familyPrefix": family }),
        case,
    )
    .await;
    assert!(
        list(&families, "families", case).is_empty(),
        "case {case:?}: a refused registration starts no family, got {families}"
    );
}

/// The round trip: a registration the rules accept is stored, so a describe reports the
/// members it was given back.
#[tokio::test]
async fn an_accepted_registration_reads_back_with_the_members_it_declared() {
    let router: MultiServiceRouter = service();
    let case = "a router beside the driver, with the execution role";
    let family = "credential-rules-roundtrip";

    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "executionRoleArn": execution_role(),
            "networkMode": "bridge",
            "containerDefinitions": [
                container(
                    "app",
                    json!({
                        "secrets": [{ "name": "DB", "valueFrom": secret_arn() }],
                        "logConfiguration": { "logDriver": "awsfirelens" },
                        "ulimits": [{ "name": "nofile", "softLimit": 1024, "hardLimit": 4096 }],
                    }),
                ),
                container(
                    "log-router",
                    json!({ "firelensConfiguration": { "type": "fluentbit" } }),
                ),
            ],
        }),
        case,
    )
    .await;

    let described = ok(
        &router,
        "DescribeTaskDefinition",
        json!({ "taskDefinition": format!("{family}:1") }),
        case,
    )
    .await;
    let definition = &described["taskDefinition"];
    assert_eq!(
        definition["executionRoleArn"],
        json!(execution_role()),
        "case {case:?}: the stored revision reports the execution role it was given, got \
         {definition}"
    );
    let containers = list(definition, "containerDefinitions", case);
    assert_eq!(
        containers.len(),
        2,
        "case {case:?}: both containers are stored, got {containers:?}"
    );
    assert_eq!(
        containers[0]["secrets"][0]["valueFrom"],
        json!(secret_arn()),
        "case {case:?}: the stored container reports the secret it references, got \
         {containers:?}"
    );
    assert_eq!(
        containers[0]["ulimits"][0]["softLimit"],
        json!(1024),
        "case {case:?}: the stored container reports the ulimit it declared, got \
         {containers:?}"
    );
    assert_eq!(
        containers[1]["firelensConfiguration"]["type"],
        json!("fluentbit"),
        "case {case:?}: the stored log router reports its configuration, got {containers:?}"
    );
}
