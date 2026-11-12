// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares the CodeDeploy resource types really creates them
//! in CodeDeploy, and the handles the stack hands back are the ones that service answers
//! to.
//!
//! The rule under test: **a resource of a type this emulator provisions is created in the
//! service that OWNS it, through that service's own public API, with the configuration the
//! template declared — so once the stack completes, the identifier it publishes for the
//! resource is one the owning service resolves, and deleting the stack deletes the resource
//! there.** For the three CodeDeploy types the identifier each one publishes is its NAME
//! (`Ref` returns the application name, the deployment group name and the deployment
//! configuration name), so `codedeploy GetApplication`, `GetDeploymentGroup` and
//! `GetDeploymentConfig` on what the stack reports resolve the resources the template
//! declared instead of answering `ApplicationDoesNotExistException` /
//! `DeploymentGroupDoesNotExistException`. Two crates have to agree for that to hold
//! (CloudFormation makes the calls, CodeDeploy serves them), so the suite lives here rather
//! than in either crate.
//!
//! The rows differ along what the rule generalises over: which of the three types the
//! template declares and how many of them, whether it NAMES each resource or leaves
//! CloudFormation to generate the name, which compute platform the application runs on
//! (`Server`, `Lambda`, `ECS`), which of the group's properties it declares — a scalar, a
//! list of scalars, a structure whose members CodeDeploy spells in lower camel
//! (`DeploymentStyle`, `AlarmConfiguration`, `LoadBalancerInfo`), one whose members
//! CodeDeploy spells in upper camel (`Ec2TagFilters`, whose `EC2TagFilter` keeps `Key`,
//! `Value` and `Type`) and a wrapper holding the second inside the first (`Ec2TagSet`) —
//! whether the group names a deployment configuration of the same stack or takes
//! CodeDeploy's own default, whether it asks for a revision to be deployed as soon as it
//! exists (`Deployment`), whether the template is written in JSON or YAML, and whether a
//! value reaches a property literally, through a template `Parameter` or through a `Ref` to
//! another resource of the same stack.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the GENERATED names of the rows that declare none,
//! which are CloudFormation's to compute: what is checked about those is that they are names
//! inside the bound CodeDeploy's own model publishes and that CodeDeploy resolves them.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary,
};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due, so the stack reports the deployment it completed.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The longest name CodeDeploy accepts for any of these resources: `ApplicationName`,
/// `DeploymentGroupName` and `DeploymentConfigName` are each `@length(min: 1, max: 100)`
/// with pattern `^[A-Za-z0-9+=,.@_-]*$` in that service's own Smithy model.
const MAX_CODEDEPLOY_NAME: usize = 100;

/// Long enough for a deployment to reach a terminal status, and how often it is looked at.
///
/// The stack's deletion waits for that, because a deployment configuration a deployment is
/// still RUNNING under cannot be deleted — CodeDeploy's own rule, which a client deleting
/// the same stack would meet the same way. It is a WAIT rather than a fixed span because a
/// deployment under a time-based `trafficRoutingConfig` is held open for the window its
/// traffic increments describe, which is longer than the span one that cuts over in a
/// single step takes.
const PAST_DEPLOYMENT_SPAN: Duration = Duration::from_secs(60);
/// See [`PAST_DEPLOYMENT_SPAN`].
const DEPLOYMENT_POLL: Duration = Duration::from_millis(50);

/// A service-role ARN of the form `CreateDeploymentGroup.serviceRoleArn` takes, as a
/// caller would hand one to a template.
const SERVICE_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/CodeDeployServiceRole";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every
/// other suite here; the in-process callback CloudFormation makes signs its request either
/// way (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_codedeploy_codedeploy_20141006::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
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
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one CodeDeploy operation over the awsJson1_1 wire its model declares, signed for
/// `region`.
async fn codedeploy(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("codedeploy.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CodeDeploy_20141006.{operation}"))
        .header("authorization", authorization("codedeploy", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// Create a deployment on `application`/`group`, waiting out the refusal a deployment group
/// that is already running one answers with.
///
/// A deployment group runs ONE deployment at a time, so a second `CreateDeployment` against
/// it is refused with `DeploymentLimitExceededException` until the first is terminal — which
/// a client waits out, and so does this: a stack that declared a `Deployment` leaves one in
/// flight behind it.
async fn deployment_on_free_group(
    router: &MultiServiceRouter,
    region: &str,
    application: &str,
    group: &str,
    case: &str,
) -> (u16, Value) {
    let deadline = std::time::Instant::now() + PAST_DEPLOYMENT_SPAN;
    loop {
        let answered = codedeploy(
            router,
            region,
            "CreateDeployment",
            &json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "revision": {
                    "revisionType": "S3",
                    "s3Location": {
                        "bucket": "release-artifacts",
                        "key": "later.zip",
                        "bundleType": "zip",
                    },
                },
            }),
        )
        .await;
        if answered.0 == 200 {
            return answered;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected the deployment group {group} to take a deployment within \
             {PAST_DEPLOYMENT_SPAN:?}, last {:?}",
            answered.1
        );
        ::tokio::time::sleep(DEPLOYMENT_POLL).await;
    }
}

