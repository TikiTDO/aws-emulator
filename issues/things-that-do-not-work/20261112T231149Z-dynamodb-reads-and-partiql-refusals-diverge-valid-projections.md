Issue-Source: trace-sweep-divergence
Issue-Trace: ddbval-nesting-depth-over-32-levels-rejected
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ddbval-nesting-depth-over-32-levels-rejected ddbval-reserved-keyword-in-projection-rejected dynamodb-get-item-expression-attribute-name-numeric-value-rejected dynamodb-item-and-batch-operations-scalar-type-mismatch-rejected dynamodb-item-write-ops-map-member-scalar-rejected dynamodb-refarch-rekreup-stale-row-read-addressing dynamodb-redundant-parentheses-expression-rejected dynamodb-batch-execute-statement-and-execute-transaction-reject-unconvertible-n-parameter
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|GetItem|2xx->4xx|$.Item
Issue-Key: dynamodb|GetItem|2xx->4xx|status
Issue-Key: dynamodb|GetItem|4xx->4xx|$.Message
Issue-Key: dynamodb|GetItem|2xx->2xx|$
Issue-Key: dynamodb|BatchExecuteStatement|2xx->2xx|$.Responses[*].Error.Message
Issue-Related: 20261104T061030Z-dynamodb-serves-the-wrong-error-class-for-malformed.md
Issue-Commit: c3c8a5d5636f74138d44709d078c46bc0a5a7c2e

# dynamodb: reads and PartiQL refusals diverge — valid projections refused, a stored item lost, wrong error class and message (4 causes)

