Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__lambda.lambda-sdk-iot-publish-missing-permission-negative
Issue-Key: iot__lambda.lambda-sdk-iot-publish-missing-permission-negative|_invoke_until_delivered#13|Timeout|last_body = _invoke(ctx, fn_name, payload)
Issue-Commit: dedd6f8a44979b422a25c34412c44daa0d34e6e0

# iot__lambda: the in-function iot-data Publish permission boundary reaches no verdict — the publish -> topic-rule -> SQS positive control hangs to the harness cap (1 cause)

> _Note: this issue was identified against the code base at commit `dedd6f8a44979b422a25c34412c44daa0d34e6e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: the trajectory that contrasts an authorized in-function iot-data Publish with an unauthorized one never terminates, so neither the delivery nor the refusal is observable.

AWS: Smithy: iotdataplane#Publish is POST /topics/{topic} (topic httpLabel, qos httpQuery range 0..1, payload httpPayload blob) on the account's data endpoint, which is discovered with iot#DescribeEndpoint(endpointType='iot:Data-ATS'). A publish by a caller whose IAM role grants iot:Publish on arn:aws:iot:<region>:<account>:topic/<topic> succeeds (HTTP 200, empty body) and an IoT topic rule whose SQL is `SELECT * FROM '<topic>'` with an sqs action delivers the message body to the named queue. Recorded behavior only for the negative leg: when the caller's role does not grant iot:Publish, the data-plane publish is refused and the SDK surfaces error code `ForbiddenException` (HTTP 403) — note the model for Publish lists UnauthorizedException (401) and does not model ForbiddenException, so this code/status pair comes from the recording, not the model — and nothing is delivered to the rule's SQS action. Fixed part of that refusal is the error code itself; the message's variable parts are the topic and the caller's role/ARN taken from the request and account state.

Emulator: the check reaches no verdict. The suite reports `Error: Check timed out after 60.0s` with no assertion message, so the first leg — invoke the granted function, then poll the rule's SQS queue for the published marker — never resolves within the harness budget, and the unauthorized leg (which must answer ForbiddenException and deliver nothing) is never attempted. Hypothesis: the data endpoint DescribeEndpoint publishes is not served, or the topic rule never dispatches its sqs action, so the positive control can never be measured. Neighbouring open issues describe the same shape for other checks — 20261110T210703Z-iot-lambda-an-iot-data-publish-from-a.md, 20261107T145004Z-iot-lambda-a-message-published-from-a-lambda.md, 20261109T120740Z-iot-lambda-every-rule-lambda-check-is-blocked.md and 20261111T081112Z-iot-lambda-an-in-function-iot-data-publish-never-completes.md — but none of them carries this check's replay key, so this cause is filed separately and those are named here only as neighbours.

Why: a client cannot tell an authorization decision from an unreachable data plane. An over-permissive role (publishing successfully with no iot:Publish grant) and a correctly refused one are indistinguishable, because neither the 403/ForbiddenException refusal nor the authorized publish's downstream SQS delivery is ever produced; the whole permission boundary is unverifiable.

Fix: the iot-data Publish handler and the data-plane endpoint it is reached on (it must be the address iot DescribeEndpoint announces), its IAM evaluation of iot:Publish against arn:aws:iot:<region>:<account>:topic/<topic> so an ungranted caller is refused with the recorded code and status, and the IoT rules-engine dispatch of a topic rule's sqs action so an authorized publish lands in the queue.

Traces: iot__lambda.lambda-sdk-iot-publish-missing-permission-negative.

## Evidence
Failing check: [mismatch] iot__lambda.lambda-sdk-iot-publish-missing-permission-negative
Replay detail (verbatim): `Error: Check timed out after 60.0s`

Expected (AWS): (a) a Lambda whose role grants iot:DescribeEndpoint + iot:Publish on arn:aws:iot:<region>:<account>:topic/<topic> publishes successfully and the topic rule `SELECT * FROM '<topic>'` with an sqs action delivers exactly the published JSON body to the queue; (b) a sibling Lambda whose role grants only iot:DescribeEndpoint gets error code ForbiddenException (HTTP 403) from iot-data Publish and nothing arrives in the queue for its marker.

Actual (emulator): no verdict — the run is terminated at the 60 s harness cap before the positive control's marker is ever received from the queue, so neither leg produces an observation.

## Steps to Reproduce
1. Create an SQS queue and an IoT rule role that may sqs:SendMessage to it.
2. CreateTopicRule with sql `SELECT * FROM '<topic>'` and an sqs action (roleArn = that role, queueUrl = the queue, useBase64 false).
3. Create two Lambda functions from the same handler: one role grants iot:DescribeEndpoint + iot:Publish on arn:aws:iot:<region>:<account>:topic/<topic>, the other grants only iot:DescribeEndpoint.
4. The handler calls iot DescribeEndpoint(endpointType='iot:Data-ATS') and then iot-data Publish(topic=<topic>, qos=0, payload=<json marker>) against https://<that endpoint>.
5. Invoke the granted function and poll the queue for its marker (expect delivery), then invoke the ungranted function (expect error code ForbiddenException) and hold the queue absent of its marker.
Observed: the run never gets past step 5's first poll — it is cut off at the 60 s harness cap with no assertion.
