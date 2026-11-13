Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch__ecr__ecs__elbv2__logs.cfn-nested-stack-microservice-platform-alarm-echoes-declared-period
Issue-Key: cloudformation__cloudwatch__ecr__ecs__elbv2__logs.cfn-nested-stack-microservice-platform-alarm-echoes-declared-period|<lambda>#21|Timeout|lambda: cfn.describe_stacks(StackName=name)["Stacks"][0]["StackStatus"],
Issue-Commit: 9bd5b7da5aa90741deae627d938338ffb9a7a206

# cloudformation__cloudwatch__ecr__ecs__elbv2__logs: the nested-stack microservice platform deploy never completes — the whole check is cut off at its 60s budget with no leg finishing

> _Note: this issue was identified against the code base at commit `9bd5b7da5aa90741deae627d938338ffb9a7a206`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: a parent stack with three AWS::CloudFormation::Stack children (network, service, observability) never gets far enough for its declared outputs to be read — the twelve-leg deploy makes no measurable progress and is cut off by the harness after 60s, with no leg report emitted at all.

AWS: Smithy (cloudformation) CreateStack/DescribeStacks — a parent template whose children are AWS::CloudFormation::Stack resources creates real child stacks, the parent reaches StackStatus CREATE_COMPLETE, and DescribeStacks reports Stacks[].Outputs[] as Output{OutputKey, OutputValue, Description, ExportName} with OutputValue resolved for every declared output (the ALB DNS name, the ECR repository URI, the cluster/service names, the log group). Those outputs, wired through the parent's Fn::GetAtt on the children, are what the rest of the estate (ALB/Fargate serving HTTP, a sibling child's AWS::CloudWatch::Alarm holding its declared Period, a failing child rolling the parent back, drift omitting child stacks) is read from. The per-leg timing of a real deploy is recorded behavior only; what the recording shows is that the whole flow completes and each leg can be observed.

Emulator: the check produces no API-level diagnosis at all — the replay detail is exactly 'Check timed out after 60.0s'. Nothing was returned to compare: no status, no error code, no leg summary, so not one of the 12 legs completed inside the budget. This is a REGRESSION in failure mode for this same check: open issue 20261110T220525Z-cloudformation-cloudwatch-ecr-ecs-elbv2-logs-a-nested.md (and the sibling-check issue 20261111T111216Z-cloudformation-cloudwatch-ecr-ecs-elbv2-nested-stack-microservice-deploy-fails-because-stack-outputs.md) record this check failing FAST at the deploy leg with KeyError 'OutputValue' after 1/12 legs passed; it now hangs instead of answering, so neither issue's replay key matches this failure and the suite reports it as live. Hypothesis (neither the model nor the replay states it): the parent or one of its children never reaches a terminal StackStatus, so the deploy poll never returns.

Why: a client deploying a nested-stack estate gets neither a completed stack nor a reported failure — it blocks. Every downstream read (the ALB endpoint it must serve from, the sibling alarm's Period, the rollback of a bad child, the parent's drift view) is unreachable, and in CI the divergence surfaces as a hang rather than as anything the CloudFormation API said.

Fix: the CloudFormation nested-stack create/settle path (child-stack creation and the parent's transition to a terminal CREATE_COMPLETE/ROLLBACK_COMPLETE status) together with the stack-output resolver and the DescribeStacks serializer that must emit OutputValue for every declared output of a terminal stack.

Traces: the failing check id in trace_ids.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator.

[mismatch] cloudformation__cloudwatch__ecr__ecs__elbv2__logs.cfn-nested-stack-microservice-platform-alarm-echoes-declared-period
  Error: Check timed out after 60.0s

The check (GROUP deployment-order, SERVICES cloudformation/cloudwatch/ecr/ecs/elbv2/logs) uploads platform-network.yaml, platform-service.yaml and platform-observability.yaml, deploys platform-parent.yaml that wires them by Fn::GetAtt on their Outputs (leg budget timeout_s=2400 for the deploy leg), then runs 12 legs over the resulting estate.

Expected (AWS): the parent reaches CREATE_COMPLETE, DescribeStacks reports every declared output with both OutputKey and a resolved OutputValue, and the legs proceed (ALB serves HTTP via Fargate, the sibling child's alarm keeps its declared Period, an invalid child rolls the parent back, drift omits child stacks).
Actual (emulator): the check never completes — the harness cuts it off at its 60s wall budget and no leg summary is produced (contrast the previously recorded failure for this same check: 'LEGS 1/12 PASS | bootstrap=PASS deploy=FAIL / WORKFLOW FAILED at deploy: OutputValue').

Smithy consulted (services/service-com-amazonaws-cloudformation-cloudformation): DescribeStacks -> Stacks[].Outputs[] of shape Output{OutputKey, OutputValue, Description, ExportName}.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the check id in trace_ids reproduces.
2. Upload three child templates to S3 and CreateStack a parent template that declares them as AWS::CloudFormation::Stack resources (TemplateURL + Parameters) and whose Outputs use Fn::GetAtt <Child>.Outputs.<Key>.
3. Poll DescribeStacks on the parent for a terminal StackStatus.
4. Expected: CREATE_COMPLETE within a bounded time, and Stacks[0].Outputs entries each carrying OutputKey and OutputValue. Actual: the deploy never settles into a state the caller can continue from, so the flow hangs and the 60s check budget expires with no leg completed.
5. Re-replay the id in trace_ids to confirm the fix.
