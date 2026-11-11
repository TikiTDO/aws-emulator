Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-legacy-attributevaluelist-without-comparator-and-mixed-type-in
Issue-Step: task#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-legacy-attributevaluelist-without-comparator-and-mixed-type-in dynamodb-legacy-expected-between-reversed-rejected dynamodb-legacy-write-expected-comparisonoperator-type-mismatch dynamodb-search-vectors-request-validation-order-and-type-rejected dynamodb-vector-index-and-search-lower-bound-validation-rejected scan-filter-function-operand-count-and-type-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|UpdateItem|4xx->2xx|$
Issue-Key: dynamodb|UpdateItem|4xx->2xx|status
Issue-Key: dynamodb|Scan|2xx->4xx|$.Count
Issue-Key: dynamodb|Scan|2xx->4xx|$.Items
Issue-Key: dynamodb|Scan|2xx->4xx|$.ScannedCount
Issue-Key: dynamodb|Scan|2xx->4xx|status
Issue-Related: 20261109T055139Z-dynamodb-validation-refusal-messages-are-emulator-authored-for.md
Issue-Related: 20261111T221143Z-dynamodb-item-and-batch-operations-answer-validationexception-where.md
Issue-Related: 20261111T221143Z-dynamodb-malformed-request-bodies-and-explicit-nulls-are.md
Issue-Commit: 0e3e027770f68a801846f0512bd9649d89ae04a7

# dynamodb: legacy condition and operand validation reaches the wrong verdict on four request shapes

> _Note: this issue was identified against the code base at commit `0e3e027770f68a801846f0512bd9649d89ae04a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateItem

## Description
Four request shapes where the validator reaches the wrong verdict: two legacy condition defects are accepted or mis-classified, a vector-operand refusal is reworded, and a valid Scan filter is refused. 4 causes.

Cause 1 of 4: UpdateItem accepts a legacy AttributeValueList whose members are of mixed type.
AWS: recorded behavior — status 400, __type 'com.amazon.coral.validate#ValidationException', message '1 validation error detected: One or more parameter values were invalid: AttributeValues inside AttributeValueList must be of same type' (fixed text, no variable part).
Emulator: status 200 with an empty body — the update is applied with no __type and no message.
Why: a conditional/legacy update AWS refuses is performed, so the item is mutated by a request the client would have had to correct.
Fix: the legacy Expected/AttributeUpdates condition validation reached by UpdateItem — enforce that all AttributeValueList members share one AttributeValue type before the write.
Traces: dynamodb-legacy-attributevaluelist-without-comparator-and-mixed-type-in#task#13.

Cause 2 of 4: DeleteItem reports a reversed legacy BETWEEN range as a failed condition rather than an invalid one.
AWS: recorded behavior — status 400, __type 'com.amazon.coral.validate#ValidationException', message '1 validation error detected: One or more parameter values were invalid: The BETWEEN condition was provided a range where the lower bound is greater than the upper bound' (fixed text, no variable part). The range is rejected as malformed; it is never evaluated.
Emulator: status 400 but __type 'com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException' with message 'The conditional request failed' — it evaluated the malformed range and reported the evaluation result.
Why: the error class is wrong in a way that changes client behaviour: ConditionalCheckFailedException means "your precondition did not hold, the data is as it is" and is normally handled as an expected outcome (retry, read-modify-write), whereas ValidationException means "your request is malformed". A client silently treats a broken condition as a lost race and loops.
Fix: the legacy Expected condition validation — validate BETWEEN bound ordering before evaluating the condition, and raise ValidationException.
Traces: dynamodb-legacy-expected-between-reversed-rejected#task#6.

Cause 3 of 4: SearchVectors' invalid-search-vector refusal is reworded into AWS's constraint envelope.
AWS: recorded behavior — status 400 ValidationException, message 'Search vector contains invalid values. All values in the search vector must be a 32-bit floating-point number attribute' (fixed text, no variable part; not wrapped in the '1 validation error detected: ' envelope). Smithy: SearchVectors.SearchVector is @required with target SearchVectorList (@length 1..4096), so the element-type rule is service-enforced rather than modeled.
Emulator: message "1 validation error detected: Value at 'searchVector' failed to satisfy constraint: Member must hold only numbers, each a 32-bit IEEE-754 floating point value" — it wrapped an emulator-authored constraint sentence in the generic envelope AWS does not use for this refusal.
Why: tooling and user-facing surfaces that match AWS's sentence do not match, and the envelope misrepresents a service-level rule as a modeled constraint violation.
Fix: SearchVectors' input validation — emit AWS's fixed sentence directly, without the constraint envelope.
Traces: dynamodb-search-vectors-request-validation-order-and-type-rejected#task#5.

Cause 4 of 4: Scan refuses a valid filter-function request.
AWS: recorded behavior — status 200 with $.Count = 1, $.ScannedCount = 1 and the matching item in $.Items; the filter function's operand count and types are valid here.
Emulator: status 400 — the scan is refused and Count, ScannedCount and Items are all absent.
Why: over-rejection is worse than silent acceptance for a client: a correct query simply cannot be run, so the code path is broken outright rather than returning a wrong answer.
Fix: the Scan FilterExpression function-operand validation — its operand-count/type rule is too strict and refuses an arity AWS accepts.
Traces: scan-filter-function-operand-count-and-type-rejected#task#14.

## Evidence
Cause 1: UpdateItem — status expected 400, got 200 with an empty body; expected {'__type': 'com.amazon.coral.validate#ValidationException', 'message': '1 validation error detected: One or more parameter values were invalid: AttributeValues inside AttributeValueList must be of same type'}.
Cause 2: DeleteItem, status 400 both sides — $.__type expected 'com.amazon.coral.validate#ValidationException', got 'com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException'; $.message expected '1 validation error detected: One or more parameter values were invalid: The BETWEEN condition was provided a range where the lower bound is greater than the upper bound', got 'The conditional request failed'.
Cause 3: SearchVectors, status 400 both sides — $.message expected 'Search vector contains invalid values. All values in the search vector must be a 32-bit floating-point number attribute', got "1 validation error detected: Value at 'searchVector' failed to satisfy constraint: Member must hold only numbers, each a 32-bit IEEE-754 floating point value".
Cause 4: Scan — status expected 200, got 400; $.Count expected 1, $.ScannedCount expected 1 and $.Items expected the matching item, all missing.

## Steps to Reproduce
1. Replay dynamodb-legacy-attributevaluelist-without-comparator-and-mixed-type-in#task#13: UpdateItem must be refused 400 ValidationException with the mixed-type AttributeValueList sentence, and the item must not be updated.
2. Replay dynamodb-legacy-expected-between-reversed-rejected#task#6: DeleteItem must answer com.amazon.coral.validate#ValidationException with the reversed-BETWEEN sentence, not ConditionalCheckFailedException.
3. Replay dynamodb-search-vectors-request-validation-order-and-type-rejected#task#5 and compare the message to AWS's fixed 'Search vector contains invalid values. ...' sentence.
4. Replay scan-filter-function-operand-count-and-type-rejected#task#14: Scan must answer 200 with Count 1, ScannedCount 1 and the matching item.
