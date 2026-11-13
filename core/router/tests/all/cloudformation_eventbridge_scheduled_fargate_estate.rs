// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack's declared `AWS::ECS::TaskDefinition` is REGISTERED in Amazon ECS,
//! and the ARN the stack publishes for it is the one Amazon ECS allocated.
//!
//! The rule under test: **a stack that reaches a successful terminal state has created each
//! declared resource through the owning service's own create API, so the identifier the stack
//! publishes for it resolves in that service — and for a type whose identity that service
//! ASSIGNS, the published identifier is the one it answered rather than one CloudFormation
//! computed.** For an `AWS::ECS::TaskDefinition` the create is `RegisterTaskDefinition` and
//! the identity is the `family:revision` ARN, whose revision Amazon ECS allocates per
//! registration — so a family the account already holds a revision of is registered at the
//! next number, and the stack's own `Ref` has to carry that number.
//!
//! Seven crates have to agree for the whole estate below to deploy (CloudFormation makes the
//! calls; Amazon S3, CloudWatch Logs, Amazon EC2, IAM, Amazon ECS and Amazon EventBridge
//! serve them), so the suite lives here: a crate-local test can only ever see one side of the
//! call.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! family or leaves it to CloudFormation, whether the family is one Amazon ECS already holds
//! a revision of, which launch type and network mode the revision declares, whether the
//! template is JSON or YAML, and whether the container configuration carries maps whose keys
//! are the caller's own data (docker labels, a log driver's options).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{form_encode, opt_element, outputs_of};

/// The region every request here is signed for, and so the region every resource and ARN
/// below belongs to.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// How a stack read waits for the deployment's clock-owed span to come due: look again, with
/// a deadline generous enough that a loaded host still reaches it.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const POLL_ATTEMPTS: usize = 80;

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way (the
/// credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, String)],
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

/// Call one awsJson1_1 operation of a service addressed by its signing name and
/// `X-Amz-Target` prefix.
async fn json_rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

async fn ecs(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    json_rpc(
        router,
        "ecs",
        "AmazonEC2ContainerServiceV20141113",
        operation,
        request,
    )
    .await
}

async fn events(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    json_rpc(router, "events", "AWSEvents", operation, request).await
}

async fn logs(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    json_rpc(router, "logs", "Logs_20140328", operation, request).await
}

/// Read a stack until it reports a status no longer in progress, and answer that read's body.
async fn poll_to_terminal(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let mut described = String::new();
    for _ in 0..POLL_ATTEMPTS {
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack.to_string())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
        );
        described = body;
        if !opt_element(&described, "StackStatus")
            .unwrap_or_default()
            .ends_with("_IN_PROGRESS")
        {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    described
}

/// The stack's own events, which is what names the resource a failed deployment stopped on.
async fn stack_events(router: &MultiServiceRouter, stack: &str) -> String {
    let (_, body) = cloudformation(
        router,
        "DescribeStackEvents",
        &[("StackName", stack.to_string())],
    )
    .await;
    body
}

/// Create a stack from `template` with `parameters`, and read it to a terminal status that
/// must be `CREATE_COMPLETE`. The answer is that read's body.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    template: &str,
    parameters: &[(&str, String)],
) -> String {
    let mut members = vec![
        ("StackName", stack.to_string()),
        ("TemplateBody", template.to_string()),
        ("Capabilities.member.1", "CAPABILITY_IAM".to_string()),
    ];
    for (index, (key, value)) in parameters.iter().enumerate() {
        let position = index + 1;
        members.push((
            match position {
                1 => "Parameters.member.1.ParameterKey",
                2 => "Parameters.member.2.ParameterKey",
                _ => "Parameters.member.3.ParameterKey",
            },
            (*key).to_string(),
        ));
        members.push((
            match position {
                1 => "Parameters.member.1.ParameterValue",
                2 => "Parameters.member.2.ParameterValue",
                _ => "Parameters.member.3.ParameterValue",
            },
            value.clone(),
        ));
    }
    let (status, body) = cloudformation(router, "CreateStack", &members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    let described = poll_to_terminal(router, case, stack).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete; reason {:?}; events {}",
        opt_element(&described, "StackStatusReason"),
        stack_events(router, stack).await
    );
    described
}

