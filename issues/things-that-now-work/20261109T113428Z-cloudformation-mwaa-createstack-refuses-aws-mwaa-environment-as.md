Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__mwaa.deploy-mwaa-environment-round-trip-and-delete
Issue-Key: cloudformation__mwaa|CreateStack|ValidationError
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# cloudformation__mwaa: CreateStack refuses AWS::MWAA::Environment as an unrecognized resource type, so no MWAA environment can be provisioned from a template

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause in the CloudFormation emulator's template resource-type surface, grounded in the failing behavioural check cloudformation__mwaa.deploy-mwaa-environment-round-trip-and-delete.

The sibling check under this same heading (cloudformation__mwaa.deploy-mwaa-environment-network-configuration-single-subnet-rejected) fails on the identical refusal via CreateChangeSet, but the suite marks it OWNED by 20261107T012236Z-cloudformation-ssm-createchangeset-on-a-stack-in-review.md, so it is skipped here and not listed in trace_ids — fixing the cause below is nevertheless what unblocks it too. No open issue covers AWS::MWAA::Environment: the existing unrecognized-resource-type issues name other types (AWS::SSM::MaintenanceWindowTarget, AWS::CDK::Metadata, AWS::IoTSiteWise::AssetModel, AWS::DirectoryService::MicrosoftAD, AWS::EC2::TransitGatewayRouteTable*, AWS::EKS::PodIdentityAssociation, …).

