Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb.metrics-dynamodb-gsi-consumption-under-index-dimension-not-folded-into-table
Issue-Key: cloudwatch__dynamodb.metrics-dynamodb-gsi-consumption-under-index-dimension-not-folded-into-table|_await_exact#24|AssertionError|raise AssertionError(
Issue-Commit: 40434d85add499d02eca6ac15c85c4cfa7b8d96f

# cloudwatch__dynamodb: GSI consumption is never published under {TableName, GlobalSecondaryIndexName}, so per-index capacity accounting is unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `40434d85add499d02eca6ac15c85c4cfa7b8d96f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: a table carrying a global secondary index produces no AWS/DynamoDB per-index CloudWatch series, and no base-table series either, so the split between base-table and index consumption cannot be read.

AWS: recorded behavior only — metric emission is not expressible in the Smithy model, so the requirement is taken from the recording. On real AWS, writes and index-served reads on a table with a GSI are published into AWS/DynamoDB on TWO distinct dimension sets, and each set carries exactly the units the data plane itemised for it in ConsumedCapacity with ReturnConsumedCapacity='INDEXES':
  * {TableName} carries ONLY the base-table portion (the `ConsumedCapacity.Table.CapacityUnits` of every request), never the folded total.
  * {TableName, GlobalSecondaryIndexName} carries the index portion (the `ConsumedCapacity.GlobalSecondaryIndexes.<IndexName>.CapacityUnits` of every request).
  * A write that does not carry the index key adds to {TableName} only and contributes nothing to the index series; a Query served by the index bills 0.0 on the base table and its RCU on the index series.
  * {GlobalSecondaryIndexName} WITHOUT TableName is not a dimension set DynamoDB publishes, so a read of it must return no datapoints.
The recording shows these datapoints readable through GetMetricStatistics(Namespace='AWS/DynamoDB', MetricName='ConsumedWriteCapacityUnits'|'ConsumedReadCapacityUnits', Period=60, Statistics=['Sum']) within about a minute of the traffic, with the Sum over the window equal to the in-band itemised totals.

