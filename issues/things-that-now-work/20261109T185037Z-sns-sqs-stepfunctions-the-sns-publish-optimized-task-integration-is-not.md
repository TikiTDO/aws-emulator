Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns__sqs__stepfunctions.sns-publish-attributes-reach-subscriber-and-invalid-attribute-fails-task
Issue-Key: sns__sqs__stepfunctions.sns-publish-attributes-reach-subscriber-and-invalid-attribute-fails-task|run#12|AssertionError|assert good["status"] == "SUCCEEDED", (
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# The sns:publish optimized task integration is not implemented, so a Task state naming it runs nothing

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: a Task state with Resource 'arn:aws:states:::sns:publish' fails with States.Runtime because SNS is not among the services the emulator's task-integration layer can call.

AWS: 'arn:aws:states:::sns:publish' is an optimized service integration: the state publishes to the topic with the given Message/MessageAttributes, the subscriber receives it, and an invalid attribute fails the task with the SNS error (so a Catch can handle it). Recorded behavior: the well-formed publish SUCCEEDs.
Emulator: the execution FAILS with error 'States.Runtime' and cause "state 'Publish': 'arn:aws:states:::sns:publish' names an integration with 'sns', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it" — an emulator-authored sentence, and the publish never happens.
Why: one of the most common state-machine steps (notify on completion) cannot run, and the failure arrives as States.Runtime rather than an SNS error, so no Catch/Retry written for the real integration applies.
Fix: the Step Functions task-integration dispatcher (bind the sns:publish optimized integration, including its error mapping).
Traces: sns__sqs__stepfunctions.sns-publish-attributes-reach-subscriber-and-invalid-attribute-fails-task. Related open issue filed for a sibling check: 20261027T000406Z-sfn-sns-sqs-the-sns-publish-optimized-task-integration-is-never.md.

## Evidence
[mismatch] sns__sqs__stepfunctions.sns-publish-attributes-reach-subscriber-and-invalid-attribute-fails-task
AssertionError: the well-formed publish must succeed, got status 'FAILED': 'States.Runtime' "state 'Publish': 'arn:aws:states:::sns:publish' names an integration with 'sns', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it"

## Steps to Reproduce
1. CreateStateMachine with a Task state Resource='arn:aws:states:::sns:publish' and Parameters {TopicArn, Message, MessageAttributes}.
2. StartExecution — expect SUCCEEDED and the subscriber receiving the message; the emulator fails with States.Runtime.
