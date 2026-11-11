Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb.ddb-consumed-capacity-two-plane-numeric-agreement
Issue-Key: cloudwatch__dynamodb.ddb-consumed-capacity-two-plane-numeric-agreement|run#44|AssertionError|assert samples is not None, (
Issue-Commit: 4315fe29349a8ccf086f6caac897ebb4dc04700d

# AWS/DynamoDB SuccessfulRequestLatency is not published under the per-operation (TableName, Operation) dimension pair (1 cause)

> _Note: this issue was identified against the code base at commit `4315fe29349a8ccf086f6caac897ebb4dc04700d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
Cause 1 of 1: table requests produce no SuccessfulRequestLatency series under the per-operation dimension pair, so the latency family cannot be read per API call. This is adjacent to, but not covered by, the open issue 20261111T021316Z-cloudwatch-dynamodb-no-aws-dynamodb-consumedwritecapacityunits-or-successfulrequestlatency.md, which carries a different check's replay key.

AWS: recorded behavior only for the series' existence — AWS documents (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/metrics-dimensions.html) that AWS/DynamoDB SuccessfulRequestLatency is emitted with the dimension pair (TableName, Operation), where Operation is the DynamoDB API name that was served (PutItem, GetItem, Query, ...), with SampleCount equal to the number of successful requests of that operation in the period and Average/Minimum/Maximum in milliseconds. The recording reads the series for Operation='PutItem' on the table it just wrote to and gets a non-empty datapoint set.

Emulator: PutItem (and the other read/write calls in the scenario) succeed on the data plane, but GetMetricStatistics/GetMetricData for AWS/DynamoDB SuccessfulRequestLatency with Dimensions=[{TableName}, {Operation=PutItem}] returns no datapoints at all, so no SampleCount is available to agree with the number of requests issued.

Why: a client cannot observe per-operation latency or request counts for a table. Operational dashboards and alarms keyed to (TableName, Operation) — the standard DynamoDB latency and request-rate panels — stay empty, and an alarm over them never evaluates, so the two planes (the data-plane ConsumedCapacity a request reports and the metric plane) can never be reconciled.

Fix: the DynamoDB request-serving path's metric publication component — each successfully served table request must publish a SuccessfulRequestLatency datapoint into the CloudWatch metric store under both the (TableName) and the (TableName, Operation) dimension sets, with the served operation's name as the Operation value.

Traces: cloudwatch__dynamodb.ddb-consumed-capacity-two-plane-numeric-agreement.

## Evidence
Check cloudwatch__dynamodb.ddb-consumed-capacity-two-plane-numeric-agreement [mismatch]:

AssertionError: AWS/DynamoDB SuccessfulRequestLatency has no datapoints under (TableName, Operation='PutItem'); the per-operation dimension must be published

Expected: a SuccessfulRequestLatency datapoint set under Dimensions=[{Name: TableName, Value: <table>}, {Name: Operation, Value: PutItem}] whose SampleCount accounts for the PutItem calls made (15 in the recording).
Actual: the series is absent — no datapoints under the per-operation dimension pair (the table name and request counts are per-run values).

## Steps to Reproduce
1. Create a DynamoDB table.
2. Issue a batch of PutItem calls (the recording writes 15 items of mixed sizes), then GetItem/Query reads.
3. cloudwatch GetMetricStatistics for Namespace=AWS/DynamoDB, MetricName=SuccessfulRequestLatency, Dimensions=[{Name: TableName, Value: <table>}, {Name: Operation, Value: PutItem}], Statistics=[SampleCount, Average] over the window.
4. AWS returns datapoints whose SampleCount matches the PutItem calls; the emulator returns none.
