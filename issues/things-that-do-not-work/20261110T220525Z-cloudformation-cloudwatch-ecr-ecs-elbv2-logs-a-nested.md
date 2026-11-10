Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch__ecr__ecs__elbv2__logs.cfn-nested-stack-microservice-platform-alarm-echoes-declared-period
Issue-Key: cloudformation__cloudwatch__ecr__ecs__elbv2__logs.cfn-nested-stack-microservice-platform-alarm-echoes-declared-period|hard#21|AssertionError|raise AssertionError(self._summary(name, _one_line(exc))) from exc
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# cloudformation__cloudwatch__ecr__ecs__elbv2__logs: a nested-stack deploy fails because DescribeStacks Outputs entries carry no OutputValue

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: DescribeStacks returns Outputs entries without the OutputValue member, so a nested-stack deployment that reads a declared output cannot proceed.
AWS: Smithy (cloudformation) DescribeStacks -> Stacks[].Outputs[] of shape Output {OutputKey, OutputValue, Description, ExportName}. A template that declares `Outputs: { <Key>: { Value: <expr> } }` and reaches CREATE_COMPLETE always reports that output with both OutputKey and the resolved OutputValue; the resolved value is whatever the template's expression evaluates to (a resource attribute, a Ref, a joined string), which is request/state derived, but its PRESENCE on every declared output of a CREATE_COMPLETE stack is the requirement.
Emulator: the deploy leg fails with a bare KeyError on 'OutputValue' — the entries exist under Stacks[].Outputs but omit the member entirely. Replay: AssertionError "LEGS 1/12 PASS | bootstrap=PASS deploy=FAIL / WORKFLOW FAILED at deploy: 'OutputValue'".
Why: nested stacks and CDK/CLI workflows chain almost exclusively through outputs — a parent passes a child's output into the next resource, and a client reads the stack's outputs to learn the ALB DNS name, the log group, the repository URI. An output with no value makes a CREATE_COMPLETE stack unusable downstream, and the failure surfaces as a client-side KeyError rather than as anything the API reported.
Fix: the CloudFormation stack-output component — the Outputs resolver and the DescribeStacks serializer must evaluate each declared output and emit Output.OutputValue (alongside OutputKey, and Description/ExportName when declared) for every stack in a terminal CREATE_COMPLETE/UPDATE_COMPLETE state, including nested child stacks.
Related open filing that does NOT carry this replay key: 20261109T153103Z-cloudformation-ec2-elbv2-2-net-new-causes-a.md reports Outputs entries carrying no value under the cloudformation__ec2__elbv2 heading; the suite did not mark this check OWNED by it, so it survives as a live failure under this heading.
Traces: cloudformation__cloudwatch__ecr__ecs__elbv2__logs.cfn-nested-stack-microservice-platform-alarm-echoes-declared-period

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator.

[mismatch] cloudformation__cloudwatch__ecr__ecs__elbv2__logs.cfn-nested-stack-microservice-platform-alarm-echoes-declared-period
  File "check_...cfn_nested_stack_microservice_platform_alarm_echoes_declared_period.py", line 112, in run -> stack = legs.hard(...)
  File "...", line 225, in hard -> raise AssertionError(self._summary(name, _one_line(exc)))
  AssertionError: LEGS 1/12 PASS | bootstrap=PASS deploy=FAIL
  WORKFLOW FAILED at deploy: 'OutputValue'

The check deploys a parent template (platform-parent.yaml) with network/service/observability child stacks and reads the parent's outputs to continue.
Expected (AWS): every declared output of the CREATE_COMPLETE parent appears in DescribeStacks as {'OutputKey': <key>, 'OutputValue': <resolved value>}.
Actual (emulator): the entry has no 'OutputValue' key at all, so indexing it raises KeyError: 'OutputValue'.

Smithy consulted (model services/service-com-amazonaws-cloudformation-cloudformation): DescribeStacks output Stacks[].Outputs[] -> Output {OutputKey, OutputValue, Description, ExportName}.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the check id in trace_ids reproduces.
2. Deploy a parent template with AWS::CloudFormation::Stack children and an `Outputs` section whose entries have `Value:` expressions (e.g. !GetAtt on a child stack output, a Ref to an ALB, a log group name).
3. Wait for StackStatus CREATE_COMPLETE, then call cloudformation.describe_stacks(StackName=<stack id>) and read Stacks[0]['Outputs'].
4. Expected: each entry has both 'OutputKey' and 'OutputValue'. Actual: 'OutputValue' is absent, so any consumer indexing it raises KeyError: 'OutputValue'.
5. Re-replay the id in trace_ids to confirm the fix.
