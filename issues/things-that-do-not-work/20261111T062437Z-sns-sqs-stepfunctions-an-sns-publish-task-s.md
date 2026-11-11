Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns__sqs__stepfunctions.sns-publish-attributes-reach-subscriber-and-invalid-attribute-fails-task
Issue-Key: sns__sqs__stepfunctions.sns-publish-attributes-reach-subscriber-and-invalid-attribute-fails-task|run#29|AssertionError|assert bad["error"] == "SNS.InvalidParameterValueException", (
Issue-Commit: c43f13612f17253679972bb58b777ab0e011820f

# sns__sqs__stepfunctions: an sns:publish Task's error name is built from SNS's wire error code — States reports SNS.ParameterValueInvalidException where AWS reports SNS.InvalidParameterValueException

> _Note: this issue was identified against the code base at commit `c43f13612f17253679972bb58b777ab0e011820f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
One net-new root cause, grounded in a failing behavioral check the replay report left unmarked (no ALREADY-FILED marker, no OWNED marker).

Cause 1 of 1 — when an optimized Task on arn:aws:states:::sns:publish is refused by SNS, Step Functions names the state's error from SNS's awsQuery wire error code with "Exception" appended, instead of from the modeled exception shape name.
AWS: Smithy (services/service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/com-amazonaws-sns-amazonsimplenotificationservice.json) models the refusal for a message attribute whose value does not match its DataType as com.amazonaws.sns#InvalidParameterValueException — errorKind client, httpError 400, awsQueryError {code: "ParameterValueInvalid", httpResponseCode: 400}. Step Functions names a failed service-integration task "<ServicePrefix>.<ExceptionShapeName>", so the Task's error is "SNS.InvalidParameterValueException" — the SDK exception shape name, not the awsQuery code. The cause is SNS's message relayed verbatim, whose fixed text for this class is "Could not cast message attribute '<name>' value to number. (Service: AmazonSNS; Status Code: 400; Error Code: ParameterValueInvalid; Request ID: <request-id>; Proxy: null)" — <name> is the offending attribute key from the request's MessageAttributes and <request-id> is the inner call's per-request value; the wire Error Code segment is where "ParameterValueInvalid" legitimately appears.
Emulator: the execution does end FAILED, but DescribeExecution reports error 'SNS.ParameterValueInvalidException' — the awsQuery code "ParameterValueInvalid" with the literal suffix "Exception", i.e. the task-error name is derived from the wire error code rather than from the modeled exception shape. (The check asserts the error name before the cause, so the cause's wording was not reached and is not claimed here; it should be re-asserted once the name is fixed.)
Why: Retry and Catch in a state machine match on ErrorEquals string literals. A client that writes the documented ErrorEquals: ["SNS.InvalidParameterValueException"] never matches, so its retrier or catcher does not fire and the execution fails where AWS would have handled it — and a client that matched the emulator's spelling would break against AWS. Because the rule is generic (shape name, not wire code), every SNS error whose awsQueryError code differs from its shape name is mis-named the same way, so the defect is not limited to this one attribute rejection.
Fix: the optimized sns:publish integration's error-mapping step in the Task handler — build the "<ServicePrefix>.<Exception>" task-error name from the modeled error shape's name, not from the awsQuery error code, and keep the wire code only inside the relayed cause's "Error Code:" segment.
Traces: sns__sqs__stepfunctions.sns-publish-attributes-reach-subscriber-and-invalid-attribute-fails-task. Related but not covering it: 20261111T004529Z-sns-sqs-stepfunctions-2-net-new-causes-the.md concerns the sns:publish AuthorizationError's principal and action casing, a different cause in the same integration.

## Evidence
sns__sqs__stepfunctions.sns-publish-attributes-reach-subscriber-and-invalid-attribute-fails-task — verdict mismatch.
The positive leg passed: a Task on arn:aws:states:::sns:publish with String and Number MessageAttributes succeeded, its output carried MessageId, and the raw-delivery-off SQS subscriber received the envelope with matching MessageId and MessageAttributes.
The rejection leg diverges. Publishing with MessageAttributes {"count": {"DataType": "Number", "StringValue": "not-a-number"}} correctly ends the execution FAILED, but:
Expected: DescribeExecution error == "SNS.InvalidParameterValueException" (the modeled shape name), with cause "Could not cast message attribute 'count' value to number. (Service: AmazonSNS; Status Code: 400; Error Code: ParameterValueInvalid; Request ID: <request-id>; Proxy: null)".
Actual (AssertionError from the check): "the rejected attribute must fail with Error 'SNS.InvalidParameterValueException', got 'SNS.ParameterValueInvalidException'".
The attribute name 'count' above is the request's own key (variable part); the surrounding sentence and the parenthesised Service/Status Code/Error Code/Proxy segments are the fixed text. The cause assertion and the "a rejected publish is never delivered" assertion sit after the error-name assertion and were not reached.

## Steps to Reproduce
1. sns:CreateTopic; sqs:CreateQueue; grant the topic sqs:SendMessage on the queue via its Policy and sns:Subscribe(Protocol=sqs, Endpoint=<queue ARN>) with raw message delivery left off; wait for PendingConfirmation == "false".
2. iam:CreateRole trusting states.amazonaws.com with an inline Allow of sns:Publish on the topic ARN.
3. sfn:CreateStateMachine(type=STANDARD) with a single state: {"Type":"Task","Resource":"arn:aws:states:::sns:publish","Parameters":{"TopicArn":"<topic>","Message.$":"$.message","MessageAttributes.$":"$.attributes"},"End":true}.
4. sfn:StartExecution with input {"message":"<any>","attributes":{"count":{"DataType":"Number","StringValue":"not-a-number"}}}.
5. Poll sfn:DescribeExecution to a terminal status and read error — expected "SNS.InvalidParameterValueException". Observed "SNS.ParameterValueInvalidException".
6. Then read cause — expected "Could not cast message attribute 'count' value to number. (Service: AmazonSNS; Status Code: 400; Error Code: ParameterValueInvalid; Request ID: <id>; Proxy: null)" — and confirm the rejected message never reaches the subscribed queue.