/// The ARN Amazon ECS identifies the revision `revision` of `family` by.
fn task_definition_arn(family: &str, revision: i64) -> String {
    format!("arn:aws:ecs:{REGION}:{ACCOUNT_ID}:task-definition/{family}:{revision}")
}

// ---------------------------------------------------------------------------
// The recorded estate: an EventBridge schedule running a Fargate task
// ---------------------------------------------------------------------------

/// The estate a client deploys to have an EventBridge schedule run a Fargate task, written
/// as the resource types and properties the whole "EventBridge rule runs a Fargate task"
/// shape declares.
fn estate_template() -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Parameters": {
            "VpcId": { "Type": "String" },
            "SubnetIds": { "Type": "CommaDelimitedList" },
            "S3Location": { "Type": "String" }
        },
        "Resources": {
            "DeploymentBucket": {
                "Type": "AWS::S3::Bucket",
                "Properties": {
                    "BucketEncryption": {
                        "ServerSideEncryptionConfiguration": [
                            { "ServerSideEncryptionByDefault": { "SSEAlgorithm": "AES256" } }
                        ]
                    },
                    "PublicAccessBlockConfiguration": {
                        "BlockPublicAcls": true,
                        "BlockPublicPolicy": true,
                        "IgnorePublicAcls": true,
                        "RestrictPublicBuckets": true
                    }
                }
            },
            "DeploymentBucketPolicy": {
                "Type": "AWS::S3::BucketPolicy",
                "Properties": {
                    "Bucket": { "Ref": "DeploymentBucket" },
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Sid": "DenyInsecureTransport",
                            "Effect": "Deny",
                            "Principal": "*",
                            "Action": "s3:*",
                            "Resource": [
                                { "Fn::GetAtt": ["DeploymentBucket", "Arn"] },
                                { "Fn::Join": ["", [{ "Fn::GetAtt": ["DeploymentBucket", "Arn"] }, "/*"]] }
                            ],
                            "Condition": { "Bool": { "aws:SecureTransport": "false" } }
                        }]
                    }
                }
            },
            "TaskLogGroup": {
                "Type": "AWS::Logs::LogGroup",
                "Properties": { "LogGroupClass": "STANDARD", "RetentionInDays": 7 }
            },
            "TaskSecurityGroup": {
                "Type": "AWS::EC2::SecurityGroup",
                "Properties": {
                    "GroupDescription": "egress only for the scheduled task",
                    "VpcId": { "Ref": "VpcId" },
                    "SecurityGroupEgress": [
                        { "IpProtocol": "-1", "CidrIp": "0.0.0.0/0", "Description": "all egress" }
                    ]
                }
            },
            "TaskExecutionRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "ecs-tasks.amazonaws.com" },
                            "Action": "sts:AssumeRole"
                        }]
                    },
                    "ManagedPolicyArns": [
                        { "Fn::Join": ["", [
                            "arn:", { "Ref": "AWS::Partition" },
                            ":iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
                        ]] }
                    ]
                }
            },
            "EventBridgeRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "events.amazonaws.com" },
                            "Action": "sts:AssumeRole"
                        }]
                    },
                    "MaxSessionDuration": 3600
                }
            },
            "TaskArtifactPolicy": {
                "Type": "AWS::IAM::Policy",
                "Properties": {
                    "PolicyName": "read-the-deployment-bucket",
                    "Roles": [{ "Ref": "TaskExecutionRole" }],
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Action": ["s3:GetObject"],
                            "Resource": { "Fn::Join": ["", [{ "Fn::GetAtt": ["DeploymentBucket", "Arn"] }, "/*"]] }
                        }]
                    }
                }
            },
            "EventBridgeRolePolicy": {
                "Type": "AWS::IAM::Policy",
                "Properties": {
                    "PolicyName": "run-the-scheduled-task",
                    "Roles": [{ "Ref": "EventBridgeRole" }],
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [
                            { "Effect": "Allow", "Action": ["ecs:RunTask"], "Resource": "*" },
                            { "Effect": "Allow", "Action": ["iam:PassRole"], "Resource": "*" }
                        ]
                    }
                }
            },
            "ScheduledTaskCluster": {
                "Type": "AWS::ECS::Cluster",
                "Properties": { "CapacityProviders": ["FARGATE", "FARGATE_SPOT"] }
            },
            "TaskDefinition": {
                "Type": "AWS::ECS::TaskDefinition",
                "Properties": {
                    "Family": "scheduled-fargate-worker",
                    "Cpu": "256",
                    "Memory": "512",
                    "NetworkMode": "awsvpc",
                    "RequiresCompatibilities": ["FARGATE"],
                    "RuntimePlatform": { "CpuArchitecture": "ARM64", "OperatingSystemFamily": "LINUX" },
                    "ExecutionRoleArn": { "Fn::GetAtt": ["TaskExecutionRole", "Arn"] },
                    "ContainerDefinitions": [{
                        "Name": "worker",
                        "Image": "public.ecr.aws/amazonlinux/amazonlinux:2",
                        "Essential": true,
                        "Environment": [
                            { "Name": "S3_LOCATION", "Value": { "Ref": "S3Location" } }
                        ],
                        "LogConfiguration": {
                            "LogDriver": "awslogs",
                            "Options": {
                                "awslogs-group": { "Ref": "TaskLogGroup" },
                                "awslogs-region": { "Ref": "AWS::Region" },
                                "awslogs-stream-prefix": "worker"
                            }
                        }
                    }]
                }
            },
            "ScheduleRule": {
                "Type": "AWS::Events::Rule",
                "DependsOn": "EventBridgeRolePolicy",
                "Properties": {
                    "ScheduleExpression": "cron(05 23 27 * ? *)",
                    "State": "ENABLED",
                    "Targets": [{
                        "Id": "FargateTask",
                        "Arn": { "Fn::GetAtt": ["ScheduledTaskCluster", "Arn"] },
                        "RoleArn": { "Fn::GetAtt": ["EventBridgeRole", "Arn"] },
                        "EcsParameters": {
                            "TaskDefinitionArn": { "Ref": "TaskDefinition" },
                            "TaskCount": 1,
                            "LaunchType": "FARGATE",
                            "PlatformVersion": "LATEST",
                            "NetworkConfiguration": {
                                "AwsVpcConfiguration": {
                                    "Subnets": { "Ref": "SubnetIds" },
                                    "SecurityGroups": [{ "Fn::GetAtt": ["TaskSecurityGroup", "GroupId"] }],
                                    "AssignPublicIp": "ENABLED"
                                }
                            }
                        }
                    }]
                }
            }
        },
        "Outputs": {
            "DeploymentBucketName": { "Value": { "Ref": "DeploymentBucket" } },
            "TaskLogGroupName": { "Value": { "Ref": "TaskLogGroup" } },
            "EcsClusterName": { "Value": { "Ref": "ScheduledTaskCluster" } },
            "TaskDefinitionArn": { "Value": { "Ref": "TaskDefinition" } },
            "EventBridgeRuleName": { "Value": { "Ref": "ScheduleRule" } }
        }
    })
    .to_string()
}

