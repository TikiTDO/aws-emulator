Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-condition-expression-putitem-aliased-contains-same-path-rejected
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-condition-expression-putitem-aliased-contains-same-path-rejected dynamodb-put-item-condition-expression-operator-limit dynamodb-document-path-into-key-or-index-key-rejected dynamodb-projection-expression-key-attribute-dereference-rejected dynamodb-projection-map-vs-list-prefix-conflict dynamodb-legacy-key-conditions-and-query-filter-between-reversed-rejected dynamodb-legacy-query-scan-comparisonoperator-type-mismatch dynamodb-redundant-parentheses-expression-rejected query-base-table-full-key-equality-rejects-exclusive-start-key dynamodb-filter-expression-non-distinct-operand-comparison-and-contains-rejected dynamodb-legacy-attributevaluelist-without-comparator-and-mixed-type-in dynamodb-legacy-scan-filter-between-reversed-rejected query-scan-filterexpression-key-attribute-nested-path-rejected dynamodb-query-key-condition-between-reversed-bounds-rejected dynamodb-transact-write-items-expression-defect-outranks-table-and-target
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|PutItem|4xx->2xx|$
Issue-Key: dynamodb|PutItem|4xx->2xx|status
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.data.S
Issue-Key: dynamodb|GetItem|4xx->2xx|$.__type
Issue-Key: dynamodb|GetItem|4xx->2xx|$.message
Issue-Key: dynamodb|GetItem|4xx->2xx|status
Issue-Key: dynamodb|Query|4xx->2xx|$.__type
Issue-Key: dynamodb|Query|4xx->2xx|$.message
Issue-Key: dynamodb|Query|4xx->2xx|status
Issue-Key: dynamodb|Scan|4xx->2xx|$.__type
Issue-Key: dynamodb|Scan|4xx->2xx|$.message
Issue-Key: dynamodb|Scan|4xx->2xx|status
Issue-Key: dynamodb|DeleteItem|4xx->2xx|$
Issue-Key: dynamodb|DeleteItem|4xx->2xx|status
Issue-Key: dynamodb|Query|2xx->2xx|$.Count
Issue-Key: dynamodb|Query|2xx->2xx|$.Items[*]
Issue-Key: dynamodb|Query|2xx->2xx|$.ScannedCount
Issue-Key: dynamodb|Scan|2xx->2xx|$.Count
Issue-Key: dynamodb|Scan|2xx->2xx|$.ScannedCount
Issue-Key: dynamodb|TransactWriteItems|4xx->4xx|$.__type
Issue-Related: 20261110T232401Z-dynamodb-expression-diagnostics-4-net-new-causes-operand.md
Issue-Related: 20261110T232401Z-dynamodb-response-state-and-shape-diverge-on-replica.md
Issue-Related: 20261110T232401Z-dynamodb-validation-and-not-found-refusal-messages-are.md
Issue-Related: 20261111T221143Z-dynamodb-item-and-batch-operations-answer-validationexception-where.md
Issue-Commit: 0e3e027770f68a801846f0512bd9649d89ae04a7

# dynamodb: Condition, Key and Filter expression defects are accepted or outranked by the table lookup

