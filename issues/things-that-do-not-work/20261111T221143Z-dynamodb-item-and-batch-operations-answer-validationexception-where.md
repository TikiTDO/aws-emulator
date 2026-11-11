Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-batch-get-write-item-key-and-item-slot-scalar-rejected
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-batch-get-write-item-key-and-item-slot-scalar-rejected dynamodb-item-and-batch-operations-scalar-type-mismatch-rejected dynamodb-item-write-ops-map-member-scalar-rejected dynamodb-putitem-attributevalue-member-type-mismatches-rejected dynamodb-legacy-expected-and-attribute-updates-value-scalar-rejected dynamodb-legacy-query-key-conditions-attribute-value-list-scalar-rejected dynamodb-query-and-scan-scalar-type-mismatch-rejected dynamodb-query-scan-list-and-map-member-scalar-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|BatchGetItem|4xx->4xx|$.Message
Issue-Key: dynamodb|BatchGetItem|4xx->4xx|$.__type
Issue-Key: dynamodb|BatchWriteItem|4xx->4xx|$.Message
Issue-Key: dynamodb|BatchWriteItem|4xx->4xx|$.__type
Issue-Key: dynamodb|PutItem|4xx->4xx|$.Message
Issue-Key: dynamodb|DeleteItem|4xx->4xx|$.Message
Issue-Key: dynamodb|DeleteItem|4xx->4xx|$.__type
Issue-Key: dynamodb|UpdateItem|4xx->4xx|$.Message
Issue-Key: dynamodb|Query|4xx->4xx|$.Message
Issue-Key: dynamodb|Query|4xx->4xx|$.__type
Issue-Related: 20261104T061030Z-dynamodb-serves-the-wrong-error-class-for-malformed.md
Issue-Commit: 0e3e027770f68a801846f0512bd9649d89ae04a7

# dynamodb: item and batch operations answer ValidationException where AWS answers SerializationException

> _Note: this issue was identified against the code base at commit `0e3e027770f68a801846f0512bd9649d89ae04a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
When a JSON scalar in the request payload has the wrong primitive type for its modeled member, AWS fails DESERIALIZATION, not validation: it answers 400 with __type 'com.amazon.coral.service#SerializationException' and a capital-M 'Message' member. The emulator instead reaches its validator and answers __type 'ValidationException' with a lowercase 'message', so both the error class and the message slot are wrong. 6 causes, one per data-plane operation.

AWS (common to all six): recorded behavior plus the awsJson1_0 protocol contract — status 400, body {"__type": "com.amazon.coral.service#SerializationException", "Message": "<fixed deserializer text>"}. The fixed texts observed are exactly 'NUMBER_VALUE cannot be converted to String' (a JSON number supplied where the model declares a string, e.g. an AttributeValue 'S' slot or a key/table-name string), 'Unexpected field type' (a JSON scalar supplied where the model declares a structure/list/map) and 'Unexpected value type in payload' (a scalar inside an AttributeValue list slot). These strings have no variable parts: AWS does not name the member or echo the value. Note the member name is 'Message', not 'message' — Coral's SerializationException uses the capitalised slot.
Emulator (common to all six): status 400 is right, but __type is the bare 'ValidationException' (not even namespaced) and the fixed deserializer text is absent — $.Message is missing and the emulator writes its own prose into $.message instead.
Why (common to all six): SDKs branch on __type. A client that distinguishes a client-side marshalling bug (SerializationException — retry is pointless, the request is malformed) from a server-side input-validation refusal (ValidationException) takes the wrong branch, and error-message extraction reads the 'Message' slot and finds nothing, so the failure surfaces with an empty reason.
Fix (common to all six): the DynamoDB awsJson1_0 request deserializer — a primitive-type mismatch must be raised by the deserializer as com.amazon.coral.service#SerializationException with the fixed text in a 'Message' member, before the operation's validation layer is entered. The per-operation handlers should not see these payloads at all.

Cause 1 of 6: BatchGetItem — key slot given a JSON number where a string is modeled. Traces: dynamodb-batch-get-write-item-key-and-item-slot-scalar-rejected#task#3. Expected Message 'NUMBER_VALUE cannot be converted to String'.
Cause 2 of 6: BatchWriteItem — a JSON scalar where a structure is modeled. Traces: dynamodb-batch-get-write-item-key-and-item-slot-scalar-rejected#task#4. Expected Message 'Unexpected field type'.
Cause 3 of 6: PutItem — AttributeValue member type mismatch. Traces: dynamodb-item-and-batch-operations-scalar-type-mismatch-rejected#task#1. Expected Message 'NUMBER_VALUE cannot be converted to String'.
Cause 4 of 6: DeleteItem — same mismatch on the key map. Traces: dynamodb-item-and-batch-operations-scalar-type-mismatch-rejected#task#3. Expected Message 'NUMBER_VALUE cannot be converted to String'.
Cause 5 of 6: UpdateItem — same mismatch. Traces: dynamodb-item-and-batch-operations-scalar-type-mismatch-rejected#task#5. Expected Message 'NUMBER_VALUE cannot be converted to String'.
Cause 6 of 6: Query — scalar supplied inside a legacy KeyConditions AttributeValueList. Traces: dynamodb-legacy-query-key-conditions-attribute-value-list-scalar-rejected#task#4. Expected Message 'Unexpected value type in payload'.

Related but not covering: open issue 20261111T170613Z-dynamodb-non-constraint-validationexception-messages-are-wrapped-in.md concerns the wording of ValidationException messages; these six causes are about responses that must not be ValidationException at all.

## Evidence
All six: expected status 400 with $.__type = 'com.amazon.coral.service#SerializationException' and $.Message = the fixed deserializer text; got status 400 with $.__type = 'ValidationException' and $.Message missing.
Cause 1 (BatchGetItem): expected $.Message 'NUMBER_VALUE cannot be converted to String'.
Cause 2 (BatchWriteItem): expected $.Message 'Unexpected field type'.
Causes 3-5 (PutItem / DeleteItem / UpdateItem): expected $.Message 'NUMBER_VALUE cannot be converted to String'.
Cause 6 (Query): expected $.Message 'Unexpected value type in payload'.

## Steps to Reproduce
1. Replay dynamodb-batch-get-write-item-key-and-item-slot-scalar-rejected#task#3 and #task#4 (BatchGetItem, BatchWriteItem).
2. Replay dynamodb-item-and-batch-operations-scalar-type-mismatch-rejected#task#1, #task#3 and #task#5 (PutItem, DeleteItem, UpdateItem).
3. Replay dynamodb-legacy-query-key-conditions-attribute-value-list-scalar-rejected#task#4 (Query).
In each response assert $.__type == 'com.amazon.coral.service#SerializationException' and that $.Message holds the fixed deserializer sentence; assert $.message is absent.
