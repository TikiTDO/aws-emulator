Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-failures-counted-by-metric-filter-subscriber-and-insights
Issue-Key: cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-failures-counted-by-metric-filter-subscriber-and-insights|_poll_until_settled#27|AssertionError|raise AssertionError(
Issue-Commit: f28dfafa5fc9e548e0b0df8415a880c6df425385

# cloudwatch__lambda__logs__stepfunctions: a CloudWatch Logs metric filter publishes no metric — the transformed metric has no datapoints at all

> _Note: this issue was identified against the code base at commit `f28dfafa5fc9e548e0b0df8415a880c6df425385`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricFilter

## Description
Cause 1 of 1: a metric filter's metricTransformations never produce a CloudWatch metric.
AWS: PutMetricFilter installs a filter on a log group; every log event ingested AFTER the filter exists and matching its filterPattern publishes a datapoint of metricValue into the named metricNamespace/metricName, which GetMetricStatistics then reports. Here the filter pattern is `{$.type = "ExecutionFailed"}` with metricValue "1", three EXPRESS executions fail, and the recorded behavior is Sum == 3 for that namespace/metric over the window, holding steady. Smithy: logs PutMetricFilter MetricTransformation carries metricName/metricNamespace/metricValue; CloudWatch GetMetricStatistics returns Datapoints for the series. This check's preceding surface already passed, so the ExecutionFailed/ExecutionSucceeded records demonstrably reached the log group — only the metric derived from them is missing.
Emulator: GetMetricStatistics(Namespace=<filter namespace>, MetricName=ExpressExecutionFailed, Period=60, Statistics=[Sum]) returns NO datapoints for the entire 420 s delivery budget — the harness reports `metric-filter metric <namespace>/ExpressExecutionFailed never reached its expected value and held it for 10s within 420.0s; last reading was None`. `None` is the harness's rendering of an empty Datapoints list, so the series is never created, not merely under-counted. The namespace and metric name in that line are per-run values; the requirement is that the filter's transformation publishes to whatever namespace/metric the request names.
Why: log-derived metrics are the standard way to count events that have no vended metric (here, EXPRESS execution failures). With no datapoints, every alarm, dashboard and autoscaling policy built on a metric filter stays in INSUFFICIENT_DATA forever and a client cannot distinguish "nothing happened" from "the metric does not exist".
Fix: the CloudWatch Logs ingestion path's metric-filter evaluation — on PutLogEvents, matching events must be fed through each installed filter's metricTransformations into the CloudWatch metric store (state shared with GetMetricStatistics/GetMetricData). A related open issue covers Logs ScheduledQuery-backed alarms (20261109T143515Z-cloudwatch-logs-sns-sqs-...) and the sibling check in this same service is owned by 20261029T182452Z-cloudwatch-lambda-logs-sfn-putsubscriptionfilter-...; neither covers the metric-filter transformation, so this is filed separately.
Traces: cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-failures-counted-by-metric-filter-subscriber-and-insights.

## Evidence
Check cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-failures-counted-by-metric-filter-subscriber-and-insights, verdict mismatch:

AssertionError: metric-filter metric Sim2Real/obssfn3-f7c09c235968/ExpressExecutionFailed never reached its expected value and held it for 10s within 420.0s; last reading was None

Expected: after PutMetricFilter(filterPattern='{$.type = "ExecutionFailed"}', metricTransformations=[{metricName: ExpressExecutionFailed, metricNamespace: <ns>, metricValue: "1"}]) and 3 failing EXPRESS executions whose history reaches the log group, GetMetricStatistics(Namespace=<ns>, MetricName=ExpressExecutionFailed, Period=60, Statistics=['Sum']) sums to 3.0 and holds.
Actual: Datapoints is empty for the whole 420 s budget (reading None). The earlier surface of the same check confirmed the ExecutionFailed x3 / ExecutionSucceeded x2 records were present in the log group, so the inputs to the filter existed.

## Steps to Reproduce
1. Create a log group and call PutMetricFilter on it with filterPattern '{$.type = "ExecutionFailed"}' and metricTransformations=[{"metricName": "ExpressExecutionFailed", "metricNamespace": "<ns>", "metricValue": "1"}].
2. Create an EXPRESS state machine logging level ALL to that group and run 3 executions that end FAILED (plus 2 that succeed).
3. Confirm with FilterLogEvents/GetLogEvents that the group holds the ExecutionFailed records.
4. Call GetMetricStatistics(Namespace="<ns>", MetricName="ExpressExecutionFailed", Period=60, Statistics=["Sum"]) over the window: AWS reports Sum 3.0; the emulator returns an empty Datapoints list indefinitely.
