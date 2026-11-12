// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::MWAA::Environment` deploys: the environment
//! is created in Amazon Managed Workflows for Apache Airflow's own control plane, the stack
//! publishes the values that service reports for it, a stack update applies the change to the
//! environment it created, deleting the stack takes the environment away — and a declaration
//! that service REFUSES fails the resource with its own refusal and rolls the stack back.
//!
//! The rule under test: **a resource type AWS publishes is one a template may declare, and a
//! declaration of it is deployed by calling the service that OWNS the type — so what the stack
//! publishes is what that service answered, and what that service refuses is a FAILED RESOURCE
//! rather than a template-format error.** Three crates have to agree for that to hold
//! (CloudFormation submits the template and makes the calls, Amazon MWAA serves them, and
//! CloudWatch Logs holds the log groups an enabled Apache Airflow log type publishes to), so
//! the suite lives here: a crate-local test can only ever see one side of it.
//!
//! The rows of the deploy table differ along what the rule generalises over: whether the
//! declaration NAMES the environment or leaves the name to CloudFormation, the environment
//! class and the fleet counts (a `mw1.micro` capped at one worker, a `mw1.small` scaled to
//! ten), the web-server access mode, how many security groups the placement names, which
//! optional members the declaration carries (an Apache Airflow version, a maintenance window,
//! the schedulers count, the plugins and requirements objects, an enabled log type), which
//! published return value each output reads (`Ref` and each of the six `Fn::GetAtt` values a
//! row can resolve), whether the declaration carries the type's own `Tags`, the template
//! spelling (JSON with long-form intrinsics, YAML with the short-form tags), and how many
//! environments one stack declares.
//!
//! The refusal rows differ along the CLASS of declaration Amazon MWAA refuses: a placement
//! that names one subnet where the modelled list takes exactly two, an environment class that
//! service does not run, an Apache Airflow version it does not run, and a worker floor above
//! its own ceiling. Each is the owning service's own refusal, reported on the resource that
//! failed — never the `Template format error: Unrecognized resource types` a type
//! CloudFormation does not know is refused with, which no row of either table may ever see.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{elements, form_encode, opt_element, outputs_of};

/// The region every request here is signed for, and so the region the stacks belong to.
const REGION: &str = "us-east-1";

/// How long one poll of a stack's status waits before asking again, and how many times: the
/// stack's own clock-owed span and Amazon MWAA's are each half a second, so a create settles
/// well inside this budget and nothing is waited out past the first observation.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLLS: usize = 60;

/// The fixed text CloudFormation refuses a template declaring a type it does NOT know with,
/// which is what the defect this suite pins answered for a type AWS publishes.
const UNRECOGNIZED_TYPES: &str = "Unrecognized resource types";

/// The `EnvironmentStatus` a usable environment is reported in.
const AVAILABLE: &str = "AVAILABLE";

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation action over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one Amazon MWAA control-plane operation over its restJson1 wire, on the URI that
/// operation's own `@http` binding declares. Its signing name is `airflow`.
async fn mwaa(router: &MultiServiceRouter, method: &str, uri: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("airflow.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("airflow"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let document: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, document)
}

/// `GetEnvironment` for one name: the status it answered and the environment record it
/// reported, which is `Value::Null` for a refusal.
async fn get_environment(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    let (status, document) = mwaa(router, "GET", &format!("/environments/{name}")).await;
    let environment = document
        .get("Environment")
        .cloned()
        .unwrap_or(Value::Null);
    (status, if status == 200 { environment } else { document })
}

/// Every environment name `ListEnvironments` reports.
async fn list_environments(router: &MultiServiceRouter) -> Vec<String> {
    let (_, document) = mwaa(router, "GET", "/environments").await;
    document
        .get("Environments")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The `StackStatus` the stack reports right now, with the answer it was read from.
async fn stack_status(router: &MultiServiceRouter, stack: &str) -> (String, String) {
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "DescribeStacks {stack:?}: expected HTTP 200, got {status} with body {body}"
    );
    (opt_element(&body, "StackStatus").unwrap_or_default(), body)
}

/// Poll `stack` until it first reports `wanted`, and answer the last status it reported with
/// the `DescribeStacks` answer that status was read from: the operation's completion is owed
/// from the instant Amazon MWAA reports the environment up, which no fixed sleep can name.
async fn wait_for_stack_status(
    router: &MultiServiceRouter,
    stack: &str,
    wanted: &str,
) -> (String, String) {
    let mut last = stack_status(router, stack).await;
    for _ in 0..POLLS {
        if last.0 == wanted {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        last = stack_status(router, stack).await;
    }
    last
}

/// Submit a template, assert the type was RECOGNISED, and wait for the stack to reach
/// `wanted`; answers the `DescribeStacks` body it was last read from.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    stack_name: &str,
    members: &[(&str, &str)],
    wanted: &str,
) -> String {
    let (status, body) = cloudformation(router, action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action} — an AWS::MWAA::Environment is a type \
         a template may declare — got {status} with body {body}"
    );
    assert!(
        !body.contains(UNRECOGNIZED_TYPES),
        "case {case:?}: expected the type to be recognised, got {body}"
    );
    let (reached, described) = wait_for_stack_status(router, stack_name, wanted).await;
    assert_eq!(
        reached, wanted,
        "case {case:?}: expected the stack to reach {wanted}, got {described}"
    );
    described
}

