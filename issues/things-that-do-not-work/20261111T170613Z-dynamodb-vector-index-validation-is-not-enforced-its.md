Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.create-table-vector-index-two-hash-search-schema-elements-rejected dynamodb.update-table-vector-index-requires-pay-per-request dynamodb.put-item-vector-hash-search-schema-missing-vs-empty-string dynamodb.vector-index-creating-blocks-other-index-but-accepts-self-delete dynamodb.vector-index-creating-blocks-table-and-index-delete dynamodb.vector-index-query-and-scan-rejected dynamodb.search-vectors-dot-product-order-and-score-precision dynamodb.search-vectors-consumed-capacity-is-vector-bytes-only-floored-and-scales dynamodb.vector-capacity-sized-by-dimension-count-not-number-width dynamodb.vector-write-request-bytes-floor-and-per-dimension-cost dynamodb.vector-write-request-bytes-name-and-projection-terms dynamodb.put-item-vector-index-charged-only-when-projection-changes
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.create-table-vector-index-two-hash-search-schema-elements-rejected|_error#9|AssertionError|raise AssertionError(f'expected the call to be refused, but it succeeded: {result!r}')
Issue-Key: dynamodb.update-table-vector-index-requires-pay-per-request|_error#6|AssertionError|raise AssertionError(f'expected the call to be refused, but it succeeded: {result!r}')
Issue-Key: dynamodb.put-item-vector-hash-search-schema-missing-vs-empty-string|_error#21|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: dynamodb.vector-index-creating-blocks-other-index-but-accepts-self-delete|run#14|AssertionError|assert err.get("Code") == "LimitExceededException", (
Issue-Key: dynamodb.vector-index-creating-blocks-table-and-index-delete|run#17|AssertionError|assert delete_index_err.get("Message") == delete_index_msg, (
Issue-Key: dynamodb.vector-index-query-and-scan-rejected|run#12|AssertionError|assert query_got == _QUERY_REJECTED, (
Issue-Key: dynamodb.search-vectors-dot-product-order-and-score-precision|run#22|KeyError|searched_emb = by_pk['d']['emb']['L'][0]['N']
Issue-Key: dynamodb.search-vectors-consumed-capacity-is-vector-bytes-only-floored-and-scales|run#11|AssertionError|assert floor_bytes == 1024.0, (
Issue-Key: dynamodb.vector-capacity-sized-by-dimension-count-not-number-width|run#14|AssertionError|assert edge_small.get("Table", {}).get("CapacityUnits") == 2.0, (
Issue-Key: dynamodb.vector-write-request-bytes-name-and-projection-terms|run#23|AssertionError|assert p_b == 1221.0, (
Issue-Key: dynamodb.put-item-vector-index-charged-only-when-projection-changes|run#11|AssertionError|assert set(cc2) == {'idx-all'}, (
Issue-Related: 20261109T021218Z-dynamodb-3-net-new-vector-index-causes-createtable.md
Issue-Commit: cdd06381ed07ce191d4321618e22d6aefb363f32

# dynamodb: vector-index validation is not enforced, its CREATING-state refusals use the generic table message, and vector capacity is mis-sized (9 causes)

