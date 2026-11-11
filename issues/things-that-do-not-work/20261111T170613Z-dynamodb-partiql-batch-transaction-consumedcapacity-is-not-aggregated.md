Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.batch-execute-statement-consistent-read-is-billed-per-member dynamodb.execute-transaction-read-response-shapes-and-write-consumed-capacity dynamodb.execute-transaction-read-responses-three-way-shape-and-write-consumed-capacity dynamodb.execute-transaction-duplicate-insert-rolls-back-sibling-insert dynamodb.execute-transaction-mixed-read-and-write-statements-rejected dynamodb.partiql-select-index-qualified-capacity-table-and-index-arms
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.batch-execute-statement-consistent-read-is-billed-per-member|run#26|AssertionError|assert isinstance(mixed_cc, list) and len(mixed_cc) == 1, (
Issue-Key: dynamodb.execute-transaction-read-response-shapes-and-write-consumed-capacity|run#14|AssertionError|assert len(consumed) == 1, f"expected exactly one ConsumedCapacity entry, got {consumed!r}"
Issue-Key: dynamodb.execute-transaction-read-responses-three-way-shape-and-write-consumed-capacity|run#19|AssertionError|assert len(consumed) == 1, f"expected exactly one ConsumedCapacity entry, got {len(consumed)}: {consumed!r}"
Issue-Key: dynamodb.execute-transaction-duplicate-insert-rolls-back-sibling-insert|run#15|AssertionError|assert err['Error'].get('Message') == expected_message, (
Issue-Key: dynamodb.execute-transaction-mixed-read-and-write-statements-rejected|run#10|AssertionError|assert message == expected_message, f"unexpected message: {message!r}"
Issue-Key: dynamodb.partiql-select-index-qualified-capacity-table-and-index-arms|run#10|AssertionError|assert cc['Table']['CapacityUnits'] == 0, (
Issue-Commit: cdd06381ed07ce191d4321618e22d6aefb363f32

# dynamodb: PartiQL batch/transaction ConsumedCapacity is not aggregated per table, and transaction cancellation reasons diverge (5 causes)

> _Note: this issue was identified against the code base at commit `cdd06381ed07ce191d4321618e22d6aefb363f32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteTransaction

## Description
Five net-new root causes in how the PartiQL batch and transaction paths report consumed capacity and cancellation. Smithy: BatchExecuteStatement.ConsumedCapacity, ExecuteTransaction.ConsumedCapacity and TransactGetItems.ConsumedCapacity are all ConsumedCapacityMultiple — a list with ONE entry per distinct table touched, whose CapacityUnits is the SUM over the statements that touched it, not one entry per statement. ExecuteTransaction's modeled failure for a rejected member is TransactionCanceledException (400), whose message is 'Transaction cancelled, please refer cancellation reasons for specific reasons [<reason>, <reason>, ...]' — the bracketed list is one reason code per statement slot, in request order, and is the only variable part.

Cause 1 of 5 — BatchExecuteStatement emits one ConsumedCapacity entry per statement. AWS: two statements against the same table produce ONE entry whose CapacityUnits is their sum (a ConsistentRead=true 1-item SELECT costs 1.0, an eventual one 0.5, so a mixed batch sums to 1.5). Emulator: two entries of 1.0 and 0.5 for the same table. Why: a client totalling CapacityUnits per table double-counts tables and mis-sizes its own throttling. Fix: the BatchExecuteStatement handler's capacity aggregation. Traces: batch-execute-statement-consistent-read-is-billed-per-member.

Cause 2 of 5 — ExecuteTransaction emits one ConsumedCapacity entry per statement, same defect, same table. Fix: the ExecuteTransaction handler's capacity aggregation. Traces: execute-transaction-read-response-shapes-and-write-consumed-capacity, execute-transaction-read-responses-three-way-shape-and-write-consumed-capacity.

Cause 3 of 5 — a duplicate INSERT inside an ExecuteTransaction reports the wrong cancellation reason. AWS: the slot that duplicates a key reports DuplicateItem, so the message's bracket list reads [DuplicateItem, None] and the sibling INSERT rolls back. Emulator reports [ConditionalCheckFailed, None]. Why: a client distinguishing a key collision from a failed condition takes the wrong branch (retry vs abort). Fix: the ExecuteTransaction cancellation-reason classifier. Traces: execute-transaction-duplicate-insert-rolls-back-sibling-insert.

Cause 4 of 5 — the mixed read/write transaction refusal is emulator-authored. AWS refuses a transaction mixing read and write statements with its own fixed ValidationException text; the emulator serves 'The entire transaction must consist of either read statements or write statements, you cannot mix both in one transaction'. Fix: ExecuteTransaction request validation's message. Traces: execute-transaction-mixed-read-and-write-statements-rejected.

Cause 5 of 5 — an index-qualified PartiQL SELECT charges the base-table arm. AWS: a keyed SELECT through a GSI charges the Index arm only; the Table arm is zero/absent. Emulator charges the Table arm (0.5 RCU here) as well. Why: per-table capacity reported to a caller is inflated for every index read. Fix: capacity accounting in the PartiQL SELECT read path. Traces: partiql-select-index-qualified-capacity-table-and-index-arms.

## Evidence
Expected vs actual, verbatim from the replay:
- batch-execute-statement-consistent-read-is-billed-per-member: 'both statements read the same table, expected one entry, got [{'TableName': 'sim2real-behav-46287f276f3c', 'CapacityUnits': 1.0, 'ReadCapacityUnits': 1.0}, {'TableName': 'sim2real-behav-46287f276f3c', 'CapacityUnits': 0.5, 'ReadCapacityUnits': 0.5}]' — AWS would return one entry with CapacityUnits 1.5 (the table name is this run's value).
- execute-transaction-read-response-shapes-and-write-consumed-capacity: 'expected exactly one ConsumedCapacity entry, got [{...CapacityUnits 2.0...}, {...CapacityUnits 2.0...}]' for a single table.
- execute-transaction-read-responses-three-way-shape-and-write-consumed-capacity: 'expected exactly one ConsumedCapacity entry, got 2'.
- execute-transaction-duplicate-insert-rolls-back-sibling-insert: expected message 'Transaction cancelled, please refer cancellation reasons for specific reasons [DuplicateItem, None]', got '... [ConditionalCheckFailed, None]'.
- execute-transaction-mixed-read-and-write-statements-rejected: unexpected message 'The entire transaction must consist of either read statements or write statements, you cannot mix both in one transaction'.
- partiql-select-index-qualified-capacity-table-and-index-arms: 'a keyed GSI SELECT must charge the Table arm nothing, got {'ReadCapacityUnits': 0.5, 'CapacityUnits': 0.5}'.

## Steps to Reproduce
1. Create a PAY_PER_REQUEST table with a GSI and seed two items.
2. BatchExecuteStatement two SELECTs against that one table, one with ConsistentRead=true and one with ConsistentRead=false, ReturnConsumedCapacity='TOTAL'; count ConsumedCapacity entries and check the single entry sums to 1.5.
3. ExecuteTransaction two write statements against that one table with ReturnConsumedCapacity='TOTAL'; count ConsumedCapacity entries.
4. ExecuteTransaction two INSERTs where the second duplicates the first's key; read the TransactionCanceledException message's bracketed reason list.
5. ExecuteTransaction one SELECT plus one INSERT; read the ValidationException message.
6. ExecuteStatement a keyed SELECT qualified by the GSI with ReturnConsumedCapacity='INDEXES'; check the Table arm is not charged.