> _Note: this issue was identified against the code base at commit `0e3e027770f68a801846f0512bd9649d89ae04a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
The expression validator misses four classes of malformed expression entirely — so the write or read is PERFORMED — and when it does refuse, it lets the table lookup outrank the expression defect. Because the missed refusals are on PutItem and DeleteItem, this silently mutates stored items. 10 causes.

Cause 1 of 10: PutItem accepts a ConditionExpression whose first operand is not distinct from the rest.
AWS: recorded behavior — status 400, __type 'com.amazon.coral.validate#ValidationException', message '1 validation error detected: Invalid ConditionExpression: The first operand must be distinct from the remaining operands for this operator or function; operator: <operator>, first operand: [<path>]'. Fixed text '1 validation error detected: Invalid ConditionExpression: The first operand must be distinct from the remaining operands for this operator or function; operator: ' / ', first operand: ['/']'; the substituted parts are the operator or function name (here 'contains') and the resolved document path, in brackets — resolved through ExpressionAttributeNames aliases, which is the point of this case.
Emulator: status 200 with an empty body — the condition is ignored and the put is applied.
Why: a conditional write that AWS refuses is performed, so a client's optimistic-concurrency guard silently does nothing and data is overwritten.
Fix: PutItem's ConditionExpression validation — the distinct-operand check must run after alias resolution, before the write.
Traces: dynamodb-condition-expression-putitem-aliased-contains-same-path-rejected#task#4.

Cause 2 of 10: the item is observably overwritten as a result.
AWS: recorded behavior — after the refusal of cause 1 the stored item is unchanged, so GetItem returns the pre-existing attribute value.
Emulator: GetItem returns the newly written value.
Why: this is the data loss cause 1 causes, now visible on read.
Fix: follows from cause 1 — a refused PutItem must not reach item state.
Traces: dynamodb-condition-expression-putitem-aliased-contains-same-path-rejected#task#5.

Cause 3 of 10: GetItem accepts a document path into a key attribute.
AWS: recorded behavior — status 400, __type 'com.amazon.coral.validate#ValidationException', message "Key attributes must be scalars; list random access '[]' and map lookup '.' are not allowed: Key: <keyAttributeName>". Fixed text "Key attributes must be scalars; list random access '[]' and map lookup '.' are not allowed: Key: "; the substituted part is the offending key attribute's name, taken from the request's key schema.
Emulator: status 200 with no __type and no message — it serves the read.
Why: a malformed key reference returns data instead of an error, so a client's key handling bug is never surfaced and it reads items it did not address.
Fix: the shared key-expression validation reached by GetItem — reject non-scalar key paths before the read.
Traces: dynamodb-document-path-into-key-or-index-key-rejected#task#4.

Cause 4 of 10: Query accepts the same non-scalar key path (on the sort key).
AWS: as cause 3, with <keyAttributeName> = the sort key name.
Emulator: status 200, no refusal.
Why: as cause 3, on a multi-item read.
Fix: as cause 3, on Query's key-condition path.
Traces: dynamodb-document-path-into-key-or-index-key-rejected#task#6.

Cause 5 of 10: Scan accepts a FilterExpression whose first operand is not distinct from the rest.
AWS: recorded behavior — status 400, __type 'com.amazon.coral.validate#ValidationException', message 'Invalid FilterExpression: The first operand must be distinct from the remaining operands for this operator or function; operator: <operator>, first operand: [<path>]'. Fixed text as in cause 1 but prefixed 'Invalid FilterExpression: ' and without the '1 validation error detected: ' envelope; substituted parts are the operator (here '=') and the resolved path.
Emulator: status 200 — the scan runs with the malformed filter.
Why: a client gets rows back from a filter AWS refuses, so a filter bug reads as a working query and the result set is wrong.
Fix: Scan's FilterExpression validation (shared with Query's).
Traces: dynamodb-filter-expression-non-distinct-operand-comparison-and-contains-rejected#task#4.

Cause 6 of 10: DeleteItem accepts a ConditionExpression with redundant parentheses.
AWS: recorded behavior — status 400, __type 'com.amazon.coral.validate#ValidationException', message '1 validation error detected: Invalid ConditionExpression: The expression has redundant parentheses;' (fixed text including the trailing semicolon; no variable part).
Emulator: status 200 with an empty body — the delete is applied.
Why: the guarded delete AWS refuses is performed, destroying the item.
Fix: the expression parser's redundant-parenthesis check, on the DeleteItem ConditionExpression path.
Traces: dynamodb-redundant-parentheses-expression-rejected#task#6.

Cause 7 of 10: the deletion is observable on Query.
AWS: recorded behavior — after the refusal of cause 6 the item is still present, so Query returns Count 1, ScannedCount 1 and the item.
Emulator: Count 0, ScannedCount 0, no items.
Why: this is cause 6's data loss, visible on read.
Fix: follows from cause 6 — a refused DeleteItem must not reach item state.
Traces: dynamodb-redundant-parentheses-expression-rejected#task#8.

Cause 8 of 10: the same deletion is observable on Scan.
AWS: as cause 7 — Count 1, ScannedCount 1 and the item returned.
Emulator: Count 0, ScannedCount 0, no items.
Why: as cause 7.
Fix: follows from cause 6.
Traces: dynamodb-redundant-parentheses-expression-rejected#task#10.

Cause 9 of 10: Query's table lookup outranks a KeyConditionExpression defect.
AWS: recorded behavior — the expression is validated first: status 400, __type 'com.amazon.coral.validate#ValidationException', message 'Invalid KeyConditionExpression: The BETWEEN operator requires upper bound to be greater than or equal to lower bound; lower bound operand: AttributeValue: {<type>:<value>}, upper bound operand: AttributeValue: {<type>:<value>}'. Fixed text 'Invalid KeyConditionExpression: The BETWEEN operator requires upper bound to be greater than or equal to lower bound; lower bound operand: AttributeValue: ' / ', upper bound operand: AttributeValue: '; the substituted parts are the two operands rendered as AttributeValue: {<T>:<v>} from the request's ExpressionAttributeValues.
Emulator: status 400 but __type 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException' and message 'Requested resource not found' — it resolved the table before parsing the expression, and reported the lookup failure instead.
Why: the client is told its table is missing when the real defect is in its expression, so it retries table creation instead of fixing the query; the error class drives the wrong recovery path.
Fix: Query's validation ordering — parse and validate KeyConditionExpression before resolving the table.
Traces: dynamodb-query-key-condition-between-reversed-bounds-rejected#task#1.

Cause 10 of 10: TransactWriteItems' table lookup outranks a ConditionExpression syntax error.
AWS: recorded behavior — status 400, __type 'com.amazon.coral.validate#ValidationException', message 'Invalid ConditionExpression: Syntax error; token: "<token>", near: "<context>"'. Fixed text 'Invalid ConditionExpression: Syntax error; token: "' / '", near: "' / '"'; the substituted parts are the offending token (here the end-of-input sentinel "<EOF>") and the surrounding expression text, both taken from the request's expression.
Emulator: status 400 with __type 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException' and message 'Requested resource not found'.
Why: as cause 9, and in a transaction the client cannot tell which of several targets is at fault.
Fix: TransactWriteItems' validation ordering — validate every item's expressions before resolving tables and targets.
Traces: dynamodb-transact-write-items-expression-defect-outranks-table-and-target#task#6.

Related but not covering: open issues 20261111T170613Z-dynamodb-filter-projection-update-expression-validation-accepts-malformed.md and 20261110T232401Z-dynamodb-expression-diagnostics-4-net-new-causes-operand.md cover other expression-validation causes; neither carries these clusters' replay keys.

## Evidence
Cause 1: PutItem — status expected 400, got 200 with an empty body; expected {'__type': 'com.amazon.coral.validate#ValidationException', 'message': '1 validation error detected: Invalid ConditionExpression: The first operand must be distinct from the remaining operands for this operator or function; operator: contains, first operand: [data]'}.
Cause 2: GetItem 200 — $.Item.<attr>.S expected the pre-existing value, got the newly written value.
Cause 3: GetItem — status expected 400, got 200; expected $.__type 'com.amazon.coral.validate#ValidationException' and $.message "Key attributes must be scalars; list random access '[]' and map lookup '.' are not allowed: Key: pk"; both missing.
Cause 4: Query — same, with 'Key: sk'.
Cause 5: Scan — status expected 400, got 200; expected $.message 'Invalid FilterExpression: The first operand must be distinct from the remaining operands for this operator or function; operator: =, first operand: [a]'; missing.
Cause 6: DeleteItem — status expected 400, got 200 with an empty body; expected message '1 validation error detected: Invalid ConditionExpression: The expression has redundant parentheses;'.
Cause 7: Query 200 — $.Count expected 1 got 0, $.ScannedCount expected 1 got 0, $.Items[0] expected the seeded item, got nothing.
Cause 8: Scan 200 — identical Count/ScannedCount/Items divergence.
Cause 9: Query 400 — $.__type expected 'com.amazon.coral.validate#ValidationException', got 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException'; $.message expected the 'Invalid KeyConditionExpression: The BETWEEN operator requires upper bound to be greater than or equal to lower bound; ...' sentence, got 'Requested resource not found'.
Cause 10: TransactWriteItems 400 — $.__type expected 'com.amazon.coral.validate#ValidationException', got ResourceNotFoundException; $.message expected 'Invalid ConditionExpression: Syntax error; token: "<EOF>", near: "("', got 'Requested resource not found'.

## Steps to Reproduce
1. Replay dynamodb-condition-expression-putitem-aliased-contains-same-path-rejected#task#4 (PutItem must be refused 400) and #task#5 (GetItem must still return the pre-existing value).
2. Replay dynamodb-document-path-into-key-or-index-key-rejected#task#4 (GetItem) and #task#6 (Query): both must be refused with the 'Key attributes must be scalars...' message naming the offending key.
3. Replay dynamodb-filter-expression-non-distinct-operand-comparison-and-contains-rejected#task#4: Scan must be refused with the 'Invalid FilterExpression: The first operand must be distinct...' message.
4. Replay dynamodb-redundant-parentheses-expression-rejected#task#6 (DeleteItem must be refused), then #task#8 and #task#10 (Query and Scan must still see Count 1 and the item).
5. Replay dynamodb-query-key-condition-between-reversed-bounds-rejected#task#1 and dynamodb-transact-write-items-expression-defect-outranks-table-and-target#task#6: each must answer the expression ValidationException, not ResourceNotFoundException.
