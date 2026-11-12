Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-create-table-table-throughput-mode-member
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-create-table-table-throughput-mode-member dynamodb-csvtraj-single-column-csv-hash-reused-as-range dynamodb-update-table-throughput-mode-provisioned-round-trip dynamodb-csvtraj-gsi-added-after-create-via-update-table
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|CreateTable|2xx->4xx|$.TableDescription
Issue-Key: dynamodb|CreateTable|2xx->4xx|status
Issue-Key: dynamodb|DescribeTable|2xx->4xx|$
Issue-Key: dynamodb|DescribeTable|2xx->4xx|$.Table
Issue-Key: dynamodb|DescribeTable|2xx->4xx|$.Table.TableStatus
Issue-Key: dynamodb|DescribeTable|2xx->4xx|status
Issue-Key: dynamodb|UpdateTable|2xx->4xx|$.TableDescription
Issue-Key: dynamodb|UpdateTable|2xx->4xx|status
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.BillingModeSummary.BillingMode
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.ProvisionedThroughput.ReadCapacityUnits
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.ProvisionedThroughput.WriteCapacityUnits
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.TableThroughputModeSummary.TableThroughputMode
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.GlobalSecondaryIndexes
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.TableStatus
Issue-Key: dynamodb|DeleteTable|2xx->4xx|status
Issue-Key: dynamodb|DescribeTable|4xx->2xx|$
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.AttributeDefinitions[*].AttributeName
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.GlobalSecondaryIndexes[*].KeySchema[*].AttributeName
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.KeySchema[*].AttributeName
Issue-Related: 20261104T061030Z-dynamodb-invalid-dynamodb-requests-are-accepted-and-answered-200.md
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# DynamoDB CreateTable and UpdateTable drop the throughput-mode member and leave tables stuck

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Eight net-new root causes in the table control plane: the read/write throughput-mode member is refused outright, and table and index states do not settle, which in turn blocks DeleteTable and leaves a deleted table readable. Each is a separate fix.

Cause 1 of 8 — CreateTable refuses a request carrying the table read/write throughput-mode member.
AWS: Smithy models the response side of this feature — TableDescription.TableThroughputModeSummary -> TableThroughputModeSummary {TableThroughputMode: BillingMode enum (PROVISIONED|PAY_PER_REQUEST), LastUpdateToPayPerRequestDateTime}. The corresponding request member is not present in this checkout's CreateTableInput, so for the request shape this is recorded behavior only: the recording shows AWS answering 200 with a TableDescription whose TableThroughputModeSummary.TableThroughputMode echoes the requested mode and whose BillingModeSummary.BillingMode matches it.
Emulator: 400 and no TableDescription — the member is rejected, so the table is never created.
Why: every client that sets the throughput mode at creation time fails at the first call; nothing downstream in the trajectory can run.
Fix: CreateTable input parsing/validation (accept the throughput-mode member) and the TableDescription serializer (emit TableThroughputModeSummary).
Traces: dynamodb-create-table-table-throughput-mode-member#task#1 (also dynamodb-csvtraj-single-column-csv-hash-reused-as-range#task#4).

Cause 2 of 8 — DescribeTable cannot see the table from cause 1.
AWS: once created, DescribeTable answers 200 with TableStatus ACTIVE and the table's TableThroughputModeSummary and WarmThroughput.
Emulator: 400 ResourceNotFoundException 'Requested resource not found: Table: <name> not found' for the whole 60 s poll window.
Why: a create-then-wait loop never terminates.
Fix: follows cause 1 (the create must land); DescribeTable's TableDescription serializer must then carry TableThroughputModeSummary.
Traces: dynamodb-create-table-table-throughput-mode-member#task#2.

Cause 3 of 8 — UpdateTable refuses a switch to PROVISIONED expressed with the throughput-mode member.
AWS: 200 with a TableDescription whose BillingModeSummary.BillingMode and TableThroughputModeSummary.TableThroughputMode are PROVISIONED, whose ProvisionedThroughput carries the requested read/write capacity units and a LastIncreaseDateTime, and whose TableStatus is UPDATING (recorded behavior for the request member, as in cause 1).
Emulator: 400, no TableDescription.
Why: a client cannot move a table between capacity modes at all.
Fix: UpdateTable input parsing/validation plus its TableDescription serializer.
Traces: dynamodb-update-table-throughput-mode-provisioned-round-trip#task#4 (also #task#6).

Cause 4 of 8 — DescribeTable does not reflect a completed switch to PROVISIONED.
AWS: after the update settles, BillingModeSummary.BillingMode and TableThroughputModeSummary.TableThroughputMode are PROVISIONED and ProvisionedThroughput.ReadCapacityUnits/WriteCapacityUnits equal the values the update requested.
Emulator: still PAY_PER_REQUEST with zero read and write capacity units.
Why: capacity-mode state is unobservable, so any assertion or autoscaling decision based on it is wrong.
Fix: table state (persist the throughput mode and provisioned units on update) and the DescribeTable serializer.
Traces: dynamodb-update-table-throughput-mode-provisioned-round-trip#task#5.

