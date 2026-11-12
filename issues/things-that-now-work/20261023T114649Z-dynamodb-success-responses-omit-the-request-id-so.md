Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__lambda.response-request-id-is-a-real-uuid
Issue-Key: cloudformation__dynamodb__lambda.response-request-id-is-a-real-uuid|_assert_request_id#3|AssertionError|assert "RequestId" in metadata, (
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# DynamoDB success responses omit the request id, so ResponseMetadata carries no RequestId

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
Cause 1 of 1: a successful DynamoDB response carries no request-id header, so the SDK's ResponseMetadata has no RequestId.

AWS: every DynamoDB response - success as well as error - carries the x-amzn-RequestId header (awsJson1_0), which botocore surfaces as ResponseMetadata['RequestId'] as a UUID-shaped string. It is the identifier a caller quotes for support and uses to correlate a call with its server-side record, and SDKs log it on every retry.

Emulator: the header is absent on the success path. The check reads ResponseMetadata == {'HTTPStatusCode': 200, 'RetryAttempts': 0} - no RequestId key at all.

Why: a client cannot correlate or report a call. Any code or test that reads ResponseMetadata['RequestId'] raises KeyError against the emulator while working on AWS, and request-id-based logging produces nothing.

Fix: the response serializer / header emission for the DynamoDB service - set x-amzn-RequestId (a fresh UUID per request) on success responses, not only on errors.

Traces: cloudformation__dynamodb__lambda.response-request-id-is-a-real-uuid, whose failing leg is labelled [dynamodb/success].

## Evidence
Expected: ResponseMetadata contains RequestId, a UUID-shaped string (from the x-amzn-RequestId response header).
Actual: "[dynamodb/success] the response must carry a RequestId; ResponseMetadata was {'HTTPStatusCode': 200, 'RetryAttempts': 0}".

## Steps to Reproduce
1. CreateTable and wait for ACTIVE.
2. Call PutItem (or any successful DynamoDB operation) against the emulator.
3. Read response['ResponseMetadata']['RequestId'] -> the key is absent.
