Issue-Source: trace-sweep-divergence
Issue-Trace: create-table-rejects-empty-secondary-index-lists
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: create-table-rejects-empty-secondary-index-lists dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected dynamodb-create-table-secondary-index-key-schema-structure dynamodb-create-table-vector-index-on-provisioned-table-rejected dynamodb-table-name-missing-or-pattern-invalid-across-admin-operations
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|CreateTable|4xx->2xx|$.__type
Issue-Key: dynamodb|CreateTable|4xx->2xx|$.message
Issue-Key: dynamodb|CreateTable|4xx->2xx|status
Issue-Key: dynamodb|DescribeTable|4xx->2xx|$.__type
Issue-Key: dynamodb|DescribeTable|4xx->2xx|$.message
Issue-Key: dynamodb|DescribeTable|4xx->2xx|status
Issue-Key: dynamodb|UpdateTable|4xx->2xx|$.__type
Issue-Key: dynamodb|UpdateTable|4xx->2xx|$.message
Issue-Key: dynamodb|UpdateTable|4xx->2xx|status
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.ProvisionedThroughput.LastIncreaseDateTime
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.AttributeDefinitions[*]
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.LocalSecondaryIndexes[*].IndexArn
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.LocalSecondaryIndexes[*].IndexName
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.LocalSecondaryIndexes[*].KeySchema[*].AttributeName
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.LocalSecondaryIndexes[*].KeySchema[*].KeyType
Issue-Key: dynamodb|DeleteTable|4xx->4xx|$.message
Issue-Related: 20261104T061030Z-dynamodb-createtable-and-updatetable-drop-the-throughput-mode.md
Issue-Related: 20261109T055139Z-dynamodb-throughput-and-capacity-reporting-diverges-across-updatetable.md
Issue-Related: 20261111T221143Z-dynamodb-table-and-stream-timestamps-are-serialized-with.md
Issue-Commit: 0e3e027770f68a801846f0512bd9649d89ae04a7

# dynamodb: CreateTable and UpdateTable accept index and throughput definitions AWS refuses

