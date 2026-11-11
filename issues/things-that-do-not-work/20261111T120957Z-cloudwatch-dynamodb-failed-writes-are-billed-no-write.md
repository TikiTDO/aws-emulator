Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb.metrics-dynamodb-cancelled-transact-write-items-still-consumes-write-capacity cloudwatch__dynamodb.metrics-dynamodb-refused-conditional-putitem-write-capacity-sizing
Issue-Key: cloudwatch__dynamodb.metrics-dynamodb-cancelled-transact-write-items-still-consumes-write-capacity|run#31|AssertionError|assert per_cancellation == committed_wcu, (
Issue-Key: cloudwatch__dynamodb.metrics-dynamodb-refused-conditional-putitem-write-capacity-sizing|run#34|AssertionError|assert per_refusal_large_existing == 3.0, (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# cloudwatch__dynamodb: failed writes are billed no write capacity — a cancelled TransactWriteItems and a condition-refused PutItem add 0 WCU to ConsumedWriteCapacityUnits (2 causes)

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TransactWriteItems

## Description
Two net-new causes, both in DynamoDB's write-capacity accounting for writes that did not commit. In both cases AWS/DynamoDB ConsumedWriteCapacityUnits does exist for the table (the seed traffic is accounted for), so this is not the 'no series at all' family — it is the billing rule for a failed write.

Cause 1 of 2: a cancelled TransactWriteItems consumes no write capacity.
AWS: recorded behavior only (capacity metering is not expressible in the Smithy model). A transactional write is charged twice the capacity of the equivalent non-transactional write — the prepare and the commit phase — and a transaction that is CANCELLED (TransactionCanceledException, cancellation reason ConditionalCheckFailed) is charged exactly the same capacity as the committed transaction it was shaped like, because DynamoDB has already done the prepare and the write attempt. For a single Put of a 1024 B item that is 2.0 WCU per cancelled transaction, the same as the committed control.
Emulator: five cancelled single-Put transactions on a 1024 B item add nothing to ConsumedWriteCapacityUnits for the table (table total 1.0 against a seed of 1.0, i.e. -0.4 WCU per cancellation once the check's arithmetic is applied), while the committed control is charged 2.0. Failing check: cloudwatch__dynamodb.metrics-dynamodb-cancelled-transact-write-items-still-consumes-write-capacity.

Cause 2 of 2: a condition-refused PutItem is billed by the size of the write it attempted (and in practice 0 WCU), not by the size of the item already stored at the key.
AWS: recorded behavior only. A write refused by a failed condition expression (ConditionalCheckFailedException) still consumes write capacity, and it is sized by the item that is already stored at that key, not by the payload of the rejected write — so five refusals of a 20 B write against a 3072 B stored item each cost 3.0 WCU (3 KB rounded up per 1 KB), and ConsumedWriteCapacityUnits rises by 15.0 over the seed. The error body itself reports no ConsumedCapacity; the metric is where the charge is observable.
Emulator: each refusal adds 0.0 WCU (table total 3.0 against a seed of 3.0), so the charge is absent altogether and, a fortiori, is not sized by the stored item. Failing check: cloudwatch__dynamodb.metrics-dynamodb-refused-conditional-putitem-write-capacity-sizing.

Why (both): capacity dashboards, provisioned-throughput planning and throttle alarms under-report real consumption, and a workload dominated by conditional-write contention or cancelled transactions looks free. Any reconciliation between the API plane's ConsumedCapacity and the AWS/DynamoDB metric disagrees, and a client cannot see the cost of the contention it is actually generating.

Fix: the write-capacity accounting in the DynamoDB data plane's PutItem/TransactWriteItems handlers and the AWS/DynamoDB metric emitter — a refused or cancelled write must emit the same ConsumedWriteCapacityUnits it would have cost had it committed (transactional writes doubled; refused conditional writes sized from the stored item).

Traces: the two failing checks listed in trace_ids.

Related open issue that does NOT carry these replay keys and therefore does not cover them: 20261102T120041Z-cloudwatch-dynamodb-dynamodb-publishes-no-aws-dynamodb-cloudwatch.md (it describes the namespace being wholly empty; here the series exists and only failed-write charges are missing).

## Evidence
Check cloudwatch__dynamodb.metrics-dynamodb-cancelled-transact-write-items-still-consumes-write-capacity [mismatch]:
AssertionError: 5 cancelled single-Put transactions on a 1024 B item added -0.4 WCU each to ConsumedWriteCapacityUnits (table total 1.0, seed 1.0, committed control 2.0); a cancelled transaction must consume the same write capacity as the committed one it was shaped like, 2.0 WCU, not just its prepare phase
Expected: +2.0 WCU per cancelled transaction. Actual: +0 (table total unchanged from the seed).

Check cloudwatch__dynamodb.metrics-dynamodb-refused-conditional-putitem-write-capacity-sizing [mismatch]:
AssertionError: 5 refusals of a 20 B write against a 3072 B existing item added 0.0 WCU each (table total 3.0, seed 3.0); a refused write is billed by the size of the item already stored at the key, not the size of the write it attempted
Expected: +3.0 WCU per refusal (3 KB stored item, rounded up per 1 KB). Actual: +0.0.

In both checks the seed traffic IS accounted for in ConsumedWriteCapacityUnits, so the series exists and only the failed-write charge is missing.

## Steps to Reproduce
Cause 1:
1. Create a DynamoDB table and seed it so ConsumedWriteCapacityUnits has a known baseline.
2. Commit one TransactWriteItems with a single Put of a 1024 B item (control: 2.0 WCU).
3. Issue 5 TransactWriteItems with a single Put of a 1024 B item whose condition fails, each answered TransactionCanceledException ('Transaction cancelled, please refer cancellation reasons for specific reasons [ConditionalCheckFailed]').
4. Read cloudwatch GetMetricStatistics Namespace='AWS/DynamoDB', MetricName='ConsumedWriteCapacityUnits', Dimensions=[{TableName}], Statistics=['Sum'] over the window: expect seed + 2.0 (control) + 5 x 2.0; observe no increase for the cancellations.

Cause 2:
1. PutItem a 3072 B item at a key; note the ConsumedWriteCapacityUnits baseline.
2. Issue 5 PutItems of a 20 B body at that same key with a condition expression that fails (ConditionalCheckFailedException).
3. Read ConsumedWriteCapacityUnits (Sum) for {TableName}: expect +3.0 per refusal (sized by the 3 KB stored item); observe +0.0.
