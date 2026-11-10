Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__lambda.lambda-sdk-iot-publish-missing-permission-negative
Issue-Key: iot__lambda.lambda-sdk-iot-publish-missing-permission-negative|_invoke#13|Timeout|resp = lam.invoke(FunctionName=fn_name, Payload=json.dumps(payload).encode("utf-8"))
Issue-Commit: 2d0b651e448fe6a44fa56d00833f67c02b248ecb

# iot__lambda: an iot-data Publish from a role without iot:Publish never reaches a verdict — the ForbiddenException/no-delivery contrast hangs to the 60 s budget

> _Note: this issue was identified against the code base at commit `2d0b651e448fe6a44fa56d00833f67c02b248ecb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish (iot-data) under a role lacking iot:Publish

## Description
Cause 1 of 1: the authorization boundary of the IoT data plane's Publish is not observable from a Lambda handler — the check is cut off at the harness's 60 s budget. Failing check: iot__lambda.lambda-sdk-iot-publish-missing-permission-negative.

AWS: Smithy (iot-data / iotmoonrakerservice): Publish is authorized by iot:Publish on the topic; a caller without it is refused with ForbiddenException (HTTP 403) and the message is never published, so an IoT topic rule selecting that topic fires no action and nothing reaches the rule's SQS queue. A sibling function whose role does carry iot:Publish publishes successfully and its marker is delivered through the rule to the queue.

Emulator: the check reaches no verdict at all — "Error: Check timed out after 60.0s". Neither the refusal (ForbiddenException with no delivery) nor the positive control (granted role, marker delivered via the topic rule) settles inside the budget.

Why: a client cannot tell an unauthorized publish from an authorized one: it receives neither the 403 ForbiddenException that tells it to fix its policy nor the delivered message that tells it the policy is sufficient, so least-privilege IoT policies cannot be validated.

Fix: iot-data Publish authorization (iot:Publish evaluation and the ForbiddenException refusal) and the IoT topic-rule action dispatch that must deliver the granted publish to the rule's SQS action; each path must terminate rather than hang.

Traces: the failing check id above.

## Evidence
Expected (AWS): the ungranted handler's iot-data publish is refused ForbiddenException (HTTP 403) and the rule's SQS queue stays empty; the granted sibling's publish is delivered and its marker appears on the queue.
Actual (emulator): Error: Check timed out after 60.0s — neither leg produces a result.

## Steps to Reproduce
1. Create an SQS queue and an IoT topic rule role allowed to send to it; CreateTopicRule with SQL "SELECT * FROM '<topic>'" and an sqs action.
2. Create two Lambda functions, one whose execution role lacks iot:Publish and one that has it, each calling iot-data Publish to <topic>.
3. Invoke both and read the queue.
4. Observe neither the ForbiddenException nor the delivered marker is produced before the 60 s cap.
