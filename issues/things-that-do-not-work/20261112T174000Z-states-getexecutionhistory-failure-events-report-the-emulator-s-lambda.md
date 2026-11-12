Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-failing-lambda-task-execution-diagnosis
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-failing-lambda-task-execution-diagnosis sfn-legacy-lambda-arn-task-failure-events
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].executionFailedEventDetails.error
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].taskFailedEventDetails.error
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].lambdaFunctionFailedEventDetails.cause
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].lambdaFunctionFailedEventDetails.error
Issue-Related: 20261109T234059Z-states-execution-history-failure-events-carry-the-wrong-cause.md
Issue-Related: 20261109T234059Z-states-getexecutionhistory-omits-the-task-lambdafunction-started-event-shifting.md
Issue-Commit: 342b40951f6ce3f2662763d4a3c08d3bb913fe47

# GetExecutionHistory failure events report the emulator's Lambda IAM denial instead of the function's error, and mislink previousEventId

> _Note: this issue was identified against the code base at commit `342b40951f6ce3f2662763d4a3c08d3bb913fe47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExecutionHistory

## Description
Every history read of an execution whose Lambda Task failed carries the WRONG failure identity on its failure events: the `error` member of TaskFailed / LambdaFunctionFailed / ExecutionFailed is the emulator's own Step Functions-side error name `Lambda.AWSLambdaException`, and the `cause` member is an IAM authorization sentence the emulator composes ("User: <role arn> is not authorized to perform: lambda:InvokeFunction ...") rather than the failure the invoked function actually produced. The HTTP status (200), the event types and the event ordering are right; the failure identity recorded on the events is not. One read additionally mislinks an event's `previousEventId`. 4 causes, all unowned by any open issue.

Cause 1 of 4: TaskFailed and ExecutionFailed on an optimized lambda:invoke Task carry the emulator's error name and an IAM-denial cause.
AWS: Smithy: GetExecutionHistory.output.events -> HistoryEventList of HistoryEvent; HistoryEvent.taskFailedEventDetails -> TaskFailedEventDetails{resourceType, resource, error -> SensitiveError @length 0..256, cause -> SensitiveCause @length 0..32768}; HistoryEvent.executionFailedEventDetails -> ExecutionFailedEventDetails{error, cause}. When an optimized lambda:invoke Task's function raises an unhandled error, the error name recorded on BOTH the TaskFailed event and the terminal ExecutionFailed event is the function's own unhandled-error type (the recording shows the Python exception class `RuntimeError`), and the cause is the function's own failure payload (the corpus asserts its shape, not a literal value). AWS records a Lambda service error name such as Lambda.AWSLambdaException only when the Lambda API call itself was refused; a function that ran and raised is not that class.
Emulator: both events report error `Lambda.AWSLambdaException` and cause `User: <sfn role arn> is not authorized to perform: lambda:InvokeFunction on resource: <function arn> because no identity-based policy allows the lambda:InvokeFunction action (Service: AWSLambda; Status Code: 400; Error Code: AccessDeniedException; Proxy: null)` — a denial no part of the recorded run produced. In that string the role ARN and the function ARN are values taken from the request/state; the rest is fixed text the emulator authors for a failure class that did not occur.
Why: Retry/Catch `ErrorEquals` on the function's error name is the documented way to branch on a function failure, and it never matches, so error-handling definitions behave differently against the emulator than against AWS. An operator reading the history is pointed at a non-existent IAM problem instead of the function's exception, and loses the function's failure payload entirely.
Fix: the lambda:invoke Task integration's failure mapping in the states execution interpreter — the component that derives error/cause for the TaskFailed and ExecutionFailed history events must carry the invoked function's error type and payload through instead of substituting a Lambda-API authorization refusal.
Traces: sfn-failing-lambda-task-execution-diagnosis#task#10 (cluster also covers #task#12)

Cause 2 of 4: the same wrong error/cause is served on the tail/reverse read of the same history, so no read path returns AWS's failure identity.
AWS: as cause 1 — the stored event is read back identically whichever page, order or filter the client asks for; executionFailedEventDetails.error is the function's error type.
Emulator: on this read the terminal failure lands at $.events[0] and still reports error `Lambda.AWSLambdaException` with the IAM-denial cause.
Why: a client cannot work around the defect by re-reading the history a different way; the failure identity is wrong at the source.
Fix: same cause builder as cause 1 — it is the recorded event, not the read/paging path, that is wrong. ($.nextToken on this step is also non-opaque ('4'); that defect is owned by the open 20261109T234059Z-states-read-responses-echo-internal-values-non-opaque.md and is not asserted here.)
Traces: sfn-failing-lambda-task-execution-diagnosis#task#11

Cause 3 of 4: after a redrive, the re-read history repeats the wrong error/cause AND links an event to the wrong previous event.
AWS: Smithy: HistoryEvent.id is "the id of the event. Events are numbered sequentially, starting at one" and HistoryEvent.previousEventId is "the id of the previous event" — the id of the event that actually precedes it in the causal chain. On the recorded redriven execution the event at this position links back to event id 2; its error/cause are the function's as in cause 1.
Emulator: the same `Lambda.AWSLambdaException` / IAM-denial pair on the ExecutionFailed and TaskFailed events, and previousEventId 7 where the recording links to 2 — the event is chained to the wrong predecessor.
Why: previousEventId is how a client (and the console) reconstructs the causal chain of a redriven execution; a wrong link makes the reconstructed history attribute the failure to the wrong step, and tooling that walks the chain from the terminal event reaches the wrong state-entered event. Related open issue 20261109T234059Z-states-getexecutionhistory-omits-the-task-lambdafunction-started-event-shifting.md covers a missing Started event shifting later ids; it does not carry this cluster's replay key and the replay reports this cluster unowned.
Fix: the history recorder's previousEventId assignment on the redrive path (the event-chaining in the execution's history store), plus the cause-1 failure mapping.
Traces: sfn-failing-lambda-task-execution-diagnosis#task#19

Cause 4 of 4: a legacy direct-ARN Lambda Task records the same wrong identity on LambdaFunctionFailed.
AWS: Smithy: HistoryEvent.lambdaFunctionFailedEventDetails -> LambdaFunctionFailedEventDetails{error -> SensitiveError, cause -> SensitiveCause}, "Contains details about a Lambda function that failed during an execution". For a Task whose Resource is a bare function ARN, AWS records the function's own unhandled-error type (`RuntimeError` in the recording) on the LambdaFunctionFailed event and on the terminal ExecutionFailed event, with the function's payload as the cause.
Emulator: both events report error `Lambda.AWSLambdaException` and the same emulator-authored IAM-denial cause sentence.
Why: the legacy integration shape is the one most older state machines use, so the same Catch/Retry breakage applies there too; neither integration style surfaces the function's failure.
Fix: same failure-mapping component, on the legacy direct-ARN Lambda task path that emits LambdaFunctionFailed.
Traces: sfn-legacy-lambda-arn-task-failure-events#task#10

Note on already-filed issues: 20261103T052008Z-states-execution-outcome-error-name-and-cause-text-diverge.md owns the same substitution as seen through DescribeExecution's execution-level error/cause, and 20261109T234059Z-states-execution-history-failure-events-carry-the-wrong-cause.md owns the state-evaluation and task-timeout cause wording on GetExecutionHistory. Neither carries the replay keys for these four clusters, which the replay reports with no owner.

## Evidence
All four causes are mismatch clusters from the replay of simparity-wire-aws-v1 against the emulator built from this checkout. Every step answered HTTP 200 as expected; only the body diverged.

Cause 1 — sfn-failing-lambda-task-execution-diagnosis#task#10, GetExecutionHistory, 200 -> 200 (cluster also covers #task#12):
  $.events[4].taskFailedEventDetails.error: expected 'RuntimeError', got 'Lambda.AWSLambdaException'
  $.events[4].taskFailedEventDetails.cause: expected a shaped (not literally asserted) value, got 'User: arn:aws:iam::<account>:role/<sfn role> is not authorized to perform: lambda:InvokeFunction on resource: arn:aws:lambda:<region>:<account>:function:<function> because no identity-based policy allows the lambda:InvokeFunction action (Service: AWSLambda; Status Code: 400; Error Code: AccessDeniedException; Proxy: null)'
  $.events[5].executionFailedEventDetails.error / .cause: the same pair.
  In the actual string the role ARN and function ARN are per-environment values; the divergence asserted is the error NAME and the fact that the cause is an IAM-denial sentence rather than the function's failure payload.

Cause 2 — sfn-failing-lambda-task-execution-diagnosis#task#11, GetExecutionHistory, 200 -> 200:
  $.events[0].executionFailedEventDetails.error: expected 'RuntimeError', got 'Lambda.AWSLambdaException'
  $.events[0].executionFailedEventDetails.cause: expected a shaped value, got the same IAM-denial sentence.
  ($.nextToken: expected an opaque token, got '4' — owned by the open non-opaque-token issue, not asserted here.)

Cause 3 — sfn-failing-lambda-task-execution-diagnosis#task#19, GetExecutionHistory, 200 -> 200:
  $.events[0].executionFailedEventDetails.error / .cause and $.events[1].taskFailedEventDetails.error / .cause: same expected-vs-actual pairs as cause 1.
  $.events[3].previousEventId: expected 2, got 7.
  ($.nextToken: expected an opaque token, got '5' — owned elsewhere.)

Cause 4 — sfn-legacy-lambda-arn-task-failure-events#task#10, GetExecutionHistory, 200 -> 200:
  $.events[4].lambdaFunctionFailedEventDetails.error: expected 'RuntimeError', got 'Lambda.AWSLambdaException'
  $.events[4].lambdaFunctionFailedEventDetails.cause: expected a shaped value, got the IAM-denial sentence naming the legacy role and function ARNs
  $.events[5].executionFailedEventDetails.error / .cause: the same pair.

## Steps to Reproduce
1. Build the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Cause 1 (representative): fixture sfn-failing-lambda-task-execution-diagnosis, step task#10 — start an execution whose Task uses the optimized arn:aws:states:::lambda:invoke integration against a function that raises an unhandled error, wait for the terminal status, then call GetExecutionHistory and compare $.events[4].taskFailedEventDetails.error/.cause and $.events[5].executionFailedEventDetails.error/.cause: the recording carries the function's error type (RuntimeError) and its payload; the emulator carries 'Lambda.AWSLambdaException' and an IAM lambda:InvokeFunction denial sentence. Step task#12 of the same fixture shows the identical cluster.
3. Cause 2: same fixture, step task#11 — read the same history on the tail/reverse page and compare $.events[0].executionFailedEventDetails.error/.cause.
4. Cause 3: same fixture, step task#19 — redrive the execution, re-read the history and compare $.events[0] / $.events[1] failure details plus $.events[3].previousEventId (recording links to event id 2, emulator to 7).
5. Cause 4: fixture sfn-legacy-lambda-arn-task-failure-events, step task#10 — same run shape but with a Task whose Resource is the bare function ARN; compare $.events[4].lambdaFunctionFailedEventDetails.error/.cause and $.events[5].executionFailedEventDetails.error/.cause.