/// Poll `GetDeployment` until `deployment_id` holds a terminal status.
///
/// The terminal statuses are the members of the model's `DeploymentStatus` enum that no
/// further transition leaves (`smithy_query(query="shape",shape="DeploymentStatus")`).
async fn deployment_settles(
    router: &MultiServiceRouter,
    region: &str,
    deployment_id: &str,
    case: &str,
) {
    let deadline = std::time::Instant::now() + PAST_DEPLOYMENT_SPAN;
    loop {
        let (status, read) = codedeploy(
            router,
            region,
            "GetDeployment",
            &json!({ "deploymentId": deployment_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetDeployment, got {status} with body {read}"
        );
        let reported = read["deploymentInfo"]["status"].as_str().map(str::to_string);
        if matches!(
            reported.as_deref(),
            Some("Succeeded" | "Failed" | "Stopped")
        ) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: deployment {deployment_id} did not reach a terminal status within \
             {PAST_DEPLOYMENT_SPAN:?}, last {reported:?}"
        );
        ::tokio::time::sleep(DEPLOYMENT_POLL).await;
    }
}

/// The physical resource id the stack reports for each of its logical ids.
async fn physical_ids(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
) -> BTreeMap<String, String> {
    let (status, body) = cloudformation(
        router,
        region,
        "DescribeStackResources",
        &[("StackName", stack)],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from DescribeStackResources({stack}), got {status} with body {body}"
    );
    elements(&body, "LogicalResourceId")
        .into_iter()
        .zip(elements(&body, "PhysicalResourceId"))
        .collect()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// Assert that `actual` carries every member, element and scalar `expected` states, at the
/// same place — the subset check a "the owning service reports what the template declared"
/// assertion is, since that service also reports members no property declares (a minted id,
/// a creation time, a server-side default).
fn reports(actual: &Value, expected: &Value, case: &str, path: &str) {
    match expected {
        Value::Object(members) => {
            for (name, value) in members {
                let found = actual.get(name).cloned().unwrap_or(Value::Null);
                reports(&found, value, case, &format!("{path}.{name}"));
            }
        }
        Value::Array(items) => {
            for (index, value) in items.iter().enumerate() {
                let found = actual.get(index).cloned().unwrap_or(Value::Null);
                reports(&found, value, case, &format!("{path}[{index}]"));
            }
        }
        scalar => assert_eq!(
            actual, scalar,
            "case {case:?}: expected {path} to be {scalar}, got {actual}"
        ),
    }
}

/// One resource the row declares: its logical id, the name the template states for it (or
/// `None` where it leaves CloudFormation to generate one), and what the owning service must
/// report for it.
struct Declared {
    logical_id: &'static str,
    name: Option<&'static str>,
    reported: Value,
}

/// One row of the table: a template, and what the rule says the CodeDeploy resources it
/// declares must then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region its resources live in.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: String,
    /// The `Parameters` the row's `CreateStack` carries.
    parameters: &'static [(&'static str, &'static str)],
    /// The application every row declares.
    application: Declared,
    /// The deployment group, for a row that declares one.
    group: Option<Declared>,
    /// The deployment configuration, for a row that declares one.
    configuration: Option<Declared>,
    /// What the group's own deployment must report, for a row whose group asks for one
    /// through its `Deployment` property.
    deployment: Option<Value>,
    /// The output keys whose value must be the application's name.
    application_outputs: &'static [&'static str],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a named application and group in JSON, the group pointing at the \
                   application with Ref",
            region: "us-east-1",
            template: json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "a CodeDeploy application and deployment group",
                "Resources": {
                    "App": {
                        "Type": "AWS::CodeDeploy::Application",
                        "Properties": {
                            "ApplicationName": "cd-app-one",
                            "ComputePlatform": "Server",
                            "Tags": [{ "Key": "team", "Value": "delivery" }]
                        }
                    },
                    "Group": {
                        "Type": "AWS::CodeDeploy::DeploymentGroup",
                        "Properties": {
                            "ApplicationName": { "Ref": "App" },
                            "DeploymentGroupName": "cd-dg-one",
                            "ServiceRoleArn": SERVICE_ROLE_ARN,
                            "Ec2TagFilters": [
                                { "Key": "Name", "Value": "web", "Type": "KEY_AND_VALUE" }
                            ],
                            "DeploymentStyle": {
                                "DeploymentType": "IN_PLACE",
                                "DeploymentOption": "WITHOUT_TRAFFIC_CONTROL"
                            }
                        }
                    }
                },
                "Outputs": {
                    "ApplicationRef": { "Value": { "Ref": "App" } },
                    "ApplicationSub": {
                        "Value": { "Fn::Sub": ["${Name}", { "Name": { "Ref": "App" } }] }
                    }
                }
            })
            .to_string(),
            parameters: &[],
            application: Declared {
                logical_id: "App",
                name: Some("cd-app-one"),
                reported: json!({
                    "applicationName": "cd-app-one",
                    "computePlatform": "Server",
                }),
            },
            group: Some(Declared {
                logical_id: "Group",
                name: Some("cd-dg-one"),
                reported: json!({
                    "applicationName": "cd-app-one",
                    "deploymentGroupName": "cd-dg-one",
                    "serviceRoleArn": SERVICE_ROLE_ARN,
                    // `EC2TagFilter` spells its members in upper camel in CodeDeploy's own
                    // model, exactly as the property does, so they reach the service as the
                    // template wrote them.
                    "ec2TagFilters": [
                        { "Key": "Name", "Value": "web", "Type": "KEY_AND_VALUE" }
                    ],
                    // `DeploymentStyle`'s members CodeDeploy spells in lower camel.
                    "deploymentStyle": {
                        "deploymentType": "IN_PLACE",
                        "deploymentOption": "WITHOUT_TRAFFIC_CONTROL",
                    },
                }),
            }),
            configuration: None,
            deployment: None,
            application_outputs: &["ApplicationRef", "ApplicationSub"],
        },
        Row {
            case: "generated names in YAML, a Lambda application and a deployment \
                   configuration of the same stack",
            region: "us-west-2",
            template: [
                "Parameters:",
                "  RoleArn:",
                "    Type: String",
                "Resources:",
                "  App:",
                "    Type: AWS::CodeDeploy::Application",
                "    Properties:",
                "      ComputePlatform: Lambda",
                "  Config:",
                "    Type: AWS::CodeDeploy::DeploymentConfig",
                "    Properties:",
                "      ComputePlatform: Lambda",
                "      TrafficRoutingConfig:",
                "        Type: TimeBasedLinear",
                "        TimeBasedLinear:",
                "          LinearInterval: 1",
                "          LinearPercentage: 10",
                "  Group:",
                "    Type: AWS::CodeDeploy::DeploymentGroup",
                "    Properties:",
                "      ApplicationName: !Ref App",
                "      ServiceRoleArn: !Ref RoleArn",
                "      DeploymentConfigName: !Ref Config",
                "Outputs:",
                "  ApplicationRef:",
                "    Value: !Ref App",
                "",
            ]
            .join("\n"),
            parameters: &[("RoleArn", SERVICE_ROLE_ARN)],
            application: Declared {
                logical_id: "App",
                name: None,
                reported: json!({ "computePlatform": "Lambda" }),
            },
            group: Some(Declared {
                logical_id: "Group",
                name: None,
                reported: json!({ "serviceRoleArn": SERVICE_ROLE_ARN }),
            }),
            configuration: Some(Declared {
                logical_id: "Config",
                name: None,
                reported: json!({
                    "computePlatform": "Lambda",
                    "trafficRoutingConfig": {
                        "type": "TimeBasedLinear",
                        "timeBasedLinear": { "linearInterval": 1, "linearPercentage": 10 },
                    },
                }),
            }),
            deployment: None,
            application_outputs: &["ApplicationRef"],
        },
        Row {
            case: "a group selecting instances through the tag SET form, with alarms, \
                   triggers and an automatic rollback",
            region: "eu-west-1",
            template: json!({
                "Resources": {
                    "App": {
                        "Type": "AWS::CodeDeploy::Application",
                        "Properties": { "ApplicationName": "cd-app-three" }
                    },
                    "Group": {
                        "Type": "AWS::CodeDeploy::DeploymentGroup",
                        "Properties": {
                            "ApplicationName": { "Ref": "App" },
                            "DeploymentGroupName": "cd-dg-three",
                            "ServiceRoleArn": SERVICE_ROLE_ARN,
                            "AutoScalingGroups": ["asg-blue", "asg-green"],
                            "Ec2TagSet": {
                                "Ec2TagSetList": [
                                    [{ "Key": "stage", "Value": "blue", "Type": "KEY_AND_VALUE" }]
                                ]
                            },
                            "AlarmConfiguration": {
                                "Enabled": true,
                                "IgnorePollAlarmFailure": false,
                                "Alarms": [{ "Name": "cpu-high" }]
                            },
                            "AutoRollbackConfiguration": {
                                "Enabled": true,
                                "Events": ["DEPLOYMENT_FAILURE"]
                            },
                            "TriggerConfigurations": [{
                                "TriggerName": "on-failure",
                                "TriggerTargetArn": "arn:aws:sns:eu-west-1:000000000000:deploys",
                                "TriggerEvents": ["DeploymentFailure"]
                            }],
                            "OutdatedInstancesStrategy": "IGNORE",
                            "TerminationHookEnabled": true
                        }
                    }
                },
                "Outputs": { "ApplicationRef": { "Value": { "Ref": "App" } } }
            })
            .to_string(),
            parameters: &[],
            application: Declared {
                logical_id: "App",
                name: Some("cd-app-three"),
                reported: json!({
                    "applicationName": "cd-app-three",
                    // A declaration stating no compute platform takes CodeDeploy's own
                    // default for one, which is the platform the application really runs on.
                    "computePlatform": "Server",
                }),
            },
            group: Some(Declared {
                logical_id: "Group",
                name: Some("cd-dg-three"),
                reported: json!({
                    "deploymentGroupName": "cd-dg-three",
                    "autoScalingGroups": [{ "name": "asg-blue" }, { "name": "asg-green" }],
                    // The wrapper's own member is lower camel while the filter groups inside
                    // it keep the upper-camel members CodeDeploy's filter shape spells.
                    "ec2TagSet": {
                        "ec2TagSetList": [
                            [{ "Key": "stage", "Value": "blue", "Type": "KEY_AND_VALUE" }]
                        ]
                    },
                    "alarmConfiguration": {
                        "enabled": true,
                        "ignorePollAlarmFailure": false,
                        "alarms": [{ "name": "cpu-high" }],
                    },
                    "autoRollbackConfiguration": {
                        "enabled": true,
                        "events": ["DEPLOYMENT_FAILURE"],
                    },
                    "triggerConfigurations": [{
                        "triggerName": "on-failure",
                        "triggerTargetArn": "arn:aws:sns:eu-west-1:000000000000:deploys",
                        "triggerEvents": ["DeploymentFailure"],
                    }],
                    "outdatedInstancesStrategy": "IGNORE",
                    "terminationHookEnabled": true,
                }),
            }),
            configuration: None,
            deployment: None,
            application_outputs: &["ApplicationRef"],
        },
        Row {
            case: "a group that deploys a revision as soon as it exists",
            region: "ap-northeast-1",
            template: json!({
                "Resources": {
                    "App": {
                        "Type": "AWS::CodeDeploy::Application",
                        "Properties": {
                            "ApplicationName": "cd-app-four",
                            "ComputePlatform": "Server"
                        }
                    },
                    "Group": {
                        "Type": "AWS::CodeDeploy::DeploymentGroup",
                        "Properties": {
                            "ApplicationName": { "Ref": "App" },
                            "DeploymentGroupName": "cd-dg-four",
                            "ServiceRoleArn": SERVICE_ROLE_ARN,
                            "Deployment": {
                                "Description": "the first revision",
                                "IgnoreApplicationStopFailures": true,
                                "Revision": {
                                    "RevisionType": "S3",
                                    "S3Location": {
                                        "Bucket": "release-artifacts",
                                        "Key": "app.zip",
                                        "BundleType": "zip",
                                        "ETag": "d41d8cd98f00b204e9800998ecf8427e"
                                    }
                                }
                            }
                        }
                    }
                },
                "Outputs": { "ApplicationRef": { "Value": { "Ref": "App" } } }
            })
            .to_string(),
            parameters: &[],
            application: Declared {
                logical_id: "App",
                name: Some("cd-app-four"),
                reported: json!({ "applicationName": "cd-app-four" }),
            },
            group: Some(Declared {
                logical_id: "Group",
                name: Some("cd-dg-four"),
                reported: json!({ "deploymentGroupName": "cd-dg-four" }),
            }),
            configuration: None,
            deployment: Some(json!({
                "applicationName": "cd-app-four",
                "deploymentGroupName": "cd-dg-four",
                "description": "the first revision",
                "ignoreApplicationStopFailures": true,
                "revision": {
                    "revisionType": "S3",
                    "s3Location": {
                        "bucket": "release-artifacts",
                        "key": "app.zip",
                        "bundleType": "zip",
                        // `S3Location.ETag` is `s3Location.eTag`: only the initial letter of
                        // a member name moves.
                        "eTag": "d41d8cd98f00b204e9800998ecf8427e",
                    },
                },
            })),
            application_outputs: &["ApplicationRef"],
        },
        Row {
            case: "an ECS application whose group names its services and its target groups",
            region: "us-east-2",
            template: json!({
                "Resources": {
                    "App": {
                        "Type": "AWS::CodeDeploy::Application",
                        "Properties": {
                            "ApplicationName": "cd-app-five",
                            "ComputePlatform": "ECS"
                        }
                    },
                    "Group": {
                        "Type": "AWS::CodeDeploy::DeploymentGroup",
                        "Properties": {
                            "ApplicationName": { "Ref": "App" },
                            "DeploymentGroupName": "cd-dg-five",
                            "ServiceRoleArn": SERVICE_ROLE_ARN,
                            "ECSServices": [
                                { "ClusterName": "orders", "ServiceName": "checkout" }
                            ],
                            "DeploymentStyle": {
                                "DeploymentType": "BLUE_GREEN",
                                "DeploymentOption": "WITH_TRAFFIC_CONTROL"
                            },
                            "BlueGreenDeploymentConfiguration": {
                                "DeploymentReadyOption": {
                                    "ActionOnTimeout": "CONTINUE_DEPLOYMENT"
                                },
                                "TerminateBlueInstancesOnDeploymentSuccess": {
                                    "Action": "TERMINATE",
                                    "TerminationWaitTimeInMinutes": 5
                                }
                            },
                            "LoadBalancerInfo": {
                                "TargetGroupPairInfoList": [{
                                    "ProdTrafficRoute": {
                                        "ListenerArns": [
                                            "arn:aws:elasticloadbalancing:us-east-2:000000000000:listener/app/prod/1/2"
                                        ]
                                    },
                                    "TargetGroups": [
                                        { "Name": "tg-blue" },
                                        { "Name": "tg-green" }
                                    ]
                                }]
                            }
                        }
                    }
                },
                "Outputs": { "ApplicationRef": { "Value": { "Ref": "App" } } }
            })
            .to_string(),
            parameters: &[],
            application: Declared {
                logical_id: "App",
                name: Some("cd-app-five"),
                reported: json!({
                    "applicationName": "cd-app-five",
                    "computePlatform": "ECS",
                }),
            },
            group: Some(Declared {
                logical_id: "Group",
                name: Some("cd-dg-five"),
                reported: json!({
                    "deploymentGroupName": "cd-dg-five",
                    "ecsServices": [{ "clusterName": "orders", "serviceName": "checkout" }],
                    "deploymentStyle": {
                        "deploymentType": "BLUE_GREEN",
                        "deploymentOption": "WITH_TRAFFIC_CONTROL",
                    },
                    "blueGreenDeploymentConfiguration": {
                        "deploymentReadyOption": {
                            "actionOnTimeout": "CONTINUE_DEPLOYMENT",
                        },
                        "terminateBlueInstancesOnDeploymentSuccess": {
                            "action": "TERMINATE",
                            "terminationWaitTimeInMinutes": 5,
                        },
                    },
                    "loadBalancerInfo": {
                        "targetGroupPairInfoList": [{
                            "prodTrafficRoute": {
                                "listenerArns": [
                                    "arn:aws:elasticloadbalancing:us-east-2:000000000000:listener/app/prod/1/2"
                                ],
                            },
                            "targetGroups": [{ "name": "tg-blue" }, { "name": "tg-green" }],
                        }],
                    },
                }),
            }),
            configuration: None,
            deployment: None,
            application_outputs: &["ApplicationRef"],
        },
    ]
}

