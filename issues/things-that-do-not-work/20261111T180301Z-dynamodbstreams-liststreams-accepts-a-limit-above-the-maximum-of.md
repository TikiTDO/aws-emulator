Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodbstreams.list-streams-limit-over-100-rejected
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodbstreams.list-streams-limit-over-100-rejected|expect_error#2|AssertionError|raise AssertionError(f"expected {code} but the call succeeded: {result!r}")
Issue-Commit: 69c900cbbb228bcd9ce31c977c27cf9cdda27129

# ListStreams accepts a Limit above the maximum of 100 instead of refusing it with ValidationException

> _Note: this issue was identified against the code base at commit `69c900cbbb228bcd9ce31c977c27cf9cdda27129`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStreams

## Description
One net-new root cause.

Cause 1 of 1: DynamoDB Streams ListStreams does not enforce the documented upper bound of 100 on the Limit input member, so an out-of-range Limit is accepted and served a normal 200 result page instead of being refused.

AWS: Smithy: ListStreams.Limit targets PositiveIntegerObject with @range min 1 and no modeled maximum, but the member's own documentation states the bound: "The maximum number of streams to return. The upper limit is 100." Recorded behavior shows AWS refusing any Limit above that upper limit with HTTP 400, error code ValidationException, and message "One or more parameter values were invalid: Limit cannot be greater than the maximum allowed size: 100". Note this error class is NOT the "<n> validation error detected: Value at '<member>' ..." envelope used for modeled @range/@length violations (see open issue 20261029T172902Z-dynamodbstreams-constraint-violation-messages-interpolate-the-rejected-value-which.md, which covers that other formatter and does not cover this bound at all) — it is the service-side "One or more parameter values were invalid: ..." wording. In that message the fixed text is "One or more parameter values were invalid: Limit cannot be greater than the maximum allowed size: " and the only variable part is the enforced maximum, 100, which is the documented ceiling for this member and not a per-run value. The boundary value Limit=100 is itself valid and must still succeed; only values strictly greater than 100 are refused.

Emulator: the call succeeds. ListStreams with Limit=101 returns HTTP 200 with content-type application/x-amz-json-1.0 and a normal result body ({"Streams": []} for an empty account), with no error code and no error message. The emulator applies only the modeled lower bound (>= 1) and has no upper-bound check for this member.

Why: a client that relies on AWS rejecting an over-large page size gets no signal that its request was invalid. Pagination code tuned against the real service (and SDK/integration tests that assert the ValidationException and its exact text) passes against the emulator and then fails in production; conversely a caller that deliberately probes the ceiling cannot distinguish the valid boundary (100) from an invalid request (101), because both return 200.

Fix: ListStreams input validation in the DynamoDB Streams handler — add the documented upper-bound check on the Limit member, rejecting Limit > 100 with HTTP 400 / ValidationException and the "One or more parameter values were invalid: Limit cannot be greater than the maximum allowed size: 100" message, while leaving Limit = 100 accepted. It belongs with the existing lower-bound (>= 1) validation for the same member, not in the serializer or state layer.

Traces: behavioral check dynamodbstreams.list-streams-limit-over-100-rejected (the check also exercises the positive control at Limit=100, which must keep succeeding after the fix).

## Evidence
Failing behavioral check dynamodbstreams.list-streams-limit-over-100-rejected [mismatch].

Expected (AWS): ListStreams(Limit=101) -> HTTP 400, Error.Code "ValidationException", Error.Message "One or more parameter values were invalid: Limit cannot be greater than the maximum allowed size: 100".

Actual (emulator): the call succeeded. Check assertion: `AssertionError: expected ValidationException but the call succeeded: {'Streams': [], 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', 'content-length': '14', ...}}}` (the empty Streams list and the header values are environment-specific; the divergence is the 200 with no error at all).

Model support: smithy_query ListStreams -> input member Limit: target PositiveIntegerObject, range {min: 1}, no max; member documentation "The maximum number of streams to return. The upper limit is 100." The 100 ceiling is therefore documentation + recorded behavior, not a modeled @range max.

## Steps to Reproduce
1. Boot the emulator and point a dynamodbstreams client at it.
2. Call ListStreams with Limit=101. Expected: HTTP 400 with ValidationException and message "One or more parameter values were invalid: Limit cannot be greater than the maximum allowed size: 100". Observed: HTTP 200 with a normal ListStreams body.
3. Positive control, must keep working: call ListStreams with Limit=100 — this is the valid boundary and must return 200.
4. Equivalently, re-replay behavioral check dynamodbstreams.list-streams-limit-over-100-rejected.
