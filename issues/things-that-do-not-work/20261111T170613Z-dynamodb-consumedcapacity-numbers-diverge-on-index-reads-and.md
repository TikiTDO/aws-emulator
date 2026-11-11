Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.lsi-non-projected-query-charges-base-item-on-top-of-the-keys-only-index-read dynamodb.transact-write-items-token-replay-bills-exact-capacity-for-item-size dynamodb.put-item-fractional-and-negative-hash-key-omits-consumed-capacity dynamodb.batch-write-item-and-delete-item-index-capacity-arms-follow-item-estate dynamodb.update-item-index-capacity-ladder-follows-key-vs-projected-change dynamodb.hot-partition-key-throttled-while-distributed-keys-at-same-rate-rejected dynamodb.validation-messages-are-customer-intelligible
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.lsi-non-projected-query-charges-base-item-on-top-of-the-keys-only-index-read|run#21|AssertionError|assert fetched_cc['Table']['CapacityUnits'] == _BASE_RCU, (
Issue-Key: dynamodb.transact-write-items-token-replay-bills-exact-capacity-for-item-size|run#16|AssertionError|assert replay.get('ReadCapacityUnits') == 2.0, (
Issue-Key: dynamodb.put-item-fractional-and-negative-hash-key-omits-consumed-capacity|run#12|AssertionError|assert put_cc == {'TableName': t, 'CapacityUnits': 1.0}, (
Issue-Key: dynamodb.batch-write-item-and-delete-item-index-capacity-arms-follow-item-estate|_wait_active#6|AssertionError|raise AssertionError(f"table {table} did not reach ACTIVE with its GSI before the deadline")
Issue-Key: dynamodb.update-item-index-capacity-ladder-follows-key-vs-projected-change|_wait_active#6|AssertionError|raise AssertionError(f"table {table} did not reach ACTIVE with its GSI before the deadline")
Issue-Key: dynamodb.hot-partition-key-throttled-while-distributed-keys-at-same-rate-rejected|_paced_arm#16|Timeout|f.result()
Issue-Key: dynamodb.validation-messages-are-customer-intelligible|_error#7|AssertionError|raise AssertionError(f'real AWS rejects this call, but it SUCCEEDED and returned {result!r}')
Issue-Commit: cdd06381ed07ce191d4321618e22d6aefb363f32

# dynamodb: ConsumedCapacity numbers diverge on index reads and transaction replays, GSIs never reach ACTIVE, and a hot partition is never throttled (6 causes)

