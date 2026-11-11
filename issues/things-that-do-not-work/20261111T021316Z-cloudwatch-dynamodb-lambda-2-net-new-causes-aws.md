Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.ddb-stream-lambda-error-metric-on-handler-exception cloudwatch__dynamodb__lambda.ddb-stream-lambda-throttled-esm-metric
Issue-Key: cloudwatch__dynamodb__lambda.ddb-stream-lambda-error-metric-on-handler-exception|_poll_until#11|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__dynamodb__lambda.ddb-stream-lambda-throttled-esm-metric|_poll_until#12|AssertionError|raise AssertionError(
Issue-Commit: 1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30

# cloudwatch__dynamodb__lambda: 2 net-new causes — AWS/Lambda Errors and Throttles are never published for ESM-driven invocations

> _Note: this issue was identified against the code base at commit `1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Two net-new root causes, both in the Lambda-to-CloudWatch metric path for invocations driven by a DynamoDB-Streams event source mapping. Both must be closed before this issue is submitted.

Cause 1 of 2 — the AWS/Lambda Errors metric is never incremented when a handler raises.
Failing check: cloudwatch__dynamodb__lambda.ddb-stream-lambda-error-metric-on-handler-exception
AWS: recorded behavior only (the metric catalogue is not in the Smithy model). AWS Lambda publishes the AWS/Lambda metric Errors (dimension FunctionName) once per invocation whose handler raised. With a DynamoDB-Streams ESM configured BatchSize=5, MaximumRetryAttempts=1, BisectBatchOnFunctionError=false and a handler that always raises, the ESM invokes once and retries once before discarding the batch, so the Errors Sum for that FunctionName settles at exactly 2.
Emulator: the Errors Sum stays 0 for the full 240 s polling window — no AWS/Lambda Errors datapoint is published at all for the failing invocations, even though the ESM did drive the function.
Why: the Errors metric is the primary failure signal for a stream consumer; an alarm or dashboard built on it never leaves INSUFFICIENT_DATA/OK against the emulator, so a client's failure-detection wiring is validated as working when on AWS it would fire.
Fix: the Lambda invocation path must publish AWS/Lambda Errors (dimension FunctionName) into the CloudWatch metric store for every invocation that ends in a handler error, including invocations issued by an event source mapping and its retries.
Traces: cloudwatch__dynamodb__lambda.ddb-stream-lambda-error-metric-on-handler-exception.

Cause 2 of 2 — the AWS/Lambda Throttles metric is never incremented when reserved concurrency refuses an invocation.
Failing check: cloudwatch__dynamodb__lambda.ddb-stream-lambda-throttled-esm-metric
AWS: recorded behavior only. With PutFunctionConcurrency ReservedConcurrentExecutions=0, every invocation attempt is refused as a throttle (TooManyRequestsException), and AWS Lambda publishes the AWS/Lambda metric Throttles (dimension FunctionName) with Sum >= 1 for the attempts the ESM made.
Emulator: the Throttles Sum stays 0 for the full 240 s polling window — no AWS/Lambda Throttles datapoint is published for the refused ESM invocations.
Why: a client tuning reserved concurrency, or alarming on throttling of a stream consumer, sees no signal at all; back-pressure that is plainly occurring is invisible, so capacity and retry logic cannot be exercised.
Fix: the Lambda concurrency-admission path must publish AWS/Lambda Throttles (dimension FunctionName) into the CloudWatch metric store whenever an invocation is refused by reserved concurrency, for ESM-driven invocations as well as direct ones.
Traces: cloudwatch__dynamodb__lambda.ddb-stream-lambda-throttled-esm-metric.

## Evidence
Cause 1 — ddb-stream-lambda-error-metric-on-handler-exception:
  expected AWS/Lambda Errors Sum (dimension FunctionName) >= 2, then settling at exactly 2 (1 invocation + MaximumRetryAttempts=1 retry)
  got      'effect not met within 240s. Last result: 0'  — the Errors Sum never rose above 0

Cause 2 — ddb-stream-lambda-throttled-esm-metric:
  expected AWS/Lambda Throttles Sum (dimension FunctionName) >= 1 with ReservedConcurrentExecutions=0
  got      'effect not met within 240s. Last result: 0'  — the Throttles Sum never rose above 0

## Steps to Reproduce
Cause 1: create an execution role and a DynamoDB table with NEW_AND_OLD_IMAGES streams; create a Python function whose handler raises; create an event source mapping on the stream with BatchSize=5, MaximumRetryAttempts=1, BisectBatchOnFunctionError=false; PutItem one row. Then poll CloudWatch GetMetricStatistics for namespace AWS/Lambda, MetricName Errors, dimension FunctionName=<fn>, Statistic Sum — it must reach 2.

Cause 2: same role/table/function (handler returns successfully); call PutFunctionConcurrency with ReservedConcurrentExecutions=0 BEFORE wiring the mapping; create the ESM with BatchSize=5, MaximumRetryAttempts=2; PutItem one row. Poll CloudWatch for namespace AWS/Lambda, MetricName Throttles, dimension FunctionName=<fn>, Statistic Sum — it must reach at least 1.
