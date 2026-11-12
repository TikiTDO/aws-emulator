Issue-Source: trace-sweep-divergence
Issue-Trace: ddb-sfa-traj-partiql-is-not-missing-route
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ddb-sfa-traj-partiql-is-not-missing-route dynamodb-batch-execute-statement-returning-update-vs-delete dynamodb-ddbstrip-traj-mistyped-key-remove-is-silent-noop dynamodb-number-precision-38-significant-digits dynamodb-ddbstrip-traj-partiql-bulk-update-mistakes dynamodb-ddbstrip-traj-request-shape-mistakes dynamodb-partiql-select-nested-document-paths dynamodb-trajwf-scan-limit-equals-item-count-pages-once-more dynamodb-vector-write-request-bytes-floor-and-per-dimension-cost dynamodb-rbp-traj-arn-sent-to-the-other-region dynamodb-rbp-traj-paged-list-describe-get dynamodb-rbp-traj-stale-arn-after-table-deletion dynamodb-rbp-traj-stream-arn-has-no-table-policy dynamodb-rbp-traj-table-name-instead-of-arn dynamodb-ddbkds-traj-reference-active-inventory
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|ExecuteStatement|2xx->2xx|$.Items[*]
Issue-Key: dynamodb|BatchExecuteStatement|2xx->2xx|$.Responses[*].Error.Message
Issue-Key: dynamodb|UpdateItem|2xx->2xx|$
Issue-Key: dynamodb|GetItem|2xx->2xx|$
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item
Issue-Key: dynamodb|ExecuteStatement|2xx->2xx|$.Items[*].l[*]
Issue-Key: dynamodb|Scan|2xx->2xx|$.LastEvaluatedKey
Issue-Key: dynamodb|PutItem|2xx->2xx|$.ConsumedCapacity.CapacityUnits
Issue-Key: dynamodb|PutItem|2xx->2xx|$.ConsumedCapacity.Table.CapacityUnits
Issue-Key: dynamodb|PutItem|2xx->2xx|$.ConsumedCapacity.VectorIndexes
Issue-Key: dynamodb|GetResourcePolicy|2xx->2xx|$.RevisionId
Issue-Key: dynamodb|DisableKinesisStreamingDestination|2xx->4xx|$.DestinationStatus
Issue-Key: dynamodb|DisableKinesisStreamingDestination|2xx->4xx|$.StreamArn
Issue-Key: dynamodb|DisableKinesisStreamingDestination|2xx->4xx|$.TableName
Issue-Key: dynamodb|DisableKinesisStreamingDestination|2xx->4xx|status
Issue-Related: 20261030T060227Z-dynamodb-transaction-and-partiql-responses-omit-capacity-table.md
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# DynamoDB item and PartiQL responses return wrong items and omit modeled members

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Ten net-new root causes where the status matches AWS but the response body does not: wrong item sets, writes that do not take effect, members AWS models that are omitted, and one valid call refused. Related open issues cover neighbouring symptoms (20261030T060227Z-dynamodb-transaction-and-partiql-responses-omit-capacity-table.md, 20261030T060227Z-dynamodb-scan-returns-the-wrong-item-set-and.md); none of the ten steps below is owned by them per the replay report.

Cause 1 of 10 — ExecuteStatement returns a result set that does not satisfy the statement's predicate.
AWS: Smithy ExecuteStatementOutput.Items -> ItemList. A SELECT whose WHERE clause tests an attribute for presence returns exactly the items that satisfy it.
Emulator: items from a different key prefix are returned and items that do satisfy the predicate are missing, so both the membership and the length of Items are wrong.
Why: a client reads rows that do not match its query and misses rows that do — silent data corruption, with no error to key off.
Fix: the PartiQL WHERE/predicate evaluation in the ExecuteStatement handler.
Traces: ddb-sfa-traj-partiql-is-not-missing-route#task#6.

Cause 2 of 10 — BatchExecuteStatement: Responses entries omit TableName, and the DELETE returning-clause refusal is emulator-authored.
AWS: Smithy BatchStatementResponse.TableName (target TableName, length 3..255, pattern [a-zA-Z0-9_.-]+) is present on each response slot. A DELETE statement that asks for ALL NEW is refused in-band with Error.Message 'Invalid returning clause: RETURNING ALL NEW *. Only RETURNING ALL OLD * is allowed in DELETE statements.' — fixed text naming the offending clause.
Emulator: every Responses[i].TableName is absent, and the error text is 'Invalid Statement: a DELETE can only return the item as it stood before it: ALL OLD or MODIFIED OLD'.
Why: a client cannot correlate a batch response slot with its table, and does not recognise the refusal text.
Fix: the BatchExecuteStatement response serializer (emit TableName per slot) and its PartiQL returning-clause refusal message.
Traces: dynamodb-batch-execute-statement-returning-update-vs-delete#task#6.

