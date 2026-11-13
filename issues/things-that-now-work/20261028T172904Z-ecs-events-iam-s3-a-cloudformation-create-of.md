Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__iam__s3.e2e-eventbridge-ecs-task-trigger-debugging-lifecycle
Issue-Key: ecs__events__iam__s3.e2e-eventbridge-ecs-task-trigger-debugging-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 68e4a9e742a6835063083b9abc795c05ac8a482e

# ecs__events__iam__s3: a CloudFormation create of the EventBridge-scheduled Fargate estate ends ROLLBACK_COMPLETE instead of CREATE_COMPLETE, so the ECS cluster, task definition and rule are never provisioned

> _Note: this issue was identified against the code base at commit `68e4a9e742a6835063083b9abc795c05ac8a482e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CreateStack of a valid ECS/EventBridge/IAM/S3/Logs template is rolled back instead of created, so every resource and Output the stack declares is missing.

AWS: CreateStack on a well-formed template whose resources are all individually creatable provisions them and the stack settles in StackStatus CREATE_COMPLETE; DescribeStacks then reports that status and the template's Outputs. Smithy: cloudformation#StackStatus enumerates CREATE_IN_PROGRESS, CREATE_FAILED, CREATE_COMPLETE, ROLLBACK_IN_PROGRESS, ROLLBACK_COMPLETE, ... — ROLLBACK_COMPLETE is reached only after a resource create actually FAILS and the already-created resources are rolled back. The template in question is the recorded one and declares, in dependency order: AWS::S3::Bucket (SSE-S3 BucketEncryption + PublicAccessBlockConfiguration), AWS::S3::BucketPolicy (aws:SecureTransport Deny), AWS::Logs::LogGroup (LogGroupClass STANDARD), AWS::EC2::SecurityGroup (in a caller-supplied VpcId, all-egress), two AWS::IAM::Role (one assumed by ecs-tasks.amazonaws.com carrying ManagedPolicyArns arn:<AWS::Partition>:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy, one assumed by events.amazonaws.com with MaxSessionDuration 3600), two AWS::IAM::Policy, AWS::ECS::Cluster with CapacityProviders [FARGATE, FARGATE_SPOT], AWS::ECS::TaskDefinition (awsvpc, RequiresCompatibilities FARGATE, RuntimePlatform ARM64/LINUX, awslogs LogConfiguration whose awslogs-group is a Ref to the log group), and AWS::Events::Rule (ScheduleExpression cron(05 23 27 * ? *), State ENABLED, one ECS target whose EcsParameters.TaskDefinitionArn is a Ref to the task definition and whose RoleArn is a GetAtt on the EventBridge role, DependsOn the EventBridge role policy). The request passes Capabilities [CAPABILITY_IAM], so the IAM resources are authorised. AWS creates this stack; recorded behavior confirms CREATE_COMPLETE with the five Outputs DeploymentBucketName, TaskLogGroupName, EcsClusterName, TaskDefinitionArn (a revision-qualified task-definition ARN from Ref) and EventBridgeRuleName.

Emulator: the CreateStack call is accepted, and the stack then settles in StackStatus 'ROLLBACK_COMPLETE' (the check polls DescribeStacks until a terminal status and asserts CREATE_COMPLETE; it got ROLLBACK_COMPLETE). Nothing after the create can run: there is no cluster, no task definition, no rule and no Outputs, so DescribeRule / DescribeTaskDefinition / ListTargetsByRule / IAM and S3 reads of the stack's resources all have nothing to read. The prerequisite EC2 VPC and subnet the check creates for VpcId/SubnetIds DID succeed, so the rollback is inside the stack's own resource provisioning, not in its parameters. Hypothesis: one of the resource types in the list above is unprovisionable in the emulator's CFN resource registry (the IAM role carrying an AWS-managed ManagedPolicyArn built with Fn::Join over AWS::Partition, the S3::BucketPolicy, the Logs::LogGroup LogGroupClass, or the Events::Rule ECS target with EcsParameters) and its failure rolls the whole stack back; the stack's own events name which one.

Why: a client deploying the standard "EventBridge rule runs a Fargate task" estate — the shape CDK and the ECS console emit — gets an empty, rolled-back stack where AWS gives it a working one. Every downstream call fails with ResourceNotFound-class errors against resources the client believes it deployed, and because the stack is in ROLLBACK_COMPLETE the client's only recovery on real AWS (read the events, fix the one bad resource) finds nothing wrong with the template.

Note: this is NOT the inverse defect already filed as 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back-failed.md, nor 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md / 20261023T113357Z-cloudformation-reports-create-complete-for-logs-cloudwatch-ecs.md (those are stacks wrongly reporting CREATE_COMPLETE). The closest open analogue is 20261028T033849Z-cloudwatch-iam-lambda-logs-opensearch-s3-cfn-stack-whose-only-resource-is-an-aws.md (an AWS::OpenSearchService::Domain stack rolling back), and a same-shaped defect for ResolverRule was closed as 20261025T225734Z; neither covers this resource set.

Fix: the CloudFormation stack-create orchestration and the per-resource-type provisioners for the declared types — the create path for AWS::ECS::Cluster / AWS::ECS::TaskDefinition / AWS::Events::Rule (ECS target with EcsParameters) / AWS::IAM::Role (ManagedPolicyArns) / AWS::IAM::Policy / AWS::S3::Bucket + BucketPolicy / AWS::Logs::LogGroup / AWS::EC2::SecurityGroup — in the cloudformation service crate, plus the stack's Outputs resolution (Ref on AWS::ECS::TaskDefinition must yield the revision-qualified ARN). The stack must reach CREATE_COMPLETE and expose the five Outputs rather than rolling back.

Traces: behavioral check ecs__events__iam__s3.e2e-eventbridge-ecs-task-trigger-debugging-lifecycle, phase 1/4 'deploy'.

## Evidence
Failing check: [mismatch] ecs__events__iam__s3.e2e-eventbridge-ecs-task-trigger-debugging-lifecycle

AssertionError: [phase 1/4 deploy] stack sim2real-ebecs-e2e-<suffix> reached 'ROLLBACK_COMPLETE', not CREATE_COMPLETE — read its stack events for the failure

Expected (AWS): DescribeStacks(StackName=<stack>).Stacks[0].StackStatus == "CREATE_COMPLETE", with Outputs {DeploymentBucketName, TaskLogGroupName, EcsClusterName, TaskDefinitionArn, EventBridgeRuleName}; TaskDefinitionArn matching arn:aws[\w-]*:ecs:<region>:<account>:task-definition/<family>:1; DescribeRule returning (ScheduleExpression, State) == ("cron(05 23 27 * ? *)", "ENABLED"); DescribeTaskDefinition returning the container's S3_LOCATION environment value verbatim.

Actual (emulator): StackStatus "ROLLBACK_COMPLETE" at the terminal poll; no Outputs and none of the declared resources exist, so the assertions on the rule, the task definition and the task role are never reached. The stack name, suffix and account id above are per-run values, not part of the requirement; the requirement is the terminal status and the Outputs set.

The EC2 prerequisites created outside the stack (CreateVpc 10.91.0.0/16, CreateSubnet 10.91.1.0/24) succeeded, and CreateStack itself returned 200 — the divergence is the stack's provisioning outcome.

## Steps to Reproduce
1. ec2.create_vpc(CidrBlock="10.91.0.0/16"); ec2.create_subnet(VpcId=<vpc>, CidrBlock="10.91.1.0/24", AvailabilityZone=<first available AZ>).
2. cloudformation.create_stack(StackName=<name>, TemplateBody=<the eventbridge-ecs-task template: S3 Bucket + BucketPolicy, Logs LogGroup, EC2 SecurityGroup, two IAM Roles (one with ManagedPolicyArns AmazonECSTaskExecutionRolePolicy, one assumed by events.amazonaws.com), two IAM Policies, ECS Cluster with CapacityProviders [FARGATE, FARGATE_SPOT], ECS TaskDefinition (awsvpc/FARGATE/ARM64 with awslogs), Events Rule cron(05 23 27 * ? *) ENABLED with one ECS target pinning Ref TaskDefinition>, Capabilities=["CAPABILITY_IAM"], Parameters=[VpcId=<vpc>, SubnetIds=<subnet>, S3Location="s3://<name>-absent/pipelines/build-0001/artifact"]).
3. Poll cloudformation.describe_stacks(StackName=<name>) until StackStatus is terminal.
4. Observe StackStatus == "ROLLBACK_COMPLETE" with no Outputs, where AWS reports CREATE_COMPLETE with the five declared Outputs. Read describe_stack_events for the stack to see which resource provisioner failed.