/// Create the VPC and the subnet the estate's parameters name, through Amazon EC2's own
/// ec2Query wire.
async fn ec2_network(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let vpc_answer = ec2_query(
        router,
        &format!(
            "Action=CreateVpc&Version=2016-11-15&CidrBlock={}",
            form_encode("10.91.0.0/16")
        ),
    )
    .await;
    let vpc = opt_element(&vpc_answer, "vpcId").unwrap_or_else(|| {
        panic!("case {case:?}: expected a <vpcId> in the CreateVpc answer {vpc_answer}")
    });
    let subnet_answer = ec2_query(
        router,
        &format!(
            "Action=CreateSubnet&Version=2016-11-15&VpcId={vpc}&CidrBlock={}&\
             AvailabilityZone={REGION}a",
            form_encode("10.91.1.0/24")
        ),
    )
    .await;
    let subnet = opt_element(&subnet_answer, "subnetId").unwrap_or_else(|| {
        panic!("case {case:?}: expected a <subnetId> in the CreateSubnet answer {subnet_answer}")
    });
    (vpc, subnet)
}

/// Call one Amazon EC2 operation over its ec2Query wire, and answer the body.
async fn ec2_query(router: &MultiServiceRouter, body: &str) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ec2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("ec2"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    String::from_utf8_lossy(response.body()).to_string()
}

