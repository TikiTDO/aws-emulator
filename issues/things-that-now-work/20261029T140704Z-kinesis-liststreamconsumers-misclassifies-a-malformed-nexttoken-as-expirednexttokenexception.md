Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.list-stream-consumers-malformed-next-token-rejected
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis.list-stream-consumers-malformed-next-token-rejected|run#17|AssertionError|assert code == "InvalidArgumentException", (
Issue-Commit: 02c390dd8e861c3d98b16a550da31d4601fa546f

# kinesis: ListStreamConsumers misclassifies a malformed NextToken as ExpiredNextTokenException instead of InvalidArgumentException "Invalid NextToken."

> _Note: this issue was identified against the code base at commit `02c390dd8e861c3d98b16a550da31d4601fa546f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStreamConsumers

## Description
Cause 1 of 1: ListStreamConsumers' pagination-token validation collapses two distinct modeled error classes into one — any token it cannot accept is reported as an EXPIRED token, so a structurally invalid (mangled/corrupt) NextToken is never reported as an invalid argument.

AWS: Smithy model (services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json) binds ListStreamConsumers to BOTH InvalidArgumentException and ExpiredNextTokenException (each client-kind, HTTP 400). The two have disjoint meanings in the model's own docs: ExpiredNextTokenException is "The pagination token passed to the operation is expired." — i.e. a well-formed token whose TTL (300 s for ListStreamConsumers) has elapsed — while InvalidArgumentException is "A specified parameter exceeds its restrictions, is not supported, or can't be used." On real AWS a NextToken that is not a token the service issued (its bytes have been altered, so it does not decode/verify) is a parameter that "can't be used" and is refused with HTTP 400, error code InvalidArgumentException and message "Invalid NextToken." — that message is entirely fixed text for this error class, with no request- or state-derived variable part. The expired case keeps ExpiredNextTokenException. ListStreamConsumers.NextToken is modeled only with @length 1..1048576, so the model itself does not describe the token's internal structure; the split between "unparseable" and "parseable but stale" is the documented/recorded behavior this check pins.

Emulator: against the emulator the same call — ListStreamConsumers(StreamARN=<stream>, NextToken=<token with its last 8 characters replaced by "deadbeef">), issued seconds after a valid first page was fetched with MaxResults=1 — is refused with error code 'ExpiredNextTokenException'. The check fails at the code assertion before the message is compared: "expected InvalidArgumentException for a malformed NextToken, got 'ExpiredNextTokenException'". The token used was freshly issued and demonstrably still inside its TTL: the check verifies immediately before AND after that the UNmangled token still returns a Consumers page, so nothing about this request is expired. The emulator therefore reaches the expired branch for a token whose only defect is that it does not decode — it has no distinct "not a token I issued" branch, and (symptom, same cause) no way to return InvalidArgumentException with "Invalid NextToken." for this class at all.

Why: a client cannot distinguish a bug from a timeout. SDK and application paginators treat ExpiredNextTokenException as retryable/restartable — restart the scan from page one, because an expired token is an expected consequence of a slow consumer — while InvalidArgumentException is a non-retryable programming error. A corrupted or hand-built token (truncated through a queue, round-tripped through a lossy store, or constructed by the caller) makes such a client loop: restart, corrupt again, be told "expired" again, forever, instead of failing fast with the invalid-argument signal AWS gives. Error-handling tests that assert on the invalid-argument path, and retry policies keyed on these two codes, pass against AWS and break against the emulator.

Fix: the NextToken decoding/validation step of the ListStreamConsumers handler (services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/list_stream_consumers.rs and whatever shared pagination-token helper it calls). Validation must be ordered: a token that fails structural decode/integrity verification is refused with InvalidArgumentException and the fixed message "Invalid NextToken."; only a token that DECODES SUCCESSFULLY and whose embedded issue time is older than the TTL is refused with ExpiredNextTokenException. Both remain HTTP 400. The same helper should be reused by the other NextToken-taking Kinesis list operations so the classification is consistent.

Traces: kinesis.list-stream-consumers-malformed-next-token-rejected (mismatch). Not covered by any open issue: the open kinesis bundles 20261027T011654Z-kinesis-9-net-new-root-causes-dryrun-not.md (invented DryRun/not-found/name-ARN-mismatch refusal messages) and 20261027T172504Z-kinesis-10-net-new-root-causes-the-per.md (per-shard quota / PutRecords) concern other operations and carry neither this check nor pagination-token classification.

## Evidence
Check: kinesis.list-stream-consumers-malformed-next-token-rejected [mismatch]

Scenario (from the check body): create a 1-shard stream, wait ACTIVE, RegisterStreamConsumer twice, ListStreamConsumers(StreamARN, MaxResults=1) -> NextToken; confirm the fresh token works; then mangle it (last 8 chars -> "deadbeef") and call ListStreamConsumers(StreamARN, NextToken=<mangled>).

Expected (real AWS): HTTP 400, Error.Code = "InvalidArgumentException", Error.Message = "Invalid NextToken." (fixed text; no variable part). ExpiredNextTokenException is reserved for a well-formed token past its ~300 s TTL.

Actual (emulator): HTTP 400 with Error.Code = "ExpiredNextTokenException".
AssertionError: expected InvalidArgumentException for a malformed NextToken, got 'ExpiredNextTokenException'
  at check_kinesis_list_stream_consumers_malformed_next_token_rejected.py line 57:
    assert code == "InvalidArgumentException"

Control facts in the same run that rule out a genuine expiry: the unmangled token returned a Consumers page immediately before the mangled call, and the check's later assertions (unpaginated listing of 2 consumers, the original token still usable) are on the same still-valid token within its TTL.

Model evidence: ListStreamConsumers errors = [ExpiredNextTokenException, InvalidArgumentException, LimitExceededException, ResourceInUseException, ResourceNotFoundException], all client-kind HTTP 400. ExpiredNextTokenException doc: "The pagination token passed to the operation is expired." InvalidArgumentException doc: "A specified parameter exceeds its restrictions, is not supported, or can't be used." NextToken: @length min 1, max 1048576 (no structural constraint modeled).

## Steps to Reproduce
1. kinesis.CreateStream(StreamName=<name>, ShardCount=1); poll DescribeStreamSummary until StreamStatus == ACTIVE; take StreamARN.
2. kinesis.RegisterStreamConsumer(StreamARN=<arn>, ConsumerName=<c0>) and again with <c1>, so the stream has 2 consumers.
3. page = kinesis.ListStreamConsumers(StreamARN=<arn>, MaxResults=1); token = page["NextToken"] (assert it is present).
4. Sanity: kinesis.ListStreamConsumers(StreamARN=<arn>, NextToken=token) -> succeeds with a "Consumers" key (token is inside its TTL).
5. bogus = token[:-8] + "deadbeef"; kinesis.ListStreamConsumers(StreamARN=<arn>, NextToken=bogus).
6. Observe the error: emulator returns Error.Code "ExpiredNextTokenException"; AWS returns HTTP 400 InvalidArgumentException with Message "Invalid NextToken."
7. Re-replay with: kinesis.list-stream-consumers-malformed-next-token-rejected
