Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.update-item-size-ceiling-counts-key-and-untouched-attribute-bytes dynamodb.put-item-and-batch-write-item-size-limit-includes-all-projected-lsi-entry dynamodb.update-item-growth-over-size-limit-with-all-projected-lsi-entry dynamodb.number-byte-cost-exact-at-400kb-boundary-put-item-and-batch-write-item dynamodb.transact-write-items-put-and-update-accept-flat-409600-byte-item dynamodb.update-item-if-not-exists-size-checked-before-condition dynamodb.partiql-insert-and-update-item-size-boundary-at-409600-bytes dynamodb.transact-get-items-aggregate-size-over-4mb-rejected dynamodb.batch-get-item-oversized-echoes-consistent-read-in-unprocessed-keys dynamodb.batchwriteitem-request-over-16mib-rejected-413-before-auth dynamodb.batch-write-over-25-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.update-item-size-ceiling-counts-key-and-untouched-attribute-bytes|_error#17|AssertionError|raise AssertionError(f"expected UpdateItem to be rejected, but it succeeded: {result!r}")
Issue-Key: dynamodb.put-item-and-batch-write-item-size-limit-includes-all-projected-lsi-entry|_error#15|AssertionError|raise AssertionError(f'expected the call to be refused, but it succeeded: {result!r}')
Issue-Key: dynamodb.update-item-growth-over-size-limit-with-all-projected-lsi-entry|_error#16|AssertionError|raise AssertionError(f'expected the call to be refused, but it succeeded: {result!r}')
Issue-Key: dynamodb.number-byte-cost-exact-at-400kb-boundary-put-item-and-batch-write-item|expect_error#6|AssertionError|raise AssertionError(f'expected the call to be refused, but it succeeded: {result!r}')
Issue-Key: dynamodb.transact-write-items-put-and-update-accept-flat-409600-byte-item|run#26|AssertionError|assert code == ERR_TXN_CANCELLED, (
Issue-Key: dynamodb.update-item-if-not-exists-size-checked-before-condition|_expect_error#4|AssertionError|assert got == (code, message), f'expected {(code, message)!r}, got {got!r}'
Issue-Key: dynamodb.transact-get-items-aggregate-size-over-4mb-rejected|_error#13|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: dynamodb.batch-get-item-oversized-echoes-consistent-read-in-unprocessed-keys|run#10|AssertionError|assert spec is not None, (
Issue-Key: dynamodb.batchwriteitem-request-over-16mib-rejected-413-before-auth|run#18|AssertionError|assert status == 400, f"a small unsigned body gave {status!r}, not 400: {body!r}"
Issue-Key: dynamodb.batch-write-over-25-rejected|run#15|AssertionError|assert message == expected, f'expected message {expected!r}, got {message!r}'
Issue-Related: 20261111T170613Z-dynamodb-partiql-statement-parser-rejects-aws-s-grammar.md
Issue-Commit: cdd06381ed07ce191d4321618e22d6aefb363f32

# dynamodb: item-size, batch-size and request-size ceilings are not enforced from the stored item, so oversized writes are accepted (10 causes)

