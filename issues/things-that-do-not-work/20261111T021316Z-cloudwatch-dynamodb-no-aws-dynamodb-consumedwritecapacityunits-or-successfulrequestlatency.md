Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb.metrics-dynamodb-transactional-write-costs-double-in-both-planes
Issue-Key: cloudwatch__dynamodb.metrics-dynamodb-transactional-write-costs-double-in-both-planes|_await_exact#14|AssertionError|raise AssertionError(
Issue-Commit: 1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30

# cloudwatch__dynamodb: no AWS/DynamoDB ConsumedWriteCapacityUnits or SuccessfulRequestLatency datapoints are published, so transactional write cost cannot be observed in CloudWatch

> _Note: this issue was identified against the code base at commit `1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
Cause 1 of 1 — DynamoDB publishes no AWS/DynamoDB metrics, so the CloudWatch plane never agrees with the data plane's ConsumedCapacity.
Failing check: cloudwatch__dynamodb.metrics-dynamodb-transactional-write-costs-double-in-both-planes
AWS: recorded behavior only (the metric catalogue is not in the Smithy model). AWS DynamoDB emits, per table, the AWS/DynamoDB metrics ConsumedWriteCapacityUnits (Sum, dimension TableName, and Operation for request-level metrics) and SuccessfulRequestLatency (dimensions TableName + Operation). A transactional write costs exactly twice a non-transactional write of the same item: for three copies each of a 1024-byte and a 1025-byte item, PutItem accounts 9.0 WCU and a single-Put TransactWriteItems accounts 18.0 WCU, and SuccessfulRequestLatency appears under Operation=PutItem for the plain table and Operation=TransactWriteItems for the transactional one — and under neither table for the operation it did not use.
Emulator: every one of those six metric readings is absent. After the writes and 200 s of polling, GetMetricData/GetMetricStatistics against AWS/DynamoDB returns no datapoints at all: plain WCU Sum = None, txn WCU Sum = None, and all four SuccessfulRequestLatency probes = None. The data-plane ConsumedCapacity is not the subject here; the metric namespace is simply never written to.
Why: a client cannot build any DynamoDB capacity dashboard, alarm or autoscaling trigger against the emulator — every alarm sits in INSUFFICIENT_DATA — and the doubled cost of transactional writes, which is how a caller sizes provisioned capacity, is unobservable. Code that validates its capacity model via CloudWatch silently passes.
Fix: the DynamoDB request-accounting path must publish AWS/DynamoDB datapoints into the CloudWatch metric store (ConsumedWriteCapacityUnits and SuccessfulRequestLatency, with the TableName and Operation dimension sets), and the transactional multiplier must be reflected in the published value as well as in ConsumedCapacity.
Traces: cloudwatch__dynamodb.metrics-dynamodb-transactional-write-costs-double-in-both-planes. Related open issues 20261102T120041Z-cloudwatch-dynamodb-dynamodb-publishes-no-aws-dynamodb-cloudwatch.md and 20261029T180026Z-cloudwatch-dynamodb-no-aws-dynamodb-cloudwatch-metrics-are-published-and.md describe the same family but do not own this check.

## Evidence
AWS/DynamoDB write accounting for twin tables (PutItem table expected 9.0 WCU, TransactWriteItems table expected 18.0 WCU):
  expected {'plain WCU Sum': 9.0, 'txn WCU Sum': 18.0, 'txn latency TransactWriteItems': 6.0, 'txn latency PutItem': None, 'plain latency PutItem': 6.0, 'plain latency TransactWriteItems': None}
  never reached within 200 s; last reading {'plain WCU Sum': None, 'txn WCU Sum': None, 'txn latency TransactWriteItems': None, 'txn latency PutItem': None, 'plain latency PutItem': None, 'plain latency TransactWriteItems': None}
Every AWS/DynamoDB datapoint is missing, not merely mis-valued.

## Steps to Reproduce
Create two identical on-demand tables. Into table A write three 1024-byte items and three 1025-byte items with PutItem; into table B write the same six items, each as a single-Put TransactWriteItems. Then query CloudWatch for namespace AWS/DynamoDB: ConsumedWriteCapacityUnits Sum with dimension TableName for each table, and SuccessfulRequestLatency with dimensions TableName+Operation for Operation=PutItem and Operation=TransactWriteItems on both tables. Table A must report 9.0 WCU and latency only under PutItem; table B must report 18.0 WCU and latency only under TransactWriteItems.