/// Whether `name` is inside the name form CodeDeploy's own model publishes for these
/// resources: 1 to [`MAX_CODEDEPLOY_NAME`] characters of `^[A-Za-z0-9+=,.@_-]*$`.
fn is_codedeploy_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_CODEDEPLOY_NAME
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+=,.@_-".contains(c))
}

/// The name the stack reports for one declared resource, held to the name the row states or
/// — where it states none — to the form CloudFormation generates and CodeDeploy accepts.
fn reported_name(
    case: &str,
    stack_name: &str,
    declared: &Declared,
    reported: &BTreeMap<String, String>,
) -> String {
    let name = reported
        .get(declared.logical_id)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the stack reports no resource for {:?}, got {reported:?}",
                declared.logical_id
            )
        })
        .clone();
    match declared.name {
        Some(stated) => assert_eq!(
            name, stated,
            "case {case:?}: expected the physical id of {} to be the declared name \
             {stated:?}, got {name:?}",
            declared.logical_id
        ),
        None => {
            assert!(
                name.starts_with(stack_name) && name.contains(declared.logical_id),
                "case {case:?}: expected a generated <stack>-<logical id>-<unique> name for \
                 {}, got {name:?}",
                declared.logical_id
            );
            assert!(
                is_codedeploy_name(&name),
                "case {case:?}: expected a name inside CodeDeploy's own 1..{MAX_CODEDEPLOY_NAME} \
                 form for {}, got {name:?}",
                declared.logical_id
            );
        }
    }
    name
}