/// The recorded shape: the stack settles in `CREATE_COMPLETE`, publishes the five identifiers
/// its `Outputs` declare, and every one of them resolves in the service that owns it — the
/// task definition included, by the revision-qualified ARN Amazon ECS allocated.
#[tokio::test]
async fn the_eventbridge_scheduled_fargate_estate_resolves_in_every_owning_service() {
    let router = build_router();
    let case = "the EventBridge-scheduled Fargate estate";
    let (vpc, subnet) = ec2_network(&router, case).await;
    let stack = "scheduled-fargate-estate";
    let s3_location = "s3://scheduled-fargate-artifacts/pipelines/build-0001/artifact";
    let described = deploy(
        &router,
        case,
        stack,
        &estate_template(),
        &[
            ("VpcId", vpc),
            ("SubnetIds", subnet),
            ("S3Location", s3_location.to_string()),
        ],
    )
    .await;

    let outputs = outputs_of(&described);
    for key in [
        "DeploymentBucketName",
        "TaskLogGroupName",
        "EcsClusterName",
        "TaskDefinitionArn",
        "EventBridgeRuleName",
    ] {
        let value = outputs.get(key).unwrap_or_else(|| {
            panic!("case {case:?}: no {key:?} output with a value, got {outputs:?}")
        });
        assert!(
            !value.is_empty() && !value.contains("${"),
            "case {case:?}: output {key:?} carries {value:?}"
        );
    }

    // The task definition: the revision-qualified ARN of the declared family, resolved by
    // Amazon ECS's own read — the read that answers `ClientException` for an ARN the stack
    // published but nothing registered.
    let definition = &outputs["TaskDefinitionArn"];
    assert_eq!(
        definition.as_str(),
        task_definition_arn("scheduled-fargate-worker", 1),
        "case {case:?}: expected the revision-qualified task-definition ARN, got {definition:?}"
    );
    let (status, reported) = ecs(
        &router,
        "DescribeTaskDefinition",
        &json!({ "taskDefinition": definition }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ecs DescribeTaskDefinition on {definition:?} to answer 200, \
         got {status} with body {reported}"
    );
    assert_eq!(
        reported["taskDefinition"]["containerDefinitions"][0]["environment"][0]["value"].as_str(),
        Some(s3_location),
        "case {case:?}: expected the container's S3_LOCATION verbatim, got {reported}"
    );
    assert_eq!(
        (
            reported["taskDefinition"]["networkMode"].as_str(),
            reported["taskDefinition"]["runtimePlatform"]["cpuArchitecture"].as_str(),
            reported["taskDefinition"]["requiresCompatibilities"][0].as_str(),
        ),
        (Some("awsvpc"), Some("ARM64"), Some("FARGATE")),
        "case {case:?}: expected the declared launch configuration, got {reported}"
    );

    // The rule, its target pinned to the revision the stack registered, the log group and the
    // cluster each resolve in their own service.
    let rule = &outputs["EventBridgeRuleName"];
    let (status, reported) = events(&router, "DescribeRule", &json!({ "Name": rule })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected events DescribeRule on {rule:?} to answer 200, got {status} \
         with body {reported}"
    );
    assert_eq!(
        (
            reported["ScheduleExpression"].as_str(),
            reported["State"].as_str()
        ),
        (Some("cron(05 23 27 * ? *)"), Some("ENABLED")),
        "case {case:?}: expected the declared schedule and state, got {reported}"
    );
    let (status, reported) = events(&router, "ListTargetsByRule", &json!({ "Rule": rule })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected events ListTargetsByRule to answer 200, got {status} with \
         body {reported}"
    );
    assert_eq!(
        reported["Targets"][0]["EcsParameters"]["TaskDefinitionArn"].as_str(),
        Some(definition.as_str()),
        "case {case:?}: expected the target pinned to the revision the stack registered, got \
         {reported}"
    );

    let log_group = &outputs["TaskLogGroupName"];
    let (status, reported) = logs(
        &router,
        "DescribeLogGroups",
        &json!({ "logGroupNamePrefix": log_group }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected logs DescribeLogGroups to answer 200, got {status} with body \
         {reported}"
    );
    assert_eq!(
        reported["logGroups"][0]["logGroupName"].as_str(),
        Some(log_group.as_str()),
        "case {case:?}: expected CloudWatch Logs to hold the stack's log group, got {reported}"
    );

    let cluster = &outputs["EcsClusterName"];
    let (status, reported) =
        ecs(&router, "DescribeClusters", &json!({ "clusters": [cluster] })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ecs DescribeClusters to answer 200, got {status} with body \
         {reported}"
    );
    assert_eq!(
        reported["clusters"][0]["clusterName"].as_str(),
        Some(cluster.as_str()),
        "case {case:?}: expected Amazon ECS to hold the stack's cluster, got {reported}"
    );
}

// ---------------------------------------------------------------------------
// The rule over a set of declarations
// ---------------------------------------------------------------------------

/// One row of the table: a template declaring a task definition, and what Amazon ECS must
/// then hold for it.
struct Row {
    case: &'static str,
    stack: &'static str,
    template: &'static str,
    /// The family the revision must be registered under, or `None` where CloudFormation
    /// generates it (the row then asserts the generated FORM instead).
    family: Option<&'static str>,
    /// The revision Amazon ECS must have allocated — the second registration of a family
    /// gets the next number, which is why this is a row's own expectation and not a constant.
    revision: i64,
    /// The logical id the generated family is derived from, for the `None` row.
    logical_id: &'static str,
}

/// The rows are deployed in order on ONE router, so the two that declare the same family
/// exercise the revision counter.
const ROWS: &[Row] = &[
    Row {
        case: "a family the template names, on Fargate",
        stack: "orders-worker-stack",
        template: r#"{"Resources":{"Task":{"Type":"AWS::ECS::TaskDefinition","Properties":{
            "Family":"orders-worker","Cpu":"512","Memory":"1024","NetworkMode":"awsvpc",
            "RequiresCompatibilities":["FARGATE"],
            "ContainerDefinitions":[{"Name":"orders","Image":"public.ecr.aws/example/orders:1",
            "Essential":true}]}}},
          "Outputs":{"TaskDefinitionArn":{"Value":{"Ref":"Task"}}}}"#,
        family: Some("orders-worker"),
        revision: 1,
        logical_id: "Task",
    },
    Row {
        case: "a second stack declaring the family the first already registered",
        stack: "orders-worker-stack-two",
        template: r#"{"Resources":{"Task":{"Type":"AWS::ECS::TaskDefinition","Properties":{
            "Family":"orders-worker","Cpu":"1024","Memory":"2048","NetworkMode":"awsvpc",
            "RequiresCompatibilities":["FARGATE"],
            "ContainerDefinitions":[{"Name":"orders","Image":"public.ecr.aws/example/orders:2",
            "Essential":true}]}}},
          "Outputs":{"TaskDefinitionArn":{"Value":{"Ref":"Task"}}}}"#,
        family: Some("orders-worker"),
        revision: 2,
        logical_id: "Task",
    },
    Row {
        case: "a task definition the template does not name, in the YAML spelling, on EC2",
        stack: "unnamed-worker-stack",
        template: concat!(
            "Resources:\n",
            "  Batch:\n",
            "    Type: AWS::ECS::TaskDefinition\n",
            "    Properties:\n",
            "      NetworkMode: bridge\n",
            "      ContainerDefinitions:\n",
            "        - Name: batch\n",
            "          Image: public.ecr.aws/example/batch:1\n",
            "          Memory: 256\n",
            "          Essential: true\n",
            "Outputs:\n",
            "  TaskDefinitionArn:\n",
            "    Value: !Ref Batch\n",
        ),
        family: None,
        revision: 1,
        logical_id: "Batch",
    },
    Row {
        case: "a task definition carrying maps whose keys are the caller's own data",
        stack: "labelled-worker-stack",
        template: concat!(
            "Resources:\n",
            "  Task:\n",
            "    Type: AWS::ECS::TaskDefinition\n",
            "    Properties:\n",
            "      Family: labelled-worker\n",
            "      NetworkMode: bridge\n",
            "      Volumes:\n",
            "        - Name: shared\n",
            "          DockerVolumeConfiguration:\n",
            "            Scope: task\n",
            "            Driver: local\n",
            "            Labels:\n",
            "              Team: reports\n",
            "      ContainerDefinitions:\n",
            "        - Name: labelled\n",
            "          Image: public.ecr.aws/example/labelled:1\n",
            "          Memory: 256\n",
            "          Essential: true\n",
            "          DockerLabels:\n",
            "            Team: reports\n",
            "            tier: batch\n",
            "          LogConfiguration:\n",
            "            LogDriver: awslogs\n",
            "            Options:\n",
            "              awslogs-group: /aws/ecs/labelled\n",
            "              awslogs-stream-prefix: labelled\n",
            "Outputs:\n",
            "  TaskDefinitionArn:\n",
            "    Value: !Ref Task\n",
        ),
        family: Some("labelled-worker"),
        revision: 1,
        logical_id: "Task",
    },
];