> _Note: this issue was identified against the code base at commit `0e3e027770f68a801846f0512bd9649d89ae04a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
The table-admin validator lets through table definitions AWS refuses, so the emulator ends up holding tables that cannot exist on AWS; the resulting state is then served back as if valid. 7 causes.

Cause 1 of 7: an empty GlobalSecondaryIndexes list is accepted.
AWS: recorded behavior — status 400, __type 'com.amazon.coral.validate#ValidationException', message 'One or more parameter values were invalid: List of GlobalSecondaryIndexes is empty'. Fixed text throughout; the member name 'GlobalSecondaryIndexes' is the only substituted part (the same rule applies to LocalSecondaryIndexes). Smithy models GlobalSecondaryIndexList with no @length min, so the model does not state the rule — AWS enforces it in the service.
Emulator: status 200 — the table is created, no __type and no message.
Why: a client's malformed CreateTable silently succeeds and it proceeds against a table AWS would never have made, so the error is only discovered in production.
Fix: CreateTable's validation — reject a present-but-empty secondary-index list before the table is written to state.
Traces: create-table-rejects-empty-secondary-index-lists#task#1.

Cause 2 of 7: because cause 1 created the table, DescribeTable answers 200 where it must answer not-found.
AWS: recorded behavior — status 400, __type 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException', message 'Requested resource not found: Table: <tableName> not found'. Fixed text 'Requested resource not found: Table: ' and ' not found'; <tableName> comes from the request.
Emulator: status 200 with a full table description.
Why: the downstream read confirms a table that must not exist, so a client's "did my bad CreateTable get rejected?" check reports the opposite.
Fix: follows from cause 1 (nothing must be written to table state); the DescribeTable not-found path itself is correct.
Traces: create-table-rejects-empty-secondary-index-lists#task#2.

Cause 3 of 7: UpdateTable accepts a ReadCapacityUnits above the service ceiling.
AWS: recorded behavior — status 400, __type 'com.amazon.coral.validate#ValidationException', message 'Given value <value> for ReadCapacityUnits is out of bounds'. Fixed text 'Given value ' / ' for ' / ' is out of bounds'; <value> is the request's value and the member name is the offending member (ReadCapacityUnits or WriteCapacityUnits). Smithy: ProvisionedThroughput.ReadCapacityUnits targets PositiveLongObject with @range min 1 and no modeled max, so the upper bound is a service quota AWS enforces outside the model.
Emulator: status 200 — the update is applied.
Why: a client can drive the table into a throughput configuration AWS rejects, and capacity-planning code never sees the bound.
Fix: UpdateTable's validation of ProvisionedThroughput — enforce the upper bound as well as the modeled @range min.
Traces: dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected#task#6.

Cause 4 of 7: the accepted out-of-bounds update is then served back, and the table never settles.
AWS: recorded behavior — after the refusal of cause 3 the table is unchanged: TableStatus 'ACTIVE' and ProvisionedThroughput.ReadCapacityUnits / WarmThroughput.ReadUnitsPerSecond at their prior value.
Emulator: DescribeTable returns TableStatus 'UPDATING' with the out-of-bounds value echoed into both ProvisionedThroughput.ReadCapacityUnits and WarmThroughput.ReadUnitsPerSecond.
Why: a client waiting for ACTIVE after a (correctly refused) update waits forever, and reads a capacity figure AWS cannot report.
Fix: follows from cause 3 — a refused UpdateTable must leave table state and TableStatus untouched.
Traces: dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected#task#9.

Cause 5 of 7: a later legitimate UpdateTable is then mis-merged on top of the bad state.
AWS: recorded behavior — DescribeTable reports the newly requested capacity on both read and write (and the matching WarmThroughput units), with ProvisionedThroughput.LastIncreaseDateTime set.
Emulator: keeps the out-of-bounds read capacity and the stale write capacity instead of the requested values, on both ProvisionedThroughput and WarmThroughput.
Why: throughput changes a client makes after the bad update are silently lost; the table's reported capacity no longer matches any request.
Fix: follows from cause 3; additionally UpdateTable's state merge must apply the requested units rather than retaining prior ones.
Traces: dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected#task#11.

Cause 6 of 7: an invalid secondary-index key schema is accepted and stored, so DescribeTable reports a table structure AWS cannot produce.
AWS: recorded behavior — the valid definition is stored: AttributeDefinitions holds every declared key attribute (gsi_hash, gsi_sort, lsi_sk, pk, sk), GlobalSecondaryIndexes holds the declared GSI with its HASH+RANGE KeySchema, Projection, ProvisionedThroughput and IndexArn, and the LSI is the declared one with a HASH followed by a RANGE element. An index key schema with two HASH elements and no RANGE is refused at CreateTable, not stored.
Emulator: DescribeTable reports a different table: AttributeDefinitions truncated to three entries and renamed (h2, pk, sk instead of the five declared ones), GlobalSecondaryIndexes absent entirely, and the single LocalSecondaryIndex named after the rejected two-HASH definition ('lsi-hash-hash') with its second KeySchema element typed HASH instead of RANGE.
Why: the table a client created is not the table it can read back — its GSI is simply gone, its attribute definitions are wrong, and its LSI has a key schema with two partition keys, so every Query/Scan against the declared index fails or returns nothing.
Fix: CreateTable's secondary-index validation (one HASH plus at most one RANGE per index key schema, every key attribute present in AttributeDefinitions) and the table-state writer that currently drops the GSI and keeps the invalid LSI.
Traces: dynamodb-create-table-secondary-index-key-schema-structure#task#9.

Cause 7 of 7: a missing TableName is refused with emulator-authored prose instead of AWS's constraint sentence.
AWS: recorded behavior — status 400 ValidationException with message "1 validation error detected: Value null at 'tableName' failed to satisfy constraint: Member must not be null". Fixed text '1 validation error detected: Value null at '' / '' failed to satisfy constraint: Member must not be null'; the substituted part is the lowerCamel member path 'tableName'. Smithy: DeleteTable.TableName is @required.
Emulator: message "The parameter 'TableName' is required but was not present in the request" — its own sentence, with the member in PascalCase.
Why: tooling that parses AWS's constraint envelope to extract the offending member path gets nothing, and the text differs from every other AWS service.
Fix: DeleteTable's (and the shared admin) required-member validation must render AWS's constraint envelope with the lowerCamel member path.
Traces: dynamodb-table-name-missing-or-pattern-invalid-across-admin-operations#task#3.

## Evidence
Cause 1: CreateTable — status expected 400, got 200; $.__type expected 'com.amazon.coral.validate#ValidationException' (missing); $.message expected 'One or more parameter values were invalid: List of GlobalSecondaryIndexes is empty' (missing).
Cause 2: DescribeTable — status expected 400, got 200; $.__type expected 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException' (missing); $.message expected 'Requested resource not found: Table: <tableName> not found' (missing).
Cause 3: UpdateTable — status expected 400, got 200; $.__type expected 'com.amazon.coral.validate#ValidationException' (missing); $.message expected 'Given value <value> for ReadCapacityUnits is out of bounds' (missing).
Cause 4: DescribeTable 200 — $.Table.TableStatus expected 'ACTIVE', got 'UPDATING'; $.Table.ProvisionedThroughput.ReadCapacityUnits and $.Table.WarmThroughput.ReadUnitsPerSecond expected the prior value, got the out-of-bounds value.
Cause 5: DescribeTable 200 — ProvisionedThroughput.ReadCapacityUnits/WriteCapacityUnits and WarmThroughput.ReadUnitsPerSecond/WriteUnitsPerSecond expected the newly requested units, got the out-of-bounds read value and the stale write value.
Cause 6: DescribeTable 200 — AttributeDefinitions[0..2].AttributeName wrong and [3],[4] missing; $.Table.GlobalSecondaryIndexes missing entirely; LocalSecondaryIndexes[0].IndexName/IndexArn name the rejected 'lsi-hash-hash' definition and KeySchema[1].KeyType is 'HASH' where 'RANGE' is required.
Cause 7: DeleteTable 400 — $.message expected "1 validation error detected: Value null at 'tableName' failed to satisfy constraint: Member must not be null", got "The parameter 'TableName' is required but was not present in the request".

## Steps to Reproduce
1. Replay create-table-rejects-empty-secondary-index-lists#task#1 (CreateTable with an empty GlobalSecondaryIndexes list must be refused 400 ValidationException) and #task#2 (DescribeTable on that name must then answer ResourceNotFoundException).
2. Replay dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected#task#6 (UpdateTable with an out-of-bounds ReadCapacityUnits must be refused), then #task#9 and #task#11 (DescribeTable must show the table ACTIVE at its prior capacity, and a subsequent valid update must be applied).
3. Replay dynamodb-create-table-secondary-index-key-schema-structure#task#9: DescribeTable must report the five declared AttributeDefinitions, the declared GSI, and an LSI whose second KeySchema element is RANGE.
4. Replay dynamodb-table-name-missing-or-pattern-invalid-across-admin-operations#task#3 and compare the DeleteTable message against AWS's constraint envelope.
