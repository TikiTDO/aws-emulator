Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.partiql-update-negative-list-index-rejected dynamodb.partiql-update-set-list-append-non-list-literal-operand-rejected dynamodb.partiql-update-set-literal-and-set-add-reject-mixed-type-set-members dynamodb.partiql-update-set-subtraction-applies-and-multiply-divide-rejected dynamodb.execute-statement-select-multi-attribute-gsi-key-rejected dynamodb.batch-execute-statement-multi-attribute-gsi-select-rejected-per-slot dynamodb.execute-statement-vector-index-qualified-select-rejected dynamodb.partiql-select-index-unprojected-filter-refusal-scoped-to-key-condition dynamodb.partiql-select-index-qualifier-projection-follows-index-type dynamodb.batch-execute-statement-non-key-and-index-key-select-rejected-no-table-name dynamodb.batch-execute-statement-or-sort-key-overlap-rejected-per-slot dynamodb.batch-execute-statement-returning-delete-old-and-update-remove-modified-new
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.partiql-update-negative-list-index-rejected|run#12|AssertionError|assert (code, message) == want, f"SET tags[-1] answered {(code, message)!r}, expected {want!r}"
Issue-Key: dynamodb.partiql-update-set-list-append-non-list-literal-operand-rejected|run#11|AssertionError|assert message == expected, f"list_append refusal message is {message!r}, expected {expected!r}"
Issue-Key: dynamodb.execute-statement-select-multi-attribute-gsi-key-rejected|expect_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: dynamodb.batch-execute-statement-multi-attribute-gsi-select-rejected-per-slot|run#15|AssertionError|assert (error.get("Code"), error.get("Message")) == _MULTI_ATTR_ERROR, (
Issue-Key: dynamodb.execute-statement-vector-index-qualified-select-rejected|expect_error#13|AssertionError|assert got_message == message, f"unexpected message {got_message!r}"
Issue-Key: dynamodb.partiql-select-index-qualifier-projection-follows-index-type|run#19|AssertionError|assert lsi_items[0] == {'extra': {'S': 'unprojected-value'}}, (
Issue-Key: dynamodb.batch-execute-statement-non-key-and-index-key-select-rejected-no-table-name|run#8|AssertionError|assert "TableName" not in slot, (
Issue-Key: dynamodb.batch-execute-statement-or-sort-key-overlap-rejected-per-slot|run#15|AssertionError|assert error.get("Message") == (
Issue-Key: dynamodb.batch-execute-statement-returning-delete-old-and-update-remove-modified-new|run#23|AssertionError|assert (insert_error.get("Code"), insert_error.get("Message")) == _EXPECTED_INSERT_ERROR, (
Issue-Related: 20261111T170613Z-dynamodb-partiql-statement-parser-rejects-aws-s-grammar.md
Issue-Commit: cdd06381ed07ce191d4321618e22d6aefb363f32

# dynamodb: PartiQL semantic validation — index-qualified SELECT, UPDATE operand and BatchExecuteStatement per-slot refusals diverge (10 causes)

