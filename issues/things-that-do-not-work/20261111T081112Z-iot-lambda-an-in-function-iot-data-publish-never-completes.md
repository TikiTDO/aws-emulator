Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__lambda.lambda-sdk-iot-publish-positive
Issue-Key: iot__lambda.lambda-sdk-iot-publish-positive|_invoke#11|Timeout|return body, resp.get("FunctionError")
Issue-Commit: f4372f081be041f1c06808452ec2531466fafcb7

# An in-function iot-data Publish never completes, so the positive publish check never reaches a verdict

> _Note: this issue was identified against the code base at commit `f4372f081be041f1c06808452ec2531466fafcb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: a Lambda handler calling `iot-data` Publish with a grant that allows it never gets an answer.
AWS: recorded behavior only - `iot:Publish` on an allowed topic answers HTTP 200 with an empty body, the handler returns, and the published message is observable on the topic. A refusal would be an explicit error, not an absence of one.
Emulator: the step never reaches a verdict - the call does not return and the check ends `Check timed out after 60.0s`.
Why: the IoT data plane is unusable from a function: a client gets neither the 200 nor an error code, so it cannot tell a missing grant from an unreachable endpoint, and its handler blocks until its own timeout. Note the negative twin (lambda-sdk-iot-publish-missing-permission-negative) is already filed under 20261110T210703Z-iot-lambda-an-iot-data-publish-from-a.md, which carries only that key, not this one.
Fix: the iot-data Publish handler and the endpoint the emulator publishes for the IoT data plane (DescribeEndpoint), which the in-function SDK call resolves to.

Traces: iot__lambda.lambda-sdk-iot-publish-positive.

## Evidence
Expected: iot-data Publish answers 200 and the handler returns the published topic/payload.
Actual: Error: Check timed out after 60.0s - the Publish call never returns.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. CreateFunction with an execution role granting iot:Publish and a handler that calls boto3 client('iot-data').publish(topic=..., payload=...).
3. Invoke the function and observe that the call never returns.
4. Re-replay iot__lambda.lambda-sdk-iot-publish-positive.