Cause 5 of 8 — DescribeTable reports a table and index stuck mid-transition.
AWS: after the GSI-creating UpdateTable is refused (the AttributeDefinitions rule — see the companion issue on requests accepted with 200), the table stays TableStatus ACTIVE with no GlobalSecondaryIndexes.
Emulator: TableStatus UPDATING with a GlobalSecondaryIndexes entry whose IndexStatus is CREATING, and neither settles.
Why: a client that waits for ACTIVE before writing hangs; an index exists that AWS never created.
Fix: the table/index state machine — transitions must settle to a terminal state, and a refused update must leave no index behind.
Traces: dynamodb-csvtraj-gsi-added-after-create-via-update-table#task#6.

Cause 6 of 8 — DeleteTable is refused while the table sits in that non-settling state.
AWS: 200 — the delete is accepted.
Emulator: 400.
Why: teardown is impossible, so the table leaks and every later fixture sharing the name collides.
Fix: DeleteTable's state guard (and cause 5, which put the table in the blocking state).
Traces: dynamodb-csvtraj-gsi-added-after-create-via-update-table#teardown#7.

Cause 7 of 8 — DescribeTable keeps answering 200 for a deleted table.
AWS: after the delete completes, 400 ResourceNotFoundException 'Requested resource not found: Table: <name> not found'.
Emulator: 200 with a full ACTIVE Table description for the entire 60 s poll window.
Why: a delete-then-wait-for-absence loop never terminates and stale state is served as live.
Fix: DeleteTable state transition to DELETING and removal, plus DescribeTable's lookup.
Traces: dynamodb-csvtraj-gsi-added-after-create-via-update-table#teardown#8.

Cause 8 of 8 — DescribeTable reports key attribute names that the created table does not have.
AWS: AttributeDefinitions, KeySchema and each GlobalSecondaryIndexes[].KeySchema name exactly the attributes the successful CreateTable declared.
Emulator: the sort-key attribute name in AttributeDefinitions[0].AttributeName, KeySchema[1].AttributeName and the GSI key schema is a different attribute than the one created — the stored schema does not match the accepted request (hypothesis: an earlier, refused CreateTable attempt for the same table name left its schema in state and it is served instead).
Why: reads and writes against the declared sort key fail with key-schema errors even though the create succeeded.
Fix: CreateTable state — store the accepted request's schema and never retain a refused attempt's.
Traces: dynamodb-csvtraj-single-column-csv-hash-reused-as-range#task#5.

## Evidence
Replay of simparity-wire-aws-v1, expected -> actual:
1. CreateTable dynamodb-create-table-table-throughput-mode-member#task#1: 200 + TableDescription incl. TableThroughputModeSummary {TableThroughputMode: PAY_PER_REQUEST} -> 400, $.TableDescription missing (2 steps).
2. DescribeTable dynamodb-create-table-table-throughput-mode-member#task#2: 200 + Table ACTIVE incl. TableThroughputModeSummary and WarmThroughput -> 400 + com.amazonaws.dynamodb.v20120810#ResourceNotFoundException 'Requested resource not found: Table: <name> not found' (poll capped at 60 s).
3. UpdateTable dynamodb-update-table-throughput-mode-provisioned-round-trip#task#4: 200 + TableDescription (BillingMode PROVISIONED, TableThroughputMode PROVISIONED, requested capacity units, TableStatus UPDATING) -> 400, $.TableDescription missing (2 steps).
4. DescribeTable dynamodb-update-table-throughput-mode-provisioned-round-trip#task#5: BillingModeSummary.BillingMode PROVISIONED / TableThroughputMode PROVISIONED / ReadCapacityUnits and WriteCapacityUnits as requested -> PAY_PER_REQUEST / PAY_PER_REQUEST / 0 / 0.
5. DescribeTable dynamodb-csvtraj-gsi-added-after-create-via-update-table#task#6: Table.TableStatus ACTIVE and no GlobalSecondaryIndexes -> TableStatus UPDATING with one GSI at IndexStatus CREATING.
6. DeleteTable dynamodb-csvtraj-gsi-added-after-create-via-update-table#teardown#7: 200 -> 400.
7. DescribeTable dynamodb-csvtraj-gsi-added-after-create-via-update-table#teardown#8: 400 + ResourceNotFoundException -> 200 with the table still ACTIVE (poll capped at 60 s).
8. DescribeTable dynamodb-csvtraj-single-column-csv-hash-reused-as-range#task#5: AttributeDefinitions[0].AttributeName / KeySchema[1].AttributeName / GSI KeySchema[0].AttributeName = the declared sort-key attribute -> a different attribute name.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Representative: replay fixture dynamodb-create-table-table-throughput-mode-member, step task#1 — CreateTable carrying the table throughput-mode member; the emulator answers 400 where the recording has 200 with a TableDescription containing TableThroughputModeSummary.
3. For each remaining cause replay the fixture#step in trace_ids: the DescribeTable poll for that table, the UpdateTable switch to PROVISIONED and its DescribeTable round trip, then dynamodb-csvtraj-gsi-added-after-create-via-update-table task#6 / teardown#7 / teardown#8 for the non-settling table and index states and the blocked delete, and dynamodb-csvtraj-single-column-csv-hash-reused-as-range#task#5 for the mismatched key schema.
