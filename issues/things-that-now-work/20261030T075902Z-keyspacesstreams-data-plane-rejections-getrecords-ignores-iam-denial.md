Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: keyspacesstreams.get-records-denied-by-iam-policy-rejected keyspacesstreams.get-records-malformed-iterator-rejected keyspacesstreams.get-shard-iterator-missing-sequence-number-rejected keyspacesstreams.get-stream-nonexistent-arn-not-found
Issue-Shape-Id: com.amazonaws.keyspacesstreams#KeyspacesStreams
Issue-Key: keyspacesstreams.get-records-denied-by-iam-policy-rejected|_error#22|AssertionError|raise AssertionError(
Issue-Key: keyspacesstreams.get-records-malformed-iterator-rejected|run#14|AssertionError|assert code == "ValidationException", (
Issue-Key: keyspacesstreams.get-shard-iterator-missing-sequence-number-rejected|run#15|AssertionError|assert message == "One or more parameter values were invalid", (
Issue-Key: keyspacesstreams.get-stream-nonexistent-arn-not-found|run#13|AssertionError|assert err["Message"] == "Requested resource not found", (
Issue-Commit: 4a59741c1e9770f94255e8601a291141b8fe338c

# KeyspacesStreams data-plane rejections: GetRecords ignores IAM denial, and GetRecords/GetShardIterator/GetStream return the wrong error class and invented messages

