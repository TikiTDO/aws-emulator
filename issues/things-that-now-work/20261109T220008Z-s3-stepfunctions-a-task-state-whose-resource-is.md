Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__stepfunctions.sfn-aws-sdk-s3-getobject-result-over-256kib-rejected s3__stepfunctions.sfn-aws-sdk-s3-streaming-round-trip-then-getobject-after-delete-caught
Issue-Key: s3__stepfunctions.sfn-aws-sdk-s3-getobject-result-over-256kib-rejected|run#10|AssertionError|assert ok["status"] == "SUCCEEDED", (
Issue-Key: s3__stepfunctions.sfn-aws-sdk-s3-streaming-round-trip-then-getobject-after-delete-caught|run#6|AssertionError|assert exe["status"] == "SUCCEEDED", (
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# s3__stepfunctions: a Task state whose Resource is arn:aws:states:::aws-sdk:s3:<op> fails States.Runtime — the S3 AWS SDK service integration is not dispatched at all

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One cause.

Cause 1 of 1: Step Functions does not make AWS SDK integration calls to S3, so any state machine naming arn:aws:states:::aws-sdk:s3:<operation> fails at run time.

AWS: the AWS SDK service integration lets a Task state call any S3 API — 'arn:aws:states:::aws-sdk:s3:getObject', ':headBucket', ':putObject' — and the state's output is that API's response (with a 256 KiB payload limit: a GetObject result above the quota fails States.DataLimitExceeded, which is exactly the negative verdict under test, while a 200 KiB result succeeds). Smithy: s3 GetObject/HeadBucket are ordinary modeled operations; the integration dispatches them as the execution role.

Emulator: the execution ends 'FAILED' with error 'States.Runtime' and cause "state '<Name>': 'arn:aws:states:::aws-sdk:s3:getObject' names an integration with 's3', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it" (same for headBucket). The fixed text is 'names an integration with '<service>', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it'; the state name and the integration ARN come from the definition.

Why: a very common workflow shape — a state machine that reads/writes S3 directly instead of through a Lambda — cannot run at all, and the failure is reported as States.Runtime rather than as the real S3 outcome, so neither the happy path (streaming round trip, GetObject after delete caught by a Catch) nor the payload-limit refusal can be exercised.

Fix: the Step Functions Task-state AWS SDK integration dispatcher must include s3 in the services it calls, invoking the named operation as the execution role and applying the state-output size quota to the result.

Traces: s3__stepfunctions.sfn-aws-sdk-s3-getobject-result-over-256kib-rejected, s3__stepfunctions.sfn-aws-sdk-s3-streaming-round-trip-then-getobject-after-delete-caught. The open issue 20261107T185637Z-s3-stepfunctions-step-functions-aws-sdk-service-integrations-are-not.md names the same area for a different check and does not carry these two checks' replay keys.

## Evidence
Expected: a Task state with Resource 'arn:aws:states:::aws-sdk:s3:getObject' returning a 204800-byte object SUCCEEDS (and only a result over 256 KiB fails on the payload quota); a round trip putObject -> headBucket/getObject -> deleteObject -> getObject-with-Catch SUCCEEDS.
Actual: AssertionError: positive control: getting a 204800-byte object ended 'FAILED': 'States.Runtime' "state 'Get': 'arn:aws:states:::aws-sdk:s3:getObject' names an integration with 's3', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it".
Actual: AssertionError: the round trip ended 'FAILED': 'States.Runtime' "state 'Head': 'arn:aws:states:::aws-sdk:s3:headBucket' names an integration with 's3', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it".

## Steps to Reproduce
1. Create a bucket and an object of ~200 KiB.
2. Create a state machine whose Task state has Resource 'arn:aws:states:::aws-sdk:s3:getObject' (and another with ':headBucket'/':putObject'/':deleteObject').
3. start_execution and read the execution's status/error/cause -> FAILED with States.Runtime naming s3 as a service the emulator does not dispatch.