/// Every row's declared task definition is registered in Amazon ECS, and the ARN the stack
/// publishes is the one that service allocated — family and revision both.
#[tokio::test]
async fn a_stacks_declared_task_definition_is_registered_in_ecs() {
    let router = build_router();
    for row in ROWS {
        let described = deploy(&router, row.case, row.stack, row.template, &[]).await;
        let outputs = outputs_of(&described);
        let published = outputs.get("TaskDefinitionArn").unwrap_or_else(|| {
            panic!(
                "case {:?}: no TaskDefinitionArn output with a value, got {outputs:?}",
                row.case
            )
        });

        // The family: the one the template named, or — where it named none — the name
        // CloudFormation generated for the resource, in the form
        // `<stack name>-<logical id>-<unique id>`.
        let expected_family = match row.family {
            Some(family) => family.to_string(),
            None => {
                let prefix = format!("{}-{}-", row.stack, row.logical_id);
                let generated = published
                    .rsplit_once(':')
                    .and_then(|(path, _)| path.rsplit_once('/'))
                    .map(|(_, family)| family.to_string())
                    .unwrap_or_default();
                assert!(
                    generated.starts_with(&prefix),
                    "case {:?}: expected CloudFormation's generated family to start with \
                     {prefix:?}, got {generated:?} out of {published:?}",
                    row.case
                );
                generated
            }
        };
        let expected = task_definition_arn(&expected_family, row.revision);
        assert_eq!(
            published, &expected,
            "case {:?}: expected the stack to publish {expected:?}, got {published:?}",
            row.case
        );

        // …and Amazon ECS really holds that revision, ACTIVE, under that family and number.
        let (status, reported) = ecs(
            &router,
            "DescribeTaskDefinition",
            &json!({ "taskDefinition": published }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected ecs DescribeTaskDefinition on {published:?} to answer 200, \
             got {status} with body {reported}",
            row.case
        );
        let held = &reported["taskDefinition"];
        assert_eq!(
            (
                held["family"].as_str(),
                held["revision"].as_i64(),
                held["status"].as_str(),
                held["taskDefinitionArn"].as_str(),
            ),
            (
                Some(expected_family.as_str()),
                Some(row.revision),
                Some("ACTIVE"),
                Some(expected.as_str()),
            ),
            "case {:?}: expected Amazon ECS to hold revision {} of {expected_family:?} as \
             ACTIVE, got {reported}",
            row.case,
            row.revision
        );
    }

    // The data maps of the last row reach Amazon ECS with the keys the template wrote: a
    // docker label, a docker volume label and a log-driver option are the CALLER's own names,
    // not modelled member names, so none of them may be re-cased on the way.
    let case = "a task definition carrying maps whose keys are the caller's own data";
    let (status, reported) = ecs(
        &router,
        "DescribeTaskDefinition",
        &json!({ "taskDefinition": task_definition_arn("labelled-worker", 1) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ecs DescribeTaskDefinition to answer 200, got {status} with \
         body {reported}"
    );
    let container = &reported["taskDefinition"]["containerDefinitions"][0];
    let expected_labels = json!({ "Team": "reports", "tier": "batch" });
    assert_eq!(
        container["dockerLabels"], expected_labels,
        "case {case:?}: expected the docker labels the template wrote ({expected_labels}), got \
         {reported}"
    );
    let expected_options = json!({
        "awslogs-group": "/aws/ecs/labelled",
        "awslogs-stream-prefix": "labelled"
    });
    assert_eq!(
        container["logConfiguration"]["options"], expected_options,
        "case {case:?}: expected the log-driver options the template wrote \
         ({expected_options}), got {reported}"
    );
    assert_eq!(
        reported["taskDefinition"]["volumes"][0]["dockerVolumeConfiguration"]["labels"],
        json!({ "Team": "reports" }),
        "case {case:?}: expected the volume labels the template wrote, got {reported}"
    );
}

/// Deleting the stack deregisters the revision it registered: Amazon ECS keeps an INACTIVE
/// revision discoverable, so the read is what says the stack gave the revision up rather than
/// leaving it ACTIVE behind it.
#[tokio::test]
async fn deleting_the_stack_deregisters_the_revision_it_registered() {
    let router = build_router();
    let case = "a stack whose one resource is a named task definition";
    let template = concat!(
        "Resources:\n",
        "  Task:\n",
        "    Type: AWS::ECS::TaskDefinition\n",
        "    Properties:\n",
        "      Family: transient-worker\n",
        "      NetworkMode: bridge\n",
        "      ContainerDefinitions:\n",
        "        - Name: transient\n",
        "          Image: public.ecr.aws/example/transient:1\n",
        "          Memory: 256\n",
        "          Essential: true\n",
        "Outputs:\n",
        "  TaskDefinitionArn:\n",
        "    Value: !Ref Task\n",
    );
    let described = deploy(&router, case, "transient-worker-stack", template, &[]).await;
    let published = outputs_of(&described)
        .get("TaskDefinitionArn")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no TaskDefinitionArn output in {described}"));

    let (status, body) = cloudformation(
        &router,
        "DeleteStack",
        &[("StackName", "transient-worker-stack".to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );

    // The deletion is read to its own terminal answer, which for a deleted stack is no
    // description at all.
    let mut reported = Value::Null;
    for _ in 0..POLL_ATTEMPTS {
        let (status, body) = ecs(
            &router,
            "DescribeTaskDefinition",
            &json!({ "taskDefinition": &published }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the revision to stay discoverable, got {status} with body \
             {body}"
        );
        reported = body;
        if reported["taskDefinition"]["status"].as_str() == Some("INACTIVE") {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    assert_eq!(
        reported["taskDefinition"]["status"].as_str(),
        Some("INACTIVE"),
        "case {case:?}: expected the stack's deletion to deregister {published:?}, got \
         {reported}"
    );
}