> _Note: this issue was identified against the code base at commit `4a59741c1e9770f94255e8601a291141b8fe338c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecords

## Description
Four distinct net-new root causes in the keyspacesstreams emulator, all on the refusal path: one authorization bypass and three error-classification/message defects. Causes 2 and 4 are the behavioral-corpus manifestation of the already-open wire issue 20261029T173658Z-keyspacesstreams-maps-invalid-input-errors-to-wrong-type.md (its causes 1 and 5); the replay report did not mark these checks as owned by it, so they are filed here with that issue named — a fixer closing that issue should re-replay these check ids too. Causes 1 and 3 are not described anywhere in that issue.

Cause 1 of 4: GetRecords performs no identity-based authorization, so a principal without cassandra:GetRecords reads the stream.
AWS: Smithy (services/service-com-amazonaws-keyspacesstreams-keyspacesstreams/smithy-model/com-amazonaws-keyspacesstreams-keyspacesstreams.json) models AccessDeniedException on GetRecords with httpError 403. An IAM user whose only identity-based policy allows cassandra:GetStream and cassandra:GetShardIterator on the stream ARN, but not cassandra:GetRecords, is refused: HTTP 403, error code AccessDeniedException, message of the fixed form "User: <principal-arn> is not authorized to perform: cassandra:GetRecords on resource: <resource-arn> because no identity-based policy allows the cassandra:GetRecords action". Fixed text: "User: ", " is not authorized to perform: ", " on resource: ", " because no identity-based policy allows the ", " action"; variable parts taken from the request/caller: the caller's IAM user ARN (arn:aws:iam::<account>:user/<user-name>), the action name cassandra:GetRecords, and the stream ARN from the request's streamArn-derived shardIterator. The same principal, once granted cassandra:GetRecords, succeeds — so the refusal is purely authorization, not resource state.
Emulator: the call SUCCEEDS with HTTP 200 and a normal GetRecords body ({changeRecords: [], nextShardIterator: "ksstreams-iterator-...", iteratorDescription: {iteratorPosition: "AT_TIP"}}). No AccessDeniedException is ever produced; the restricted principal's credentials are accepted exactly like the admin caller's.
Why: the emulator is overly permissive on a data-plane read. Any test or control that asserts a least-privilege IAM boundary holds for CDC stream reads passes against the emulator and fails in AWS (or, worse, is believed verified); change-data records are handed to a principal AWS would refuse, so authorization regressions in customer code are invisible here.
Fix: GetRecords handler's authorization step — evaluate the caller's identity-based policy for cassandra:GetRecords on the stream resource before reading state, and raise AccessDeniedException (403) with AWS's message template when no statement allows it.
Traces: keyspacesstreams.get-records-denied-by-iam-policy-rejected.

Cause 2 of 4: GetRecords with a shardIterator the service never issued is reported as a missing resource instead of an input-format failure.
AWS: Smithy GetRecords.shardIterator is @required, target ShardIterator @length(1,4096); modeled errors include ValidationException (httpError 400) whose members are message and errorCode (enum ValidationExceptionType: InvalidFormat, TrimmedDataAccess, ExpiredIterator, ExpiredNextToken). A syntactically bogus iterator of valid length is refused with HTTP 400, code ValidationException, errorCode InvalidFormat, message "Invalid ShardIterator" — entirely fixed text, nothing interpolated from the request.
Emulator: returns ResourceNotFoundException (the check asserts the code and reports "got ResourceNotFoundException"), with no errorCode member.
Why: a permanent client-side format bug is surfaced in the resource-missing/retryable bucket, so callers that branch on ValidationException.errorCode == InvalidFormat (to tell a corrupted iterator from an expired one, which is recoverable by re-fetching the iterator) never take that branch.
Fix: GetRecords input validation plus the shared error serializer — validate iterator format before the state lookup and emit ValidationException/InvalidFormat at 400. Same root cause as cause 1 of 20261029T173658Z-keyspacesstreams-maps-invalid-input-errors-to-wrong-type.md.
Traces: keyspacesstreams.get-records-malformed-iterator-rejected.

Cause 3 of 4: GetShardIterator with shardIteratorType=AT_SEQUENCE_NUMBER and no sequenceNumber returns a self-invented message.
AWS: Smithy GetShardIterator requires streamArn, shardId and shardIteratorType; sequenceNumber (SequenceNumber @length(21,48)) is optional in the model, so the AT_SEQUENCE_NUMBER/AT-no-sequence combination is enforced at the handler. Recorded behavior: HTTP 400, code ValidationException, message "One or more parameter values were invalid" — wholly fixed text; it names no member, no value and no identifier from the request.
Emulator: code ValidationException is correct, but the message is "sequenceNumber is required when shardIteratorType is AT_SEQUENCE_NUMBER" — invented wording that also leaks the member and enum names AWS does not include.
Why: clients and conformance tests matching AWS's documented invalid-parameter text do not match, and the emulator's extra detail trains callers to parse a message AWS never sends.
Fix: GetShardIterator parameter-combination validation — raise ValidationException with AWS's fixed message "One or more parameter values were invalid".
Traces: keyspacesstreams.get-shard-iterator-missing-sequence-number-rejected.

Cause 4 of 4: GetStream on a well-formed but nonexistent stream ARN echoes the request ARN instead of AWS's fixed not-found message.
AWS: Smithy GetStream.streamArn @required, StreamArn @length(37,1024); ResourceNotFoundException is modeled with member message only. Recorded behavior: code ResourceNotFoundException with message "Requested resource not found" — entirely fixed text, with no keyspace, table or stream ARN interpolated.
Emulator: code ResourceNotFoundException is correct, but the message is of the form "stream <streamArn> does not exist" (observed with the request's own ARN, including the generated keyspace name and the stream's creation timestamp, substituted in).
Why: the message is both wrong wording and environment-dependent — it embeds a per-run keyspace name and timestamp — so no client or test can assert on it, and error-text based routing diverges from AWS.
Fix: GetStream handler's not-found path plus the shared error serializer — emit the fixed message "Requested resource not found" and interpolate nothing. Same root cause as cause 5 of 20261029T173658Z-keyspacesstreams-maps-invalid-input-errors-to-wrong-type.md.
Traces: keyspacesstreams.get-stream-nonexistent-arn-not-found.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: keyspacesstreams 10/20 passed, 4 mismatched (6 not implemented, already filed). All four mismatches are service=keyspacesstreams, which get_bootstrap_status reports as bootstrapped.

Cause 1 — keyspacesstreams.get-records-denied-by-iam-policy-rejected [mismatch]
  expected: GetRecords by an IAM user allowed only cassandra:GetStream + cassandra:GetShardIterator -> 403 AccessDeniedException, message "User: arn:aws:iam::<account>:user/<user-name> is not authorized to perform: cassandra:GetRecords on resource: <streamArn> because no identity-based policy allows the cassandra:GetRecords action"
  actual:   HTTP 200 with a successful body: {'changeRecords': [], 'nextShardIterator': 'ksstreams-iterator-<hex>', 'iteratorDescription': {'iteratorPosition': 'AT_TIP'}}. Harness message: "expected GetRecords to be denied, but the call SUCCEEDED". The admin positive control before the break also succeeds, so the target was healthy for this call.

Cause 2 — keyspacesstreams.get-records-malformed-iterator-rejected [mismatch]
  request:  GetRecords shardIterator = "not-a-real-iterator-" + 40 x 'x' (never issued by GetShardIterator, within @length(1,4096))
  expected: 400 ValidationException, errorCode InvalidFormat, message "Invalid ShardIterator"
  actual:   ResourceNotFoundException (harness: "expected ValidationException for a malformed shard iterator, got ResourceNotFoundException")

Cause 3 — keyspacesstreams.get-shard-iterator-missing-sequence-number-rejected [mismatch]
  request:  GetShardIterator(streamArn=<real>, shardId=<real>, shardIteratorType="AT_SEQUENCE_NUMBER") with sequenceNumber omitted
  expected: ValidationException, message "One or more parameter values were invalid"
  actual:   ValidationException, message "sequenceNumber is required when shardIteratorType is AT_SEQUENCE_NUMBER"

Cause 4 — keyspacesstreams.get-stream-nonexistent-arn-not-found [mismatch]
  request:  GetStream with the real stream ARN's /table/<tbl>/ segment replaced by /table/s2rnonexistenttbl/
  expected: ResourceNotFoundException, message "Requested resource not found"
  actual:   ResourceNotFoundException, message "stream arn:aws:cassandra:<region>:<account>:/keyspace/<generated-ks>/table/s2rnonexistenttbl/stream/<creation-timestamp> does not exist" (the per-run keyspace name and timestamp are environment values, not part of any requirement; the defect is the invented template and the interpolation itself)

Model references (services/service-com-amazonaws-keyspacesstreams-keyspacesstreams/smithy-model/com-amazonaws-keyspacesstreams-keyspacesstreams.json, protocol awsJson1_0): GetRecords errors {AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400}; GetRecords.shardIterator @required @length(1,4096); GetShardIterator.{streamArn,shardId,shardIteratorType} @required, sequenceNumber @length(21,48) optional; GetStream.streamArn @required @length(37,1024); ValidationException members {message, errorCode: ValidationExceptionType}; ResourceNotFoundException member {message}.

Related open issue (named, not duplicated blindly): 20261029T173658Z-keyspacesstreams-maps-invalid-input-errors-to-wrong-type.md covers the same underlying mapping for causes 2 and 4 from the wire corpus; causes 1 and 3 are new.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it (git_clone, then run_behavioral_suite).
2. Replay the keyspacesstreams behavioral checks listed in trace_ids.
3. Cause 1: create a keyspace and a CDC-enabled table, wait for the stream to reach ENABLED, and take a TRIM_HORIZON shard iterator. Confirm the admin caller's GetRecords succeeds (positive control). Create an IAM user whose only policy allows cassandra:GetStream and cassandra:GetShardIterator on that stream ARN (plus cassandra:Select on the system keyspace) and NOT cassandra:GetRecords; call GetRecords with that principal's credentials -> the emulator returns 200 with a records body; AWS returns 403 AccessDeniedException with the "is not authorized to perform: cassandra:GetRecords ... because no identity-based policy allows ..." message. Then grant cassandra:GetRecords to the same user and confirm the identical call succeeds.
4. Cause 2: call GetRecords with shardIterator set to an arbitrary string of valid length that GetShardIterator never issued -> the emulator answers ResourceNotFoundException; AWS answers 400 ValidationException with errorCode InvalidFormat and message "Invalid ShardIterator". A genuine iterator for the same shard must still succeed.
5. Cause 3: call GetShardIterator with a real streamArn and shardId, shardIteratorType=AT_SEQUENCE_NUMBER and no sequenceNumber -> the emulator's ValidationException message is "sequenceNumber is required when shardIteratorType is AT_SEQUENCE_NUMBER"; AWS's is "One or more parameter values were invalid". A TRIM_HORIZON request on the same shard must still return a usable iterator.
6. Cause 4: call GetStream with a well-formed stream ARN whose table segment names a table that was never created -> the emulator's ResourceNotFoundException message echoes the ARN; AWS's is the fixed "Requested resource not found". GetStream on the real ARN must still describe it.
7. Re-replay all four check ids in trace_ids to confirm every cause is closed.
