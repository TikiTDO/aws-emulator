Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__docdb__dynamodb__ecs__elbv2__firehose__lambda__rds.cdk-nine-stack-estate-delivery-stream-echoes-buffering-hints
Issue-Key: cloudformation__docdb__dynamodb__ecs__elbv2__firehose__lambda__rds.cdk-nine-stack-estate-delivery-stream-echoes-buffering-hints|_run_cli#14|AssertionError|assert result.returncode == 0, (
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# cloudformation: a Custom:: resource never leaves CREATE_IN_PROGRESS, so a multi-stack CDK deploy fails with "was still CREATE_IN_PROGRESS when the wait for it to be created ran out"

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: a CloudFormation custom resource (here Custom::S3AutoDeleteObjects, the CDK auto-delete provider) is never driven to a terminal state — it sits in CREATE_IN_PROGRESS until the deployer's wait expires, failing the whole stack and therefore the whole `cdk deploy --all`.
AWS: Smithy (cloudformation) CreateStack / DescribeStackEvents -> StackEvents[].{ResourceStatus, ResourceStatusReason}; DescribeStackResources -> StackResources[].ResourceStatus. A custom resource is provisioned by CloudFormation invoking the backing provider (a Lambda function named by ServiceToken) with a request that carries a ResponseURL; the provider PUTs SUCCESS or FAILED to that URL, and CloudFormation then moves the resource to CREATE_COMPLETE or CREATE_FAILED. The requirement is that every custom resource reaches one of those two terminal states (CloudFormation's own timeout for an unanswered provider is one hour, surfaced as CREATE_FAILED with a reason naming the custom resource).
Emulator: the resource stays CREATE_IN_PROGRESS indefinitely. Replay: `cdk deploy --all` exits 1 with "❌ sim2real-est-<id>-DocDBAthenaNLB failed to deploy / DeploymentError: Resource updates failed: .../AutoDeleteObjectsCustomResource/Default (Custom::S3AutoDeleteObjects ...) was still CREATE_IN_PROGRESS when the wait for it to be created ran out" — the stack and logical-id names are per-run values; the defect is the non-terminal status.
Why: any template that uses a custom resource — CDK's own auto-delete-objects, bucket-notification and log-retention providers, plus every user-written Custom:: provider — never completes. The stack hangs rather than succeeding or failing, so a client gets neither the resources nor a diagnosable CREATE_FAILED reason, and a nine-stack estate cannot be stood up at all.
Fix: the CloudFormation custom-resource provisioning component — the path that invokes the ServiceToken provider with the Create request and consumes its ResponseURL callback must transition the resource to CREATE_COMPLETE/CREATE_FAILED, and must fail the resource with a ResourceStatusReason when no response arrives within its timeout.
Traces: cloudformation__docdb__dynamodb__ecs__elbv2__firehose__lambda__rds.cdk-nine-stack-estate-delivery-stream-echoes-buffering-hints

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator.

[mismatch] cloudformation__docdb__dynamodb__ecs__elbv2__firehose__lambda__rds.cdk-nine-stack-estate-delivery-stream-echoes-buffering-hints
  File "check_...cdk_nine_stack_estate_delivery_stream_echoes_buffering_hints.py", line 633, in _cdk -> _run_cli(ctx, cmd, work, timeout_s, "`cdk deploy --all`")
  AssertionError: `cdk deploy --all` exited 1, expected 0; output tail: "... ❌  sim2real-est-0ab5275ede32-DocDBAthenaNLB failed to deploy
  ‣ DeploymentError: Resource updates failed:
  sim2real-est-0ab5275ede32-DocDBAthenaNLB/athena-analytics-data-bucket/AutoDeleteObjectsCustomResource/Default  (Custom::S3AutoDeleteObjects athenaanalyticsdatabucketAutoDeleteObjectsCustomResource8B68670E)
    Custom::S3AutoDeleteObjects
    sim2real-est-0ab5275ede32-DocDBAthenaNLB-athenaanalyticsdatabucketAutoDeleteObjectsCustomResource8B68670E-D2C75B59CC07
    was still CREATE_IN_PROGRESS when the wait for it to be created ran out"

Expected (AWS): the Custom::S3AutoDeleteObjects resource reaches CREATE_COMPLETE (its provider Lambda is invoked and PUTs SUCCESS to the ResponseURL), or CREATE_FAILED with a ResourceStatusReason; the stack then reaches CREATE_COMPLETE and `cdk deploy --all` exits 0.
Actual (emulator): the resource remains CREATE_IN_PROGRESS for the whole deploy window; no terminal status or reason is ever recorded.

Smithy consulted (model services/service-com-amazonaws-cloudformation-cloudformation): CreateStack; DescribeStackEvents output StackEvents[].{LogicalResourceId, PhysicalResourceId, ResourceStatus, ResourceStatusReason}; DescribeStackResources output StackResources[].ResourceStatus.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the check id in trace_ids reproduces.
2. Deploy any template containing a custom resource, e.g. an S3 bucket with CDK's autoDeleteObjects (which synthesizes a Custom::S3AutoDeleteObjects resource plus its provider Lambda), or a minimal hand-written `Type: Custom::Thing` with a `ServiceToken` pointing at a Lambda that PUTs SUCCESS to its ResponseURL.
3. Poll DescribeStackEvents / DescribeStackResources for that logical id.
4. Expected: the resource moves CREATE_IN_PROGRESS -> CREATE_COMPLETE (or CREATE_FAILED with a reason) and the stack reaches a terminal status. Actual: it stays CREATE_IN_PROGRESS until the client's wait expires.
5. Re-replay the id in trace_ids to confirm the fix.
