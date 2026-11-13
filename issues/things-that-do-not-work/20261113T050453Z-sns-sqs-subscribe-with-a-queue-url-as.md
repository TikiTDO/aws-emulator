Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns__sqs.subscribe-with-a-queue-url-instead-of-its-arn-rejected
Issue-Key: sns__sqs.subscribe-with-a-queue-url-instead-of-its-arn-rejected|run#5|AssertionError|assert (code, message) == ("InvalidParameter", "Invalid parameter: SQS endpoint ARN"), (
Issue-Commit: 55ffe08148b0f5da621dea606ca05d7920bfd4b8

# sns__sqs: Subscribe with a queue URL as the sqs Endpoint is refused with the generic "Invalid parameter: Endpoint" instead of "Invalid parameter: SQS endpoint ARN"

> _Note: this issue was identified against the code base at commit `55ffe08148b0f5da621dea606ca05d7920bfd4b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Subscribe

## Description
One net-new root cause, grounded in one failing check listed under '### sns__sqs' with no OWNED / ALREADY FILED marker.

Cause 1 of 1: Subscribe's refusal of a queue URL given where the sqs protocol requires a queue ARN does not name the endpoint form.
AWS: for Protocol='sqs' the Endpoint must be the queue's ARN (Smithy: SubscribeInput$Endpoint — "For the sqs protocol, the endpoint is the ARN of an Amazon SQS queue"). Passing the queue URL that CreateQueue returns is refused with HTTP 400 and Error.Code 'InvalidParameter' (Smithy: Subscribe errors include InvalidParameterException, errorKind client, httpError 400, awsQueryError code 'InvalidParameter' / httpResponseCode 400; the model documents only "Indicates that a request parameter does not comply with the associated constraints", so the wording is recorded behavior only). The recorded Message is "Invalid parameter: SQS endpoint ARN": entirely FIXED text naming the endpoint FORM the protocol requires — the rejected URL, the queue name and the protocol string are NOT part of it. Nothing is created (ListSubscriptionsByTopic on the topic is empty afterwards), and the same queue then subscribes by its ARN and is listed exactly once.
Emulator: the call is refused 400 InvalidParameter but with Message 'Invalid parameter: Endpoint' — the protocol-independent constant INVALID_ENDPOINT_REFUSAL in services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/subscribe.rs, documented there as deliberately identical for every protocol. Replay: AssertionError: Subscribe with a queue URL as the sqs Endpoint raised ('InvalidParameter', 'Invalid parameter: Endpoint').
Why: the queue URL is exactly what CreateQueue hands back, so this is the mistake callers actually make when wiring SNS→SQS. AWS's sentence says an ARN is wanted; the emulator's says only that some endpoint was bad, so the caller cannot tell a wrong FORM from a wrong queue, and any client or script matching the refusal text sees a sentence AWS never sends.
Fix: Subscribe's endpoint validation in the sns crate — derive the InvalidParameter message from the requested Protocol instead of emitting one protocol-independent constant. The same constant produces the lambda-protocol and single-service sqs divergences filed under the 'lambda__sns' and 'sns' service headings in this sweep (kept as separate issues because each service heading takes its own gate verdict). Open issue 20261111T063459Z-sns-sqs-the-out-of-charset-message-attribute.md re-words a Publish ParameterValueInvalid message, a different operation and message, and does not carry this check.
Traces: sns__sqs.subscribe-with-a-queue-url-instead-of-its-arn-rejected.

## Evidence
Behavioral suite: 452/602 passed. The check is reported under '### sns__sqs' with no OWNED / ALREADY FILED marker.

[mismatch] sns__sqs.subscribe-with-a-queue-url-instead-of-its-arn-rejected
  Expected: Subscribe(TopicArn=<topic>, Protocol='sqs', Endpoint=<the QueueUrl from CreateQueue>) -> HTTP 400, Error.Code 'InvalidParameter', Error.Message "Invalid parameter: SQS endpoint ARN" (fixed text; nothing from the request in it); ListSubscriptionsByTopic == [] afterwards; the same queue's ARN then subscribes and is listed exactly once.
  Actual: ('InvalidParameter', 'Invalid parameter: Endpoint').
  Replay detail: File "check_sns__sqs_subscribe_with_a_queue_url_instead_of_its_arn_rejected.py", line 27, in run — AssertionError: Subscribe with a queue URL as the sqs Endpoint raised ('InvalidParameter', 'Invalid parameter: Endpoint').

  Source: services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/subscribe.rs — `const INVALID_ENDPOINT_REFUSAL: &str = "Invalid parameter: Endpoint";` used for every protocol.

## Steps to Reproduce
CreateTopic a standard SNS topic and CreateQueue a standard SQS queue; keep both the QueueUrl and (via GetQueueAttributes QueueArn) the queue ARN. Call Subscribe(TopicArn=<topic>, Protocol='sqs', Endpoint=<QueueUrl>): it must be refused with Code 'InvalidParameter' and Message exactly "Invalid parameter: SQS endpoint ARN". ListSubscriptionsByTopic(<topic>) must then return no subscriptions. Control: Subscribe(TopicArn=<topic>, Protocol='sqs', Endpoint=<QueueArn>) succeeds and ListSubscriptions shows exactly that one (SubscriptionArn, Endpoint) pair. Replay: sns__sqs.subscribe-with-a-queue-url-instead-of-its-arn-rejected.
