Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.batch-write-item-put-request-accepts-32-levels-rejects-33 dynamodb.transact-write-items-put-condition-value-nesting-depth-rejected dynamodb.transact-write-items-put-string-set-with-zero-members-rejected dynamodb.number-magnitude-floor-put-update-batchwrite dynamodb.put-update-delete-item-return-consumed-capacity-invalid-enum-rejected dynamodb.put-item-and-search-vectors-return-consumed-capacity-invalid-enum-rejected dynamodb.create-table-rejects-invalid-table-class-value dynamodb.execute-transaction-exceeds-100-statements-rejected dynamodb.search-vectors-non-numeric-search-vector-element-rejected dynamodb.batch-request-shape-defects-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.batch-write-item-put-request-accepts-32-levels-rejects-33|run#20|AssertionError|assert message == _NESTING_MESSAGE, f"expected the exact ValidationException message, got {message!r}"
Issue-Key: dynamodb.transact-write-items-put-condition-value-nesting-depth-rejected|run#15|AssertionError|assert message == _NESTING_MESSAGE, (
Issue-Key: dynamodb.transact-write-items-put-string-set-with-zero-members-rejected|run#12|AssertionError|assert message == expected_message, (
Issue-Key: dynamodb|BatchWriteItem|ValidationException
Issue-Key: dynamodb.put-update-delete-item-return-consumed-capacity-invalid-enum-rejected|expect_error#11|AssertionError|assert got == (code, message), f"expected {(code, message)!r}, got {got!r}"
Issue-Key: dynamodb.put-item-and-search-vectors-return-consumed-capacity-invalid-enum-rejected|expect_error#11|AssertionError|assert got == (code, message), f"expected {(code, message)!r}, got {got!r}"
Issue-Key: dynamodb|CreateTable|ValidationException
Issue-Key: dynamodb.search-vectors-non-numeric-search-vector-element-rejected|expect_error#10|AssertionError|assert got_message == message, f'expected message {message!r}, got {got_message!r}'
Issue-Key: dynamodb.batch-request-shape-defects-rejected|run#6|AssertionError|assert (code, msg) == ("ValidationException", want), (
Issue-Related: 20261109T172142Z-dynamodb-7-net-new-causes-validationexception-message-text.md
Issue-Commit: cdd06381ed07ce191d4321618e22d6aefb363f32

# dynamodb: non-constraint ValidationException messages are wrapped in the "1 validation error detected: " envelope AWS does not use (8 causes)

