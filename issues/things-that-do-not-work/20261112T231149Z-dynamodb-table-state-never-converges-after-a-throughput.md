Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected dynamodb-create-table-secondary-index-key-schema-structure
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|UpdateTable|2xx->4xx|$.TableDescription
Issue-Key: dynamodb|UpdateTable|2xx->4xx|status
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.ProvisionedThroughput.ReadCapacityUnits
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.TableStatus
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.ProvisionedThroughput.WriteCapacityUnits
Issue-Key: dynamodb|CreateTable|2xx->4xx|$.TableDescription
Issue-Key: dynamodb|CreateTable|2xx->4xx|status
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.AttributeDefinitions[*].AttributeName
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.GlobalSecondaryIndexes
Issue-Related: 20261109T055139Z-dynamodb-throughput-and-capacity-reporting-diverges-across-updatetable.md
Issue-Related: 20261111T221143Z-dynamodb-createtable-and-updatetable-accept-index-and-throughput.md
Issue-Related: 20261111T221143Z-dynamodb-table-and-stream-timestamps-are-serialized-with.md
Issue-Commit: c3c8a5d5636f74138d44709d078c46bc0a5a7c2e

# dynamodb: table state never converges after a throughput or index definition AWS refuses (5 causes)

> _Note: this issue was identified against the code base at commit `c3c8a5d5636f74138d44709d078c46bc0a5a7c2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTable

## Description
Five net-new causes in the DynamoDB control plane. They share one fault: when a CreateTable/UpdateTable request that AWS REFUSES is instead applied, the emulator's table state is permanently poisoned — the out-of-range or malformed definition is persisted, the table is left in a non-terminal status, and every LATER, well-formed control-plane call on that table name is then answered wrongly (a valid UpdateTable/CreateTable is refused 400, and DescribeTable serves the definition AWS never created). The acceptance of the refused definition itself is already tracked by open issue 20261111T221143Z-dynamodb-createtable-and-updatetable-accept-index-and-throughput.md; that issue does NOT carry the replay keys below, and none of these five steps is marked owned by it, so the convergence failures are filed here and must be closed in their own right: after the input validation is fixed, the state machine must also return the table to ACTIVE and leave the name free.

Cause 1 of 5: UpdateTable persists a ProvisionedThroughput.ReadCapacityUnits value far beyond the table quota, and the table never leaves UPDATING.
AWS: Smithy: UpdateTable.ProvisionedThroughput -> ProvisionedThroughput, whose ReadCapacityUnits/WriteCapacityUnits are PositiveLongObject with @range min 1 and whose documentation defers the maximum to "Service, Account, and Table Quotas"; an above-quota request is refused with ValidationException (HTTP 400) and leaves the table unchanged. The recording shows AWS keeping ReadCapacityUnits=1 and WarmThroughput.ReadUnitsPerSecond=1 and returning the table to TableStatus ACTIVE once the (rejected) update is over.
Emulator: DescribeTable answers 200 with ReadCapacityUnits and WarmThroughput.ReadUnitsPerSecond set to the rejected request's value (the recording's run used 1000000000001, which is the request value, not a requirement) and TableStatus 'UPDATING' instead of 'ACTIVE'.
Why: a client that raises capacity above its quota gets no refusal, then sees a table that is permanently UPDATING — so any client that waits for ACTIVE (the SDK table-exists waiter, CloudFormation, CDK) blocks forever, and capacity read back from DescribeTable is a value AWS would never report.
Fix: the UpdateTable handler's throughput validation plus the table state machine that stamps TableStatus (reject above-quota capacity before mutating state; drive UPDATING back to ACTIVE when the update settles).
Traces: dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected#task#9 (DescribeTable).

Cause 2 of 5: a well-formed UpdateTable is refused 400 because the table is stuck in the state cause 1 left it in.
AWS: Smithy: UpdateTable errors are InternalServerError, InvalidEndpointException, LimitExceededException, ResourceInUseException, ResourceNotFoundException. An UpdateTable that sets in-range ProvisionedThroughput (ReadCapacityUnits/WriteCapacityUnits >= 1) on an existing table succeeds with HTTP 200 and a TableDescription whose ProvisionedThroughput carries the requested units and whose TableStatus is UPDATING.
Emulator: the same request answers HTTP 400 with no TableDescription at all.
Why: throughput can never be corrected after a bad request — the table is unusable for any further capacity change, and the client sees a refusal for a request AWS accepts.
Fix: the UpdateTable handler (it must accept an in-range update on an existing table rather than refusing while its own state says the table is busy).
Traces: dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected#task#10 (UpdateTable) — the representative step of this issue.

Cause 3 of 5: applied throughput is written from the wrong source — WriteCapacityUnits is not updated and WarmThroughput mirrors ReadCapacityUnits.
AWS: Smithy: ProvisionedThroughput.ReadCapacityUnits and .WriteCapacityUnits are independent required members; WarmThroughput.ReadUnitsPerSecond/.WriteUnitsPerSecond are independent members of WarmThroughput. After a successful UpdateTable, DescribeTable reports exactly the requested read and write units in both structures (the recording's accepted update requested the same value for each, and AWS reported it for each).
Emulator: DescribeTable reports the stale/rejected read value in ProvisionedThroughput.ReadCapacityUnits and WarmThroughput.ReadUnitsPerSecond, while WriteCapacityUnits/WriteUnitsPerSecond stay at the table's creation value — i.e. the write side of the update is dropped and the warm-throughput members are filled from the read capacity instead of from the request's WarmThroughput.
Why: capacity reported by DescribeTable does not describe the table, so cost/throughput assertions and autoscaling logic that read it are wrong.
Fix: the UpdateTable handler's state mutation and the DescribeTable serializer for ProvisionedThroughput/WarmThroughput.
Traces: dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected#task#11 (DescribeTable).

Cause 4 of 5: a valid CreateTable (base table with one GSI and one LSI) is refused 400 because a definition AWS rejected still holds the table name.
AWS: Smithy: CreateTable takes LocalSecondaryIndexes/GlobalSecondaryIndexes; LocalSecondaryIndex.KeySchema and GlobalSecondaryIndex.KeySchema are required KeySchema lists of KeySchemaElement (AttributeName + KeyType HASH|RANGE), and an LSI KeySchema with no RANGE element is refused with ValidationException 'One or more parameter values were invalid: Index KeySchema does not have a range key for index: <indexName>' (fixed text; <indexName> is the request's IndexName). Because AWS refuses that create, the name is free and a subsequent CreateTable with a HASH+RANGE GSI and a HASH+RANGE LSI answers HTTP 200 with a TableDescription (TableStatus CREATING, the GSI IndexStatus CREATING, both index definitions echoed).
Emulator: the valid CreateTable answers HTTP 400 with no TableDescription, because the earlier malformed definition was created and still occupies the name (the step before answers ResourceInUseException 'Attempt to change a resource which is still in use: Table is being created: <tableName>').
Why: a client cannot create the table it asked for — the name is held by a table AWS would never have created — so every downstream read, write and index query in that workflow fails.
Fix: CreateTable's secondary-index validation (reject an index key schema without a RANGE element before any state is written) and the table-creation state path that must not leave a refused definition registered.
Traces: dynamodb-create-table-secondary-index-key-schema-structure#task#8 (CreateTable).

Cause 5 of 5: DescribeTable serves the wrongly-created table instead of the one the workflow created.
AWS: Smithy: DescribeTable returns TableDescription for the named table — AttributeDefinitions, KeySchema, GlobalSecondaryIndexes and LocalSecondaryIndexes as created. In the recording AWS reports the five attribute definitions of the valid table, the GSI (HASH+RANGE, IndexStatus ACTIVE) and the LSI whose KeySchema is HASH on the table partition key plus RANGE on the LSI sort attribute.
Emulator: DescribeTable answers 200 describing the earlier, refused definition — GlobalSecondaryIndexes absent entirely, only three attribute definitions, and a LocalSecondaryIndexes[0] whose IndexName/IndexArn are the malformed index's and whose second KeySchema element has KeyType 'HASH' instead of 'RANGE'. (The timestamp members differing here are the separately-tracked precision defect, open issue 20261111T221143Z-dynamodb-table-and-stream-timestamps-are-serialized-with.md.)
Why: the described table is not the table the client created; an LSI with two HASH elements is a shape AWS cannot produce, so clients that introspect the schema (SDK waiters, ORMs, index-aware query planners) build queries against indexes that do not exist.
Fix: CreateTable's index validation plus table state (a refused definition must never be readable through DescribeTable).
Traces: dynamodb-create-table-secondary-index-key-schema-structure#task#9 (DescribeTable).

## Evidence
Cause 1 — dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected#task#9 DescribeTable, both 200: expected $.Table.ProvisionedThroughput.ReadCapacityUnits 1, got 1000000000001; expected $.Table.TableStatus 'ACTIVE', got 'UPDATING'; expected $.Table.WarmThroughput.ReadUnitsPerSecond 1, got 1000000000001.
Cause 2 — same fixture #task#10 UpdateTable: expected HTTP 200 with $.TableDescription (ProvisionedThroughput ReadCapacityUnits 1 / WriteCapacityUnits 1, WarmThroughput 1/1, TableStatus UPDATING); got HTTP 400 and $.TableDescription missing.
Cause 3 — same fixture #task#11 DescribeTable, both 200: expected ProvisionedThroughput.ReadCapacityUnits 3 / WriteCapacityUnits 3 and WarmThroughput 3/3; got ReadCapacityUnits 1000000000001, WriteCapacityUnits 1, WarmThroughput.ReadUnitsPerSecond 1000000000001, WarmThroughput.WriteUnitsPerSecond 1.
Cause 4 — dynamodb-create-table-secondary-index-key-schema-structure#task#8 CreateTable: expected HTTP 200 with TableDescription containing GlobalSecondaryIndexes[0] 'gsi-ok' (KeySchema gsi_hash HASH + gsi_sort RANGE, IndexStatus CREATING) and LocalSecondaryIndexes[0] 'lsi-ok' (pk HASH + lsi_sk RANGE); got HTTP 400 with no body shape. The preceding step (#task#5) shows the emulator answering ResourceInUseException 'Attempt to change a resource which is still in use: Table is being created: <tableName>' where AWS answers ValidationException 'One or more parameter values were invalid: Index KeySchema does not have a range key for index: lsi-no-range'.
Cause 5 — same fixture #task#9 DescribeTable, both 200: expected AttributeDefinitions [gsi_hash, gsi_sort, lsi_sk, pk, sk] and GlobalSecondaryIndexes [gsi-ok ... IndexStatus ACTIVE]; got AttributeDefinitions [h2, pk, sk], GlobalSecondaryIndexes missing, LocalSecondaryIndexes[0].IndexName 'lsi-hash-hash' (expected 'lsi-ok') and LocalSecondaryIndexes[0].KeySchema[1] = {AttributeName 'h2', KeyType 'HASH'} (expected {lsi_sk, RANGE}).

## Steps to Reproduce
1. Replay fixture dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected against the emulator: create a provisioned table with ReadCapacityUnits=WriteCapacityUnits=1, then call UpdateTable with a ProvisionedThroughput read value above the table quota. Observe the emulator answers 200 and persists it (step task#9 DescribeTable shows the out-of-range value and TableStatus UPDATING instead of 1 and ACTIVE).
2. Call UpdateTable again with an in-range ProvisionedThroughput (read=write=1) — step task#10 — and observe HTTP 400 instead of 200 with a TableDescription.
3. Call UpdateTable with read=write=3 and DescribeTable — step task#11 — and observe WriteCapacityUnits unchanged and WarmThroughput mirroring the stale read value.
4. Replay fixture dynamodb-create-table-secondary-index-key-schema-structure: a CreateTable whose LocalSecondaryIndexes[0].KeySchema has two HASH elements is accepted by the emulator; then CreateTable the same table name with a valid HASH+RANGE GSI and LSI (step task#8) and observe HTTP 400, and DescribeTable (step task#9) and observe the malformed 'lsi-hash-hash' table being described.
