Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__cloudwatch.publish-cloudwatch-metrics-enabled-delivers-processedbytes-for-the-workgroup
Issue-Key: athena__cloudwatch.publish-cloudwatch-metrics-enabled-delivers-processedbytes-for-the-workgroup|_await_datapoints#28|AssertionError|raise AssertionError(
Issue-Commit: 4315fe29349a8ccf086f6caac897ebb4dc04700d

# Athena never publishes AWS/Athena ProcessedBytes for a workgroup with PublishCloudWatchMetricsEnabled (1 cause)

> _Note: this issue was identified against the code base at commit `4315fe29349a8ccf086f6caac897ebb4dc04700d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
Cause 1 of 1: a workgroup created with PublishCloudWatchMetricsEnabled=true publishes no AWS/Athena metric series at all, so ProcessedBytes (and the sibling EngineExecutionTime series the same check reads) never becomes visible to CloudWatch.

AWS: Smithy: Athena CreateWorkGroup/UpdateWorkGroup carry WorkGroupConfiguration$PublishCloudWatchMetricsEnabled; AWS documents (https://docs.aws.amazon.com/athena/latest/ug/query-metrics-viewing.html) that when that flag is on, every query run in the workgroup emits the AWS/Athena metric family — ProcessedBytes, EngineExecutionTime, TotalExecutionTime, QueryQueueTime, QueryPlanningTime — dimensioned by WorkGroup (the workgroup name as given to CreateWorkGroup), with a datapoint per terminal query. Recorded behavior: after a SUCCEEDED StartQueryExecution/GetQueryExecution cycle in the metrics-enabled workgroup, GetMetricStatistics/GetMetricData over namespace AWS/Athena, metric ProcessedBytes, dimension [{Name: WorkGroup, Value: <the workgroup name>}] returns datapoints within the metric-publication window.

Emulator: the same query runs to SUCCEEDED, but the AWS/Athena namespace stays empty: no ProcessedBytes datapoint for the WorkGroup dimension ever appears, and the check exhausts its 420 s budget with an empty datapoint list.

Why: a client that sizes or bills queries from AWS/Athena, or alarms on ProcessedBytes/EngineExecutionTime for a workgroup, reads an account in which no query ever ran. Dashboards stay blank and any alarm on these series sits in INSUFFICIENT_DATA forever instead of evaluating, so cost-control and query-health automation built on the documented workgroup metrics silently never fires.

Fix: the Athena query-execution path — the component that completes a query execution must, when the executing workgroup's configuration has PublishCloudWatchMetricsEnabled=true, publish the AWS/Athena metric family for that terminal query under the WorkGroup dimension into the CloudWatch metric store (the same store GetMetricStatistics/GetMetricData read). Nothing in the CloudWatch read path needs changing: the series is absent, not mis-shaped.

Traces: athena__cloudwatch.publish-cloudwatch-metrics-enabled-delivers-processedbytes-for-the-workgroup.

## Evidence
Check athena__cloudwatch.publish-cloudwatch-metrics-enabled-delivers-processedbytes-for-the-workgroup [mismatch]:

AssertionError: AWS/Athena/ProcessedBytes for the metrics-enabled workgroup 'sim2real-xsvc-athena-wg-on-<id>' did not appear in CloudWatch within 420s (namespace AWS/Athena, metric ProcessedBytes, dimensions [{'Name': 'WorkGroup', 'Value': 'sim2real-xsvc-athena-wg-on-<id>'}])

Expected: at least one ProcessedBytes datapoint under dimension WorkGroup=<the workgroup created with PublishCloudWatchMetricsEnabled=true> after the query reached SUCCEEDED.
Actual: no datapoints in the AWS/Athena namespace for the whole 420 s budget (the workgroup name and id above are per-run values, not part of the requirement).

## Steps to Reproduce
1. CreateWorkGroup with Configuration.PublishCloudWatchMetricsEnabled=true and a ResultConfiguration pointing at an S3 bucket.
2. StartQueryExecution in that workgroup and poll GetQueryExecution until the state is SUCCEEDED.
3. Call cloudwatch GetMetricStatistics (or GetMetricData) for Namespace=AWS/Athena, MetricName=ProcessedBytes, Dimensions=[{Name: WorkGroup, Value: <workgroup>}] over a window covering the query.
4. AWS returns at least one datapoint; the emulator returns an empty datapoint list indefinitely.
