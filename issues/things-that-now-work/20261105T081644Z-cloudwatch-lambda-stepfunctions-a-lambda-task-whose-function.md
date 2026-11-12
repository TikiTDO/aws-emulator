Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-invoke-denied-role-fails-execution-function-series-empty
Issue-Key: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series|run#13|AssertionError|assert bad["status"] == "FAILED", f"boom execution ended {bad['status']!r}"
Issue-Key: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-invoke-denied-role-fails-execution-function-series-empty|_poll#24|Timeout|time.sleep(_POLL_S)
Issue-Commit: ce1e7fa07219b3a065b602a90626dda6f8aa6c34

# cloudwatch__lambda__stepfunctions: a Lambda Task whose function raises still SUCCEEDS, and an execution role with no lambda:InvokeFunction grant is never denied (2 causes)

> _Note: this issue was identified against the code base at commit `ce1e7fa07219b3a065b602a90626dda6f8aa6c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Two net-new root causes, both in the Step Functions Task-state integration for Lambda. Neither is covered by the fixed issue 20261029T182452Z-cloudwatch-lambda-sfn-step-functions-lambda-task-invocations-never-populate-the.md (that one was about the AWS/Lambda metric series; these two are about execution outcome).

Cause 1 of 2: an unhandled Lambda handler error does not fail the Task state.
AWS: Lambda Invoke answers HTTP 200 and sets the X-Amz-Function-Error header to "Unhandled" when the handler raises (Smithy: lambda#Invoke output member FunctionError -> httpHeader X-Amz-Function-Error; ExecutedVersion -> X-Amz-Executed-Version). Step Functions inspects that header and FAILS the task: the history records LambdaFunctionFailed (Smithy sfn#HistoryEventType enumerates LambdaFunctionFailed, LambdaFunctionScheduled, LambdaFunctionStarted, LambdaFunctionSucceeded, LambdaFunctionTimedOut) followed by ExecutionFailed, the task error is the errorType from the function's error document (the thrown type, e.g. `Exception`, or `Lambda.Unknown` when the runtime reports none) and the cause is that JSON error document verbatim. With no Catch on the state the execution ends FAILED with that error/cause. The fixed text is the error-document shape; the variable parts are the errorType, errorMessage and stackTrace the function produced.
Emulator: the task is treated as successful. The state's result is the raw Invoke envelope including `"FunctionError":"Unhandled"` and `"Payload":{"errorMessage":...,"errorType":...}`, and DescribeExecution reports `status: SUCCEEDED` with no `error`/`cause`. The check's deliberately-failing ("boom") execution ended 'SUCCEEDED'.
Why: every failure path built on Lambda is unreachable — Retry, Catch, compensation branches, ExecutionsFailed metrics and alarms all stay silent, and a client that polls DescribeExecution for a terminal status is told a failed workflow succeeded.
Fix: the Step Functions ASL interpreter's Task-state Lambda integration (optimized `arn:aws:states:::lambda:invoke` and raw-function-ARN resources) must map a non-empty FunctionError on the Invoke response to a task failure and emit LambdaFunctionFailed/TaskFailed instead of a success result.
Traces: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series.

Cause 2 of 2: an execution role that is denied lambda:InvokeFunction is never refused on the data plane, so the denied arm never reaches a terminal failure.
AWS: StartExecution is accepted, then Step Functions assumes the state machine's roleArn and calls lambda:Invoke under it. When that role is not permitted lambda:InvokeFunction on the target function the Task fails and the execution ends FAILED; the task error names the authorization failure and the cause names the assumed-role principal, the action and the function ARN. Hypothesis (neither the Smithy model nor the recording states the exact token): the error is `Lambda.AccessDeniedException`. What the recording does establish is that the execution must reach a terminal FAILED status rather than SUCCEEDED, and that no AWS/Lambda data point is produced for the function.
Emulator: the execution with the denied role is never failed, so the check polling for the denied arm's terminal state exhausted the whole 60 s budget ("Check timed out after 60.0s") — the emulator performs no authorization of the execution role before making the integration call.
Why: negative IAM tests are unverifiable and a least-privilege regression is invisible: a workflow that real AWS refuses runs to completion here, with its side effects applied.
Fix: the Step Functions execution-start/Task-dispatch path must resolve and authorize the state machine's execution role (sts:AssumeRole on the trust policy, then the integration's action/resource against the role's policies) and fail the task when the call is not permitted.
Traces: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-invoke-denied-role-fails-execution-function-series-empty.

## Evidence
Cause 1 — expected: the "boom" execution (handler raises) ends FAILED with the thrown errorType as error and the JSON error document as cause. Actual: `AssertionError: boom execution ended 'SUCCEEDED'`. A sibling recording of the same emulator shows the success output it produced instead: `{"StatusCode":200,"ExecutedVersion":"$LATEST","FunctionError":"Unhandled","Payload":{"errorMessage":"intentional-failure","errorType":"Exception",...}}` carried as the state result.

Cause 2 — expected: with the execution role denied lambda:InvokeFunction, the execution reaches FAILED and the function's AWS/Lambda series stays empty. Actual: `Error: Check timed out after 60.0s` — the execution never reaches a terminal failure because the role is never authorized.

## Steps to Reproduce
1. Create an IAM role trusted by states.amazonaws.com, one arm with lambda:InvokeFunction allowed and one arm without it.
2. Create a Python Lambda whose handler raises an exception.
3. Create a STANDARD state machine with a single Task state whose Resource is arn:aws:states:::lambda:invoke and whose Parameters name that function; no Retry and no Catch.
4. StartExecution and poll DescribeExecution: observe status SUCCEEDED with the Invoke envelope (FunctionError "Unhandled") as output, instead of FAILED with the function's errorType/error document.
5. Repeat with the role that lacks lambda:InvokeFunction: observe the execution still reaches a non-failed state, so a poll for a terminal FAILED status never completes.