=== Cause 1 of 1: the template resource-type registry does not recognize AWS::MWAA::Environment, so the stack is refused at template validation ===
AWS:      recorded behavior only (the CloudFormation Smithy model carries no resource-type registry; the supported types are the service's own data). AWS::MWAA::Environment is a supported CloudFormation resource type — it is documented at https://docs.aws.amazon.com/AWSCloudFormation/latest/TemplateReference/aws-resource-mwaa-environment.html, takes Name / ExecutionRoleArn / SourceBucketArn / DagS3Path / NetworkConfiguration / EnvironmentClass / MinWorkers / MaxWorkers / WebserverAccessMode / Tags, publishes the environment Name through Ref, and publishes Arn, CeleryExecutorQueue, DatabaseVpcEndpointService, WebserverVpcEndpointService and WebserverUrl through Fn::GetAtt. A CreateStack whose template declares it is ACCEPTED: the call answers HTTP 200 with a StackId, the resource is provisioned through the MWAA control plane (which this emulator already serves — Smithy com.amazonaws.mwaa#AmazonMWAA models CreateEnvironment / GetEnvironment / ListEnvironments / DeleteEnvironment), the stack reaches CREATE_COMPLETE, and MWAA's own GetEnvironment then reports the declared properties, the stack's tags and the same GetAtt values. CloudFormation's "Template format error: Unrecognized resource types: [<types>]" ValidationError — whose fixed text is "Template format error: Unrecognized resource types: [" … "]" with the bracketed list taken from the submitted template — is the refusal AWS reserves for types it genuinely does not support, and AWS::MWAA::Environment is not one of them.
Emulator: CreateStack is refused outright with HTTP 400 ValidationError and the message "Template format error: Unrecognized resource types: [AWS::MWAA::Environment]". Every other type in the same template (AWS::EC2::VPC / Subnet / RouteTable / VPCEndpoint / SubnetRouteTableAssociation / SecurityGroup / SecurityGroupIngress, AWS::S3::Bucket, AWS::IAM::Role) is accepted — only the MWAA type is missing from the registry the template validator checks, so the refusal happens before any resource is created and no stack id is ever issued. The same refusal is produced on the CreateChangeSet path.
Why:      no MWAA environment can be provisioned as infrastructure-as-code against this emulator. Every CloudFormation, SAM or CDK stack that declares an Airflow environment — the normal way MWAA is deployed, since the environment needs a VPC, subnets, a security group, an S3 DAG bucket and an execution role created alongside it — is rejected at submit time with a ValidationError that tells the author their template is malformed. A client cannot work around it, because the failure is template validation, not a resource create: it never gets a StackId to poll, so there is no partial stack, no stack event and nothing to roll back. It also makes the type's Ref/GetAtt contract (Name, Arn, CeleryExecutorQueue, DatabaseVpcEndpointService, WebserverVpcEndpointService, WebserverUrl) completely unreachable, and it breaks the composition the whole scenario rests on: CloudFormation provisioning a resource that the already-bootstrapped MWAA control plane then serves.
Fix:      CloudFormation's template resource-type registry and the provisioning handler behind it, in the cloudformation service crate: register AWS::MWAA::Environment with its property schema, its Ref (the environment Name) and its GetAtt attributes, and back its create/update/delete lifecycle with the MWAA control-plane state the mwaa crate already owns (CreateEnvironment / GetEnvironment / DeleteEnvironment), including propagation of the stack's tags onto the environment. The refusal path itself (the "Unrecognized resource types" ValidationError) is correct and should stay for types AWS really does not support.
Traces:   cloudformation__mwaa.deploy-mwaa-environment-round-trip-and-delete (and, already owned elsewhere, cloudformation__mwaa.deploy-mwaa-environment-network-configuration-single-subnet-rejected, which hits the same refusal through CreateChangeSet).

## Evidence
Replayed behavioural suite: cloudformation__mwaa 0/2 passed, 2 failed. One of the two is marked OWNED and skipped; this one is filed.

cloudformation__mwaa.deploy-mwaa-environment-round-trip-and-delete  [error]
  Check flow: CreateStack(StackName=<unique>, TemplateBody=<template>, Capabilities=["CAPABILITY_IAM"], Tags=[{sim2real: <stack>}]) where the template declares a VPC, two private subnets with no internet route, a private route table, an S3 gateway endpoint plus one interface endpoint per MWAA dependency, a security group with self-ingress, a versioned S3 bucket, an IAM execution role, and:
    "MwaaEnvironment": {"Type": "AWS::MWAA::Environment", "Properties": {"Name": <env>, "ExecutionRoleArn": {"Fn::GetAtt": ["ExecutionRole", "Arn"]}, "SourceBucketArn": {"Fn::GetAtt": ["Bucket", "Arn"]}, "DagS3Path": "dags", "NetworkConfiguration": {"SubnetIds": [...], "SecurityGroupIds": [...]}, "EnvironmentClass": "mw1.micro", "MaxWorkers": 1, "MinWorkers": 1, "WebserverAccessMode": "PRIVATE_ONLY", "Tags": {...}}}
  with Outputs exporting Ref MwaaEnvironment and Fn::GetAtt Arn / CeleryExecutorQueue / DatabaseVpcEndpointService / WebserverVpcEndpointService / WebserverUrl.
  Expected: CreateStack answers 200 with a StackId; the stack reaches CREATE_COMPLETE; ListEnvironments and GetEnvironment report the environment AVAILABLE with the declared EnvironmentClass/WebserverAccessMode/DagS3Path/ExecutionRoleArn/SourceBucketArn/MinWorkers/MaxWorkers, the declared subnets and security group, and the stack's tags; each GetAtt equals MWAA's own value; after DeleteStack, GetEnvironment answers ResourceNotFoundException "Environment <Name> not found.".
  Actual:   CreateStack is refused and nothing is created:
            File "client.py", line 1094, in _make_api_call
              raise error_class(parsed_response, operation_name)
            botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the
              CreateStack operation: Template format error: Unrecognized resource types:
              [AWS::MWAA::Environment]
  The bracketed type list is the variable part of that message and is taken from the submitted template; "Template format error: Unrecognized resource types: [" and "]" are its fixed text. The divergence is that AWS::MWAA::Environment appears in that list at all.

Same refusal through the change-set path (ALREADY OWNED by 20261107T012236Z-cloudformation-ssm-createchangeset-on-a-stack-in-review.md, not counted as a cause here):
  cloudformation__mwaa.deploy-mwaa-environment-network-configuration-single-subnet-rejected
    botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the
      CreateChangeSet operation: Template format error: Unrecognized resource types:
      [AWS::MWAA::Environment]

Cross-check that the backing service exists: the MWAA emulator is bootstrapped and serves CreateEnvironment / GetEnvironment / ListEnvironments / DeleteEnvironment (the mwaa heading passes 13 of 19 checks in this same run), so the gap is CloudFormation's resource-type coverage, not a missing MWAA control plane.

## Steps to Reproduce
1. Boot the emulator.
2. Build a JSON template that declares an AWS::EC2::VPC, two AWS::EC2::Subnet, an AWS::EC2::RouteTable with two AWS::EC2::SubnetRouteTableAssociation, an AWS::EC2::VPCEndpoint (Gateway, com.amazonaws.<region>.s3), an AWS::EC2::SecurityGroup with AWS::EC2::SecurityGroupIngress self-ingress, an AWS::S3::Bucket with versioning enabled, an AWS::IAM::Role trusting airflow-env.amazonaws.com, and an AWS::MWAA::Environment whose Properties are Name, ExecutionRoleArn (Fn::GetAtt ExecutionRole.Arn), SourceBucketArn (Fn::GetAtt Bucket.Arn), DagS3Path "dags", NetworkConfiguration {SubnetIds, SecurityGroupIds}, EnvironmentClass "mw1.micro", MinWorkers 1, MaxWorkers 1, WebserverAccessMode "PRIVATE_ONLY", Tags. Export Ref MwaaEnvironment and Fn::GetAtt Arn / CeleryExecutorQueue / DatabaseVpcEndpointService / WebserverVpcEndpointService / WebserverUrl in Outputs.
3. Call CreateStack(StackName=<unique>, TemplateBody=<that template>, Capabilities=["CAPABILITY_IAM"]).
4. Expected: HTTP 200 with a StackId, then DescribeStacks polls to CREATE_COMPLETE. Observed: HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::MWAA::Environment]" and no stack at all.
5. Call CreateChangeSet with the same TemplateBody and ChangeSetType=CREATE: the same ValidationError is returned, confirming the resource-type registry — not a single operation — is the cause.
6. After the type is registered: DescribeStacks reaches CREATE_COMPLETE; MWAA ListEnvironments/GetEnvironment report the environment AVAILABLE with the declared properties, subnets, security group and the stack's tags; every Fn::GetAtt output equals MWAA's own value for that attribute; and after DeleteStack, GetEnvironment answers ResourceNotFoundException "Environment <Name> not found.".