/// The value one reported environment carries at a published attribute's name, which is a PATH
/// through the record for the dotted names the type publishes
/// (`LoggingConfiguration.<log type>.CloudWatchLogGroupArn`).
fn reported_attribute(environment: &Value, attribute: &str) -> Option<String> {
    let mut value = environment;
    for segment in attribute.split('.') {
        value = value.get(segment)?;
    }
    value.as_str().map(ToString::to_string)
}

/// The tags Amazon MWAA reports for one environment.
fn reported_tags(environment: &Value) -> BTreeMap<String, String> {
    environment
        .get("Tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What one row expects of one environment the stack declares.
struct Declared {
    /// The output key whose value is the environment's name, which is what `Ref` returns.
    name_output: &'static str,
    /// The name the declaration states, or `None` where CloudFormation generates it.
    declared_name: Option<&'static str>,
    /// Each output key paired with the published attribute it reads, which must equal Amazon
    /// MWAA's own value for that attribute.
    attribute_outputs: &'static [(&'static str, &'static str)],
    /// The members the environment must report, as the declaration stated them.
    members: Value,
    /// The subnets and security groups the placement names.
    subnet_ids: &'static [&'static str],
    security_group_ids: &'static [&'static str],
    /// The tags the environment must carry, declared and propagated together.
    tags: &'static [(&'static str, &'static str)],
}

/// One row: a template, the stack-level tags it is deployed with, and the environments it
/// declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    stack_tags: &'static [(&'static str, &'static str)],
    declared: Vec<Declared>,
}

/// The placement, the execution role and the DAG bucket the rows below declare. Each value is
/// of the form the member's own modelled pattern requires: `^subnet-[a-zA-Z0-9\-._]+$`,
/// `^sg-[a-zA-Z0-9\-._]+$`, `^arn:aws(-[a-z]+)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$` and
/// `^arn:aws(-[a-z]+)?:s3:::[a-z0-9.\-]+$`.
const SUBNET_A: &str = "subnet-0a1b2c3d4e5f60001";
const SUBNET_B: &str = "subnet-0a1b2c3d4e5f60002";
const SECURITY_GROUP_A: &str = "sg-0a1b2c3d4e5f60001";
const SECURITY_GROUP_B: &str = "sg-0a1b2c3d4e5f60002";
const EXECUTION_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/mwaa-execution";
const SOURCE_BUCKET_ARN: &str = "arn:aws:s3:::mwaa-dags-bucket";

