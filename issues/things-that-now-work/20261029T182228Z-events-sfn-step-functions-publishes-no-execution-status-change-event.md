Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__stepfunctions.execution-status-change-event-reaches-the-default-bus
Issue-Key: events__stepfunctions.execution-status-change-event-reaches-the-default-bus|_receive#24|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=wait)
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# Step Functions publishes no Execution Status Change event to the default EventBridge bus

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: an execution reaching a terminal status emits nothing on the default bus.
AWS: Step Functions publishes on the default bus source "aws.states", detail-type "Step Functions Execution Status Change", with detail carrying executionArn, stateMachineArn, name, status (RUNNING/SUCCEEDED/FAILED/TIMED_OUT/ABORTED), startDate, stopDate, input and output, and the execution ARN in the envelope's resources. recorded behavior only for the exact detail body.
Emulator: a rule on {"source":["aws.states"]} receives nothing after StartExecution runs to a terminal status; the check ends "Check timed out after 60.0s".
Why: any orchestration that reacts to a workflow finishing (chain the next stage, alarm on FAILED, record the output) never fires, and a client cannot distinguish a workflow that never ran from one whose event was never published.
Fix: the Step Functions execution state machine must publish the Execution Status Change event to the EventBridge bus on each status transition.
Traces: events__sfn.execution-status-change-event-reaches-the-default-bus

## Evidence
"Error: Check timed out after 60.0s" — the SQS target behind a rule on {"source":["aws.states"],"detail-type":["Step Functions Execution Status Change"]} stays empty after an execution reaches a terminal status. Expected: at least one envelope whose detail.executionArn is the started execution and whose detail.status is its terminal status.

## Steps to Reproduce
PutRule on {"source":["aws.states"]} with an SQS target and a permissive queue policy, CreateStateMachine and StartExecution, wait for DescribeExecution to report a terminal status, then poll the queue. Nothing arrives.
