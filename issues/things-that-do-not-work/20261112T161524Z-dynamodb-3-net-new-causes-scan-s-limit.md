Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.query-limit-zero-rejected dynamodb.vector-capacity-sized-by-dimension-count-not-number-width dynamodb.vector-write-request-bytes-floor-and-per-dimension-cost
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.query-limit-zero-rejected|run#19|AssertionError|assert scan_message == scan_limit_message, f'unexpected Scan message {scan_message!r}'
Issue-Key: dynamodb.vector-capacity-sized-by-dimension-count-not-number-width|run#29|AssertionError|assert read_small_cu == 5.0, (
Issue-Key: dynamodb.vector-write-request-bytes-floor-and-per-dimension-cost|run#22|AssertionError|assert 'idx-large' not in second_per_index, (
Issue-Commit: 6e13a4a18fa8b7895706e9c065c56147de35a014

# dynamodb: 3 net-new causes — Scan's Limit=0 refusal uses Query's wording, a consistent GetItem of a vector item is under-charged, and PutItem meters vector indexes whose vector did not change

> _Note: this issue was identified against the code base at commit `6e13a4a18fa8b7895706e9c065c56147de35a014`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Scan

## Description
Three net-new root causes surviving in the dynamodb behavioral replay after the per-check owners were excluded. All three are dynamodb data-plane defects: one refusal-message divergence on Scan, and two consumed-capacity accounting divergences around vector indexes. (The open issue 20261111T170613Z-dynamodb-vector-index-validation-is-not-enforced-its.md covers EARLIER assertions of the two vector checks named below — the write-side byte/WCU figures — but not the assertions that now fail; those are the net-new causes 2 and 3 here.)

Cause 1 of 3 — Scan's Limit=0 refusal is worded like Query's.
AWS: Smithy: Scan.Limit and Query.Limit both target com.amazonaws.dynamodb#PositiveIntegerObject with @range min 1, so a Limit of 0 sent without client-side validation is refused 400 ValidationException by both operations. The model does not state the message text, so the wording is recorded behavior only, and the two operations word the SAME constraint differently. Scan: "1 validation error detected: Value '<limit>' at 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1" — fixed text is everything but <limit>, which is the rejected value from the request, and the member name is rendered lowercase ('limit') with the value quoted after "Value". Query: "1 validation error detected: Value at 'Limit' failed to satisfy constraint: Member must have value greater than or equal to 1" — no value, member name capitalized ('Limit').
Emulator: Scan with Limit=0 answers ValidationException with Query's wording, "1 validation error detected: Value at 'Limit' failed to satisfy constraint: Member must have value greater than or equal to 1": the rejected value is dropped and the member name is capitalized. Query's own refusal matches.
Why: a client (or a test/log assertion) keyed on AWS's per-operation constraint text does not match, and the diagnostic loses the offending value that AWS reports back, so the caller cannot tell from the message which value was rejected.
Fix: Scan's input/constraint validation and the constraint-violation message formatter it uses — the per-operation member rendering must not be shared with Query.
Traces: dynamodb.query-limit-zero-rejected.

Cause 2 of 3 — a strongly consistent GetItem of a vector-indexed item is charged from the stored bytes, not the billed vector size.
AWS: recorded behavior only (the model does not state capacity arithmetic). On a PAY_PER_REQUEST table with a 4096-dimension vector index, the write side is sized by dimension count — VectorWriteRequestBytes 16389.0 and 17.0 table WCU, identical for single-digit and 38-significant-digit numbers — and a strongly consistent GetItem of that item consumes the capacity implied by that same billed size (5.0 RCU for a 4096-dimension item), identically for the single-digit and the 38-digit item. The read cost therefore does not track the numbers' real stored width.
Emulator: the same consistent GetItem (ReturnConsumedCapacity=TOTAL) reports ConsumedCapacity.CapacityUnits 3.0 where 5.0 is due — the read is sized from the item's real stored bytes instead of the vector's billed size. (The dimension count and the resulting RCU figure are this run's values; the rule — consistent reads of a vector item sized by the vector's billed size, invariant to number width — is the requirement.)
Why: a client that reconciles data-plane ConsumedCapacity with cost or with throttle budgets under-counts reads of vector items, and capacity planning built on the emulator diverges from AWS.
Fix: the read-capacity sizing used by the GetItem handler — the item-size function behind ConsumedCapacity must use the vector billed size for vector attributes.
Traces: dynamodb.vector-capacity-sized-by-dimension-count-not-number-width.

