Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__docdb__dynamodb__ecs__elbv2__firehose__lambda__rds.cdk-nine-stack-estate-deploys-serves-and-destroys
Issue-Key: cloudformation__docdb__dynamodb__ecs__elbv2__firehose__lambda__rds.cdk-nine-stack-estate-deploys-serves-and-destroys|_run_cli#14|AssertionError|assert result.returncode == 0, (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# A nine-stack CDK estate cannot deploy: Custom::S3AutoDeleteObjects never leaves CREATE_IN_PROGRESS

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a Lambda-backed CloudFormation custom resource never completes, so the whole estate deploy fails.

AWS: a Custom:: resource whose ServiceToken is a Lambda function completes when the function PUTs its SUCCESS response to the pre-signed ResponseURL CloudFormation passed in the event; CloudFormation then records CREATE_COMPLETE for that resource and the stack continues. CDK's Custom::S3AutoDeleteObjects provider is exactly this shape, and `cdk deploy --all` exits 0.

Emulator: the custom resource stays CREATE_IN_PROGRESS until CloudFormation's wait expires. `cdk deploy --all` exits 1 with 'sim2real-est-...-Monitoring failed to deploy / DeploymentError: Resource updates failed: ... Custom::S3AutoDeleteObjects <physical-id> was still CREATE_IN_PROGRESS when the wait for it to be created ran out'. One failing child stack takes the nine-stack estate down.

Why: every CDK construct that relies on a Lambda-backed custom resource (autoDeleteObjects buckets, BucketDeployment, the custom-resources Provider framework, AwsCustomResource lookups) is unusable; a multi-stack application cannot be deployed or destroyed.

Fix: the CloudFormation custom-resource response path — the ResponseURL handed to the function and the ingest of the function's SUCCESS/FAILED response that must flip the resource out of CREATE_IN_PROGRESS.

Traces: the failing check in trace_ids. The same root cause is recorded for other services in 20261108T043811Z-cloudformation-s3-a-lambda-backed-cloudformation-custom-resource-never-completes.md and 20261111T081112Z-apigateway-cloudformation-lambda-a-cloudformation-custom-resource-delete-never-completes-so.md; neither carries this replay key.

## Evidence
Expected: `cdk deploy --all` exits 0.
Actual: AssertionError: `cdk deploy --all` exited 1, expected 0; output tail: '... failed to deploy / DeploymentError: Resource updates failed: .../FirehoseDestinationBucket/AutoDeleteObjectsCustomResource/Default (Custom::S3AutoDeleteObjects ...) was still CREATE_IN_PROGRESS when the wait for it to be created ran out'

## Steps to Reproduce
1. Synthesize the nine-stack CDK estate (DocumentDB, DynamoDB, ECS, ELBv2, Firehose, Lambda, RDS) in which the Firehose destination bucket uses autoDeleteObjects.
2. Run `cdk deploy --all`: the Monitoring stack's Custom::S3AutoDeleteObjects resource never leaves CREATE_IN_PROGRESS and the CLI exits 1.
