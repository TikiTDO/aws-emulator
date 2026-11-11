Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb.conc-ddb-optimistic-refused-conditional-write-still-billed-rejected
Issue-Key: cloudwatch__dynamodb.conc-ddb-optimistic-refused-conditional-write-still-billed-rejected|_poll_until_at_least#16|Timeout|time.sleep(_POLL_S)
Issue-Commit: 3163fad6f190575ee3da7920eeb1a958f0b14066

# cloudwatch__dynamodb: refused conditional writes are never metered — no AWS/DynamoDB ConsumedWriteCapacityUnits datapoint appears for either the losing or the winning table

> _Note: this issue was identified against the code base at commit `3163fad6f190575ee3da7920eeb1a958f0b14066`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
Cause 1 of 1: the AWS/DynamoDB ConsumedWriteCapacityUnits series for a table is never populated, so the contract that a conditional write refused with ConditionalCheckFailedException is still billed exactly as the same write applied is not observable.

AWS: Smithy: PutItem models ConditionalCheckFailedException (client, HTTP 400) and PutItem's output carries ConsumedCapacity only when ReturnConsumedCapacity is requested; AWS's message for that error is the fixed text "The conditional request failed" (no variable parts) and the refused call returns NO ConsumedCapacity member in band. Out of band, AWS meters the refused write: the table's AWS/DynamoDB ConsumedWriteCapacityUnits series (dimension TableName) accumulates the same units a successful write of the same item would cost, so a table that lost 10 conditional races and a twin that won 10 identical writes both report seed+10 = 11 units over Period=60, Statistic=Sum.

Emulator: the check exhausts its budget and is reported as `Error: Check timed out after 60.0s`. Its only polled effect is GetMetricStatistics(Namespace=AWS/DynamoDB, MetricName=ConsumedWriteCapacityUnits, Dimensions=[TableName], Period=60, Statistics=[Sum]) reaching the in-band reported total, first for the twin table and then for the subject table, so the timeout is reached with that sum still short of target. Hypothesis: no AWS/DynamoDB per-table capacity series is published at all — a sibling check in this same service reports `AWS/DynamoDB SuccessfulRequestLatency has no datapoints under (TableName, Operation='PutItem')`, and the open issues below report the same absence for other AWS/DynamoDB metrics.

Why: a client cannot observe or alarm on write-capacity consumption. Capacity dashboards read zero, provisioned-capacity alarms never breach, and the AWS-specific cost semantics of a lost optimistic-concurrency race (refused, nothing changed, still billed) are invisible — code tuned against the emulator will under-provision and mis-attribute cost.

Fix: the DynamoDB write path's CloudWatch metric publication (the component that meters consumed capacity per table and publishes AWS/DynamoDB series), including the refused-conditional-write path; not the check, and not PutItem's in-band response, which must keep omitting ConsumedCapacity on refusal.

Related open issues that do NOT cover this check (the suite did not mark it owned, so it is filed separately): 20261102T120041Z-cloudwatch-dynamodb-dynamodb-publishes-no-aws-dynamodb-cloudwatch.md and 20261111T064629Z-cloudwatch-dynamodb-aws-dynamodb-successfulrequestlatency-is-not-published-under-the.md.

Traces: cloudwatch__dynamodb.conc-ddb-optimistic-refused-conditional-write-still-billed-rejected.

## Evidence
Expected: two PAY_PER_REQUEST tables, each seeded with one item (1 WCU). Subject: 10 PutItems with a stale ConditionExpression are each refused 400 ConditionalCheckFailedException with message "The conditional request failed" and no ConsumedCapacity member, and the item is unchanged. Twin: 10 identical writes with the correct version apply and report 10 units in band. Then AWS/DynamoDB ConsumedWriteCapacityUnits (TableName dimension, Sum) reaches 11 for the twin AND 11 for the subject — a lost race costs the same as a won one.

Actual: `[mismatch] cloudwatch__dynamodb.conc-ddb-optimistic-refused-conditional-write-still-billed-rejected` → `Error: Check timed out after 60.0s`; the polled ConsumedWriteCapacityUnits sum never reaches the in-band total, so neither the twin equality nor the subject==twin billing equality is ever evaluated.

## Steps to Reproduce
1. CreateTable twice (pk HASH, BillingMode=PAY_PER_REQUEST); wait for ACTIVE.
2. PutItem {pk:"k", version:0} into both.
3. Against table A, 10× PutItem {pk:"k", version:100+i} with ConditionExpression "version = :stale", :stale=99, ReturnConsumedCapacity=TOTAL → each must be 400 ConditionalCheckFailedException, message "The conditional request failed", no ConsumedCapacity in the response.
4. Against table B, 10× the same write with the correct version → all apply, 10 units reported in band.
5. GetMetricStatistics(AWS/DynamoDB, ConsumedWriteCapacityUnits, Dimensions=[{TableName:A}] and [{TableName:B}], Period=60, Statistics=[Sum]).
Observed: the sums never reach 11 (no datapoints). Expected: 11 for both tables.