> _Note: this issue was identified against the code base at commit `cdd06381ed07ce191d4321618e22d6aefb363f32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
Ten net-new root causes in the size-limit layer. The pattern: the emulator computes an item/request size from the bytes it was handed in the call, rather than from the whole item that would be stored (key bytes, untouched attributes, projected index entries) or from the whole HTTP request, so writes AWS refuses with a 400 ValidationException are accepted and answered 200. Smithy: the item-size and aggregate-request limits are not expressible in the model — these are documented DynamoDB service limits (400 KB = 409600 bytes per item; 16 MB per BatchGetItem/BatchWriteItem response/request; 4 MB aggregate per transaction; BatchWriteItem @length 1..25 per table map) and each refusal is a 400 ValidationException whose message is the contract. Smithy does state BatchGetItem.UnprocessedKeys / BatchWriteItem.UnprocessedItems, which is how AWS defers work it will not do in one response.

Cause 1 of 10 — the UpdateItem size ceiling ignores the key and the untouched attributes already stored at that key. AWS refuses the update; the emulator answers 200. Fix: size computation in the UpdateItem state/apply path. Traces: update-item-size-ceiling-counts-key-and-untouched-attribute-bytes.

Cause 2 of 10 — an ALL-projected LSI entry is not counted toward the item size, on both the put and the update path. AWS refuses; the emulator answers 200. Fix: the item-size accounting shared by PutItem/BatchWriteItem/UpdateItem. Traces: put-item-and-batch-write-item-size-limit-includes-all-projected-lsi-entry, update-item-growth-over-size-limit-with-all-projected-lsi-entry.

Cause 3 of 10 — the byte cost of a Number attribute at the exact 400 KB boundary is under-counted, so a put one byte over the limit is accepted. Fix: the number-attribute byte sizing used by the size check. Traces: number-byte-cost-exact-at-400kb-boundary-put-item-and-batch-write-item.

Cause 4 of 10 — a TransactWriteItems member that grows an item past 409600 bytes must fail the transaction as TransactionCanceledException, not as a top-level ValidationException. Emulator: ValidationException 'Item size has exceeded the maximum allowed size'. Why: a client that distinguishes a request defect from a cancelled transaction mis-reports and may not retry. Fix: the TransactWriteItems per-member size check's error classification. Traces: transact-write-items-put-and-update-accept-flat-409600-byte-item.

Cause 5 of 10 — the update-path size refusal uses the put-path message. AWS words the update path 'Item size to update has exceeded the maximum allowed size'; the emulator serves 'Item size has exceeded the maximum allowed size' (fixed text in both; no variable part). It also applies the size check in the wrong order relative to the condition. Fix: the UpdateItem size-check message and its ordering vs ConditionExpression evaluation. Traces: update-item-if-not-exists-size-checked-before-condition.

Cause 6 of 10 — the PartiQL INSERT/UPDATE item-size boundary at 409600 bytes refuses a write that is exactly at the limit (ValidationException 'Item size has exceeded the maximum allowed size' where AWS accepts). Fix: the size check in the PartiQL write path. Traces: partiql-insert-and-update-item-size-boundary-at-409600-bytes.

Cause 7 of 10 — the TransactGetItems 4 MB aggregate response limit is not enforced; the call succeeds and returns every item. Fix: the TransactGetItems aggregate-size check. Traces: transact-get-items-aggregate-size-over-4mb-rejected.

Cause 8 of 10 — BatchGetItem never defers past the 16 MB response cap: 44 items of 389120 bytes come back whole with UnprocessedKeys {} instead of the over-cap keys (and so the echoed ConsistentRead in UnprocessedKeys is never observable). The open issue 20261031T145434Z-dynamodb-batchgetitem-never-defers-past-the-16-mb.md describes the same defect for other checks but does not carry this check's replay key. Fix: the BatchGetItem response-size accounting and UnprocessedKeys construction. Traces: batch-get-item-oversized-echoes-consistent-read-in-unprocessed-keys.

Cause 9 of 10 — a BatchWriteItem body over 16 MiB must be refused on request size (400, before credential validation); the emulator answers 403 UnrecognizedClientException 'The security token included in the request is invalid.' — it validates the signature first and never applies the request-size rule. Fix: the request-size guard ahead of the auth layer. Traces: batchwriteitem-request-over-16mib-rejected-413-before-auth.

Cause 10 of 10 — the BatchWriteItem over-25-requests refusal message is emulator-authored: AWS serializes the offending RequestItems value into "1 validation error detected: Value '<serialized request map>' at 'requestItems' failed to satisfy constraint: ..." (the serialized value comes from the request; the constraint sentence is fixed); the emulator's text diverges. Fix: the BatchWriteItem constraint-violation message builder. Traces: batch-write-over-25-rejected.

## Evidence
Expected vs actual, verbatim from the replay:
- update-item-size-ceiling-counts-key-and-untouched-attribute-bytes: 'expected UpdateItem to be rejected, but it succeeded' (HTTP 200, empty body).
- put-item-and-batch-write-item-size-limit-includes-all-projected-lsi-entry / update-item-growth-over-size-limit-with-all-projected-lsi-entry / number-byte-cost-exact-at-400kb-boundary-put-item-and-batch-write-item: 'expected the call to be refused, but it succeeded' (HTTP 200).
- transact-write-items-put-and-update-accept-flat-409600-byte-item: 'a TransactWriteItems Update growing an item one byte past 409600 bytes must be TransactionCanceledException, got 'ValidationException': 'Item size has exceeded the maximum allowed size''.
- update-item-if-not-exists-size-checked-before-condition: expected ('ValidationException', 'Item size to update has exceeded the maximum allowed size'), got ('ValidationException', 'Item size has exceeded the maximum allowed size').
- partiql-insert-and-update-item-size-boundary-at-409600-bytes: ClientError (ValidationException) 'Item size has exceeded the maximum allowed size' where AWS accepts the write.
- transact-get-items-aggregate-size-over-4mb-rejected: 'expected the call to be refused, but it succeeded' and the Responses list carried every item.
- batch-get-item-oversized-echoes-consistent-read-in-unprocessed-keys: '44 items of 389120 bytes must not fit one response, so ConsistentRead=True should leave <table> in UnprocessedKeys, got {}'.
- batchwriteitem-request-over-16mib-rejected-413-before-auth: 'a small unsigned body gave 403, not 400: b'{"__type":"UnrecognizedClientException","message":"The security token included in the request is invalid."}''.
- batch-write-over-25-rejected: expected a message of the form "1 validation error detected: Value '<requestItems serialized>' at 'requestItems' failed to satisfy constraint: ..."; the emulator's text differs.
(Item counts, byte totals and table names above are this run's values; the limits 409600 B, 4 MB, 16 MB and 25 requests are the requirement.)

## Steps to Reproduce
1. Create a PAY_PER_REQUEST table; for the LSI causes create one with an ALL-projected LSI.
2. PutItem an item just under 400 KB, then UpdateItem adding bytes that push the whole stored item (key + untouched attributes + projected LSI entry) past 409600 bytes — expect ValidationException 'Item size to update has exceeded the maximum allowed size'.
3. PutItem / BatchWriteItem an item whose Number attributes land it one byte past 409600 bytes — expect a refusal.
4. TransactWriteItems an Update that grows an item one byte past 409600 bytes — expect TransactionCanceledException.
5. ExecuteStatement a PartiQL INSERT of an item exactly 409600 bytes — expect success.
6. TransactGetItems enough large items to exceed 4 MB in aggregate — expect a refusal.
7. BatchGetItem 44 keys of ~389 KB items with ConsistentRead=true — expect a partial response naming the table in UnprocessedKeys.
8. POST a BatchWriteItem body over 16 MiB with no valid signature — expect a 400-class request-size refusal, not 403.
9. BatchWriteItem 26 PutRequests for one table — compare the message to the serialized-value form above.
