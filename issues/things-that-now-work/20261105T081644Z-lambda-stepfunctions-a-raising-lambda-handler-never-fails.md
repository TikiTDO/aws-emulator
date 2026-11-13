Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__stepfunctions.handler-exception-fails-execution-with-structured-cause lambda__stepfunctions.execution-history-attributes-failure-to-the-task-state lambda__stepfunctions.cli-derives-failed-state-and-cause-like-the-sdk lambda__stepfunctions.list-executions-status-filtered-pages-walk-only-matching-runs-of-a-mixed-machine lambda__stepfunctions.lambda-invoke-task-through-an-alias-reports-the-aliased-version-in-its-envelope lambda__stepfunctions.execution-input-array-reaches-lambda-unchanged-bare-number-rejected-at-payload
Issue-Key: lambda__stepfunctions.handler-exception-fails-execution-with-structured-cause|run#13|AssertionError|assert got["status"] == "FAILED", (
Issue-Key: lambda__stepfunctions.execution-history-attributes-failure-to-the-task-state|run#15|AssertionError|assert got["status"] == "FAILED", f"the subject execution must be FAILED, got {got['status']!r}"
Issue-Key: lambda__stepfunctions.cli-derives-failed-state-and-cause-like-the-sdk|run#13|AssertionError|assert sdk["status"] == "FAILED", f"the subject execution must be FAILED, got {sdk['status']!r}"
Issue-Key: lambda__stepfunctions.list-executions-status-filtered-pages-walk-only-matching-runs-of-a-mixed-machine|run#10|AssertionError|assert done["status"] == want, f"a run with fail={fail} must end {want}, got {done['status']!r}"
Issue-Key: lambda__stepfunctions.lambda-invoke-task-through-an-alias-reports-the-aliased-version-in-its-envelope|run#19|KeyError|header = out["SdkHttpMetadata"]["HttpHeaders"]["X-Amz-Executed-Version"]
Issue-Key: lambda__stepfunctions.execution-input-array-reaches-lambda-unchanged-bare-number-rejected-at-payload|run#14|AssertionError|assert rejected["status"] == "FAILED", (
Issue-Commit: ce1e7fa07219b3a065b602a90626dda6f8aa6c34

# lambda__stepfunctions: a raising Lambda handler never fails its Task, an alias-qualified invoke omits ExecutedVersion, and a bare-number Payload.$ is not rejected (3 causes)

