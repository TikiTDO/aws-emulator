Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-list-tables-non-json-body-serialization-exception
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-list-tables-non-json-body-serialization-exception dynamodb-list-tables-tolerates-unknown-and-null-members-then-folds-multi-errors dynamodb-search-vectors-explicit-null-member-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|ListTables|4xx->4xx|$.message
Issue-Key: dynamodb|ListTables|4xx->4xx|$headers.content-type
Issue-Key: dynamodb|ListTables|4xx->2xx|$.__type
Issue-Key: dynamodb|ListTables|4xx->2xx|status
Issue-Key: dynamodb|ListTables|2xx->4xx|$.TableNames
Issue-Key: dynamodb|ListTables|2xx->4xx|status
Issue-Key: dynamodb|SearchVectors|4xx->4xx|$.__type
Issue-Key: dynamodb|SearchVectors|4xx->4xx|$.message
Issue-Related: 20261111T221143Z-dynamodb-control-plane-and-tagging-operations-answer-validationexception.md
Issue-Commit: 0e3e027770f68a801846f0512bd9649d89ae04a7

# dynamodb: malformed request bodies and explicit nulls are mis-classified instead of SerializationException

> _Note: this issue was identified against the code base at commit `0e3e027770f68a801846f0512bd9649d89ae04a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
Body-level JSON defects are handled by the wrong layer: the emulator answers a ValidationException with its own parser prose where AWS answers a SerializationException, it lets one malformed body through as a 200, and it refuses a body AWS accepts. 5 causes.

Cause 1 of 5: a non-JSON ListTables body is reported as ValidationException.
AWS: recorded behavior — status 400 with __type 'com.amazon.coral.service#SerializationException'; the body carries no 'message' member for this class of failure.
Emulator: status 400 with __type 'ValidationException' and an emulator-authored $.message 'json parse error in lookahead'.
Why: an SDK cannot tell a protocol-level unparseable body from a semantic refusal, so it surfaces an input-validation error for what is really a transport/marshalling fault, and the invented prose leaks emulator internals.
Fix: the DynamoDB awsJson1_0 body parser — raise com.amazon.coral.service#SerializationException (no 'message' member) when the body is not JSON, before the request reaches any operation.
Traces: dynamodb-list-tables-non-json-body-serialization-exception#task#1.

Cause 2 of 5: the same non-JSON body, additionally answered with the wrong content-type.
AWS: recorded behavior — the error response's Content-Type header is 'application/json'.
Emulator: answered Content-Type 'application/x-amz-json-1.0', plus the same wrong __type and invented message as cause 1.
Why: a client or proxy keyed on the response media type mis-handles the error envelope.
Fix: the error-response serializer must set Content-Type 'application/json' on this refusal path.
Traces: dynamodb-list-tables-non-json-body-serialization-exception#task#2.

Cause 3 of 5: a third malformed ListTables body is accepted with 200.
AWS: recorded behavior — status 400 with __type 'com.amazon.coral.service#SerializationException'.
Emulator: status 200 with a normal ListTables result; no __type at all.
Why: a client sending a corrupt body gets a success and silently proceeds on a result the real service would never have produced — the malformed request is never detected.
Fix: same body parser; the lenient path that falls through to the handler must be removed.
Traces: dynamodb-list-tables-non-json-body-serialization-exception#task#3.

Cause 4 of 5: ListTables refuses a body AWS accepts (unknown and null members).
AWS: Smithy: ListTables input has only ExclusiveStartTableName and Limit, and awsJson1_0 requires unknown members and explicit nulls on non-required members to be ignored. Recorded behavior: status 200 with $.TableNames = [].
Emulator: status 400 — it refuses the request instead of ignoring the unmodelled/null members.
Why: forward-compatibility breaks. Any SDK or tool that sends a newer member, or marshals an unset optional as null, is refused outright on a request AWS serves.
Fix: the awsJson1_0 request deserializer — ignore unknown members, and treat an explicit null on a non-required member as absent.
Traces: dynamodb-list-tables-tolerates-unknown-and-null-members-then-folds-multi-errors#task#2.

Cause 5 of 5: SearchVectors reports an explicit null in a required string member as ValidationException with emulator prose.
AWS: recorded behavior — status 400 with __type 'com.amazon.coral.service#SerializationException' and message 'invalid type: null, expected a string at line 1 column 65'. The fixed part of that sentence is 'invalid type: null, expected a string at line 1 column '; the trailing column number is a value derived from the request body's own byte offset, not a constant. (Smithy: SearchVectors requires TableName (TableArn, string) and IndexName (string, @length 3..255, @pattern ^[a-zA-Z0-9_.-]+$) — a null there is a deserialization failure, not a constraint violation.)
Emulator: status 400 with __type 'ValidationException' and the emulator-authored message 'expected string, got null' — wrong class, and the message drops both the fixed wording and the request-derived offset.
Why: as cause 1, and a client that reports the parse position to the user loses it.
Fix: the awsJson1_0 deserializer must raise SerializationException for a null in a string-typed member and render the message with AWS's fixed wording plus the offset computed from the request body.
Traces: dynamodb-search-vectors-explicit-null-member-rejected#task#2.

## Evidence
Cause 1: ListTables — expected 400 $.__type 'com.amazon.coral.service#SerializationException' with no $.message; got 400 $.__type 'ValidationException', $.message 'json parse error in lookahead'.
Cause 2: ListTables — as cause 1 plus $headers.content-type expected 'application/json', got 'application/x-amz-json-1.0'.
Cause 3: ListTables — expected 400 with $.__type 'com.amazon.coral.service#SerializationException'; got 200 and no $.__type.
Cause 4: ListTables — expected 200 with $.TableNames = []; got 400.
Cause 5: SearchVectors — expected 400 $.__type 'com.amazon.coral.service#SerializationException', $.message 'invalid type: null, expected a string at line 1 column <offset>'; got 400 $.__type 'ValidationException', $.message 'expected string, got null'.

## Steps to Reproduce
1. Replay dynamodb-list-tables-non-json-body-serialization-exception#task#1, #task#2, #task#3: each must answer 400 with __type com.amazon.coral.service#SerializationException, no invented message, and Content-Type application/json; none may answer 200.
2. Replay dynamodb-list-tables-tolerates-unknown-and-null-members-then-folds-multi-errors#task#2: must answer 200 with TableNames [].
3. Replay dynamodb-search-vectors-explicit-null-member-rejected#task#2: must answer 400 SerializationException with AWS's 'invalid type: null, expected a string at line 1 column <offset>' wording.