Cause 3 of 3 — PutItem meters a vector index whose vector attribute did not change.
AWS: Smithy: ConsumedCapacity.VectorIndexes is documented as "The amount of throughput consumed on each vector index affected by the operation", each entry carrying VectorWriteRequestBytes for writes. An index whose vector attribute the write does not change is not affected and is absent from the map. Recorded: on a table with two vector indexes, re-putting the item with only the small vector's values changed returns VectorIndexes containing ONLY the small index's entry (its floored VectorWriteRequestBytes), with no entry for the untouched index.
Emulator: the second PutItem (ReturnConsumedCapacity=INDEXES) reports entries for BOTH indexes — the untouched large index is metered at its full VectorWriteRequestBytes (16397.0 in this run) alongside the changed small index (1024.0).
Why: write capacity is over-reported for every rewrite of a multi-vector item; a client reading ConsumedCapacity.VectorIndexes cannot tell which index a write actually touched, and cost/throttle accounting is inflated.
Fix: the vector write-metering in the PutItem path — each vector index's attribute must be compared against the stored item and only changed indexes emitted in ConsumedCapacity.VectorIndexes.
Traces: dynamodb.vector-write-request-bytes-floor-and-per-dimension-cost.

## Evidence
Expected vs actual, verbatim from the replay:
- dynamodb.query-limit-zero-rejected: AssertionError: unexpected Scan message "1 validation error detected: Value at 'Limit' failed to satisfy constraint: Member must have value greater than or equal to 1"; the check asserts Scan's message is "1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1" (the Query refusal, with "Value at 'Limit'", passed). Error code ValidationException matched on both.
- dynamodb.vector-capacity-sized-by-dimension-count-not-number-width: AssertionError: "a consistent GetItem of the 4096-dimension item cost 3.0 RCU, expected 5.0". The preceding write-side assertions passed: VectorWriteRequestBytes 1024.0 at 254 dims (floor), 1025.0 at 255 dims, 16389.0 at 4096 dims, and table WCU 1.0/2.0/17.0.
- dynamodb.vector-write-request-bytes-floor-and-per-dimension-cost: AssertionError: "the large vector's value did not change, but idx-large was metered anyway: {'idx-small': {'VectorWriteRequestBytes': 1024.0}, 'idx-large': {'VectorWriteRequestBytes': 16397.0}}" — expected VectorIndexes to hold only 'idx-small'. The first write's figures (1024.0, 16397.0, 17.0 table WCU) passed.
(Table and index names, the dimension counts and the concrete byte/RCU/WCU figures are this run's values; the rules — Scan's per-operation constraint wording, consistent-read sizing from the vector billed size, and metering only the vector indexes a write affects — are the requirements.)

## Steps to Reproduce
1. CreateTable PAY_PER_REQUEST with a single HASH key, wait ACTIVE, PutItem one item. With client-side parameter validation disabled, call Scan(Limit=0) and Query(Limit=0): both must be refused 400 ValidationException, Query with "1 validation error detected: Value at 'Limit' failed to satisfy constraint: Member must have value greater than or equal to 1" and Scan with "1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1".
2. CreateTable PAY_PER_REQUEST with VectorIndexes of 254, 255 and 4096 dimensions (COSINE, KEYS_ONLY), wait for the table and every vector index to be ACTIVE. PutItem items whose vector attributes hold single-digit and 38-digit numbers, then GetItem(ConsistentRead=True, ReturnConsumedCapacity='TOTAL') the 4096-dimension items: both must report CapacityUnits 5.0.
3. CreateTable PAY_PER_REQUEST with two vector indexes (4 dimensions on emb_small, 4096 on emb_large), wait ACTIVE, PutItem both vectors with ReturnConsumedCapacity='INDEXES'. Re-PutItem the same key changing ONLY emb_small's values and read ConsumedCapacity.VectorIndexes: it must contain only the small index's entry.