Emulator: the data plane itemises correctly in band (the check's own assertions on ConsumedCapacity.Table / .GlobalSecondaryIndexes all passed: base WCU 6.0, index WCU 4.0, base RCU 1.0, index RCU 0.5), but every CloudWatch series stays empty for the whole polling window — the final reading is {'base WCU': None, 'gsi WCU': None, 'base RCU': None, 'gsi RCU': None, 'index-name-only WCU': None}, i.e. GetMetricStatistics answers 200 with an empty Datapoints list for both {TableName} and {TableName, GlobalSecondaryIndexName}. No HTTP status or error code diverges here; the divergence is the permanently empty series.

Why: a client cannot tell base-table consumption from index consumption, so per-index capacity dashboards, GSI throttle/consumption alarms and any reconciliation of in-band ConsumedCapacity against CloudWatch are impossible, and an empty series is indistinguishable from "no traffic". It also hides the folding bug class entirely: a client cannot observe whether the index units were wrongly folded into the {TableName} series, because neither series exists.

Fix: the DynamoDB data-plane operations' metric emission into the shared CloudWatch metric store — PutItem/GetItem/Query must publish AWS/DynamoDB ConsumedWriteCapacityUnits and ConsumedReadCapacityUnits datapoints on the {TableName} dimension set (base-table portion only) AND on the {TableName, GlobalSecondaryIndexName} dimension set (index portion only), and must publish nothing on a {GlobalSecondaryIndexName}-only dimension set. CloudWatch's own read path is not at fault; GetMetricStatistics serves other namespaces correctly.

Related open issue that does NOT carry this check's replay key and therefore does not cover it: 20261102T120041Z-cloudwatch-dynamodb-dynamodb-publishes-no-aws-dynamodb-cloudwatch.md describes the same emission gap for the {TableName} and {TableName, Operation} dimension sets but never mentions the per-index {TableName, GlobalSecondaryIndexName} set; a fixer should merge this cause with it.

Traces: cloudwatch__dynamodb.metrics-dynamodb-gsi-consumption-under-index-dimension-not-folded-into-table.

## Evidence
Replayed against the emulator (suite service heading `### cloudwatch__dynamodb`).

Failing check cloudwatch__dynamodb.metrics-dynamodb-gsi-consumption-under-index-dimension-not-folded-into-table, verdict mismatch:

  AssertionError: AWS/DynamoDB per-index accounting for '<table>': API itemised base WCU 6.0, by-group WCU 4.0 (TOTAL 10.0 -- the value a FOLDED base series would show), base RCU 1.0, by-group RCU 0.5 over 1 Queries: never equalled {'base WCU': 6.0, 'gsi WCU': 4.0, 'base RCU': 1.0, 'gsi RCU': 0.5, 'index-name-only WCU': None} within 52s; last reading {'base WCU': None, 'gsi WCU': None, 'base RCU': None, 'gsi RCU': None, 'index-name-only WCU': None}

Expected (per the recording): Sum over the window of
  ConsumedWriteCapacityUnits, Dimensions=[{TableName: <table>}]                                  -> the base-table itemised total (here 6.0 = 4 projected + 2 sparse writes)
  ConsumedWriteCapacityUnits, Dimensions=[{TableName: <table>}, {GlobalSecondaryIndexName: <idx>}] -> the index itemised total (here 4.0, the projected writes only)
  ConsumedReadCapacityUnits,  Dimensions=[{TableName: <table>}]                                  -> the base-table read total (here 1.0 = 2 eventual GetItems)
  ConsumedReadCapacityUnits,  Dimensions=[{TableName: <table>}, {GlobalSecondaryIndexName: <idx>}] -> the index read total (here 0.5 for the one GSI Query)
  ConsumedWriteCapacityUnits, Dimensions=[{GlobalSecondaryIndexName: <idx>}]                     -> no datapoints

Actual: all five reads return no datapoints (None) for the entire 52 s poll window.

The numbers above are this run's values filling the template; the requirement is that each series' Sum equals the units the data plane itemised for that series in the same run, and that the base series is NOT the folded total (10.0 here).

The in-band plane is correct in this run: a projected PutItem itemised Table=1.0 and <idx>=1.0, a sparse PutItem itemised Table=1.0 with no GlobalSecondaryIndexes member, and a Query served by the index itemised Table=0.0 — all of those assertions passed before the CloudWatch read.

## Steps to Reproduce
1. CreateTable with BillingMode='PAY_PER_REQUEST', key schema pk (HASH), and one GlobalSecondaryIndex named e.g. 'by-group' on gk (HASH) with Projection ALL; wait for ACTIVE.
2. PutItem 4 items carrying both pk and gk with ReturnConsumedCapacity='INDEXES' — each itemises ConsumedCapacity.Table.CapacityUnits=1.0 and GlobalSecondaryIndexes['by-group'].CapacityUnits=1.0.
3. PutItem 2 items carrying pk but NOT gk with ReturnConsumedCapacity='INDEXES' — each itemises Table=1.0 and no GlobalSecondaryIndexes member.
4. GetItem 2 of the projected items (eventually consistent, ReturnConsumedCapacity='INDEXES') — 0.5 RCU each on the base table.
5. Query the table with IndexName='by-group' and ReturnConsumedCapacity='INDEXES' until the index shows the 4 projected items — Table=0.0, index RCU > 0.
6. Poll cloudwatch.get_metric_statistics(Namespace='AWS/DynamoDB', MetricName='ConsumedWriteCapacityUnits', Dimensions=[{'Name':'TableName','Value':<table>}], Period=60, Statistics=['Sum'], StartTime=<2 min before step 2>, EndTime=<now+5 min>) — expect Sum == the base-table itemised total from steps 2-3 (6.0), observe Datapoints == [].
7. Repeat step 6 with Dimensions=[{'Name':'TableName','Value':<table>},{'Name':'GlobalSecondaryIndexName','Value':'by-group'}] — expect Sum == the index itemised total (4.0), observe Datapoints == [].
8. Repeat steps 6-7 for MetricName='ConsumedReadCapacityUnits' — expect the base-table RCU (1.0) and the index RCU from step 5, observe Datapoints == [] for both.
9. Read ConsumedWriteCapacityUnits with Dimensions=[{'Name':'GlobalSecondaryIndexName','Value':'by-group'}] alone — expect no datapoints (this correctly reads empty today, but only because nothing is published at all).
