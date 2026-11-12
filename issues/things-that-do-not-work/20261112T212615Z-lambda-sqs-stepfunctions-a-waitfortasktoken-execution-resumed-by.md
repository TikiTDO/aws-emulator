Issue-Severity: MEDIUM
Issue-Commit: 4a01df4b0c4b5b1a439cb005763682e98a02301b

# lambda__sqs__stepfunctions: a waitForTaskToken execution resumed by a heartbeating worker reports a 0.5 s startDate/stopDate window although the task waited ~17 s for its token (1 cause)

> _Note: this issue was identified against the code base at commit `4a01df4b0c4b5b1a439cb005763682e98a02301b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExecution

## Description
One net-new root cause in the lambda__sqs__stepfunctions suite; it must be closed.

Cause 1 of 1 — the execution window DescribeExecution reports for a `.waitForTaskToken` task does not include the time the task spent suspended on its token.

AWS: Smithy (com-amazonaws-sfn-awsstepfunctions) DescribeExecutionOutput.startDate is a required timestamp and .stopDate an optional timestamp; AWS sets startDate at StartExecution and stopDate at the moment the execution reaches a terminal status, so stopDate-startDate is the execution's real wall-clock duration and is never shorter than the time a Task state stayed suspended. A Task with Resource `arn:aws:states:::lambda:invoke.waitForTaskToken` stays in that state until the worker answers SendTaskSuccess (SendTaskHeartbeat, modelled here with errors InvalidToken/TaskDoesNotExist/TaskTimedOut, only resets the HeartbeatSeconds window and does not advance the state). A worker that sleeps 5 s three times, heartbeating after each sleep, then sleeps 2 s and calls SendTaskSuccess therefore makes an execution whose reported duration is at least ~17 s.

Emulator: the execution does reach the right terminal state — status SUCCEEDED with output exactly the worker's SendTaskSuccess payload {"done": true} — but DescribeExecution reports stopDate - startDate = 0.5 s, i.e. a window far shorter than the suspension the same execution must have served to collect that output. (The sibling check cited in the related issue below reports the same 0.5 s constant, so the reported window looks independent of what actually happened.) Hypothesis — neither the model nor the recording states the mechanism: either the token wait is not clocked into the execution's reported window (startDate stamped late, or stopDate derived from a canned duration rather than the terminal transition), or the Lambda runtime serving the worker does not honour real sleeps, so the callback arrives immediately.

Why: startDate/stopDate are what clients use to measure, bill, alarm on and time out long-running callback workflows. A duration that is a constant unrelated to the real execution makes every duration-derived decision wrong: a 17 s callback task looks like a 0.5 s one, SLA/duration monitors never fire, and the HeartbeatSeconds liveness window the task declares cannot be reasoned about from the execution record.

Fix: the Step Functions execution state/DescribeExecution serializer — stamp startDate when StartExecution accepts the execution and stopDate at the terminal transition, counting the whole `.waitForTaskToken` suspension; and, if the suspension really is not served, the task-token wait in the Task-state driver that resumes it.

Related open issue (does NOT cover this): 20261109T185037Z-lambda-sqs-stepfunctions-4-net-new-causes-lambda-event-invocation-task.md lists this same check under its Cause 4, but describes it as the execution FAILING with States.Timeout despite the heartbeats. The execution now reaches SUCCEEDED with the worker's output and fails instead on the reported execution window, which that issue does not describe.

Traces: lambda__sqs__stepfunctions.sfn-task-token-sendtaskheartbeat-keeps-execution-alive.

## Evidence
Check lambda__sqs__stepfunctions.sfn-task-token-sendtaskheartbeat-keeps-execution-alive [mismatch]:
  AssertionError: execution completed too fast to have used heartbeats: 0.5s
  at: assert elapsed >= 15, f"execution completed too fast to have used heartbeats: {elapsed}s"
  where elapsed = (desc["stopDate"] - desc["startDate"]).total_seconds() from DescribeExecution.

The earlier assertions in the same check passed, so the emulator answered:
  status == "SUCCEEDED"
  output == {"done": true}   (the worker's SendTaskSuccess payload)
Expected: stopDate - startDate >= the worker's own runtime (3 x 5 s sleeps + heartbeats + 2 s, i.e. ~17 s), hence the check's >= 15 s floor.
Actual: stopDate - startDate = 0.5 s.

## Steps to Reproduce
1. Create an IAM role for Lambda granting states:SendTaskHeartbeat and states:SendTaskSuccess, and a role for states.amazonaws.com granting lambda:InvokeFunction.
2. Create a python3.12 function (Timeout=60) whose handler reads event['token'], loops three times doing time.sleep(5) then stepfunctions.send_task_heartbeat(taskToken=token), then time.sleep(2) and stepfunctions.send_task_success(taskToken=token, output='{"done": true}').
3. CreateStateMachine (STANDARD) with one Task state: Resource 'arn:aws:states:::lambda:invoke.waitForTaskToken', Parameters {FunctionName: <fn arn>, Payload: {"token.$": "$$.Task.Token"}}, HeartbeatSeconds 8, End true.
4. StartExecution and poll DescribeExecution until terminal.
5. Compute stopDate - startDate. Emulator: 0.5 s. AWS: >= ~17 s, the time the task stayed suspended on its token.