Cause 3 of 10 — UpdateItem returns an Attributes member that was not requested.
AWS: Attributes is returned only as the request's ReturnValues asks; with the default (NONE) the body is empty — '{}'.
Emulator: the body carries Attributes with the item's key attributes.
Why: a client that keys off the presence of Attributes concludes a pre-existing item was modified when nothing was.
Fix: the UpdateItem response serializer's ReturnValues handling.
Traces: dynamodb-ddbstrip-traj-mistyped-key-remove-is-silent-noop#task#6.

Cause 4 of 10 — GetItem returns an Item for a key that holds no item.
AWS: when no item matches, the body omits Item entirely — '{}'.
Emulator: an Item consisting of just the key attributes is returned (hypothesis: the preceding no-op UpdateItem REMOVE created a key-only item, and it is then read back).
Why: a client's 'does this item exist' check answers yes for an item AWS never stored.
Fix: UpdateItem's REMOVE path must not create an item, and GetItem must omit Item when nothing matches.
Traces: dynamodb-ddbstrip-traj-mistyped-key-remove-is-silent-noop#task#7 (also dynamodb-number-precision-38-significant-digits#task#14).

Cause 5 of 10 — a PartiQL bulk UPDATE does not apply all of its SET clauses.
AWS: after the UPDATE, GetItem returns the item with every attribute the statement set.
Emulator: the item is missing the attributes the statement set (only the previously existing attributes come back).
Why: the write reports success and the data is not there — a lost update with no error.
Fix: the PartiQL UPDATE SET application in the statement execution path.
Traces: dynamodb-ddbstrip-traj-partiql-bulk-update-mistakes#task#10 (also dynamodb-ddbstrip-traj-request-shape-mistakes#task#11).

Cause 6 of 10 — a PartiQL SELECT over a nested document path drops a list element.
AWS: the projected list keeps every element at its original position, including an element whose value is the NULL attribute type, so positional access is stable.
Emulator: the element at that position is absent, so the list is shorter and later elements shift down (recorded behavior only — the model constrains the shape of the list, not this projection rule).
Why: a client indexing into a projected list reads the wrong element.
Fix: the document-path projection in the PartiQL/ExecuteStatement item serializer (do not elide NULL-typed list members).
Traces: dynamodb-partiql-select-nested-document-paths#task#4.

Cause 7 of 10 — Scan omits LastEvaluatedKey when Limit is reached by the final matching item.
AWS: Smithy ScanOutput.LastEvaluatedKey -> Key, and Scan is paginated with outputToken LastEvaluatedKey. When the Limit is reached, AWS returns LastEvaluatedKey even if the page happened to consume the last item, so the client must issue one more page to learn the scan is done.
Emulator: LastEvaluatedKey is omitted on that page.
Why: a paginating client stops one page early; with a different item distribution it would silently miss items, and 'Limit reached' and 'end of table' become indistinguishable.
Fix: the Scan handler's pagination/termination logic and its response serializer.
Traces: dynamodb-trajwf-scan-limit-equals-item-count-pages-once-more#task#6.

Cause 8 of 10 — PutItem into a table with vector indexes under-reports consumed capacity and omits the per-index breakdown.
AWS: the write's ConsumedCapacity.CapacityUnits (and ConsumedCapacity.Table.CapacityUnits) include the write-request units charged for each vector index as well as the base item, and the response carries ConsumedCapacity.VectorIndexes keyed by index name with VectorWriteRequestBytes and VectorWriteRequestUnits per index (recorded behavior: the on-disk model carries VectorIndexes on CreateTableInput and TableDescription; the per-index capacity members appear in the recording).
Emulator: CapacityUnits counts only the base item and ConsumedCapacity.VectorIndexes is absent.
Why: a client cannot see or budget the cost of vector-index writes, and capacity-based throttling/accounting is wrong.
Fix: the write-capacity metering for vector indexes and the ConsumedCapacity serializer.
Traces: dynamodb-vector-write-request-bytes-floor-and-per-dimension-cost#task#3.