> _Note: this issue was identified against the code base at commit `cdd06381ed07ce191d4321618e22d6aefb363f32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Nine net-new root causes in the vector-index surface (CreateTable/UpdateTable VectorIndexes, PutItem against a vector index, SearchVectors, and Query/Scan naming one). Smithy: SearchVectors takes a required IndexName and SearchVector (@length 1..4096) and returns a VectorCapacity ConsumedCapacity; UpdateTable carries VectorIndexUpdates; the model expresses no SearchSchema or billing-mode rules, so each refusal's 400 ValidationException message and the capacity numbers are the contract (variable parts: index and attribute names, dimension counts, table name).

Cause 1 of 9 — a VectorIndexes SearchSchema holding TWO HASH elements is accepted. AWS refuses the CreateTable; the emulator answers 200 and reports the index CREATING with both HASH elements. Fix: CreateTable SearchSchema validation. Traces: create-table-vector-index-two-hash-search-schema-elements-rejected.

Cause 2 of 9 — UpdateTable adding a vector index to a PROVISIONED table is accepted. AWS requires PAY_PER_REQUEST and refuses; the emulator answers 200 with the index CREATING. Fix: UpdateTable VectorIndexUpdates billing-mode guard. Traces: update-table-vector-index-requires-pay-per-request.

Cause 3 of 9 — PutItem with a missing or empty-string vector search-schema key attribute is accepted. AWS refuses the write; the emulator answers 200. The related refusal message also names the attribute as an empty string ('no AttributeDefinition was provided for search schema attribute '). Fix: PutItem vector-index write validation. Traces: put-item-vector-hash-search-schema-missing-vs-empty-string.

Cause 4 of 9 — the refusal class while a vector index is CREATING. AWS refuses a second index change with LimitExceededException; the emulator answers ResourceInUseException 'Attempt to change a resource which is still in use: Table is being updated: <table>'. Why: a client that retries on LimitExceededException and aborts on ResourceInUseException takes the wrong branch. Fix: the index-concurrency guard's error class in UpdateTable. Traces: vector-index-creating-blocks-other-index-but-accepts-self-delete.

Cause 5 of 9 — deleting a vector index while it is CREATING uses the generic table message. AWS: 'Attempt to change a resource which is still in use: Index creation is in resource allocation phase. Retry deletion during backfilling phase or when the index is active. Table: <table> Index: <index>'; the emulator: 'Attempt to change a resource which is still in use: Table is being updated: <table>'. Fix: the vector-index delete-while-creating diagnostic. Traces: vector-index-creating-blocks-table-and-index-delete.

Cause 6 of 9 — Query/Scan naming a vector index. AWS: ValidationException 'Query operation not supported on this index type.'; the emulator: 'The table does not have the specified index: <index>', i.e. it reports the index as absent. Fix: index-type resolution in the Query/Scan handlers. Traces: vector-index-query-and-scan-rejected.

Cause 7 of 9 — SearchVectors does not return the vector attribute its results must carry: reading the result item raises KeyError 'emb', so the dot-product ordering and score precision are unobservable. Fix: the SearchVectors result projection/serializer. Traces: search-vectors-dot-product-order-and-score-precision.

Cause 8 of 9 — SearchVectors ConsumedCapacity does not apply the 1 KB floor and is not derived from vector bytes only: VectorSearchRequestBytes came back 7300.0 where AWS reports 1024.0 (the 1 KB minimum). Fix: SearchVectors capacity computation. Traces: search-vectors-consumed-capacity-is-vector-bytes-only-floored-and-scales.

Cause 9 of 9 — vector WRITE capacity is sized from the wrong terms. AWS sizes it by dimension count (not the width of each number), applies the per-write floor, and adds the attribute-name and projection terms. Emulator: 1.0 WCU where 2.0 is due just past the floor, 9.0 where 17.0 is due, and 1206.0 where 1221.0 is due for a projected attribute; a put that changes only an attribute one of two indexes projects is charged against both. Fix: the vector write-capacity accounting (dimension, floor, name and projection terms) in the PutItem/vector-index write path. Traces: vector-capacity-sized-by-dimension-count-not-number-width, vector-write-request-bytes-floor-and-per-dimension-cost, vector-write-request-bytes-name-and-projection-terms, put-item-vector-index-charged-only-when-projection-changes.

## Evidence
Expected vs actual, verbatim from the replay:
- create-table-vector-index-two-hash-search-schema-elements-rejected: 'expected the call to be refused, but it succeeded' — TableDescription.VectorIndexes[0].SearchSchema held two SearchSchemaElementType 'HASH' entries.
- update-table-vector-index-requires-pay-per-request: 'expected the call to be refused, but it succeeded' on a table with ProvisionedThroughput {ReadCapacityUnits: 1, WriteCapacityUnits: 1}.
- put-item-vector-hash-search-schema-missing-vs-empty-string: 'expected the call to be rejected, but it succeeded' (HTTP 200); related message 'Invalid SearchSchema for index idx: no AttributeDefinition was provided for search schema attribute '.
- vector-index-creating-blocks-other-index-but-accepts-self-delete: 'expected LimitExceededException, got 'ResourceInUseException' ('Attempt to change a resource which is still in use: Table is being updated: <table>')'.
- vector-index-creating-blocks-table-and-index-delete: expected 'Attempt to change a resource which is still in use: Index creation is in resource allocation phase. Retry deletion during backfilling phase or when the index is active. Table: <table> Index: <index>', got 'Attempt to change a resource which is still in use: Table is being updated: <table>'.
- vector-index-query-and-scan-rejected: Query naming the vector index raised ('ValidationException', 'The table does not have the specified index: vec-idx'), expected ('ValidationException', 'Query operation not supported on this index type.').
- search-vectors-dot-product-order-and-score-precision: KeyError 'emb' — the vector attribute is absent from the result item.
- search-vectors-consumed-capacity-is-vector-bytes-only-floored-and-scales: 'VectorSearchRequestBytes was 7300.0, expected 1024.0 (the 1 KB minimum)'.
- vector-capacity-sized-by-dimension-count-not-number-width: 'table WCU just past the floor was {'WriteCapacityUnits': 1.0, 'CapacityUnits': 1.0}, expected 2.0'.
- vector-write-request-bytes-floor-and-per-dimension-cost: 'table WCU for this write was 9.0, expected 17.0'.
- vector-write-request-bytes-name-and-projection-terms: "idx-p with a projected 'extra' attribute: 1206.0, expected 1221.0".
- put-item-vector-index-charged-only-when-projection-changes: "changing 'note', which only idx-all projects, must charge idx-all and not idx-keys; VectorIndexes was ['idx-all', 'idx-keys']".
(Index/table names and the specific byte and WCU figures are this run's values; the rules — dimension-count sizing, the 1 KB floor, name and projection terms, and charging only the indexes whose projection changed — are the requirement.)

## Steps to Reproduce
1. CreateTable PAY_PER_REQUEST with a VectorIndexes entry whose SearchSchema has two HASH elements — expect a refusal.
2. CreateTable PROVISIONED, then UpdateTable with VectorIndexUpdates adding a vector index — expect a refusal.
3. On a table with a vector index whose SearchSchema names a HASH attribute, PutItem omitting that attribute and PutItem with it set to "" — expect refusals.
4. While a vector index is CREATING, issue a second index change (expect LimitExceededException) and a delete of that index (expect the 'resource allocation phase' message).
5. Query and Scan naming the vector index — expect 'Query operation not supported on this index type.'
6. SearchVectors with a ProjectionExpression covering the vector attribute and read the result items; read ConsumedCapacity.VectorSearchRequestBytes for a small search vector (expect the 1024 floor).
7. PutItem vectors of known dimension counts and read the table WCU; with two indexes (ALL and KEYS_ONLY) change only an attribute the ALL index projects and check only that index is charged.
