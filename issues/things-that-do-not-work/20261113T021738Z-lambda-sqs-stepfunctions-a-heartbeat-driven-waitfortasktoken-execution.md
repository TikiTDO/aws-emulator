Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.sfn-task-token-sendtaskheartbeat-keeps-execution-alive
Issue-Key: lambda__sqs__stepfunctions.sfn-task-token-sendtaskheartbeat-keeps-execution-alive|run#13|AssertionError|assert elapsed >= 15, f"execution completed too fast to have used heartbeats: {elapsed}s"
Issue-Commit: f3c33bab928599ae9956c8a8be7ee7afed317dab

# lambda__sqs__stepfunctions: a heartbeat-driven .waitForTaskToken execution is reported as lasting 0.5 s, so DescribeExecution's startDate/stopDate do not bracket the real callback wait

> _Note: this issue was identified against the code base at commit `f3c33bab928599ae9956c8a8be7ee7afed317dab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExecution

## Description
Cause 1 of 1: a STANDARD state machine with one `arn:aws:states:::lambda:invoke.waitForTaskToken` Task (HeartbeatSeconds=8) whose worker sleeps 5 s three times, calls SendTaskHeartbeat after each sleep, sleeps 2 s more and then calls SendTaskSuccess with output {"done": true}, is reported by DescribeExecution as having run for 0.5 s.

AWS: Smithy (Step Functions) DescribeExecution output carries `startDate` and `stopDate`, both `timestamp` members — startDate is when the execution started and stopDate when it stopped, so for a callback task the span between them includes the whole time the task waited for its token. The worker here cannot answer SendTaskSuccess before ~17 s of sleeps have elapsed, so stopDate - startDate must be at least one HeartbeatSeconds window (the check requires >= 15 s), and the execution's own status/output are the ones the callback supplied.

Emulator: status is SUCCEEDED and output is exactly {"done": true} (so the SendTaskSuccess callback WAS applied and the earlier always-States.Timeout behavior is gone), but `(stopDate - startDate).total_seconds()` is 0.5 — AssertionError: execution completed too fast to have used heartbeats: 0.5s. Hypothesis (neither the model nor the recording states which): either the execution's startDate/stopDate are not real wall-clock marks of start and stop (e.g. both stamped when the execution settled), or the invoked worker's sleeps are not honored so the callback arrives immediately and no heartbeat window is ever crossed. Either way the reported duration cannot be the real one.

Why: a client that reads startDate/stopDate to measure or bill an execution, to age it out, or to prove a long-running callback really survived its heartbeat window, is told every execution is instantaneous. The liveness semantics HeartbeatSeconds exists for become unobservable: a worker that heartbeats and one that returns instantly look identical.

Fix: the Step Functions execution state / DescribeExecution serializer must record startDate at StartExecution and stopDate when the execution reaches a terminal status, so the span covers the callback wait; and the task-token wait must not release the task before its SendTaskSuccess actually arrives.

Related: open issue 20261109T185037Z-lambda-sqs-stepfunctions-4-net-new-causes-lambda-event-invocation-task.md lists this check under its Cause 4 ('SendTaskHeartbeat does not keep a task alive'), keyed on the earlier failure `expected SUCCEEDED, got FAILED: States.Timeout`. That symptom is gone — the execution now SUCCEEDS with the callback's output — and the check now fails on the later duration assertion, which that issue's key does not carry.

Traces: lambda__sqs__stepfunctions.sfn-task-token-sendtaskheartbeat-keeps-execution-alive.

## Evidence
Verdict [mismatch]:
  File "check_lambda__sqs__stepfunctions_sfn_task_token_sendtaskheartbeat_keeps_execution_alive.py", line 93, in run
    assert elapsed >= 15, f"execution completed too fast to have used heartbeats: {elapsed}s"
  AssertionError: execution completed too fast to have used heartbeats: 0.5s

Expected: DescribeExecution returns status SUCCEEDED, output {"done": true}, and stopDate - startDate >= 15 s (the worker sleeps 3x5 s with a SendTaskHeartbeat after each, then 2 s, before SendTaskSuccess; HeartbeatSeconds=8).
Actual: status SUCCEEDED and output {"done": true} (both correct), but stopDate - startDate == 0.5 s.

## Steps to Reproduce
1. Create an IAM role for lambda.amazonaws.com allowing states:SendTaskHeartbeat and states:SendTaskSuccess, and a role for states.amazonaws.com allowing lambda:InvokeFunction.
2. CreateFunction (python3.12, Timeout=60) whose handler loops 3 times: sleep 5 s then send_task_heartbeat(taskToken=event['token']); then sleep 2 s and send_task_success(taskToken=..., output='{"done": true}').
3. CreateStateMachine (type STANDARD) with one Task: Resource arn:aws:states:::lambda:invoke.waitForTaskToken, Parameters {FunctionName: <fn arn>, Payload: {"token.$": "$$.Task.Token"}}, HeartbeatSeconds 8, End true.
4. StartExecution with input {}, poll DescribeExecution to a terminal status.
5. Compute stopDate - startDate: it is ~0.5 s instead of >= 15 s.
