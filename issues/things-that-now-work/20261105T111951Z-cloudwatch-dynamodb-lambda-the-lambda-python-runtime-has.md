Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.lambda-ddb-accessdenied-logged-to-cloudwatch-negative cloudwatch__dynamodb__lambda.lambda-ddb-batchwrite-unprocessed-items-metric cloudwatch__dynamodb__lambda.lambda-ddb-putitem-consumed-capacity-metric-cloudwatch cloudwatch__dynamodb__lambda.lambda-ddb-table-not-exist-error-logged-cloudwatch cloudwatch__dynamodb__lambda.lambda-put-custom-metric-cloudwatch-positive cloudwatch__dynamodb__lambda.lambda-put-metric-missing-permission-negative cloudwatch__dynamodb__lambda.lambda-throttle-metric-visible-in-cloudwatch cloudwatch__dynamodb__lambda.cloudwatch-alarm-on-lambda-error-rate-transitions-alarm cloudwatch__dynamodb__lambda.lambda-duration-metric-recorded-cloudwatch cloudwatch__dynamodb__lambda.lambda-invocation-logs-appear-in-cloudwatch-logs cloudwatch__dynamodb__lambda.lambda-missing-logs-permission-negative
Issue-Key: cloudwatch__dynamodb__lambda.lambda-ddb-accessdenied-logged-to-cloudwatch-negative|_invoke#8|AssertionError|raise AssertionError(f"lambda invocation failed: {body}")
Issue-Key: cloudwatch__dynamodb__lambda.lambda-ddb-batchwrite-unprocessed-items-metric|_invoke#5|AssertionError|raise AssertionError(f"lambda invocation failed: {body}")
Issue-Key: cloudwatch__dynamodb__lambda.lambda-ddb-putitem-consumed-capacity-metric-cloudwatch|_invoke#5|AssertionError|raise AssertionError(f"lambda invocation failed: {body}")
Issue-Key: cloudwatch__dynamodb__lambda.lambda-ddb-table-not-exist-error-logged-cloudwatch|_invoke#4|AssertionError|raise AssertionError(f"lambda invocation failed: {body}")
Issue-Key: cloudwatch__dynamodb__lambda.lambda-put-custom-metric-cloudwatch-positive|run#7|AssertionError|assert resp.get("FunctionError") is None, f"unexpected invocation error: {resp.get('FunctionError')}"
Issue-Key: cloudwatch__dynamodb__lambda.lambda-put-metric-missing-permission-negative|run#10|AssertionError|assert neg_resp.get("FunctionError") is None, f"neg invoke raised: {neg_resp.get('FunctionError')}"
Issue-Key: cloudwatch__dynamodb__lambda.lambda-throttle-metric-visible-in-cloudwatch|run#7|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__dynamodb__lambda.cloudwatch-alarm-on-lambda-error-rate-transitions-alarm|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__dynamodb__lambda.lambda-duration-metric-recorded-cloudwatch|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__dynamodb__lambda.lambda-invocation-logs-appear-in-cloudwatch-logs|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__dynamodb__lambda.lambda-missing-logs-permission-negative|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# cloudwatch__dynamodb__lambda: the Lambda python runtime has no AWS SDK, and invocation logs and metrics never reach CloudWatch

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 2 — the managed python runtime ships no boto3/botocore.
AWS: the python3.x managed runtimes bundle boto3/botocore; a handler may `import boto3` and call dynamodb:* and cloudwatch:PutMetricData under its execution role, and Invoke returns its payload with no FunctionError.
Emulator: HTTP 200, FunctionError='Unhandled', payload {"errorMessage":"Unable to import module 'handler': No module named 'boto3'","errorType":"Runtime.ImportModuleError"} — the fixed text is "Unable to import module '<module>': No module named 'boto3'" with the configured handler module substituted.
Why: no handler can write a DynamoDB item or publish a custom metric, so neither the positive flows nor their AccessDenied controls are observable.
Fix: the python runtime's bundled site-packages / module search path in the lambda runtime component.
Traces: cloudwatch__dynamodb__lambda.lambda-ddb-accessdenied-logged-to-cloudwatch-negative, .lambda-ddb-batchwrite-unprocessed-items-metric, .lambda-ddb-putitem-consumed-capacity-metric-cloudwatch, .lambda-ddb-table-not-exist-error-logged-cloudwatch, .lambda-put-custom-metric-cloudwatch-positive, .lambda-put-metric-missing-permission-negative, .lambda-throttle-metric-visible-in-cloudwatch.

Cause 2 of 2 — an invocation writes no CloudWatch log events and publishes no AWS/Lambda metric, so every observability assertion polls to its deadline.
AWS: every invocation creates /aws/lambda/<function-name> on first use, writes START/END/REPORT plus handler output, and publishes AWS/Lambda Invocations/Duration/Errors/Throttles datapoints for the FunctionName dimension; GetMetricStatistics returns them within a period or two, and when the role lacks logs permissions no group is created.
Emulator: recorded behavior only — nothing appears in the log group and no datapoint is returned, so the checks never satisfy their predicate and the harness reports "Check timed out after 60.0s".
Why: a client cannot observe an invocation's logs, its duration or its error rate, so alarms built on AWS/Lambda metrics never fire.
Fix: the Lambda->CloudWatch Logs delivery path and the AWS/Lambda metric publication in the invoke path.
Traces: cloudwatch__dynamodb__lambda.cloudwatch-alarm-on-lambda-error-rate-transitions-alarm, .lambda-duration-metric-recorded-cloudwatch, .lambda-invocation-logs-appear-in-cloudwatch-logs, .lambda-missing-logs-permission-negative.

## Evidence
Cause 1: AssertionError: lambda invocation failed: {'errorMessage': "Unable to import module 'handler': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'stackTrace': []}.
Cause 2: Error: Check timed out after 60.0s while polling FilterLogEvents on /aws/lambda/<fn> and GetMetricStatistics for AWS/Lambda Duration/Errors. Expected: log events and datapoints within the window; the alarm transitions to ALARM on a sustained error rate.

## Steps to Reproduce
1. CreateFunction (python3.12) importing boto3 and calling dynamodb.put_item / cloudwatch.put_metric_data, then Invoke (cause 1).
2. Invoke a printing handler, then FilterLogEvents on /aws/lambda/<fn> and GetMetricStatistics for AWS/Lambda Duration with FunctionName; also PutMetricAlarm on Errors and drive failures (cause 2).
