Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.python312-and-python313-handlers-run-asyncio-zoneinfo-and-sqlite3 lambda.callback-timeout-exceeded-rejected lambda.alias-weight-ladder-observed-distribution
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.python312-and-python313-handlers-run-asyncio-zoneinfo-and-sqlite3|_invoke_sync#5|AssertionError|raise AssertionError(
Issue-Key: lambda.callback-timeout-exceeded-rejected|run#7|AssertionError|raise _never_ready(
Issue-Key: lambda.alias-weight-ladder-observed-distribution|_invoke_once#16|Timeout|return (executed, marker, None)
Issue-Commit: 53bfbc08a5ec2983a62510ba6bf3546a998019d9

# lambda: 3 net-new causes — the python3.12/3.13 runtime has no _signal so asyncio cannot be imported, a durable execution never reaches its wait_for_callback checkpoint, and a weighted-alias ladder never completes

> _Note: this issue was identified against the code base at commit `53bfbc08a5ec2983a62510ba6bf3546a998019d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Three net-new root causes in the Lambda service, all grounded in failing checks of this suite.

Cause 1 of 3: the managed Python runtime (python3.12 and python3.13) ships a crippled stdlib — the _signal extension module is missing, so `import asyncio` fails and the handler module cannot be imported at all.
AWS: recorded behavior only for the runtime's contents. A Lambda python3.12/python3.13 managed runtime is CPython with its complete standard library, so a handler that does `import asyncio`, `import sqlite3` and `from zoneinfo import ZoneInfo` imports cleanly and Invoke answers 200 with no FunctionError.
Emulator: Invoke answers with FunctionError 'Unhandled' and payload {"errorMessage":"Unable to import module 'index': No module named '_signal'","errorType":"Runtime.ImportModuleError","requestId":"","stackTrace":[]}. (Note the requestId is also empty, where AWS fills the invocation's request id.) The fixed text 'Unable to import module '<module>': No module named '<name>'' with errorType 'Runtime.ImportModuleError' is AWS's wording for a genuinely missing module; here it is reported for a stdlib module AWS always provides.
Why: any handler using asyncio — i.e. most modern Python handlers, and anything that transitively imports it — cannot be invoked at all, and the failure is reported as the caller's import error rather than as a missing runtime.
Fix: the Lambda Python managed-runtime image/interpreter the invoke path starts — provide the full CPython standard library (including the _signal, _asyncio and related extension modules) for every supported python3.x identifier.
Traces: lambda.python312-and-python313-handlers-run-asyncio-zoneinfo-and-sqlite3.
Related open issue 20261111T081112Z-lambda-managed-runtimes-are-stub-interpreters-10-node.md covers the same runtime family but does not carry this check's replay key.

Cause 2 of 3: a durable execution started on a @durable_execution handler that calls context.wait_for_callback never records a CallbackStartedDetails history event, so no callback is ever addressable.
AWS: recorded behavior only. Starting a durable execution whose handler calls context.wait_for_callback(..., config=WaitForCallbackConfig(timeout=Duration.from_seconds(N))) moves the execution to the wait_for_callback checkpoint and appends a history event carrying CallbackStartedDetails (with the callback id), after which SendCallbackSuccess/SendCallbackHeartbeat/SendCallbackFailure inside the window are accepted and the same calls after the timeout are refused CallbackTimeoutException.
Emulator: the execution is accepted (an execution ARN of the form arn:aws:lambda:<region>:<account>:function:<name>:$LATEST/durable-execution/<name>/<id> is returned) but no CallbackStartedDetails history event ever appears within the poll window, so the positive control never becomes ready and neither the in-window acceptance nor the CallbackTimeoutException refusal can be reached.
Why: a client relying on AWS's behavior cannot use the callback pattern at all — there is no callback id to signal, so every wait_for_callback execution hangs until its execution timeout, and the documented CallbackTimeoutException refusal is unobservable.
Fix: the durable-execution engine's checkpoint/history recording for wait_for_callback — run the handler to its wait_for_callback checkpoint, mint the callback, and append the CallbackStartedDetails history event before the callback window opens.
Traces: lambda.callback-timeout-exceeded-rejected.
Related open issues 20261027T203721Z-lambda-a-durable-execution-is-accepted-but-never-runs.md and 20261107T135301Z-lambda-9-net-new-root-causes-in-durable.md describe this family but do not carry this check's replay key.

Cause 3 of 3: the weighted-alias ladder never completes — four freshly created aliases with RoutingConfig weights 0.10/0.30/0.50/1.00, each sampled with 200 invokes at concurrency 8, do not finish inside the check's budget.
AWS: recorded behavior only for timing. A brand-new alias with AdditionalVersionWeights is consistent on its first sample (recorded in the check: p=0.10 -> 12/200, p=0.50 -> 98/200, p=1.00 -> 200/200, p=0.30 -> 47/200, each within ~1.2 s of CreateAlias), and 200 sequentially-dispatched invokes at concurrency 8 complete well inside a one-minute budget.
Emulator: the check is aborted with 'Check timed out after 60.0s' — the alias-ready wait plus the invoke sample never converge, so the per-rung distribution and the ordering across rungs are never measurable.
Why: a client relying on AWS's behavior gets no weighted-routing behaviour it can observe, and callers that drive traffic through a weighted alias see the invoke path stall rather than split.
Fix: the alias invoke-dispatch path and the alias readiness/Invoke latency it depends on — resolve AdditionalVersionWeights per alias on the invoke path and serve concurrent invokes without the per-call stall that exhausts the budget.
Traces: lambda.alias-weight-ladder-observed-distribution.
Related open issue 20261101T210635Z-lambda-weighted-alias-traffic-is-never-observable-across.md describes weighted-alias traffic but does not carry this check's replay key.

## Evidence
lambda.python312-and-python313-handlers-run-asyncio-zoneinfo-and-sqlite3 [mismatch]:
AssertionError: Function <name> returned FunctionError=Unhandled: {"errorMessage":"Unable to import module 'index': No module named '_signal'","errorType":"Runtime.ImportModuleError","requestId":"","stackTrace":[]}
Expected: 200 with no FunctionError; the handler imports asyncio, sqlite3 and zoneinfo and returns its results.

lambda.callback-timeout-exceeded-rejected [mismatch]:
AssertionError: positive-control durable execution arn:aws:lambda:<region>:<account>:function:<fn>:$LATEST/durable-execution/<exec-name>/<id> never reached the wait_for_callback checkpoint (no CallbackStartedDetails history event appeared within the poll window)
Expected: a CallbackStartedDetails history event, then SendCallbackSuccess/Heartbeat/Failure accepted in-window and refused CallbackTimeoutException after the 8 s callback timeout.

lambda.alias-weight-ladder-observed-distribution [mismatch]:
Error: Check timed out after 60.0s
Expected: four aliases created with AdditionalVersionWeights 0.10/0.30/0.50/1.00, each sampled 200 invokes at concurrency 8, finishing inside the budget with the measured counts inside their binomial bands and strictly ordered across rungs.

## Steps to Reproduce
Cause 1: CreateFunction with Runtime 'python3.12' (and again 'python3.13') and a handler module whose first lines are `import asyncio`, `import sqlite3`, `from zoneinfo import ZoneInfo`; Invoke it. The invoke answers FunctionError 'Unhandled' with errorType 'Runtime.ImportModuleError' and errorMessage "Unable to import module 'index': No module named '_signal'".

Cause 2: CreateFunction with a @durable_execution handler that calls context.wait_for_callback(..., WaitForCallbackConfig(timeout=Duration.from_seconds(8))); start a durable execution and poll its history. No CallbackStartedDetails event is ever appended, so no callback id exists to signal and CallbackTimeoutException can never be observed.

Cause 3: Publish a version, then CreateAlias four times with RoutingConfig AdditionalVersionWeights of 0.10, 0.30, 0.50 and 1.00 on the same function; invoke each alias 200 times with 8 concurrent invokers and tally the marker returned. The run does not complete within 60 s.
