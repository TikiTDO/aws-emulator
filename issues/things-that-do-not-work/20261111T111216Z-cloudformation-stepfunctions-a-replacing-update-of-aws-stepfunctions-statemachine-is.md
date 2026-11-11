Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__stepfunctions.cfn-stepfunctions-state-machine-in-place-update-retained-replacement-then-not-found
Issue-Key: cloudformation__stepfunctions.cfn-stepfunctions-state-machine-in-place-update-retained-replacement-then-not-found|_assert_replaced_and_retained#36|AssertionError|assert (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# A replacing update of AWS::StepFunctions::StateMachine is not reported as an update and deletes the retained old state machine

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: replacement is emitted as a bare create-then-delete and UpdateReplacePolicy: Retain is ignored.

AWS: a replacing update is reported on the logical resource as UPDATE_IN_PROGRESS (old state machine ARN), CREATE_IN_PROGRESS 'Resource creation Initiated' (new ARN), CREATE_COMPLETE (new ARN), UPDATE_COMPLETE (new ARN). With UpdateReplacePolicy: Retain the old state machine is not deleted — no DELETE_IN_PROGRESS/DELETE_COMPLETE for it — and it stays startable with its execution history intact.

Emulator: the events are only CREATE_IN_PROGRESS / CREATE_IN_PROGRESS('Resource creation Initiated') / CREATE_COMPLETE for the new ARN followed by DELETE_IN_PROGRESS / DELETE_COMPLETE naming the old ARN — no UPDATE_* events on the logical resource, and Retain is disregarded.

Why: a client reading stack events cannot tell that a replacement happened or which machine it replaced, and a state machine the template author asked to retain (with its executions) is deleted.

Fix: the CloudFormation update engine's replacement path — emit the logical resource's UPDATE_IN_PROGRESS/UPDATE_COMPLETE around the create, and honour UpdateReplacePolicy Retain by skipping the cleanup delete.

Traces: the failing check in trace_ids.

## Evidence
Expected: Machine announces its replacement (UPDATE_IN_PROGRESS on the old ARN ... UPDATE_COMPLETE on the new ARN) and the old state machine survives.
Actual: AssertionError: Machine must announce its replacement, its events were [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-cfnsm-b-...'), ('CREATE_COMPLETE', None, '...cfnsm-b-...'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-cfnsm-a-...')]

## Steps to Reproduce
1. CreateStack a template with an AWS::StepFunctions::StateMachine carrying UpdateReplacePolicy: Retain and an explicit StateMachineName.
2. UpdateStack changing StateMachineName (a replacement trigger).
3. DescribeStackEvents: no UPDATE_* events for the logical id and DELETE_COMPLETE for the old ARN despite Retain.
