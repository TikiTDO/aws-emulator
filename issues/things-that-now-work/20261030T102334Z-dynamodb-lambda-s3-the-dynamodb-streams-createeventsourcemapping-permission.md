Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3.ddb-stream-esm-creation-fails-without-permissions
Issue-Key: dynamodb__lambda__s3.ddb-stream-esm-creation-fails-without-permissions|run#13|AssertionError|assert message == (
Issue-Commit: 0e1c1b5eee37df9cd7b5f3886cad9124e3f02059

# dynamodb__lambda__s3: the DynamoDB-Streams CreateEventSourceMapping permission refusal omits its trailing "in IAM." (1 cause)

> _Note: this issue was identified against the code base at commit `0e1c1b5eee37df9cd7b5f3886cad9124e3f02059`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: the DynamoDB-Streams permission refusal message for CreateEventSourceMapping drops the trailing ` in IAM.` segment.
  AWS: recorded behavior only for the text (the message is not in the Smithy model). Smithy: Lambda `CreateEventSourceMapping` models `InvalidParameterValueException` at httpError 400 and succeeds with 202. When the EventSourceArn names a DynamoDB stream and the invoked function's execution role holds none of the stream-read grants, Lambda refuses 400 `InvalidParameterValueException` and creates no mapping, with the message rule `Cannot access stream <stream ARN>. Please ensure the role can perform the GetRecords, GetShardIterator, DescribeStream, and ListStreams Actions on your stream in IAM.` Fixed text: `Cannot access stream ` and `. Please ensure the role can perform the GetRecords, GetShardIterator, DescribeStream, and ListStreams Actions on your stream in IAM.` Variable part: the stream ARN, taken from the request's EventSourceArn. The Kinesis wording of the same error class ends `on your stream.` without the ` in IAM` segment, so the suffix is specific to the DynamoDB-Streams source.
  Emulator: the status (400), the error code (`InvalidParameterValueException`), the action list and the atomicity (no mapping left behind) are all correct, but the sentence ends `...and ListStreams Actions on your stream.` — ` in IAM` is missing, because the shard-source refusal is rendered from a single template shared with the Kinesis source.
  Why: this refusal is the signal that an ESM's execution role is short a stream-read grant. A client that matches, logs or asserts on AWS's exact text for that class does not recognise the emulator's message, so error-text-driven remediation and regression assertions diverge from AWS even though code and status agree.
  Fix: the refusal wording for the shard-source authorization refusal — give the DynamoDB-Streams source its own sentence suffix (` in IAM.`) instead of reusing the Kinesis one. File/line: services/service-com-amazonaws-lambda-awsgirapiservice/src/state/event_source_authorization.rs:204-218 (Refusal::Shards message) with the DynamoDB catalog entry at :141-151, reached from services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/create_event_source_mapping.rs:568-571.
  Traces: dynamodb__lambda__s3.ddb-stream-esm-creation-fails-without-permissions. The same template defect also shows in service dynamodb__kinesis__lambda (check-ddb-esm-fails-without-stream-permissions); that one is filed under its own service token because an issue carries one service, and the two share this single fix.

## Evidence
dynamodb__lambda__s3.ddb-stream-esm-creation-fails-without-permissions, at the message assertion (check line 65):
`AssertionError: Cannot access stream arn:aws:dynamodb:<region>:<account>:table/<table>/stream/<label>. Please ensure the role can perform the GetRecords, GetShardIterator, DescribeStream, and ListStreams Actions on your stream.`

Expected message: `Cannot access stream <the same ARN>. Please ensure the role can perform the GetRecords, GetShardIterator, DescribeStream, and ListStreams Actions on your stream in IAM.`
Actual: the identical sentence with ` in IAM` absent before the final period.

The surrounding behaviour is right and is NOT part of this finding: the check's positive control (a role holding dynamodb:DescribeStream/GetRecords/GetShardIterator/ListStreams) created the mapping and reached Enabled, the unpermitted attempt was refused 400 with code `InvalidParameterValueException`, and no mapping was created for it. The table name, stream label, account and region in the quoted message are per-run values; the requirement is the fixed text, including the ` in IAM` suffix.

## Steps to Reproduce
1. Boot the emulator.
2. CreateTable with StreamSpecification enabled (StreamViewType NEW_AND_OLD_IMAGES); take LatestStreamArn.
3. Create an IAM role trusted by lambda.amazonaws.com granting only CloudWatch Logs actions — no dynamodb permissions at all — and a python3.12 function using it; wait for State Active.
4. Call CreateEventSourceMapping(EventSourceArn=<the stream ARN>, FunctionName=<fn>, StartingPosition='LATEST', BatchSize=1).
   Expected: 400 InvalidParameterValueException with message `Cannot access stream <arn>. Please ensure the role can perform the GetRecords, GetShardIterator, DescribeStream, and ListStreams Actions on your stream in IAM.`
   Observed: the same code and status, message ending `...Actions on your stream.` (no ` in IAM`).
5. Replay dynamodb__lambda__s3.ddb-stream-esm-creation-fails-without-permissions.