> _Note: this issue was identified against the code base at commit `cdd06381ed07ce191d4321618e22d6aefb363f32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Ten net-new root causes in the PartiQL semantic layer (after parsing): index addressing, UPDATE operand validation, and the per-slot error shape of BatchExecuteStatement. Smithy: BatchExecuteStatement.Responses is a PartiQLBatchResponse list — a defective slot is reported *inside* the response, never as a thrown exception; its member set is what AWS fills, and ConsumedCapacity is a ConsumedCapacityMultiple. Nothing in the model constrains statement semantics, so each refusal's 400 ValidationException code and message text is the contract. Variable parts of every message below (table name, index name, attribute names, operand type, position) come from the request; the rest is AWS's fixed text.

Cause 1 of 10 — negative list index in a PartiQL SET. AWS: 'List index is not within the allowable range; index: [<n>] at <line>:<col>:<len>'. Emulator: "Statement wasn't well formed, can't be processed: Unexpected term". Fix: PartiQL UPDATE path validation. Traces: partiql-update-negative-list-index-rejected.

Cause 2 of 10 — list_append operand type. AWS: 'Incorrect operand type for operator or function; operator or function: list_append, operand type: N'. Emulator prefixes it with 'Invalid Statement: '. Fix: the PartiQL operand-type diagnostic's message assembly. Traces: partiql-update-set-list-append-non-list-literal-operand-rejected.

Cause 3 of 10 — mixed-type set members. AWS: 'Unsupported data type in Bag. DynamoDB only supports either numbers or strings in bags'. Emulator: 'Invalid Statement: the members of a set must all be strings, all numbers or all binary values' — emulator-authored text. Fix: PartiQL set-literal/SET ADD validation. Traces: partiql-update-set-literal-and-set-add-reject-mixed-type-set-members.

Cause 4 of 10 — arithmetic in a PartiQL UPDATE. AWS: `-` subtraction applies, and `*` / `/` are refused with 'Unsupported operator in Update Expression. Operator: <op>'. Emulator answers 'Where clause does not contain a mandatory equality on all key attributes' — a wholly unrelated refusal, so subtraction never lands either. Fix: PartiQL UPDATE arithmetic evaluation plus its unsupported-operator diagnostic. Traces: partiql-update-set-subtraction-applies-and-multiply-divide-rejected.

Cause 5 of 10 — statements addressed at an index with a multi-attribute HASH or RANGE key are not refused. AWS: 'PartiQL statements cannot be executed on tables with multi-attribute HASH keys or multi-attribute RANGE keys.' (fixed text), and that refusal takes priority over the generic where-clause rule. Emulator: ExecuteStatement SUCCEEDS and returns items; in BatchExecuteStatement the slot carries the generic 'Select statements within BatchExecuteStatement must specify the primary key in the where clause.' Why: a caller gets data AWS never serves, and the per-slot code/message it branches on is wrong. Fix: PartiQL table/index resolution. Traces: execute-statement-select-multi-attribute-gsi-key-rejected, batch-execute-statement-multi-attribute-gsi-select-rejected-per-slot.

Cause 6 of 10 — a SELECT qualified by a vector index names the index in its refusal. AWS includes the index name; the emulator answers the bare 'The table does not have the specified index'. Fix: PartiQL index resolution's not-found message. Traces: execute-statement-vector-index-qualified-select-rejected.

Cause 7 of 10 — the unprojected-filter-attribute refusal is applied too broadly. AWS scopes 'One or more parameter values were invalid: Secondary index <index> does not project one or more filter attributes: [<attrs>]' to the key-condition case only; the emulator raises it for a statement AWS runs. Fix: PartiQL index projection check. Traces: partiql-select-index-unprojected-filter-refusal-scoped-to-key-condition.

Cause 8 of 10 — an index-qualified SELECT naming an attribute the index does not project must table-fetch it from the base table. Emulator returns an empty projection ({}) instead. Fix: PartiQL SELECT projection/table-fetch. Traces: partiql-select-index-qualifier-projection-follows-index-type.

Cause 9 of 10 — a BatchExecuteStatement slot that failed *before* running carries TableName. AWS omits TableName from a pre-run error slot (it is only present for a slot that reached the table). Fix: BatchExecuteStatement's per-slot response serializer. Traces: batch-execute-statement-non-key-and-index-key-select-rejected-no-table-name.

Cause 10 of 10 — two BatchExecuteStatement slot refusals answer the generic where-clause message instead of their own: an OR / sort-key-overlap WHERE clause, and a RETURNING clause on an INSERT (AWS: 'RETURNING is not supported in INSERT'). Emulator: the generic 'Select statements within BatchExecuteStatement must specify the primary key in the where clause.' and "Statement wasn't well formed, can't be processed: Unexpected token after expression" respectively. Fix: per-slot statement validation in the BatchExecuteStatement handler. Traces: batch-execute-statement-or-sort-key-overlap-rejected-per-slot, batch-execute-statement-returning-delete-old-and-update-remove-modified-new.

## Evidence
Expected vs actual, verbatim from the replay:
- partiql-update-negative-list-index-rejected: SET tags[-1] answered ('ValidationException', "Statement wasn't well formed, can't be processed: Unexpected term"), expected ('ValidationException', 'List index is not within the allowable range; index: [-1] at 1:55:1').
- partiql-update-set-list-append-non-list-literal-operand-rejected: got 'Invalid Statement: Incorrect operand type for operator or function; operator or function: list_append, operand type: N', expected 'Incorrect operand type for operator or function; operator or function: list_append, operand type: N'.
- partiql-update-set-literal-and-set-add-reject-mixed-type-set-members: expected ('ValidationException', 'Unsupported data type in Bag. DynamoDB only supports either numbers or strings in bags'), got ('ValidationException', 'Invalid Statement: the members of a set must all be strings, all numbers or all binary values').
- partiql-update-set-subtraction-applies-and-multiply-divide-rejected: expected ('ValidationException', 'Unsupported operator in Update Expression. Operator: *'), got ('ValidationException', 'Where clause does not contain a mandatory equality on all key attributes').
- execute-statement-select-multi-attribute-gsi-key-rejected: expected the call to be refused, but it succeeded and returned the item.
- batch-execute-statement-multi-attribute-gsi-select-rejected-per-slot: expected ('ValidationError', 'PartiQL statements cannot be executed on tables with multi-attribute HASH keys or multi-attribute RANGE keys.'), got ('ValidationError', 'Select statements within BatchExecuteStatement must specify the primary key in the where clause.').
- execute-statement-vector-index-qualified-select-rejected: unexpected message 'The table does not have the specified index' (index name missing).
- partiql-select-index-unprojected-filter-refusal-scoped-to-key-condition: ClientError (ValidationException) 'One or more parameter values were invalid: Secondary index gsi1 does not project one or more filter attributes: [not_proj_attr]' on a statement AWS runs.
- partiql-select-index-qualifier-projection-follows-index-type: naming 'extra' through a KEYS_ONLY LSI must table-fetch it from the base table, got {}.
- batch-execute-statement-non-key-and-index-key-select-rejected-no-table-name: pre-run error slot was {'Error': {...}, 'TableName': 'sim2real-behav-f457a88e78ad'}; AWS omits TableName (the table name shown is this run's value).
- batch-execute-statement-or-sort-key-overlap-rejected-per-slot: unexpected message 'Select statements within BatchExecuteStatement must specify the primary key in the where clause.'
- batch-execute-statement-returning-delete-old-and-update-remove-modified-new: expected ('ValidationError', 'RETURNING is not supported in INSERT'), got ('ValidationError', "Statement wasn't well formed, can't be processed: Unexpected token after expression").

## Steps to Reproduce
1. Create a table with a HASH+RANGE key, a GSI whose KeySchema holds two HASH elements, a KEYS_ONLY LSI, and a vector index; seed one item.
2. ExecuteStatement a SELECT qualified by the multi-attribute-HASH GSI (expect refusal), by the vector index (expect the index-naming refusal), and by the KEYS_ONLY LSI naming a non-projected attribute (expect a base-table fetch).
3. ExecuteStatement UPDATEs that use SET tags[-1], list_append with an N operand, a mixed-type set literal, and `*` / `-` arithmetic.
4. BatchExecuteStatement a batch holding: the multi-attribute-HASH GSI SELECT, a non-key base-table SELECT, an OR/sort-key-overlap SELECT, and an INSERT with RETURNING; inspect each Responses slot's Error.Code, Error.Message and whether TableName is present.
