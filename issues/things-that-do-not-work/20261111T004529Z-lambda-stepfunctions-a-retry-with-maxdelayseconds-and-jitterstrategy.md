Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__stepfunctions.lambda-invoke-retry-full-jitter-caps-scheduled-gaps
Issue-Key: lambda__stepfunctions.lambda-invoke-retry-full-jitter-caps-scheduled-gaps|_run_to_terminal#8|Timeout|last = sfn.describe_execution(executionArn=exec_arn)
Issue-Commit: f28dfafa5fc9e548e0b0df8415a880c6df425385

# lambda__stepfunctions: a Retry with MaxDelaySeconds and JitterStrategy FULL is not clamped or jittered, so the retried lambda:invoke Task never finishes in time

> _Note: this issue was identified against the code base at commit `f28dfafa5fc9e548e0b0df8415a880c6df425385`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: a retrier's MaxDelaySeconds cap and JitterStrategy FULL are not applied to the computed backoff, so the waits between attempts grow unbounded and the execution does not terminate within the time a correctly clamped run needs.
AWS: Step Functions' Retry fields are documented at https://docs.aws.amazon.com/step-functions/latest/dg/concepts-error-handling.html. For a retrier with IntervalSeconds=5, BackoffRate=3.0, MaxAttempts=7, MaxDelaySeconds=10, JitterStrategy=FULL the uncapped delays would be 5, 15, 45, 135, 405, 1215, 3645 s; MaxDelaySeconds clamps each to at most 10 s (5, 10, 10, 10, 10, 10, 10) and FULL jitter then draws each actual wait uniformly from [0, that clamped delay] — jitter only ever shortens a wait. The run therefore produces MaxAttempts+1 = 8 TaskScheduled and 8 TaskFailed events, 7 inter-attempt waits each within [0, cap], and the late waits lie far below their uncapped values; the whole execution ends FAILED with error 'Exception' (the handler's own class) in well under a minute.
Emulator: the check is cut off — `Check timed out after 60.0s` — before any history assertion is graded. A run honouring the cap and FULL jitter averages ~33 s of waiting (expected value of the sum of uniform draws over 5,10,10,10,10,10,10) and fits the budget; a run that ignores MaxDelaySeconds reaches the 4th wait at 135 s and cannot. Hypothesis: MaxDelaySeconds and/or JitterStrategy are parsed but not applied when scheduling the next attempt, so the gaps follow the raw IntervalSeconds*BackoffRate^i geometric series.
Why: any state machine using the documented cap to bound a retry storm instead waits minutes-to-hours between attempts; executions that AWS finishes in seconds run past client timeouts and past the state machine's own TimeoutSeconds, and nothing about the definition explains it.
Fix: the Step Functions retrier's delay computation in the task-retry scheduler — clamp the grown delay at MaxDelaySeconds and, for JitterStrategy FULL, draw the wait from [0, clamped delay]. Many sibling lambda__stepfunctions checks are owned by 20261109T210043Z-lambda-stepfunctions-8-net-new-causes-step-functions-ignores-task.md, which does not carry this check's key.
Traces: lambda__stepfunctions.lambda-invoke-retry-full-jitter-caps-scheduled-gaps.

## Evidence
Check lambda__stepfunctions.lambda-invoke-retry-full-jitter-caps-scheduled-gaps, verdict mismatch:

Error: Check timed out after 60.0s

Definition under test: one Task, Resource arn:aws:states:::lambda:invoke on an always-raising handler, Retry [{ErrorEquals:[States.ALL], IntervalSeconds:5, MaxAttempts:7, BackoffRate:3.0, MaxDelaySeconds:10, JitterStrategy:FULL}], no Catch.
Expected: execution FAILED with error 'Exception' and cause errorMessage 'sim2real: this handler always raises'; exactly 8 TaskScheduled and 8 TaskFailed events; 7 measured gaps, each in [0, min(5*3^i, 10)] + slack, with the gaps at the clamped positions (i>=2) far below their uncapped 45/135/405/1215/3645 s.
Actual: the check never reaches those assertions — it is killed at its 60 s budget while waiting for the execution to go terminal, which a cap-honouring, FULL-jittered run (~33 s of waiting on average, 65 s worst case before jitter) does not require.

## Steps to Reproduce
1. Create a Python Lambda whose handler is `raise Exception('sim2real: this handler always raises')`.
2. Create a state machine with a single Task on arn:aws:states:::lambda:invoke of that function and Retry [{"ErrorEquals":["States.ALL"],"IntervalSeconds":5,"MaxAttempts":7,"BackoffRate":3.0,"MaxDelaySeconds":10,"JitterStrategy":"FULL"}], no Catch.
3. StartExecution and poll DescribeExecution until terminal, then GetExecutionHistory.
4. Measure the seconds between each TaskFailed and the next TaskScheduled: AWS gives 7 gaps each within [0, 10] (the first within [0, 5]) and the execution ends FAILED quickly; the emulator's gaps follow 5, 15, 45, ... so the execution is still running when the budget expires.