Cause 9 of 10 — resource-policy RevisionId is served as a content digest instead of AWS's revision token.
AWS: PutResourcePolicy/GetResourcePolicy return a RevisionId in AWS's revision-token shape — the recordings show a decimal digit string that changes per revision, not a hash of the document.
Emulator: a 64-character lowercase hex digest derived from the policy document, so two revisions with identical text share an id and the value does not have AWS's shape.
Why: optimistic-concurrency flows that pass ExpectedRevisionId cannot distinguish revisions, and a client validating the token's shape rejects it.
Fix: resource-policy state — allocate and store a monotonic revision id per policy revision rather than hashing the document.
Traces: dynamodb-rbp-traj-arn-sent-to-the-other-region#task#6 (cluster also covers dynamodb-rbp-traj-paged-list-describe-get#task#11, dynamodb-rbp-traj-stale-arn-after-table-deletion#task#4, dynamodb-rbp-traj-stream-arn-has-no-table-policy#task#6, dynamodb-rbp-traj-table-name-instead-of-arn#task#7).

Cause 10 of 10 — DisableKinesisStreamingDestination refuses a destination that is active.
AWS: 200 with DestinationStatus DISABLING, plus the StreamArn and TableName echoed from the request.
Emulator: 400, and none of those members is returned.
Why: an enabled destination can never be disabled, so the client's teardown path is unreachable.
Fix: the DisableKinesisStreamingDestination handler's state guard and its response serializer.
Traces: dynamodb-ddbkds-traj-reference-active-inventory#task#17.

## Evidence
Replay of simparity-wire-aws-v1, expected -> actual:
1. ExecuteStatement ddb-sfa-traj-partiql-is-not-missing-route#task#6: Items[1], Items[2], Items[4] are the items matching the predicate -> items from a different key prefix at [1] and [2] and nothing at [4].
2. BatchExecuteStatement dynamodb-batch-execute-statement-returning-update-vs-delete#task#6: Responses[0].TableName and Responses[1].TableName present -> absent; Responses[2].Error.Message 'Invalid returning clause: RETURNING ALL NEW *. Only RETURNING ALL OLD * is allowed in DELETE statements.' -> 'Invalid Statement: a DELETE can only return the item as it stood before it: ALL OLD or MODIFIED OLD'.
3. UpdateItem dynamodb-ddbstrip-traj-mistyped-key-remove-is-silent-noop#task#6: body {} -> body carrying Attributes with the key attributes.
4. GetItem dynamodb-ddbstrip-traj-mistyped-key-remove-is-silent-noop#task#7: body {} -> body carrying a key-only Item (2 steps).
5. GetItem dynamodb-ddbstrip-traj-partiql-bulk-update-mistakes#task#10: Item carries the attributes the PartiQL UPDATE set -> those attributes are absent (2 steps).
6. ExecuteStatement dynamodb-partiql-select-nested-document-paths#task#4: Items[0] list retains its element at index 1 -> the element is dropped.
7. Scan dynamodb-trajwf-scan-limit-equals-item-count-pages-once-more#task#6: LastEvaluatedKey present (last-item key) -> absent.
8. PutItem dynamodb-vector-write-request-bytes-floor-and-per-dimension-cost#task#3: ConsumedCapacity.CapacityUnits and .Table.CapacityUnits include the vector-index write units and ConsumedCapacity.VectorIndexes carries VectorWriteRequestBytes/VectorWriteRequestUnits per index -> base-item units only, VectorIndexes absent.
9. GetResourcePolicy dynamodb-rbp-traj-arn-sent-to-the-other-region#task#6: RevisionId in AWS's revision-token shape -> a 64-char hex digest of the policy document (5 steps; the same defect on PutResourcePolicy).
10. DisableKinesisStreamingDestination dynamodb-ddbkds-traj-reference-active-inventory#task#17: 200 + DestinationStatus DISABLING + StreamArn + TableName -> 400 with none of them.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Representative: replay fixture ddb-sfa-traj-partiql-is-not-missing-route, step task#6 — a PartiQL SELECT whose result set does not match the recorded items.
3. For each remaining cause replay the fixture#step in trace_ids and diff the response body: BatchExecuteStatement Responses slots, the no-op UpdateItem REMOVE and the GetItem that follows it, a PartiQL bulk UPDATE read back with GetItem, a nested-document-path SELECT, a Scan whose Limit equals the matching item count, a PutItem into a vector-indexed table with ReturnConsumedCapacity, a resource-policy Get/Put RevisionId, and DisableKinesisStreamingDestination against an active destination.
