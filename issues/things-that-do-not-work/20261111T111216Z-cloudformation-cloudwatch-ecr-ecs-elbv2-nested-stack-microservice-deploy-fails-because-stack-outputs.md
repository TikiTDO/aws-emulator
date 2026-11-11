Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch__ecr__ecs__elbv2__logs.cfn-nested-stack-microservice-platform-workflow
Issue-Key: cloudformation__cloudwatch__ecr__ecs__elbv2__logs.cfn-nested-stack-microservice-platform-workflow|hard#21|AssertionError|raise AssertionError(self._summary(name, _one_line(exc))) from exc
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# Nested-stack microservice deploy fails because stack Outputs carry no OutputValue

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Cause 1 of 1: the nested-stack platform workflow cannot get past its deploy leg because DescribeStacks returns Output entries without OutputValue.

AWS: DescribeStacks returns Stacks[].Outputs[] as Output structures with OutputKey and OutputValue always populated (plus optional Description/ExportName); that is how a parent stack's resolved values — here the ALB DNS name, the ECR repository URI, the ECS cluster/service names and the log group — are handed to the caller and to sibling stacks.

Emulator: the Output entries have no OutputValue member, so the workflow fails at its deploy leg with the missing key: 'LEGS 1/12 PASS | bootstrap=PASS deploy=FAIL / WORKFLOW FAILED at deploy: OutputValue'. Only 1 of the workflow's 12 legs runs.

Why: every consumer of a stack output breaks — `aws cloudformation describe-stacks`, CDK outputs files, nested/sibling stack wiring, and any client that indexes OutputValue as the modelled shape permits. A twelve-stage estate is unusable from its first stage.

Fix: the CloudFormation DescribeStacks serializer / stack-output state — resolve each declared output and always emit OutputValue.

Traces: the failing check in trace_ids. Open issues record the same missing member for other services (e.g. 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md); none carries this replay key.

## Evidence
Expected: all 12 workflow legs pass; DescribeStacks Outputs entries expose OutputValue.
Actual: AssertionError: LEGS 1/12 PASS | bootstrap=PASS deploy=FAIL / WORKFLOW FAILED at deploy: 'OutputValue'

## Steps to Reproduce
1. CreateStack the nested-stack microservice template (ECR repository, ECS cluster/service, ELBv2 load balancer, log group, alarms) with an Outputs block.
2. DescribeStacks on the parent: Stacks[0].Outputs entries have OutputKey but no OutputValue.
