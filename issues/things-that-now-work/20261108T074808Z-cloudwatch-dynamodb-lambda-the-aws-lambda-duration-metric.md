Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.lambda-duration-metric-recorded-cloudwatch
Issue-Key: cloudwatch__dynamodb__lambda.lambda-duration-metric-recorded-cloudwatch|probe#9|Timeout|return cw.get_metric_statistics(
Issue-Commit: 3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e

# cloudwatch__dynamodb__lambda: the AWS/Lambda Duration metric is never published for an invocation, so GetMetricStatistics never returns a datapoint (1 cause)

> _Note: this issue was identified against the code base at commit `3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: a successful synchronous Lambda invocation publishes no AWS/Lambda Duration datapoint, so polling CloudWatch GetMetricStatistics for it never succeeds and the check is killed at the harness budget. Failing check: cloudwatch__dynamodb__lambda.lambda-duration-metric-recorded-cloudwatch.

AWS: recorded behavior only — the metric itself is not described by the Smithy model (GetMetricStatistics simply returns Datapoints for whatever Namespace/MetricName/Dimensions are asked for). AWS Lambda publishes, for every invocation, a Duration metric in the AWS/Lambda namespace dimensioned by FunctionName, whose value is the billed wall-clock duration of the invoke in milliseconds. After an invoke whose handler sleeps, GetMetricStatistics(Namespace='AWS/Lambda', MetricName='Duration', Dimensions=[{FunctionName:<name>}]) returns at least one datapoint whose statistic reflects that duration. The namespace, metric name and dimension name are fixed text; the function name and the duration value come from the request and the invocation.

Emulator: the datapoint never appears. The replay polls GetMetricStatistics after a successful invoke (FunctionError absent) and reports `Error: Check timed out after 180.0s` — the probe never returns a Datapoints entry for AWS/Lambda Duration.

Why: a client that alarms on or dashboards Lambda Duration sees a permanently empty series. Alarms on invocation latency can never leave INSUFFICIENT_DATA, autoscaling and capacity decisions built on Duration have no input, and an operator cannot tell a slow function from a function that is not being invoked.

Fix: the Lambda invoke path's metric emission (the component that publishes AWS/Lambda platform metrics into the CloudWatch metric store) — Duration must be published per invocation with the FunctionName dimension, so CloudWatch's GetMetricStatistics can serve it.

Note: 20261105T111951Z-cloudwatch-lambda-aws-lambda-duration-statistics-never-appear.md is an open issue about Duration statistics under the cloudwatch__lambda heading; it does not carry this check's replay key under cloudwatch__dynamodb__lambda.

Traces: cloudwatch__dynamodb__lambda.lambda-duration-metric-recorded-cloudwatch.

## Evidence
Expected (AWS): after `invoke` of a function whose handler sleeps ~2 s returns with no FunctionError, GetMetricStatistics for Namespace='AWS/Lambda', MetricName='Duration', Dimensions=[{'Name':'FunctionName','Value':<function name>}] returns at least one datapoint reflecting that duration.

Actual (emulator): the probe never yields a datapoint; the check is killed — `Error: Check timed out after 180.0s`.

## Steps to Reproduce
1. Create a Python Lambda with AWSLambdaBasicExecutionRole whose handler does `time.sleep(2)` and returns.
2. Invoke it synchronously and confirm FunctionError is absent.
3. Poll cloudwatch.get_metric_statistics(Namespace='AWS/Lambda', MetricName='Duration', Dimensions=[{'Name':'FunctionName','Value':<name>}], ...) — no Datapoints ever arrive.
4. Replay cloudwatch__dynamodb__lambda.lambda-duration-metric-recorded-cloudwatch; it is killed at the 180 s budget.
