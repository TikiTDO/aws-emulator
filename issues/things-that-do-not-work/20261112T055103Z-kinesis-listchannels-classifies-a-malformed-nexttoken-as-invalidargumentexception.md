Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.list-channels-malformed-next-token-rejected
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis.list-channels-malformed-next-token-rejected|run#8|AssertionError|assert code == "ValidationException", (
Issue-Commit: 9c5abcb8cc5a2c39588ca54faad4072942c1b17e

# kinesis ListChannels classifies a malformed NextToken as InvalidArgumentException instead of ValidationException "Invalid NextToken."

> _Note: this issue was identified against the code base at commit `9c5abcb8cc5a2c39588ca54faad4072942c1b17e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListChannels

## Description
One net-new root cause in the Kinesis Data Streams control plane. It is grounded in the only failing check the replay report lists under `### kinesis` that is marked NEITHER "ALREADY FILED" nor "OWNED by" — every other kinesis failure in this replay is owned by an existing issue.

Cause 1 of 1: ListChannels rejects a syntactically-invalid NextToken with the wrong error class and message.

AWS: Smithy (services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json) binds five client errors to ListChannels — AccessDeniedException, ExpiredNextTokenException, InvalidArgumentException, LimitExceededException and ValidationException (all httpError 400) — and models ListChannels.NextToken as NextToken @length min 1 max 1048576, with @paginated inputToken/outputToken NextToken, items ChannelSummaries, pageSize MaxResults. The three token-relevant classes are not interchangeable: a well-formed token whose TTL has lapsed is ExpiredNextTokenException, while a string that is not a page cursor at all is a request-shape defect. Recorded behavior: HTTP 400, Error.Code `ValidationException`, Error.Message `Invalid NextToken.` — that sentence is ENTIRELY fixed text for this error class; the offending token is not echoed and no value from the request or from state appears in it. ListChannels with no token (MaxResults only) succeeds and returns the documented ChannelSummaries shape both before and after the rejected call.

Emulator: ListChannels(NextToken=<any string the emulator did not mint>) is refused with Error.Code `InvalidArgumentException` (the check asserted on Code and got 'InvalidArgumentException' where 'ValidationException' is required). The un-paginated ListChannels(MaxResults=1) control call succeeds immediately before and after, so the misclassification is specific to the NextToken decode/validation path and not a sick API. Note: open issues 20261103T091152Z-kinesis-4-net-new-root-causes-listchannels-classifies.md and 20261103T071946Z-kinesis-4-net-new-root-causes-listchannels-misclassifies.md describe this same call site answering `ExpiredNextTokenException`; the emulator now answers `InvalidArgumentException` instead, so their replay key no longer matches this failure (the report does not mark the check OWNED) and neither covers the current behavior. The classification is still not AWS's, and the `Invalid NextToken.` message is still not served. Open issue 20261029T140704Z-kinesis-liststreamconsumers-misclassifies-a-malformed-nexttoken-as-expirednexttokenexception.md reports the sibling defect on ListStreamConsumers and does not cover the ListChannels call site.

Why: a paginating client dispatches on the error class. ValidationException tells it the cursor it supplied is corrupt or hand-written — a caller bug to surface. InvalidArgumentException is the generic bad-parameter class the same operation uses for other input faults (e.g. an unusable StreamFilter), so a client cannot distinguish "your token is not a token" from "one of your other arguments is wrong", cannot branch on the token case, and never sees the `Invalid NextToken.` diagnosis AWS serves for it. Error-handling and retry/restart logic keyed to AWS's classes takes the wrong branch.

Fix: the NextToken decode/validation step of the ListChannels handler. It must distinguish three outcomes — undecodable / not a token this service minted => ValidationException with the fixed message `Invalid NextToken.`; decoded but past its TTL => ExpiredNextTokenException; other input faults => InvalidArgumentException — rather than funnelling an unparseable cursor into the generic argument-error branch.

Traces: kinesis.list-channels-malformed-next-token-rejected.

## Evidence
Cause 1 — kinesis.list-channels-malformed-next-token-rejected [mismatch]:

check_kinesis_list_channels_malformed_next_token_rejected.py, line 42, in run
    assert code == "ValidationException", (
AssertionError: expected ValidationException for a malformed NextToken, got 'InvalidArgumentException'

Expected (AWS): HTTP 400, Error.Code 'ValidationException', Error.Message 'Invalid NextToken.' (wholly fixed text) for ListChannels(NextToken=<a string the service never issued>).
Actual (emulator): HTTP 400 with Error.Code 'InvalidArgumentException'; the check never reached the message assertion, so the served message is not AWS's documented sentence for this class either.
Controls: ListChannels(MaxResults=1) returned ChannelSummaries both before and after the rejected call, so the API is healthy and the defect is confined to NextToken validation.

## Steps to Reproduce
1. Call kinesis.list_channels(MaxResults=1) — it succeeds and returns ChannelSummaries (positive control).
2. Call kinesis.list_channels(NextToken='sim2real-not-a-real-next-token-deadbeef') (any string the emulator did not mint as a cursor) and read Error.Code / Error.Message from the ClientError.
   Expect: Code 'ValidationException', Message 'Invalid NextToken.'.
   Observe: Code 'InvalidArgumentException'.
3. Confirm the code is also not 'ExpiredNextTokenException' — a syntactically-invalid token must not be classified like a well-formed-but-expired one.
4. Call kinesis.list_channels(MaxResults=1) again to confirm the API is otherwise healthy.
5. Re-replay kinesis.list-channels-malformed-next-token-rejected.