> _Note: this issue was identified against the code base at commit `c3c8a5d5636f74138d44709d078c46bc0a5a7c2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetItem

## Description
Four net-new causes on the DynamoDB data plane. They share one fault: the verdict the emulator reaches on an item read or a PartiQL slot does not match AWS's — a request AWS serves is refused, a request AWS refuses has already mutated state so the read serves nothing, and where a refusal does occur its error class and message text are the emulator's own rather than AWS's.

Cause 1 of 4: GetItem answers HTTP 400 for a ProjectionExpression AWS serves, so the stored item is never returned.
AWS: Smithy: GetItem.ProjectionExpression (ProjectionExpression) with ExpressionAttributeNames; output member Item (AttributeMap); modeled errors are InternalServerError, InvalidEndpointException, ProvisionedThroughputExceededException, RequestLimitExceeded, ResourceNotFoundException and ThrottlingException — none covers a well-formed projection. A document path is allowed up to the documented maximum of 32 nesting levels, and only a path BEYOND that is refused (the next step of the same fixture shows AWS refusing a 33-level path with ValidationException 'Invalid ProjectionExpression: The document path has too many nesting levels; nesting levels: 33'). At the maximum depth, and for a reserved word projected through an ExpressionAttributeNames alias, AWS answers HTTP 200 with the item.
Emulator: HTTP 400 with no Item member for both of those accepted forms, i.e. the depth check is applied one level too early (it refuses a path at the documented maximum) and the reserved-word/alias probe is refused as well.
Why: a client reading a legitimately deep document, or projecting a reserved attribute name through an alias, gets a validation refusal instead of its data — the read is simply unavailable, and retrying cannot help.
Fix: GetItem's ProjectionExpression validation (the document-path depth bound must be >= 32 levels, and alias resolution must run before the reserved-word check), in the expression-validation component rather than the handler's response path. Related open issue 20261110T232401Z-dynamodb-expression-diagnostics-4-net-new-causes-operand.md covers only the TEXT of the over-depth refusal, not this false refusal.
Traces: ddbval-nesting-depth-over-32-levels-rejected#task#7, ddbval-reserved-keyword-in-projection-rejected#task#7 (both GetItem).

Cause 2 of 4: a wrong-typed JSON member in an item request is answered as ValidationException instead of SerializationException, with no message member at all.
AWS: recorded behavior only (the fault is in the JSON-1.0 deserializer, which the model does not shape): a request whose ExpressionAttributeNames value is a JSON number where a string is modeled is refused by the protocol layer with HTTP 400, '__type' = 'com.amazon.coral.service#SerializationException' and the message carried in the capitalised 'Message' member with the fixed text 'NUMBER_VALUE cannot be converted to String' (fixed for the number-into-string case; the type names in the sentence come from the offending JSON value and the modeled target type).
Emulator: HTTP 400 with '__type' = 'ValidationException' — unqualified, where AWS sends the fully qualified coral shape id — and NO 'Message' member in the body, so the client gets no diagnostic text whatsoever.
Why: SDKs map __type to the exception type they raise, so callers that catch SerializationException (a non-retryable protocol fault) see a modeled ValidationException instead, and an empty message makes the failure undiagnosable. The clustered steps show the same wrong class on GetItem for several malformed-value shapes.
Fix: the JSON request deserializer / protocol error path shared by the item operations (classify a type-coercion failure as SerializationException, emit the fully qualified __type and the 'Message' member). Related open issue 20261111T221143Z-dynamodb-item-and-batch-operations-answer-validationexception-where.md does not carry these replay keys.
Traces: dynamodb-get-item-expression-attribute-name-numeric-value-rejected#task#4, dynamodb-item-and-batch-operations-scalar-type-mismatch-rejected#task#7, dynamodb-item-write-ops-map-member-scalar-rejected#task#6, dynamodb-refarch-rekreup-stale-row-read-addressing#task#4 (GetItem).

Cause 3 of 4: GetItem serves an empty body for an item that must still exist, because a DeleteItem AWS refuses was executed.
AWS: Smithy: GetItem returns 200 with Item (AttributeMap) when the key exists. In this fixture the item is still present at this point because the preceding DeleteItem carried a ConditionExpression with redundant parentheses, which AWS refuses with ValidationException ('Invalid ConditionExpression: The expression has redundant parentheses;') and therefore never deletes.
Emulator: the preceding DeleteItem is accepted (200) and removes the row, so GetItem answers HTTP 200 with an EMPTY body ({}) and no Item member — the emulator reports the item as absent.
Why: an invalid conditional delete silently destroys data; subsequent reads report the row as missing, which is the opposite of AWS's outcome and makes any conditional-delete workflow non-reproducible.
Fix: DeleteItem's ConditionExpression validation must refuse before the state mutation (state must not be touched when expression parsing fails). The acceptance itself is tracked in open issue 20261111T221143Z-dynamodb-condition-key-and-filter-expression-defects-are.md, which does not carry this read's replay key; the read must be re-replayed to confirm the item survives.
Traces: dynamodb-redundant-parentheses-expression-rejected#task#7 (GetItem).

Cause 4 of 4: BatchExecuteStatement's per-slot Error.Message for a parameter that cannot be converted to a number is emulator-authored.
AWS: Smithy: BatchExecuteStatement.Statements (PartiQLBatchRequest, @length 1..25) returns 200 with a Responses list whose failing entries carry Error.Code/Error.Message rather than a top-level error. For an N-typed parameter whose value is not a representable number, AWS's per-slot message is 'The parameter cannot be converted to a numeric value: <value>' — fixed text up to the colon, with the offending parameter value from the request appended verbatim.
Emulator: the same slot reports 'Number overflow. Attempting to store a number with magnitude larger than supported range' — a different sentence that neither names the rule AWS states nor echoes the request's value.
Why: callers that match or log per-slot PartiQL error text (the only place batch failures are reported, since the call is 200) cannot recognise the conversion failure, and the offending value is lost from the diagnostic.
Fix: the PartiQL parameter-conversion validator that produces per-slot Error.Message for BatchExecuteStatement/ExecuteTransaction. Related open issue 20261111T221143Z-dynamodb-partiql-statement-and-parameter-refusals-diverge-in.md does not carry this replay key.
Traces: dynamodb-batch-execute-statement-and-execute-transaction-reject-unconvertible-n-parameter#task#5 (BatchExecuteStatement).

## Evidence
Cause 1 — ddbval-nesting-depth-over-32-levels-rejected#task#7 GetItem: expected HTTP 200 with $.Item = {'d': {'M': {'m': {'M': ... 'S': 'leaf'}}}} (a document nested to the documented 32-level maximum); got HTTP 400 and $.Item missing. Same cluster: ddbval-reserved-keyword-in-projection-rejected#task#7. The next step of the same fixture (task#8, 33 levels) shows AWS DOES refuse beyond 32 ('Invalid ProjectionExpression: The document path has too many nesting levels; nesting levels: 33').
Cause 2 — dynamodb-get-item-expression-attribute-name-numeric-value-rejected#task#4 GetItem, both HTTP 400: expected $.__type 'com.amazon.coral.service#SerializationException' and $.Message 'NUMBER_VALUE cannot be converted to String'; got $.__type 'ValidationException' and $.Message missing. Cluster also covers dynamodb-item-and-batch-operations-scalar-type-mismatch-rejected#task#7, dynamodb-item-write-ops-map-member-scalar-rejected#task#6, dynamodb-refarch-rekreup-stale-row-read-addressing#task#4.
Cause 3 — dynamodb-redundant-parentheses-expression-rejected#task#7 GetItem, both HTTP 200: expected {'Item': {'pk': {'S': 'seed'}, 'sk': {'S': 'a'}, 'status': {'S': 'active'}}}; got {} (empty body). The preceding DeleteItem (task#6) answered 200 where AWS answered 400 ValidationException '1 validation error detected: Invalid ConditionExpression: The expression has redundant parentheses;'.
Cause 4 — dynamodb-batch-execute-statement-and-execute-transaction-reject-unconvertible-n-parameter#task#5 BatchExecuteStatement, both HTTP 200: expected $.Responses[1].Error.Message 'The parameter cannot be converted to a numeric value: <value>' (the run's value was 1e99999999999); got 'Number overflow. Attempting to store a number with magnitude larger than supported range'.

## Steps to Reproduce
1. Replay fixture ddbval-nesting-depth-over-32-levels-rejected: PutItem a document nested to 32 levels, then GetItem it with the projection used at step task#7 — the emulator answers HTTP 400 instead of 200 with the item. Repeat with ddbval-reserved-keyword-in-projection-rejected step task#7.
2. Replay fixture dynamodb-get-item-expression-attribute-name-numeric-value-rejected step task#4: send GetItem whose ExpressionAttributeNames value is a JSON number and inspect the error body — __type is 'ValidationException' with no Message member, instead of 'com.amazon.coral.service#SerializationException' with Message 'NUMBER_VALUE cannot be converted to String'.
3. Replay fixture dynamodb-redundant-parentheses-expression-rejected: DeleteItem with a redundant-parentheses ConditionExpression (accepted by the emulator), then GetItem the seed key at step task#7 — the body is {} instead of the stored item.
4. Replay fixture dynamodb-batch-execute-statement-and-execute-transaction-reject-unconvertible-n-parameter step task#5: BatchExecuteStatement with an N parameter that cannot be converted and read Responses[1].Error.Message.
