Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes__sqs__stepfunctions.pipes-sqs-source-starts-sfn-target-execution
Issue-Key: pipes__sqs__stepfunctions.pipes-sqs-source-starts-sfn-target-execution|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Commit: c43f13612f17253679972bb58b777ab0e011820f

# pipes__sqs__stepfunctions: a RUNNING pipe with an SQS source and a Step Functions state-machine target starts no execution, so the delivered source-record payload is never observable

> _Note: this issue was identified against the code base at commit `c43f13612f17253679972bb58b777ab0e011820f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
One net-new root cause, grounded in a failing behavioral check the replay report left unmarked (no ALREADY-FILED marker, no OWNED marker).

Cause 1 of 1 — a pipe whose Source is an SQS queue and whose Target is a Step Functions STANDARD state machine never starts an execution of that machine, so nothing the pipe is supposed to hand the target can be asserted.
AWS: recorded behavior only for the delivery itself (the Pipes Smithy model describes CreatePipe's Source/Target/SourceParameters/TargetParameters surface and the pipe's CurrentState, not the poll-and-deliver semantics). The recording shows: CreatePipe is accepted with SourceParameters.SqsQueueParameters.BatchSize=1 and TargetParameters.StepFunctionStateMachineParameters.InvocationType=FIRE_AND_FORGET (FIRE_AND_FORGET is mandatory for a STANDARD target; REQUEST_RESPONSE is a create-time ValidationException and is only valid for EXPRESS), the pipe reaches CurrentState RUNNING, and a message sent to the source queue afterwards causes exactly one sfn:StartExecution on the target machine. That execution's input is a JSON ARRAY of source records even at BatchSize 1, each record carrying the SQS record fields — messageId equal to the SendMessage response's MessageId, eventSourceARN equal to the source queue's ARN, and the body — and not a re-wrapped Pipes envelope. Because the message is durable on the queue, no re-trigger is needed: the pipe's first poll after RUNNING picks it up.
Emulator: the check is killed with "Error: Check timed out after 60.0s" — no execution of the target state machine ever appears for the poller to find (the check's own first-execution budget is 300 s at a 10 s interval, and the downstream assertions on record count, messageId, eventSourceARN and body are never reached). A pipe reported RUNNING performs no delivery to a Step Functions target.
Why: the whole point of a pipe is that the target runs. A client that wires SQS -> Pipes -> Step Functions sees a pipe the control plane reports as RUNNING and a queue whose messages are never consumed, with no error anywhere to explain it; any workflow triggered this way silently never runs, and the input contract (a batch array of raw source records) can never be relied on either.
Fix: the Pipes delivery path for a Step Functions state-machine target — the source poller must consume the SQS batch for a RUNNING pipe and the target dispatcher must call StartExecution with the batch of source records as the execution input, honouring StepFunctionStateMachineParameters.InvocationType.
Traces: pipes__sqs__stepfunctions.pipes-sqs-source-starts-sfn-target-execution. Named, not covering: the open issues 20261107T205659Z-pipes-sqs-stepfunctions-a-running-pipe-with-an.md and 20261107T233020Z-pipes-sqs-stepfunctions-a-pipe-with-an-sqs.md describe a similar non-delivery but do not carry this check's replay key, and 20261109T174004Z-pipes-sqs-stepfunctions-a-standard-state-machine-used.md concerns a STANDARD machine used as Enrichment and its execution log records, not a state-machine Target.

## Evidence
pipes__sqs__stepfunctions.pipes-sqs-source-starts-sfn-target-execution — verdict mismatch.
Setup accepted by the emulator: sqs:CreateQueue; sfn:CreateStateMachine with {"StartAt":"Echo","States":{"Echo":{"Type":"Pass","End":true}}} (a Pass state, so no target permissions are involved and its output echoes its input); iam:CreateRole trusting pipes.amazonaws.com with sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the source queue ARN and states:StartExecution on the machine ARN; pipes:CreatePipe with SourceParameters={"SqsQueueParameters":{"BatchSize":1}} and TargetParameters={"StepFunctionStateMachineParameters":{"InvocationType":"FIRE_AND_FORGET"}}, reaching RUNNING.
Expected: after sqs:SendMessage to the source queue, sfn:ListExecutions on the target machine shows exactly one execution; it reaches SUCCEEDED, and json.loads(DescribeExecution.input) is a list of exactly one record whose messageId equals the SendMessage response's MessageId and whose eventSourceARN equals the source queue's ARN.
Actual: "Error: Check timed out after 60.0s" — no execution of the target state machine existed to describe. Nothing in the pipe's reported state indicated a failure to deliver.

## Steps to Reproduce
1. sqs:CreateQueue -> source queue URL + ARN.
2. sfn:CreateStateMachine(type=STANDARD, definition={"StartAt":"Echo","States":{"Echo":{"Type":"Pass","End":true}}}, roleArn=<any role trusting states.amazonaws.com>).
3. iam:CreateRole trusting pipes.amazonaws.com with an inline policy allowing sqs:ReceiveMessage, sqs:DeleteMessage, sqs:GetQueueAttributes on the queue ARN and states:StartExecution on the state machine ARN.
4. pipes:CreatePipe(Source=<queue ARN>, Target=<state machine ARN>, RoleArn=<pipe role>, SourceParameters={"SqsQueueParameters":{"BatchSize":1}}, TargetParameters={"StepFunctionStateMachineParameters":{"InvocationType":"FIRE_AND_FORGET"}}); poll pipes:DescribePipe until CurrentState == RUNNING.
5. sqs:SendMessage(QueueUrl=<source>, MessageBody='{"marker":"<unique>"}').
6. Poll sfn:ListExecutions on the target machine — expected exactly one execution. Observed: none; the check is cut off at the 60 s harness cap.
7. (unreachable today) DescribeExecution that execution — status SUCCEEDED, and input is a one-element JSON array whose record carries messageId == the SendMessage MessageId, eventSourceARN == the source queue ARN, and the sent body.
