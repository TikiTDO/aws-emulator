Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-sfn-out-of-band-delete-drift
Issue-Step: teardown#15
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-sfn-out-of-band-delete-drift cloudformation-sfn-owning-stack-reverse-lookup
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: states|DescribeStateMachine|4xx->2xx|$
Issue-Key: states|DescribeStateMachine|4xx->2xx|$.__type
Issue-Key: states|DescribeStateMachine|4xx->2xx|$.message
Issue-Key: states|DescribeStateMachine|4xx->2xx|status
Issue-Key: states|ListTagsForResource|2xx->2xx|$.tags[*]
Issue-Commit: 2d97abf0bacad2832802b49de9c950da1a09c669

# Step Functions state machines outlive their stack and carry no CloudFormation tags

> _Note: this issue was identified against the code base at commit `2d97abf0bacad2832802b49de9c950da1a09c669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStateMachine

## Description
The Step Functions service state is not reconciled with the CloudFormation stack that owns a state machine: a deleted stack's state machine stays ACTIVE, and a stack-created state machine carries none of the reserved stack tags. This bundle carries 2 distinct causes. (Note on the bootstrap gate: this service's wire token is 'states' while its bootstrap record is the aliased 'sfn', which IS bootstrapped - the emulator answered both calls, so these are fidelity findings, not a bootstrap gap.)

Cause 1 of 2 - DescribeStateMachine still answers for a state machine whose owning stack was deleted.
AWS: once the resource is gone, DescribeStateMachine answers 400 with __type 'com.amazonaws.swf.service.v2.model#StateMachineDoesNotExist' and message "State Machine Does Not Exist: '<stateMachineArn>'" - fixed text "State Machine Does Not Exist: '" / "'" around the ARN from the request.
Emulator: 200 with the full state machine description (status 'ACTIVE', its definition, roleArn, type and configuration), so the resource survives the stack that created it.
Why: a caller confirming a teardown polls for the not-found error and never sees it; worse, the leaked state machine keeps its name taken and remains invocable after the stack that owned it is gone.
Fix: the AWS::StepFunctions::StateMachine deletion path must remove the state machine from Step Functions state so DescribeStateMachine's handler raises StateMachineDoesNotExist.
Traces: cloudformation-sfn-out-of-band-delete-drift#teardown#15.

Cause 2 of 2 - ListTagsForResource returns no aws:cloudformation tags for a stack-created state machine.
AWS: CloudFormation stamps every resource it creates with aws:cloudformation:stack-id (the stack ARN), aws:cloudformation:stack-name (the stack name) and aws:cloudformation:logical-id (the resource's logical id), and Step Functions returns them from ListTagsForResource as key/value pairs.
Emulator: tags is empty of all three.
Why: those tags are the documented route from a live state machine back to its owning stack and template position; without them the resource-to-stack reverse lookup has nothing to go on.
Fix: the AWS::StepFunctions::StateMachine provisioning path must stamp the three reserved tags into the Step Functions tag set that ListTagsForResource reads.
Traces: cloudformation-sfn-owning-stack-reverse-lookup#task#4.

## Evidence
Replay of simparity-wire-aws-v1, 2 divergence clusters, both stub_path='' and owned_by=[].
Cause 1: expected status 400 with __type 'com.amazonaws.swf.service.v2.model#StateMachineDoesNotExist' and message "State Machine Does Not Exist: '<stateMachineArn>'"; got 200 with {stateMachineArn, name, status 'ACTIVE', definition, roleArn, type 'STANDARD', loggingConfiguration, tracingConfiguration, encryptionConfiguration}. The poll also reports 'error code None not in [StateMachineDoesNotExist] after 7 attempt(s)'.
Cause 2: expected tags[0..2] = {key 'aws:cloudformation:stack-id', value the stack ARN}, {key 'aws:cloudformation:stack-name', value the stack name}, {key 'aws:cloudformation:logical-id', value the resource's logical id}; got None for all three (status 200 both sides).

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. Cause 1: fixture cloudformation-sfn-out-of-band-delete-drift, step teardown#15 - after the stack owning an AWS::StepFunctions::StateMachine is deleted, poll Step Functions DescribeStateMachine on the state machine ARN.
3. Cause 2: fixture cloudformation-sfn-owning-stack-reverse-lookup, step task#4 - deploy a stack declaring an AWS::StepFunctions::StateMachine, then call Step Functions ListTagsForResource on the state machine ARN.
4. Compare the error __type/message and the returned tags against the recorded AWS responses.
