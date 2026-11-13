Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__redshift.e2e-diagnose-redshift-cluster-health-issues-lifecycle
Issue-Key: cloudwatch__redshift.e2e-diagnose-redshift-cluster-health-issues-lifecycle|phase#7|_PhaseError|raise _PhaseError(
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# CloudFormation DescribeStacks returns stack Outputs entries without OutputValue, breaking every stack-output consumer

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
NOTE ON SERVICE ATTRIBUTION: the failing check's id is `cloudwatch__redshift.e2e-diagnose-redshift-cluster-health-issues-lifecycle`, so the service token recorded on this issue is `cloudwatch__redshift` (a multi-service check covering cloudwatch and redshift). The defect itself is in the CloudFormation deploy front-end that the check's deploy phase uses, so the fix lands in the cloudformation service.

Cause 1 of 1: DescribeStacks reports Outputs members that carry an OutputKey but no OutputValue.
AWS: for a stack in CREATE_COMPLETE, DescribeStacks returns Stacks[0].Outputs as a list of Output structures each of which carries OutputKey AND OutputValue (plus optional Description/ExportName); a resolved output always has a value. Recorded behavior only — the cloudformation Smithy model marks both members optional, so the requirement here is the recorded one: a CREATE_COMPLETE stack's outputs are resolved and valued.
Emulator: after the stack reaches CREATE_COMPLETE, the deploy phase's dict comprehension over Stacks[0]['Outputs'] raises `KeyError: 'OutputValue'` — at least one Output entry in the response has no OutputValue member at all. The phase fails immediately, so the whole four-phase lifecycle (deploy, solve, cleanup, destroy) never runs.
Why: stack outputs are the standard hand-off from a template to its consumers (endpoints, ARNs, generated names). A consumer reading `o['OutputValue']` crashes with a KeyError rather than receiving the value, and any orchestration that resolves one stack's outputs into another's parameters cannot proceed — even though the stack claims CREATE_COMPLETE.
Fix: the CloudFormation stack-output resolution and the DescribeStacks serializer — every Output emitted for a stack in a completed state must carry a resolved OutputValue (and a template output whose value cannot be resolved must make the stack fail rather than be reported as a valueless output).
Traces: cloudwatch__redshift.e2e-diagnose-redshift-cluster-health-issues-lifecycle (phase 1/4 deploy).

## Evidence
cloudwatch__redshift.e2e-diagnose-redshift-cluster-health-issues-lifecycle:
  _PhaseError: [phase 1/4 deploy] KeyError: 'OutputValue'

The failing statement in the check's deploy phase, reached only after the stack has polled to CREATE_COMPLETE:
  outs = {o["OutputKey"]: o["OutputValue"]
          for o in cfn.describe_stacks(StackName=stack)["Stacks"][0].get("Outputs", [])}

Expected: every element of Stacks[0].Outputs carries both OutputKey and OutputValue.
Actual: at least one element carries OutputKey but no OutputValue, so the lookup raises KeyError and the lifecycle aborts in its first phase.

## Steps to Reproduce
1. Boot the emulator and point a cloudformation client at it.
2. CreateStack with a template that declares an Outputs section (the check's deployments/redshift-health/template.json, parameters Token, NodeType, MasterUserPassword, AllAlarmPeriod, LeaderAlarmPeriod).
3. Poll DescribeStacks until StackStatus == CREATE_COMPLETE.
4. Read Stacks[0]['Outputs'] and index each element by 'OutputValue'. Expect a value for every declared output; observe a missing OutputValue member.
5. Replay cloudwatch__redshift.e2e-diagnose-redshift-cluster-health-issues-lifecycle to confirm the deploy phase gets past phase 1/4.
