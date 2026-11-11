Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-scan-on-index-consumed-capacity-attributes-the-index
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-scan-on-index-consumed-capacity-attributes-the-index dynamodb-transact-get-items-no-matching-projection-item-presence dynamodb-transact-get-items-top-level-vs-cancellation-error-class
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|Scan|2xx->2xx|$.ConsumedCapacity.GlobalSecondaryIndexes.gsi1.CapacityUnits
Issue-Key: dynamodb|Scan|2xx->2xx|$.ConsumedCapacity.LocalSecondaryIndexes.lsi1.CapacityUnits
Issue-Key: dynamodb|TransactGetItems|2xx->2xx|$.Responses[*].Item
Issue-Key: dynamodb|TransactGetItems|2xx->2xx|$
Issue-Related: 20261110T232401Z-dynamodb-response-state-and-shape-diverge-on-replica.md
Issue-Commit: 0e3e027770f68a801846f0512bd9649d89ae04a7

# dynamodb: index-scan ConsumedCapacity is under-reported and TransactGetItems emits an empty Item map

> _Note: this issue was identified against the code base at commit `0e3e027770f68a801846f0512bd9649d89ae04a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Scan

## Description
Two successful-response shaping defects: an index Scan reports a quarter of the capacity it consumed, and TransactGetItems materialises an empty Item map for a slot that matched nothing. 5 causes.

Cause 1 of 5: Scan on a global secondary index under-reports ConsumedCapacity.
AWS: Smithy: ConsumedCapacity.CapacityUnits is 'the total number of capacity units consumed by the operation' and ConsumedCapacity.GlobalSecondaryIndexes is a SecondaryIndexesCapacityMap of 'the amount of throughput consumed on each global index affected by the operation'. Recorded behavior: for this scan both $.ConsumedCapacity.CapacityUnits and the gsi1 entry report the same, higher figure — the capacity is computed from the bytes the index scan actually read, not from a fixed per-call minimum.
Emulator: reported the per-call eventually-consistent minimum (0.5) for both the total and the index entry, i.e. a quarter of the recorded value.
Why: any client that budgets, meters, autoscales or alarms on ConsumedCapacity reads a figure far below what the operation cost, so capacity planning and throttling simulations are wrong; the GSI entry mis-attributes index cost too.
Fix: the Scan handler's capacity accounting for index reads — size the units from the scanned index bytes rather than emitting the floor, and propagate the same figure into the GlobalSecondaryIndexes map.
Traces: dynamodb-scan-on-index-consumed-capacity-attributes-the-index#task#5.

Cause 2 of 5: the same under-reporting on a local secondary index.
AWS: as cause 1, with the figure also appearing under $.ConsumedCapacity.LocalSecondaryIndexes.<indexName>.CapacityUnits.
Emulator: 0.5 for both the total and the LSI entry.
Why: as cause 1.
Fix: as cause 1, on the LSI path.
Traces: dynamodb-scan-on-index-consumed-capacity-attributes-the-index#task#6.

Cause 3 of 5: TransactGetItems emits Item: {} for a slot that matched no item.
AWS: Smithy: TransactGetItemsOutput.Responses is an ItemResponseList of ItemResponse, whose only member Item is OPTIONAL. Recorded behavior: when a Get in the transaction matches nothing (or its ProjectionExpression selects nothing), the corresponding ItemResponse is emitted as {} — the Item member is ABSENT, not an empty map.
Emulator: emits {"Item": {}} — the member is present and empty.
Why: SDKs distinguish 'no item' from 'an item with no attributes'. A client testing Responses[i].Item for presence (hasItem / item != nil / 'Item' in resp) concludes the item exists and then reads attributes off an empty map, so misses are processed as hits.
Fix: the TransactGetItems response serializer — omit the Item member entirely when the slot matched nothing, rather than serializing an empty AttributeMap.
Traces: dynamodb-transact-get-items-no-matching-projection-item-presence#task#5.

Cause 4 of 5: the same defect on two slots of one transaction.
AWS: as cause 3, for both Responses[0] and Responses[1].
Emulator: both carry Item: {}.
Why: as cause 3.
Fix: as cause 3.
Traces: dynamodb-transact-get-items-no-matching-projection-item-presence#task#7.

Cause 5 of 5: the same defect on the single-slot response of the error-class fixture.
AWS: recorded behavior — the whole body is {"Responses": [{}]}.
Emulator: {"Responses": [{"Item": {}}]}.
Why: as cause 3.
Fix: as cause 3.
Traces: dynamodb-transact-get-items-top-level-vs-cancellation-error-class#task#8.

Related but not covering: open issue 20261111T170613Z-dynamodb-consumedcapacity-numbers-diverge-on-index-reads-and.md covers other ConsumedCapacity causes; it does not carry these clusters' replay keys.

## Evidence
Cause 1: Scan 200 — $.ConsumedCapacity.CapacityUnits expected 2.0, got 0.5; $.ConsumedCapacity.GlobalSecondaryIndexes.gsi1.CapacityUnits expected 2.0, got 0.5.
Cause 2: Scan 200 — $.ConsumedCapacity.CapacityUnits expected 2.0, got 0.5; $.ConsumedCapacity.LocalSecondaryIndexes.lsi1.CapacityUnits expected 2.0, got 0.5.
Cause 3: TransactGetItems 200 — $.Responses[0].Item expected absent, got {}.
Cause 4: TransactGetItems 200 — $.Responses[0].Item and $.Responses[1].Item both expected absent, both got {}.
Cause 5: TransactGetItems 200 — whole body expected {'Responses': [{}]}, got {'Responses': [{'Item': {}}]}.

## Steps to Reproduce
1. Replay dynamodb-scan-on-index-consumed-capacity-attributes-the-index#task#5 (GSI) and #task#6 (LSI): ConsumedCapacity.CapacityUnits and the per-index entry must both reflect the bytes the index scan read, not the 0.5 per-call floor.
2. Replay dynamodb-transact-get-items-no-matching-projection-item-presence#task#5 and #task#7, and dynamodb-transact-get-items-top-level-vs-cancellation-error-class#task#8: every ItemResponse for a slot that matched nothing must serialize as {} with no Item member.