/// Every row: the names the stack reports for its CodeDeploy resources are the ones
/// CodeDeploy itself answers to, the configuration each resource reports is the one the
/// template declared, and deleting the stack deletes them there.
#[tokio::test]
async fn a_stack_provisions_the_codedeploy_resources_its_template_declares() {
    let router = build_router();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("codedeploy-stack-{index}");
        // The deployments this row ran, which the stack's deletion waits for: a
        // deployment configuration one of them is still running under cannot be deleted.
        let mut ran: Vec<String> = Vec::new();

        let encoded: Vec<(String, String, String, String)> = row
            .parameters
            .iter()
            .enumerate()
            .map(|(position, (key, value))| {
                let member = position + 1;
                (
                    format!("Parameters.member.{member}.ParameterKey"),
                    (*key).to_string(),
                    format!("Parameters.member.{member}.ParameterValue"),
                    (*value).to_string(),
                )
            })
            .collect();
        let mut members: Vec<(&str, &str)> = vec![
            ("StackName", &stack_name),
            ("TemplateBody", &row.template),
        ];
        for (key_name, key, value_name, value) in &encoded {
            members.push((key_name, key));
            members.push((value_name, value));
        }
        let (status, body) = cloudformation(&router, region, "CreateStack", &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let ids = physical_ids(&router, region, &stack_name).await;
        let application = reported_name(case, &stack_name, &row.application, &ids);

        // The application the stack reports is one CodeDeploy holds, with the
        // configuration the template declared.
        let (status, read) = codedeploy(
            &router,
            region,
            "GetApplication",
            &json!({ "applicationName": &application }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetApplication on the name the stack \
             reported, got {status} with body {read}"
        );
        let mut expected = row.application.reported.clone();
        if row.application.name.is_none() {
            expected["applicationName"] = Value::String(application.clone());
        }
        reports(
            &read.get("application").cloned().unwrap_or(Value::Null),
            &expected,
            case,
            "application",
        );

        // And it is one of the applications the account lists.
        let (status, listed) = codedeploy(&router, region, "ListApplications", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListApplications, got {status} with body \
             {listed}"
        );
        assert!(
            listed
                .get("applications")
                .and_then(Value::as_array)
                .is_some_and(|names| names.iter().any(|name| name == &json!(&application))),
            "case {case:?}: expected ListApplications to carry {application:?}, got {listed}"
        );

        let outputs = outputs_of(&described);
        for key in row.application_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&application),
                "case {case:?}: expected output {key:?} to be the application name \
                 {application:?}, got {:?}",
                outputs.get(*key)
            );
        }

        // The deployment configuration the row declares is one CodeDeploy resolves.
        let configuration = row.configuration.as_ref().map(|declared| {
            let name = reported_name(case, &stack_name, declared, &ids);
            (name, declared.reported.clone())
        });
        if let Some((name, mut expected)) = configuration.clone() {
            let (status, read) = codedeploy(
                &router,
                region,
                "GetDeploymentConfig",
                &json!({ "deploymentConfigName": &name }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetDeploymentConfig on the name the \
                 stack reported, got {status} with body {read}"
            );
            expected["deploymentConfigName"] = Value::String(name.clone());
            reports(
                &read
                    .get("deploymentConfigInfo")
                    .cloned()
                    .unwrap_or(Value::Null),
                &expected,
                case,
                "deploymentConfigInfo",
            );
        }

        // The deployment group is one CodeDeploy resolves under the application the
        // template declared it in.
        let group = row.group.as_ref().map(|declared| {
            let name = reported_name(case, &stack_name, declared, &ids);
            (name, declared.reported.clone())
        });
        if let Some((name, mut expected)) = group.clone() {
            let (status, read) = codedeploy(
                &router,
                region,
                "GetDeploymentGroup",
                &json!({
                    "applicationName": &application,
                    "deploymentGroupName": &name,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetDeploymentGroup on the pair the \
                 stack reported, got {status} with body {read}"
            );
            expected["applicationName"] = Value::String(application.clone());
            expected["deploymentGroupName"] = Value::String(name.clone());
            if let Some((config, _)) = &configuration {
                expected["deploymentConfigName"] = Value::String(config.clone());
            }
            reports(
                &read
                    .get("deploymentGroupInfo")
                    .cloned()
                    .unwrap_or(Value::Null),
                &expected,
                case,
                "deploymentGroupInfo",
            );

            // A deployment group a stack created takes a deployment like any other —
            // once the one the stack itself may have left in flight is terminal.
            let (status, deployed) =
                deployment_on_free_group(&router, region, &application, &name, case).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from CreateDeployment on the group the stack \
                 created, got {status} with body {deployed}"
            );
            assert!(
                deployed
                    .get("deploymentId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty()),
                "case {case:?}: expected the deployment to be given an id, got {deployed}"
            );
            ran.push(
                deployed["deploymentId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }

        // The revision a group's `Deployment` property asks for was deployed to it, with
        // what that property declared.
        if let Some(expected) = &row.deployment {
            let group_name = group
                .clone()
                .map(|(name, _)| name)
                .expect("a row with a Deployment declares the group it deploys to");
            let (status, listed) = codedeploy(
                &router,
                region,
                "ListDeployments",
                &json!({
                    "applicationName": &application,
                    "deploymentGroupName": &group_name,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from ListDeployments, got {status} with body \
                 {listed}"
            );
            let ids = listed
                .get("deployments")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            // Two deployments ran on this group: the one its `Deployment` property asked
            // for, and the one made through the wire just above. The property's is the
            // OLDEST, and `ListDeployments` reports the newest first.
            let deployment_id = ids
                .last()
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no deployment was listed, got {listed}"));
            let (status, read) = codedeploy(
                &router,
                region,
                "GetDeployment",
                &json!({ "deploymentId": deployment_id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetDeployment, got {status} with body \
                 {read}"
            );
            reports(
                &read.get("deploymentInfo").cloned().unwrap_or(Value::Null),
                expected,
                case,
                "deploymentInfo",
            );
            ran.push(deployment_id.to_string());
        }

        // Deleting the stack deletes what it created in CodeDeploy — and where the stack
        // declared a deployment configuration, once the deployments this row ran under it
        // have reached a terminal status, since a configuration one is still running under
        // is not deletable.
        if configuration.is_some() {
            for deployment_id in &ran {
                deployment_settles(&router, region, deployment_id, case).await;
            }
        }
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;

        let (status, gone) = codedeploy(
            &router,
            region,
            "GetApplication",
            &json!({ "applicationName": &application }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from GetApplication once the stack deleted the \
             application, got {status} with body {gone}"
        );
        assert!(
            gone.get("__type")
                .and_then(Value::as_str)
                .is_some_and(|code| code.ends_with("ApplicationDoesNotExistException")),
            "case {case:?}: expected ApplicationDoesNotExistException once the application was \
             deleted, got {gone}"
        );
        if let Some((name, _)) = configuration {
            let (status, gone) = codedeploy(
                &router,
                region,
                "GetDeploymentConfig",
                &json!({ "deploymentConfigName": &name }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from GetDeploymentConfig once the stack \
                 deleted the configuration, got {status} with body {gone}"
            );
        }
    }
}

/// A deployment group whose declaration names no application is refused by the provisioner
/// itself, naming the property the type requires, rather than sent as a request CodeDeploy
/// would reject: `ApplicationName` is the other half of a group's identity.
#[tokio::test]
async fn a_deployment_group_declaring_no_application_fails_the_stack() {
    let router = build_router();
    let case = "a deployment group declaration with no ApplicationName";
    let region = "us-west-1";
    let template = json!({
        "Resources": {
            "Group": {
                "Type": "AWS::CodeDeploy::DeploymentGroup",
                "Properties": {
                    "DeploymentGroupName": "orphan",
                    "ServiceRoleArn": SERVICE_ROLE_ARN
                }
            }
        }
    })
    .to_string();

    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[
            ("StackName", "codedeploy-no-application"),
            ("TemplateBody", &template),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;

    let (_, described) = cloudformation(
        &router,
        region,
        "DescribeStacks",
        &[("StackName", "codedeploy-no-application")],
    )
    .await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the failed create to be rolled back, got {described}"
    );
    // The STACK's reason is the summary naming the resource that failed; WHICH property
    // the declaration lacks is that resource's own diagnosis, on its event.
    let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
    assert_eq!(
        reason,
        resource_failure_summary(failed_action::CREATE, ["Group"]),
        "case {case:?}: expected the stack reason to summarise what failed, got {reason:?}"
    );
    let (_, events) = cloudformation(
        &router,
        region,
        "DescribeStackEvents",
        &[("StackName", "codedeploy-no-application")],
    )
    .await;
    assert!(
        elements(&events, "ResourceStatusReason")
            .iter()
            .any(|reported| reported.contains("ApplicationName")),
        "case {case:?}: expected the failing resource's event to name the missing \
         property, got {events}"
    );
}

/// A stack whose template declares a deployment group CodeDeploy REFUSES does not report a
/// resource nothing created: the stack fails with the refusal that service gave, and the
/// application a client already had is untouched.
///
/// The refusal here is CodeDeploy's own `ApplicationDoesNotExistException`, reached by
/// pointing a group at an application name no application answers to — which is exactly the
/// call the provisioning makes, so a stack that completed without creating the group could
/// not happen.
#[tokio::test]
async fn a_deployment_group_the_owning_service_refuses_fails_the_stack() {
    let router = build_router();
    let case = "a group declared under an application nothing holds";
    let region = "eu-central-1";

    let (status, created) = codedeploy(
        &router,
        region,
        "CreateApplication",
        &json!({ "applicationName": "the-clients-own", "computePlatform": "Server" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the client's own CreateApplication to succeed, got {status} \
         with body {created}"
    );

    let template = json!({
        "Resources": {
            "Group": {
                "Type": "AWS::CodeDeploy::DeploymentGroup",
                "Properties": {
                    "ApplicationName": "no-such-application",
                    "DeploymentGroupName": "homeless",
                    "ServiceRoleArn": SERVICE_ROLE_ARN
                }
            }
        }
    })
    .to_string();
    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[
            ("StackName", "codedeploy-unknown-application"),
            ("TemplateBody", &template),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;

    let (_, described) = cloudformation(
        &router,
        region,
        "DescribeStacks",
        &[("StackName", "codedeploy-unknown-application")],
    )
    .await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the failed create to be rolled back, got {described}"
    );
    let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Group"),
        "case {case:?}: expected the failure to name the resource, got {reason:?}"
    );

    let (status, read) = codedeploy(
        &router,
        region,
        "GetApplication",
        &json!({ "applicationName": "the-clients-own" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the client's own application to be untouched, got {status} \
         with body {read}"
    );
}