> _Note: this issue was identified against the code base at commit `ce1e7fa07219b3a065b602a90626dda6f8aa6c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Three net-new root causes in the Step Functions Lambda Task integration.

Cause 1 of 3: an unhandled Lambda handler error does not fail the Task state, so the execution, its history and every status-filtered read report success.
AWS: Lambda Invoke answers HTTP 200 with the X-Amz-Function-Error header set to "Unhandled" when the handler raises (Smithy: lambda#Invoke output FunctionError -> httpHeader X-Amz-Function-Error). Step Functions fails the task: the history records LambdaFunctionFailed and TaskFailed (Smithy sfn#HistoryEventType enumerates both), with the task error set to the errorType from the function's error document (the thrown type, e.g. `Exception`, or `Lambda.Unknown` when the runtime reports none) and the cause set to that JSON error document ({"errorMessage","errorType","trace"/"stackTrace"}). With no Retry/Catch the execution ends FAILED carrying that error/cause, DescribeExecution and GetExecutionHistory attribute the failure to the Task state, the CLI derives the same failed state and cause, and ListExecutions with statusFilter=FAILED returns that run.
Emulator: the Task is reported successful with the whole Invoke envelope (`"FunctionError":"Unhandled"`, `"Payload":{"errorMessage":...,"errorType":...}`) as its result; DescribeExecution reports `status: SUCCEEDED` with no error/cause, the history carries no failure event, and a run that must be FAILED is absent from the FAILED page and present in the SUCCEEDED one.
Why: failure is unobservable at every surface a client uses — status, error/cause, history and status-filtered listing — so a failed workflow is indistinguishable from a successful one and error-handling/alerting built on any of them never triggers.
Fix: the Step Functions ASL interpreter's Task-state Lambda integration must map a non-empty FunctionError on the Invoke response to a task failure (LambdaFunctionFailed/TaskFailed with the function's errorType and error document) before Retry/Catch evaluation and before the execution's terminal status is set.
Traces: lambda__stepfunctions.handler-exception-fails-execution-with-structured-cause, .execution-history-attributes-failure-to-the-task-state, .cli-derives-failed-state-and-cause-like-the-sdk, .list-executions-status-filtered-pages-walk-only-matching-runs-of-a-mixed-machine.

Cause 2 of 3: an alias-qualified Lambda invoke returns no ExecutedVersion, so the Task's envelope cannot report the version the alias resolved to.
AWS: Invoke's response carries ExecutedVersion in the X-Amz-Executed-Version header (Smithy: lambda#Invoke output member ExecutedVersion -> httpHeader X-Amz-Executed-Version, target Version, @length 1..1024, @pattern ^(\$LATEST|[0-9]+)$). When the FunctionName/Qualifier names an alias, that value is the numeric version the alias points at — not `$LATEST` — and the Step Functions optimized-invoke envelope surfaces it as ExecutedVersion.
Emulator: the response/envelope has no ExecutedVersion at all for an alias-qualified invoke (the check's lookup of 'X-Amz-Executed-Version' raised KeyError), even though the same emulator does emit `"ExecutedVersion":"$LATEST"` for an unqualified invoke.
Why: a client that routes traffic through aliases loses the only signal telling it which published version actually ran, so canary/blue-green verification and version-pinned auditing are impossible.
Fix: the Lambda Invoke handler's response serializer must resolve the alias to its target version and always set the X-Amz-Executed-Version header; the Step Functions Lambda integration must carry it into the task result envelope.
Traces: lambda__stepfunctions.lambda-invoke-task-through-an-alias-reports-the-aliased-version-in-its-envelope.

Cause 3 of 3: a bare JSON scalar execution input is accepted and then silently produces an empty Task result instead of failing Payload.$ resolution.
AWS: StartExecution accepts any valid JSON text as input, including a bare number such as `42`. The optimized `arn:aws:states:::lambda:invoke` integration requires its Parameters to form a JSON object, so resolving `Payload.$: "$"` against a bare-scalar state input fails the Task at runtime and the execution ends FAILED with a States.* runtime error naming the state and the parameter that could not be resolved.
Emulator: the execution reports `status: SUCCEEDED` and the Task produces no output at all (the recorded result was None), so neither the acceptance at StartExecution nor the runtime rejection is distinguishable.
Why: a malformed input that AWS rejects at the first Task runs to completion here and reports success with no output, hiding a definition/input mismatch until production.
Fix: the Step Functions ASL interpreter's Parameters/Payload resolution for the optimized Lambda integration must reject a non-object payload resolution and fail the task rather than emitting an empty result.
Traces: lambda__stepfunctions.execution-input-array-reaches-lambda-unchanged-bare-number-rejected-at-payload.

## Evidence
Cause 1 — `AssertionError: a handler that raises must terminate the execution FAILED, got 'SUCCEEDED' — the LambdaInvoke task declares no Retry, so nothing can mask it`; `AssertionError: the subject execution must be FAILED, got 'SUCCEEDED'` (history and CLI checks); `AssertionError: a run with fail=True must end FAILED, got 'SUCCEEDED'` (status-filtered ListExecutions).

Cause 2 — `KeyError: 'X-Amz-Executed-Version'`: the alias-qualified invoke response carries no ExecutedVersion, where the model binds it to that header with pattern ^(\$LATEST|[0-9]+)$.

Cause 3 — `AssertionError: a bare JSON number input (42) is accepted by StartExecution but lambda:invoke's Payload.$ resolution of it must fail the Task; got 'SUCCEEDED' (None)`.

## Steps to Reproduce
Cause 1: Create a state machine with a single Task (arn:aws:states:::lambda:invoke, no Retry/Catch) invoking a handler that raises; StartExecution, then DescribeExecution (SUCCEEDED, no error/cause), GetExecutionHistory (no LambdaFunctionFailed/TaskFailed) and ListExecutions statusFilter=FAILED (run absent).
Cause 2: PublishVersion and CreateAlias pointing at that version, Invoke with the alias as FunctionName (or run a Task whose Resource names it), and read the response's X-Amz-Executed-Version / the envelope's ExecutedVersion — absent.
Cause 3: StartExecution with input `42` on a machine whose Task sets Parameters {"Payload.$": "$", "FunctionName": ...}; observe SUCCEEDED with no task output.