fn rows() -> Vec<Row> {
    vec![
        // The issue's own stack: one named environment on the smallest class, capped at one
        // worker, with a private web server, declared tags and every published return value a
        // row with no log type enabled can resolve, in JSON with long-form intrinsics.
        Row {
            case: "a named mw1.micro environment, every endpoint return value read, JSON",
            stack_name: "mwaa-environment-round-trip",
            template: json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": {
                    "MwaaEnvironment": {
                        "Type": "AWS::MWAA::Environment",
                        "Properties": {
                            "Name": "airflow-round-trip",
                            "ExecutionRoleArn": EXECUTION_ROLE_ARN,
                            "SourceBucketArn": SOURCE_BUCKET_ARN,
                            "DagS3Path": "dags",
                            "NetworkConfiguration": {
                                "SubnetIds": [SUBNET_A, SUBNET_B],
                                "SecurityGroupIds": [SECURITY_GROUP_A],
                            },
                            "EnvironmentClass": "mw1.micro",
                            "MinWorkers": 1,
                            "MaxWorkers": 1,
                            "WebserverAccessMode": "PRIVATE_ONLY",
                            "Tags": {"owner": "data-platform"},
                        }
                    }
                },
                "Outputs": {
                    "EnvironmentRef": {"Value": {"Ref": "MwaaEnvironment"}},
                    "EnvironmentArn": {"Value": {"Fn::GetAtt": ["MwaaEnvironment", "Arn"]}},
                    "CeleryQueue": {
                        "Value": {"Fn::GetAtt": ["MwaaEnvironment", "CeleryExecutorQueue"]}
                    },
                    "DatabaseEndpointService": {
                        "Value": {
                            "Fn::GetAtt": ["MwaaEnvironment", "DatabaseVpcEndpointService"]
                        }
                    },
                    "WebserverEndpointService": {
                        "Value": {
                            "Fn::GetAtt": ["MwaaEnvironment", "WebserverVpcEndpointService"]
                        }
                    },
                    "WebserverUrl": {
                        "Value": {"Fn::GetAtt": ["MwaaEnvironment", "WebserverUrl"]}
                    }
                }
            })
            .to_string(),
            stack_tags: &[("sim2real", "mwaa-environment-round-trip")],
            declared: vec![Declared {
                name_output: "EnvironmentRef",
                declared_name: Some("airflow-round-trip"),
                attribute_outputs: &[
                    ("EnvironmentArn", "Arn"),
                    ("CeleryQueue", "CeleryExecutorQueue"),
                    ("DatabaseEndpointService", "DatabaseVpcEndpointService"),
                    ("WebserverEndpointService", "WebserverVpcEndpointService"),
                    ("WebserverUrl", "WebserverUrl"),
                ],
                members: json!({
                    "EnvironmentClass": "mw1.micro",
                    "DagS3Path": "dags",
                    "ExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "SourceBucketArn": SOURCE_BUCKET_ARN,
                    "MinWorkers": 1,
                    "MaxWorkers": 1,
                    "WebserverAccessMode": "PRIVATE_ONLY",
                }),
                subnet_ids: &[SUBNET_A, SUBNET_B],
                security_group_ids: &[SECURITY_GROUP_A],
                tags: &[
                    ("owner", "data-platform"),
                    ("sim2real", "mwaa-environment-round-trip"),
                ],
            }],
        },
        // The same type in YAML with the short-form tags, with NO declared name — so the
        // environment carries the one CloudFormation generates — on the default class scaled
        // to ten workers, behind a public web server, with two security groups, an Apache
        // Airflow version, a maintenance window, a schedulers count and two log types
        // ENABLED, whose log-group ARNs are published return values of their own.
        Row {
            case: "a generated name, mw1.small, two security groups, two log types, YAML",
            stack_name: "mwaa-environment-logging",
            template: format!(
                "\
Resources:
  Airflow:
    Type: AWS::MWAA::Environment
    Properties:
      ExecutionRoleArn: {EXECUTION_ROLE_ARN}
      SourceBucketArn: {SOURCE_BUCKET_ARN}
      DagS3Path: workflows/dags
      NetworkConfiguration:
        SubnetIds:
          - {SUBNET_A}
          - {SUBNET_B}
        SecurityGroupIds:
          - {SECURITY_GROUP_A}
          - {SECURITY_GROUP_B}
      EnvironmentClass: mw1.small
      AirflowVersion: 2.10.3
      WeeklyMaintenanceWindowStart: \"TUE:03:30\"
      Schedulers: 2
      MinWorkers: 2
      MaxWorkers: 10
      LoggingConfiguration:
        SchedulerLogs:
          Enabled: true
          LogLevel: INFO
        TaskLogs:
          Enabled: true
          LogLevel: WARNING
Outputs:
  EnvironmentRef:
    Value: !Ref Airflow
  EnvironmentArn:
    Value: !GetAtt Airflow.Arn
  SchedulerLogGroup:
    Value: !GetAtt Airflow.LoggingConfiguration.SchedulerLogs.CloudWatchLogGroupArn
  TaskLogGroup:
    Value: !GetAtt Airflow.LoggingConfiguration.TaskLogs.CloudWatchLogGroupArn
"
            ),
            stack_tags: &[("sim2real", "mwaa-environment-logging"), ("tier", "batch")],
            declared: vec![Declared {
                name_output: "EnvironmentRef",
                declared_name: None,
                attribute_outputs: &[
                    ("EnvironmentArn", "Arn"),
                    (
                        "SchedulerLogGroup",
                        "LoggingConfiguration.SchedulerLogs.CloudWatchLogGroupArn",
                    ),
                    (
                        "TaskLogGroup",
                        "LoggingConfiguration.TaskLogs.CloudWatchLogGroupArn",
                    ),
                ],
                members: json!({
                    "EnvironmentClass": "mw1.small",
                    "DagS3Path": "workflows/dags",
                    "AirflowVersion": "2.10.3",
                    "WeeklyMaintenanceWindowStart": "TUE:03:30",
                    "Schedulers": 2,
                    "MinWorkers": 2,
                    "MaxWorkers": 10,
                }),
                subnet_ids: &[SUBNET_A, SUBNET_B],
                security_group_ids: &[SECURITY_GROUP_A, SECURITY_GROUP_B],
                tags: &[
                    ("sim2real", "mwaa-environment-logging"),
                    ("tier", "batch"),
                ],
            }],
        },
        // Two environments in one stack: one loading the plugins and requirements objects its
        // declaration names, one minimal, each publishing its OWN values.
        Row {
            case: "two environments in one stack, one loading its S3 objects",
            stack_name: "mwaa-environment-pair",
            template: json!({
                "Resources": {
                    "First": {
                        "Type": "AWS::MWAA::Environment",
                        "Properties": {
                            "Name": "airflow-pair-first",
                            "ExecutionRoleArn": EXECUTION_ROLE_ARN,
                            "SourceBucketArn": SOURCE_BUCKET_ARN,
                            "DagS3Path": "dags",
                            "NetworkConfiguration": {
                                "SubnetIds": [SUBNET_A, SUBNET_B],
                                "SecurityGroupIds": [SECURITY_GROUP_A],
                            },
                            "PluginsS3Path": "plugins.zip",
                            "PluginsS3ObjectVersion": "3sL4kqtJlcpXroDTDmJ",
                            "RequirementsS3Path": "requirements.txt",
                            "RequirementsS3ObjectVersion": "rmSpXd3dIbrHYMTRCxf",
                        }
                    },
                    "Second": {
                        "Type": "AWS::MWAA::Environment",
                        "Properties": {
                            "Name": "airflow-pair-second",
                            "ExecutionRoleArn": EXECUTION_ROLE_ARN,
                            "SourceBucketArn": SOURCE_BUCKET_ARN,
                            "DagS3Path": "",
                            "NetworkConfiguration": {
                                "SubnetIds": [SUBNET_A, SUBNET_B],
                                "SecurityGroupIds": [SECURITY_GROUP_B],
                            }
                        }
                    }
                },
                "Outputs": {
                    "FirstRef": {"Value": {"Ref": "First"}},
                    "FirstArn": {"Value": {"Fn::GetAtt": ["First", "Arn"]}},
                    "SecondRef": {"Value": {"Ref": "Second"}},
                    "SecondWebserverUrl": {
                        "Value": {"Fn::GetAtt": ["Second", "WebserverUrl"]}
                    }
                }
            })
            .to_string(),
            stack_tags: &[],
            declared: vec![
                Declared {
                    name_output: "FirstRef",
                    declared_name: Some("airflow-pair-first"),
                    attribute_outputs: &[("FirstArn", "Arn")],
                    members: json!({
                        "DagS3Path": "dags",
                        "PluginsS3Path": "plugins.zip",
                        "PluginsS3ObjectVersion": "3sL4kqtJlcpXroDTDmJ",
                        "RequirementsS3Path": "requirements.txt",
                        "RequirementsS3ObjectVersion": "rmSpXd3dIbrHYMTRCxf",
                    }),
                    subnet_ids: &[SUBNET_A, SUBNET_B],
                    security_group_ids: &[SECURITY_GROUP_A],
                    tags: &[],
                },
                Declared {
                    name_output: "SecondRef",
                    declared_name: Some("airflow-pair-second"),
                    attribute_outputs: &[("SecondWebserverUrl", "WebserverUrl")],
                    members: json!({"DagS3Path": ""}),
                    subnet_ids: &[SUBNET_A, SUBNET_B],
                    security_group_ids: &[SECURITY_GROUP_B],
                    tags: &[],
                },
            ],
        },
    ]
}