> _Note: this issue was identified against the code base at commit `cdd06381ed07ce191d4321618e22d6aefb363f32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
Eight net-new root causes with one shape: the emulator renders a refusal that AWS words as a plain service message through its Smithy-constraint message builder, so the served text gains the leading 'N validation error(s) detected: ' envelope (or the generic enum/length constraint sentence) instead of AWS's own fixed text. Smithy: ValidationException is the 400 client error these operations serve; the model carries no message text, so the wording is the whole contract. AWS uses the '1 validation error detected: Value '<v>' at '<member>' failed to satisfy constraint: ...' envelope ONLY for a modeled member-constraint violation; a service-level rule (nesting depth, empty set, number magnitude, enum of a parameter it validates itself) is served as the bare sentence. The variable parts below are the offending value and the member path from the request; the rest is fixed.

Cause 1 of 8 — the nesting-depth refusal. AWS: 'Nesting Levels have exceeded supported limits: Attributes in the item have nested levels beyond supported limit' with no envelope; the emulator prefixes '1 validation error detected: '. Seen on BatchWriteItem PutRequest (32 levels accepted / 33 refused) and on a TransactWriteItems Put condition value. Fix: the nesting-depth check's message assembly. Traces: batch-write-item-put-request-accepts-32-levels-rejects-33, transact-write-items-put-condition-value-nesting-depth-rejected.

Cause 2 of 8 — the empty-string-set refusal. AWS: 'One or more parameter values were invalid: An string set  may not be empty' (note AWS's own double space). Emulator prefixes '1 validation error detected: '. Fix: the attribute-value validation message for TransactWriteItems Put. Traces: transact-write-items-put-string-set-with-zero-members-rejected.

Cause 3 of 8 — the number-magnitude floor. AWS: 'Number underflow. Attempting to store a number with magnitude smaller than supported range'. Emulator prefixes '1 validation error detected: '. Fix: the number-magnitude check's message, across PutItem/UpdateItem/BatchWriteItem. Traces: number-magnitude-floor-put-update-batchwrite.

Cause 4 of 8 — ReturnConsumedCapacity with a bogus value. AWS: "Value 'BOGUS' at 'returnConsumedCapacity' failed to satisfy constraint: Member must satisfy enum value set: [INDEXES, TOTAL, NONE]" with NO '1 validation error detected: ' prefix. Emulator adds it. Seen on PutItem/UpdateItem/DeleteItem and on SearchVectors. Fix: the enum-constraint message builder for these inputs. Traces: put-update-delete-item-return-consumed-capacity-invalid-enum-rejected, put-item-and-search-vectors-return-consumed-capacity-invalid-enum-rejected.

Cause 5 of 8 — CreateTable TableClass with an invalid value. AWS serves its own sentence 'Invalid table-class parameter provided. Please try again with a valid table-class value: [STANDARD, STANDARD_INFREQUENT_ACCESS].'; the emulator serves the generic enum-constraint form instead. Fix: CreateTable's TableClass validation. Traces: create-table-rejects-invalid-table-class-value.

Cause 6 of 8 — ExecuteTransaction over 100 statements. Smithy: TransactStatements @length max 100. AWS renders the offending value by serializing the statement list — the message starts "1 validation error detected: Value '[" and ends "]' at 'transactStatements' failed to satisfy constraint: Member must have length less than or equal to 100". The emulator substitutes a count summary ('101 items') for the serialized value. Fix: the constraint message's value renderer. Traces: execute-transaction-exceeds-100-statements-rejected.

Cause 7 of 8 — SearchVectors with a non-numeric SearchVector element. AWS: 'Search vector contains invalid values. All values in the search vector must be a 32-bit floating-point number attribute'. Emulator serves a member-constraint sentence under 'searchVector'. Fix: SearchVectors input validation's message. Traces: search-vectors-non-numeric-search-vector-element-rejected.

Cause 8 of 8 — BatchWriteItem/BatchGetItem request-shape defects. Smithy: BatchWriteItem.RequestItems is required; AWS refuses an empty map with its own text, while the emulator serves "1 validation error detected: Value at 'requestItems' failed to satisfy constraint: Member must have length greater than or equal to 1". Why (all causes): callers and CLIs surface these strings verbatim and some branch on them; a message that is not AWS's is a behavioural difference even when the code and status match. Fix: the batch request-shape validation messages. Traces: batch-request-shape-defects-rejected.

## Evidence
Expected vs actual, verbatim from the replay:
- batch-write-item-put-request-accepts-32-levels-rejects-33 / transact-write-items-put-condition-value-nesting-depth-rejected: 'expected the exact ValidationException message, got '1 validation error detected: Nesting Levels have exceeded supported limits: Attributes in the item have nested levels beyond supported limit''.
- transact-write-items-put-string-set-with-zero-members-rejected: expected AWS's exact message 'One or more parameter values were invalid: An string set  may not be empty', got '1 validation error detected: One or more parameter values were invalid: An string set  may not be empty'.
- number-magnitude-floor-put-update-batchwrite: expected ('ValidationException', 'Number underflow. Attempting to store a number with magnitude smaller than supported range'), got ('ValidationException', '1 validation error detected: Number underflow. ...').
- put-update-delete-item-return-consumed-capacity-invalid-enum-rejected and put-item-and-search-vectors-return-consumed-capacity-invalid-enum-rejected: expected ("Value 'BOGUS' at 'returnConsumedCapacity' failed to satisfy constraint: Member must satisfy enum value set: [INDEXES, TOTAL, NONE]"), got the same text prefixed '1 validation error detected: '.
- create-table-rejects-invalid-table-class-value: expected ('ValidationException', 'Invalid table-class parameter provided. Please try again with a valid table-class value: [STANDARD, STANDARD_INFREQUENT_ACCESS].'), got ("1 validation error detected: Value 'FREQUENT_ACCESS' at 'tableClass' failed to satisfy constraint: Member must satisfy enum value set: [STANDARD, STANDARD_INFREQUENT_ACCESS]").
- execute-transaction-exceeds-100-statements-rejected: expected a message starting "1 validation error detected: Value '[" and ending "]' at 'transactStatements' failed to satisfy constraint: Member must have length less than or equal to 100", got "1 validation error detected: Value '101 items' at 'transactStatements' ...".
- search-vectors-non-numeric-search-vector-element-rejected: expected 'Search vector contains invalid values. All values in the search vector must be a 32-bit floating-point number attribute', got "1 validation error detected: Value at 'searchVector' failed to satisfy constraint: Member must hold only numbers, each a 32-bit IEEE-754 floating point value".
- batch-request-shape-defects-rejected: BatchWriteItem with an empty RequestItems map gave 'ValidationException': "1 validation error detected: Value at 'requestItems' failed to satisfy constraint: Member must have length greater than or equal to 1".
('BOGUS', 'FREQUENT_ACCESS' and 101 are this run's request values; the envelope/no-envelope distinction and the fixed sentences are the requirement.)

## Steps to Reproduce
1. Create a PAY_PER_REQUEST table.
2. BatchWriteItem a PutRequest whose item nests 33 levels, and TransactWriteItems a Put whose ConditionExpression value nests past the limit — compare the message to the bare 'Nesting Levels have exceeded supported limits: ...' text.
3. TransactWriteItems a Put holding an "SS": [] attribute; PutItem/UpdateItem/BatchWriteItem a number below the magnitude floor.
4. PutItem/UpdateItem/DeleteItem and SearchVectors with ReturnConsumedCapacity='BOGUS'.
5. CreateTable with TableClass='FREQUENT_ACCESS'.
6. ExecuteTransaction with 101 statements.
7. SearchVectors with a non-numeric element in SearchVector.
8. BatchWriteItem with RequestItems={}.
