Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs.cloudwatch-alarm-state-change-eventbridge-invokes-lambda-positive cloudwatch__lambda__logs.cloudwatch-composite-alarm-triggers-lambda-positive cloudwatch__lambda__logs.cloudwatch-alarm-missing-lambda-permission-negative cloudwatch__lambda__logs.logs-subscription-filter-invokes-lambda-positive cloudwatch__lambda__logs.logs-subscription-filter-pattern-matching-positive cloudwatch__lambda__logs.cli-create-log-subscription-filter-lambda-workflow
Issue-Key: cloudwatch__lambda__logs.cloudwatch-alarm-state-change-eventbridge-invokes-lambda-positive|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.cloudwatch-composite-alarm-triggers-lambda-positive|_poll_until#26|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.cloudwatch-alarm-missing-lambda-permission-negative|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.logs-subscription-filter-invokes-lambda-positive|_emit_until_decoded#20|Timeout|time.sleep(5.0)
Issue-Key: cloudwatch__lambda__logs.logs-subscription-filter-pattern-matching-positive|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.cli-create-log-subscription-filter-lambda-workflow|_poll_for_log_message#25|Timeout|time.sleep(interval_s)
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# CloudWatch alarm actions and CloudWatch Logs subscription filters never invoke their Lambda target

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
Cause 1 of 1: a CloudWatch alarm whose AlarmActions name a Lambda (directly or through an EventBridge alarm-state-change rule, including a composite alarm) must invoke that function when it transitions, and a CloudWatch Logs subscription filter whose destination is a Lambda must invoke it with the matching log events — and must not invoke it for non-matching lines.
AWS: recorded behavior only — each recording wires the destination, grants lambda:InvokeFunction to the alarm/logs principal, drives the trigger (a metric breach or a PutLogEvents call), and then polls the target function's log group for a marker; all six complete inside the suite's 60s budget, and the negative twins observe the absence only after the positive control has fired.
Emulator: all six checks are cut off with "Error: Check timed out after 60.0s". The alarm is created and the subscription filter is accepted, but no invocation of the destination function is ever made, so the marker never appears and the poll never terminates.
Why: a client that fans log lines or alarm transitions into a Lambda gets configuration that reports success and a pipeline that delivers nothing; because the missing-permission negatives also only time out, a client cannot tell a permission problem from a dead delivery path.
Fix: the CloudWatch alarm-action dispatcher (including the EventBridge alarm-state-change event source and composite-alarm evaluation) and the CloudWatch Logs subscription-filter delivery path that invokes a Lambda destination.
Traces: cloudwatch__lambda__logs.cloudwatch-alarm-state-change-eventbridge-invokes-lambda-positive, .cloudwatch-composite-alarm-triggers-lambda-positive, .cloudwatch-alarm-missing-lambda-permission-negative, .logs-subscription-filter-invokes-lambda-positive, .logs-subscription-filter-pattern-matching-positive, .cli-create-log-subscription-filter-lambda-workflow.

## Evidence
Expected (AWS): the alarm transition invokes the Lambda (marker appears in its log group); the subscription filter invokes the Lambda for matching lines only.
Actual (emulator): all six checks end with 'Error: Check timed out after 60.0s' — no invocation of the destination function is ever observed.

## Steps to Reproduce
1. Create a Lambda and grant lambda:InvokeFunction to logs.amazonaws.com / events.amazonaws.com as appropriate.
2. PutSubscriptionFilter on a log group with that function as destination, then PutLogEvents with a matching line; poll the function's log group for a marker.
3. PutMetricAlarm (and a composite alarm) with the function as the action target, drive the metric past the threshold, and poll for the marker.
4. Observe that neither path ever invokes the function within 60s.