/// The stack-level `Tags` of a `CreateStack`/`UpdateStack` request, in that wire's own
/// spelling.
fn stack_tag_members(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            [
                (format!("Tags.member.{position}.Key"), (*key).to_string()),
                (
                    format!("Tags.member.{position}.Value"),
                    (*value).to_string(),
                ),
            ]
        })
        .collect()
}

/// One submit's form members: the stack, its template and its stack-level tags.
fn submit_members<'a>(
    stack_name: &'a str,
    template: &'a str,
    tags: &'a [(String, String)],
) -> Vec<(&'a str, &'a str)> {
    let mut members = vec![("StackName", stack_name), ("TemplateBody", template)];
    for (key, value) in tags {
        members.push((key.as_str(), value.as_str()));
    }
    members
}

#[tokio::test]
async fn a_deployed_stack_publishes_the_environment_amazon_mwaa_created() {
    for row in rows() {
        let router = build_router();
        let case = row.case;
        let tags = stack_tag_members(row.stack_tags);
        let body = deploy(
            &router,
            case,
            "CreateStack",
            row.stack_name,
            &submit_members(row.stack_name, &row.template, &tags),
            "CREATE_COMPLETE",
        )
        .await;
        let outputs: BTreeMap<String, String> = outputs_of(&body);
        let listed = list_environments(&router).await;

        for declared in &row.declared {
            let name = outputs
                .get(declared.name_output)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no {} output in {body}", declared.name_output)
                })
                .clone();
            match declared.declared_name {
                Some(expected) => assert_eq!(
                    name, expected,
                    "case {case:?}: expected Ref to return the declared name {expected:?}, got \
                     {name:?}"
                ),
                None => assert!(
                    name.starts_with(row.stack_name) && name.len() > row.stack_name.len(),
                    "case {case:?}: expected a generated <stack>-<logical id>-<unique id>, got \
                     {name:?}"
                ),
            }

            // The identifier the stack published is one the OWNING service resolves, and the
            // environment it resolves to is the one the template declared, running.
            let (status, environment) = get_environment(&router, &name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected Amazon MWAA to report the published {name:?}, got \
                 {status} with {environment}"
            );
            assert_eq!(
                environment.get("Status").and_then(Value::as_str),
                Some(AVAILABLE),
                "case {case:?}: expected {name:?} to be {AVAILABLE}, got {environment}"
            );
            assert!(
                listed.contains(&name),
                "case {case:?}: expected ListEnvironments to report {name:?}, got {listed:?}"
            );

            for (member, expected) in declared
                .members
                .as_object()
                .expect("the row's members are an object")
            {
                let reported = environment.get(member);
                assert_eq!(
                    reported,
                    Some(expected),
                    "case {case:?}: expected {name:?} to report {member} as {expected}, got \
                     {reported:?} in {environment}"
                );
            }

            for (member, expected) in [
                ("SubnetIds", declared.subnet_ids),
                ("SecurityGroupIds", declared.security_group_ids),
            ] {
                let reported: Vec<String> = environment
                    .get("NetworkConfiguration")
                    .and_then(|placement| placement.get(member))
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(|value| value.as_str().map(ToString::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected {name:?} to be placed on {member} {expected:?}, \
                     got {reported:?} in {environment}"
                );
            }

            // The declaration's own tags AND the stack's travel to the environment.
            let carried = reported_tags(&environment);
            for (key, value) in declared.tags {
                assert_eq!(
                    carried.get(*key).map(String::as_str),
                    Some(*value),
                    "case {case:?}: expected {name:?} to carry the tag {key}={value}, got \
                     {carried:?}"
                );
            }

            // Every published return value the row reads is the OWNING service's own value for
            // that attribute, not a stand-in CloudFormation computed before the create.
            for (output, attribute) in declared.attribute_outputs {
                let published = outputs
                    .get(*output)
                    .unwrap_or_else(|| panic!("case {case:?}: no {output} output in {body}"));
                let reported = reported_attribute(&environment, attribute).unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: Amazon MWAA reports no {attribute} for {name:?} in \
                         {environment}"
                    )
                });
                assert_eq!(
                    published, &reported,
                    "case {case:?}: expected the {output} output to be Amazon MWAA's own \
                     {attribute} ({reported:?}), got {published:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_stack_update_applies_the_change_to_the_environment_amazon_mwaa_holds() {
    let router = build_router();
    let stack_name = "mwaa-environment-updated";
    let name = "airflow-updated";
    let declaration = |properties: Value| {
        json!({
            "Resources": {
                "Airflow": {"Type": "AWS::MWAA::Environment", "Properties": properties}
            },
            "Outputs": {
                "EnvironmentRef": {"Value": {"Ref": "Airflow"}},
                "EnvironmentArn": {"Value": {"Fn::GetAtt": ["Airflow", "Arn"]}}
            }
        })
        .to_string()
    };
    let base = json!({
        "Name": name,
        "ExecutionRoleArn": EXECUTION_ROLE_ARN,
        "SourceBucketArn": SOURCE_BUCKET_ARN,
        "DagS3Path": "dags",
        "NetworkConfiguration": {
            "SubnetIds": [SUBNET_A, SUBNET_B],
            "SecurityGroupIds": [SECURITY_GROUP_A],
        },
        "EnvironmentClass": "mw1.micro",
        "MinWorkers": 1,
        "MaxWorkers": 1,
    });

    let created = deploy(
        &router,
        "the first deployment",
        "CreateStack",
        stack_name,
        &[("StackName", stack_name), ("TemplateBody", &declaration(base))],
        "CREATE_COMPLETE",
    )
    .await;
    let arn = outputs_of(&created)
        .get("EnvironmentArn")
        .unwrap_or_else(|| panic!("no EnvironmentArn output in {created}"))
        .clone();

    // The rows differ along which members the re-declaration changes: the fleet alone, then
    // the class, the access mode, the per-class unit counts and the security groups together
    // — a class whose accepted unit-count range differs from the one the environment was
    // created on has to restate those counts, which is the owning service's own rule. Each
    // change is applied to the environment the stack created: its identity survives, and
    // Amazon MWAA's own read reports the new values.
    let rows: [(&str, Value, Value); 2] = [
        (
            "the worker fleet alone",
            json!({
                "Name": name,
                "ExecutionRoleArn": EXECUTION_ROLE_ARN,
                "SourceBucketArn": SOURCE_BUCKET_ARN,
                "DagS3Path": "dags",
                "NetworkConfiguration": {
                    "SubnetIds": [SUBNET_A, SUBNET_B],
                    "SecurityGroupIds": [SECURITY_GROUP_A],
                },
                "EnvironmentClass": "mw1.micro",
                "MinWorkers": 1,
                "MaxWorkers": 4,
            }),
            json!({"MaxWorkers": 4, "EnvironmentClass": "mw1.micro"}),
        ),
        (
            "the class, the access mode, the unit counts and the security groups together",
            json!({
                "Name": name,
                "ExecutionRoleArn": EXECUTION_ROLE_ARN,
                "SourceBucketArn": SOURCE_BUCKET_ARN,
                "DagS3Path": "pipelines/dags",
                "NetworkConfiguration": {
                    "SubnetIds": [SUBNET_A, SUBNET_B],
                    "SecurityGroupIds": [SECURITY_GROUP_A, SECURITY_GROUP_B],
                },
                "EnvironmentClass": "mw1.medium",
                "WebserverAccessMode": "PRIVATE_ONLY",
                "MinWorkers": 2,
                "MaxWorkers": 6,
                "Schedulers": 3,
                "MinWebservers": 2,
                "MaxWebservers": 4,
            }),
            json!({
                "EnvironmentClass": "mw1.medium",
                "WebserverAccessMode": "PRIVATE_ONLY",
                "DagS3Path": "pipelines/dags",
                "MinWorkers": 2,
                "MaxWorkers": 6,
                "Schedulers": 3,
                "MinWebservers": 2,
                "MaxWebservers": 4,
            }),
        ),
    ];

    for (case, properties, expected) in rows {
        let template = declaration(properties.clone());
        let described = deploy(
            &router,
            case,
            "UpdateStack",
            stack_name,
            &[("StackName", stack_name), ("TemplateBody", &template)],
            "UPDATE_COMPLETE",
        )
        .await;
        let outputs = outputs_of(&described);
        assert_eq!(
            outputs.get("EnvironmentRef").map(String::as_str),
            Some(name),
            "case {case:?}: expected the environment to keep its name, got {described}"
        );
        assert_eq!(
            outputs.get("EnvironmentArn"),
            Some(&arn),
            "case {case:?}: expected the environment to keep the ARN Amazon MWAA minted, got \
             {described}"
        );

        // The change reached the environment THAT service holds, not just the stack's record.
        let listed = list_environments(&router).await;
        assert_eq!(
            listed,
            vec![name.to_string()],
            "case {case:?}: expected the one environment to be changed rather than a second \
             one created, got {listed:?}"
        );
        let (status, environment) = get_environment(&router, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Amazon MWAA to still report {name:?}, got {environment}"
        );
        for (member, wanted) in expected.as_object().expect("the row's members are an object") {
            let reported = environment.get(member);
            assert_eq!(
                reported,
                Some(wanted),
                "case {case:?}: expected the re-declared {member} {wanted}, got {reported:?} \
                 in {environment}"
            );
        }
        let placement: Vec<String> = environment
            .get("NetworkConfiguration")
            .and_then(|placement| placement.get("SecurityGroupIds"))
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let declared_groups: Vec<String> = properties
            .pointer("/NetworkConfiguration/SecurityGroupIds")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            placement, declared_groups,
            "case {case:?}: expected the re-declared security groups {declared_groups:?}, got \
             {placement:?} in {environment}"
        );
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_environments_it_created() {
    let router = build_router();
    let case = "two environments deleted with their stack";
    let stack_name = "mwaa-environment-deleted";
    let names = ["airflow-deleted-first", "airflow-deleted-second"];
    let template = json!({
        "Resources": {
            "First": {
                "Type": "AWS::MWAA::Environment",
                "Properties": {
                    "Name": names[0],
                    "ExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "SourceBucketArn": SOURCE_BUCKET_ARN,
                    "DagS3Path": "dags",
                    "NetworkConfiguration": {
                        "SubnetIds": [SUBNET_A, SUBNET_B],
                        "SecurityGroupIds": [SECURITY_GROUP_A],
                    }
                }
            },
            "Second": {
                "Type": "AWS::MWAA::Environment",
                "Properties": {
                    "Name": names[1],
                    "ExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "SourceBucketArn": SOURCE_BUCKET_ARN,
                    "DagS3Path": "dags",
                    "NetworkConfiguration": {
                        "SubnetIds": [SUBNET_A, SUBNET_B],
                        "SecurityGroupIds": [SECURITY_GROUP_B],
                    }
                }
            }
        }
    })
    .to_string();

    deploy(
        &router,
        case,
        "CreateStack",
        stack_name,
        &[("StackName", stack_name), ("TemplateBody", &template)],
        "CREATE_COMPLETE",
    )
    .await;
    let listed = list_environments(&router).await;
    for name in names {
        assert!(
            listed.contains(&name.to_string()),
            "case {case:?}: expected Amazon MWAA to hold {name:?}, got {listed:?}"
        );
    }

    let (status, deleted) =
        cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {deleted}"
    );

    // The stack's deletion is not finished while Amazon MWAA is still tearing an environment
    // down, so the environments are gone by the time the stack is.
    for _ in 0..POLLS {
        if list_environments(&router).await.is_empty() {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let remaining = list_environments(&router).await;
    assert!(
        remaining.is_empty(),
        "case {case:?}: expected every environment to be gone from Amazon MWAA, got \
         {remaining:?}"
    );
    for name in names {
        let (status, refusal) = get_environment(&router, name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for the deleted {name:?}, got {status} with \
             {refusal}"
        );
        assert_eq!(
            refusal.get("message").and_then(Value::as_str),
            Some(format!("Environment {name} not found.").as_str()),
            "case {case:?}: expected Amazon MWAA's own not-found sentence for {name:?}, got \
             {refusal}"
        );
    }
}

/// One refusal row: a declaration Amazon MWAA refuses, and the FIXED text of the sentence it
/// words that class of refusal with, which the failed resource's reason has to carry.
struct Refused {
    case: &'static str,
    stack_name: &'static str,
    name: &'static str,
    properties: fn(&'static str) -> Value,
    sentence: &'static str,
}

const REFUSED: &[Refused] = &[
    // The sibling check's own refusal: `SubnetList` is `@length(2, 2)`, so a placement naming
    // one subnet is not one that service will provision.
    Refused {
        case: "a placement naming one subnet where the list takes two",
        stack_name: "mwaa-environment-single-subnet",
        name: "airflow-single-subnet",
        properties: single_subnet_properties,
        sentence: "subnetIds",
    },
    // A class Amazon MWAA does not run, refused with its own value-set sentence.
    Refused {
        case: "an environment class the service does not run",
        stack_name: "mwaa-environment-bad-class",
        name: "airflow-bad-class",
        properties: unknown_class_properties,
        sentence: "mw1.nano is not a supported EnvironmentClass. Valid values: mw1.micro, \
                   mw1.small, mw1.medium, mw1.large, mw1.xlarge, mw1.2xlarge.",
    },
    // An Apache Airflow version it does not run, which is the same rule over another member.
    Refused {
        case: "an Apache Airflow version the service does not run",
        stack_name: "mwaa-environment-bad-version",
        name: "airflow-bad-version",
        properties: unknown_version_properties,
        sentence: "1.10.12 is not a supported AirflowVersion. Valid values: 2.7.2, 2.8.1, \
                   2.9.2, 2.10.1, 2.10.3, 2.11.0, 3.0.6.",
    },
    // A worker floor above its own ceiling describes a fleet that cannot be provisioned.
    Refused {
        case: "a worker floor above its own ceiling",
        stack_name: "mwaa-environment-bad-fleet",
        name: "airflow-bad-fleet",
        properties: inverted_fleet_properties,
        sentence: "MinWorkers must be less than or equal to MaxWorkers.",
    },
];

/// The members every refusal row shares: the role, the bucket, the DAG path and a placement
/// the modelled list accepts.
fn refusal_base(name: &'static str) -> Value {
    json!({
        "Name": name,
        "ExecutionRoleArn": EXECUTION_ROLE_ARN,
        "SourceBucketArn": SOURCE_BUCKET_ARN,
        "DagS3Path": "dags",
        "NetworkConfiguration": {
            "SubnetIds": [SUBNET_A, SUBNET_B],
            "SecurityGroupIds": [SECURITY_GROUP_A],
        }
    })
}

fn single_subnet_properties(name: &'static str) -> Value {
    let mut properties = refusal_base(name);
    properties["NetworkConfiguration"]["SubnetIds"] = json!([SUBNET_A]);
    properties
}

fn unknown_class_properties(name: &'static str) -> Value {
    let mut properties = refusal_base(name);
    properties["EnvironmentClass"] = json!("mw1.nano");
    properties
}

fn unknown_version_properties(name: &'static str) -> Value {
    let mut properties = refusal_base(name);
    properties["AirflowVersion"] = json!("1.10.12");
    properties
}

fn inverted_fleet_properties(name: &'static str) -> Value {
    let mut properties = refusal_base(name);
    properties["MinWorkers"] = json!(4);
    properties["MaxWorkers"] = json!(2);
    properties
}

#[tokio::test]
async fn a_declaration_amazon_mwaa_refuses_rolls_the_stack_back() {
    for row in REFUSED {
        let router = build_router();
        let case = row.case;
        let template = json!({
            "Resources": {
                "Airflow": {
                    "Type": "AWS::MWAA::Environment",
                    "Properties": (row.properties)(row.name)
                }
            },
            "Outputs": {"EnvironmentRef": {"Value": {"Ref": "Airflow"}}}
        })
        .to_string();

        let body = deploy(
            &router,
            case,
            "CreateStack",
            row.stack_name,
            &[("StackName", row.stack_name), ("TemplateBody", &template)],
            "ROLLBACK_COMPLETE",
        )
        .await;
        assert!(
            outputs_of(&body).is_empty(),
            "case {case:?}: expected no output for a resource nothing created, got {body}"
        );

        // The failure is the OWNING service's own refusal, on the resource that failed, and
        // never a refusal of the template: no event reports an unrecognized type at all.
        let (_, events) =
            cloudformation(&router, "DescribeStackEvents", &[("StackName", row.stack_name)]).await;
        assert!(
            !events.contains(UNRECOGNIZED_TYPES),
            "case {case:?}: expected no template-format error for a type AWS publishes, got \
             {events}"
        );
        let reasons = elements(&events, "ResourceStatusReason");
        assert!(
            reasons.iter().any(|reason| reason.contains(row.sentence)),
            "case {case:?}: expected a failure reason carrying {:?}, got {reasons:?}",
            row.sentence
        );

        // And nothing was created: Amazon MWAA holds no environment of that name.
        let (status, refusal) = get_environment(&router, row.name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected Amazon MWAA to hold no environment {:?}, got {status} \
             with {refusal}",
            row.name
        );
    }
}