> _Note: this issue was identified against the code base at commit `cdd06381ed07ce191d4321618e22d6aefb363f32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Query

## Description
Six net-new root causes in capacity accounting and admission control. Smithy: ConsumedCapacity is returned by the item and query operations (ConsumedCapacityMultiple for the batch/transaction ones) and ProvisionedThroughputExceededException is the modeled 400 refusal for a table over its provisioned rate; the numbers themselves are DynamoDB's documented per-operation accounting, so the recorded values are the contract.

Cause 1 of 6 — an LSI query over a KEYS_ONLY index that must fetch non-projected attributes from the base table does not charge the base-table fetch. AWS charges the index read on the Index arm AND the base-item fetch on the Table arm (an 8019-byte item is 2 RCU of eventually-consistent read, rounded up per 4 KB). Emulator: the Table arm reads {'ReadCapacityUnits': 0.0, 'CapacityUnits': 0.0}. Why: a client sizing provisioned capacity from reported units under-provisions every non-projected index read. Fix: capacity accounting in the index-read/table-fetch path. Traces: lsi-non-projected-query-charges-base-item-on-top-of-the-keys-only-index-read.

Cause 2 of 6 — a same-ClientRequestToken replay of a TransactWriteItems bills the wrong capacity. AWS bills the replay exactly the capacity the original call cost for that item size (2.0 here); the emulator billed 4.0. Fix: the idempotency-replay path's capacity accounting. Traces: transact-write-items-token-replay-bills-exact-capacity-for-item-size.

Cause 3 of 6 — PutItem's ConsumedCapacity is wrong for a fractional/negative numeric hash key: the emulator returns a ConsumedCapacity block where AWS's accounting for this shape differs. Fix: PutItem capacity accounting. Traces: put-item-fractional-and-negative-hash-key-omits-consumed-capacity.

Cause 4 of 6 — a table created with a GSI never reaches ACTIVE with its index inside the deadline, so no caller can observe per-index capacity arms at all. The index-capacity checks fail at setup, before any assertion about the numbers. Why: every INDEXES-scoped ConsumedCapacity behaviour is unreachable, and a client that waits on table_exists/GSI ACTIVE hangs. Fix: the GSI backfill/state machine in CreateTable/UpdateTable state. Traces: batch-write-item-and-delete-item-index-capacity-arms-follow-item-estate, update-item-index-capacity-ladder-follows-key-vs-projected-change.

Cause 5 of 6 — a hot partition key driven at a rate AWS throttles is never throttled: the probe never sees a ProvisionedThroughputExceededException and the check exhausts its 60 s budget. AWS refuses writes to a single partition past its per-partition limit while identically-rated writes spread over many keys succeed. Fix: per-partition admission control in the write path. Traces: hot-partition-key-throttled-while-distributed-keys-at-same-rate-rejected.

Cause 6 of 6 — a BatchWriteItem request AWS refuses as unintelligible is accepted and answered 200 with UnprocessedItems {}. Fix: BatchWriteItem request validation. Traces: validation-messages-are-customer-intelligible.

## Evidence
Expected vs actual, verbatim from the replay:
- lsi-non-projected-query-charges-base-item-on-top-of-the-keys-only-index-read: 'the base-table fetch of a 8019-byte item must bill exactly 2 RCU on the Table arm, got {'ReadCapacityUnits': 0.0, 'CapacityUnits': 0.0}'.
- transact-write-items-token-replay-bills-exact-capacity-for-item-size: 'a same-token replay of a ~1.5KB TransactWriteItems billed ReadCapacityUnits 4.0, expected 2.0'.
- put-item-fractional-and-negative-hash-key-omits-consumed-capacity: 'PutItem of a small item with ReturnConsumedCapacity=TOTAL must charge 1.0 unit, got {'TableName': ..., 'CapacityUnits': 1.0, 'WriteCapacityUnits': 1.0}' — the ConsumedCapacity block the emulator returns is not the one AWS returns for this key shape.
- batch-write-item-and-delete-item-index-capacity-arms-follow-item-estate / update-item-index-capacity-ladder-follows-key-vs-projected-change: 'table <name> did not reach ACTIVE with its GSI before the deadline'.
- hot-partition-key-throttled-while-distributed-keys-at-same-rate-rejected: 'Check timed out after 60.0s' — no refusal was ever produced.
- validation-messages-are-customer-intelligible: 'real AWS rejects this call, but it SUCCEEDED and returned {'UnprocessedItems': {}, ...}' (HTTP 200).
(Item sizes, table names and the 60 s budget are this run's values; the per-4 KB / per-1 KB rounding, the replay-equals-original rule and the per-partition limit are the requirement.)

## Steps to Reproduce
1. Create a table with a KEYS_ONLY LSI; put an ~8 KB item; Query the LSI selecting a non-projected attribute with ReturnConsumedCapacity='INDEXES' — expect 2 RCU on the Table arm.
2. TransactWriteItems a ~1.5 KB Put with a ClientRequestToken; repeat the identical call with the same token — expect the replay's ConsumedCapacity to equal the original's.
3. PutItem with a fractional and a negative numeric hash key, ReturnConsumedCapacity='TOTAL'; compare the ConsumedCapacity block.
4. CreateTable with a GlobalSecondaryIndex and wait for TableStatus ACTIVE and IndexStatus ACTIVE — expect it to settle.
5. Drive many concurrent writes at one partition key on a provisioned table and the same rate spread over many keys — expect ProvisionedThroughputExceededException only for the hot key.
6. BatchWriteItem the malformed request from validation-messages-are-customer-intelligible — expect a ValidationException, not 200.
