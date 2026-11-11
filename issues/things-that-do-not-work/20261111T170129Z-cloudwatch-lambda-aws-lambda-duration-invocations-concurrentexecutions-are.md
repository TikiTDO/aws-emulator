Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda.metrics-lambda-duration-statistics-agree-with-the-report-line-of-each-invoke
Issue-Key: cloudwatch__lambda.metrics-lambda-duration-statistics-agree-with-the-report-line-of-each-invoke|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Commit: 3163fad6f190575ee3da7920eeb1a958f0b14066

# cloudwatch__lambda: AWS/Lambda Duration / Invocations / ConcurrentExecutions are never published for a plain synchronous invoke, so the duration-statistics check never reaches a verdict

> _Note: this issue was identified against the code base at commit `3163fad6f190575ee3da7920eeb1a958f0b14066`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: four sequential synchronous Invokes produce no AWS/Lambda Duration series, so the agreement between the service-published metric statistics and the REPORT line of each invoke is never observable.

AWS: recorded behavior only for the per-datapoint values (the metrics are published by the Lambda service, not modeled as API shapes). Smithy: Invoke(LogType=Tail) returns LogResult, the base64 tail of the invocation log, which carries the `REPORT RequestId: … Duration: <d> ms Billed Duration: <b> ms` line; the Duration AWS/Lambda publishes for the same invocation is that same value. After four sequential invokes of one function, AWS/Lambda under dimension FunctionName=<fn> has: Duration with Unit=Milliseconds, SampleCount 4, Sum equal to the sum of the four REPORT durations, Maximum/Minimum equal to their max/min (and Minimum no lower than the handler's sleep); Invocations Sum 4; and ConcurrentExecutions with Unit=Count whose Maximum peaks at exactly 1, because the invokes were strictly sequential. These series are published by the Lambda service regardless of the execution role's CloudWatch Logs permissions (the check deliberately uses a role with no logs permission).

Emulator: the check exhausts its budget and is reported as `Error: Check timed out after 60.0s`. Its first awaited effect is polling GetMetricStatistics/GetMetricData for AWS/Lambda Duration until SampleCount reaches 4, so the timeout is reached with that series still absent or short. Hypothesis: the Lambda invoke path publishes no AWS/Lambda metrics at all — the same absence is already reported for Duration and Errors under the cloudwatch__lambda__logs heading.

Why: a client cannot measure or alarm on function latency, throughput or concurrency. Duration p-statistics dashboards and Duration/ConcurrentExecutions alarms (including the concurrency headroom alarms used to size reserved concurrency) stay empty and never breach, and the two-plane agreement a client relies on — the REPORT line and the metric reporting the same number — cannot be checked at all.

Fix: the AWS/Lambda metric publication in the Lambda invoke path (the component that, per invocation, emits Duration in Milliseconds, Invocations, Errors and the ConcurrentExecutions gauge under the FunctionName dimension), not the check and not the log tail.

Related open issue that does NOT cover this check (the suite did not mark it owned, and it is filed under a different service heading): 20261109T143515Z-cloudwatch-lambda-logs-aws-lambda-duration-and-errors-metrics-are-never.md.

Traces: cloudwatch__lambda.metrics-lambda-duration-statistics-agree-with-the-report-line-of-each-invoke.

## Evidence
Expected: 4× Invoke(FunctionName=<fn>, Payload={"sleep_ms":150}, LogType=Tail), each returning a REPORT line with Duration ≥ 150 ms and Duration ≤ Billed Duration; then AWS/Lambda under FunctionName=<fn> reports Duration Unit=Milliseconds with SampleCount 4, Sum equal to the sum of the four REPORT durations (±0.024 ms), Maximum/Minimum equal to their max/min (±0.006 ms), Invocations Sum 4, and ConcurrentExecutions Unit=Count with Maximum exactly 1.

Actual: `[mismatch] cloudwatch__lambda.metrics-lambda-duration-statistics-agree-with-the-report-line-of-each-invoke` → `Error: Check timed out after 60.0s`; the poll for "AWS/Lambda Duration SampleCount for <fn> reaching 4" never satisfies its predicate, so none of the Sum/Max/Min/Invocations/ConcurrentExecutions assertions are evaluated.

## Steps to Reproduce
1. iam CreateRole trusting lambda.amazonaws.com with NO CloudWatch Logs permission.
2. lambda CreateFunction (python3.12, handler sleeps event["sleep_ms"] ms and returns).
3. Invoke it 4 times sequentially with Payload {"sleep_ms":150} and LogType=Tail; record the REPORT Duration of each.
4. Poll cloudwatch for namespace AWS/Lambda, MetricName Duration, Dimensions=[{FunctionName:<fn>}], Statistics=[SampleCount,Sum,Minimum,Maximum].
Observed: no datapoints (SampleCount never reaches 4) for the whole budget. Expected: SampleCount 4, Unit Milliseconds, Sum/Max/Min matching the recorded REPORT durations; then Invocations Sum 4 and ConcurrentExecutions Maximum 1.
