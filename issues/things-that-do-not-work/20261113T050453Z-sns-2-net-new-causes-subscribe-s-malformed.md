Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.subscribe-invalid-endpoint-rejected sns.topic-attributes-subscription-counters-track-state
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.subscribe-invalid-endpoint-rejected|run#9|AssertionError|assert message == "Invalid parameter: SQS endpoint ARN", (
Issue-Key: sns.topic-attributes-subscription-counters-track-state|_poll#20|Timeout|time.sleep(interval_s)
Issue-Commit: 55ffe08148b0f5da621dea606ca05d7920bfd4b8

# sns: 2 net-new causes — Subscribe's malformed-endpoint refusal is the generic "Invalid parameter: Endpoint" for every protocol, and SubscriptionsDeleted is withheld for 15 minutes after Unsubscribe

> _Note: this issue was identified against the code base at commit `55ffe08148b0f5da621dea606ca05d7920bfd4b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Subscribe

## Description
Two net-new root causes in SNS. Both failing checks are listed under '### sns' with no OWNED / ALREADY FILED marker, so no open issue carries their replay keys; the neighbouring open issues named per cause below do not cover them.

Cause 1 of 2: Subscribe refuses an Endpoint that is not in the form the requested Protocol requires with one generic sentence, instead of the per-protocol sentence that names the endpoint form.
AWS: Subscribe with Protocol=sqs and an Endpoint that is not an SQS queue ARN at all (a bare string such as 'not-an-arn') is refused with HTTP 400 and Error.Code 'InvalidParameter' (Smithy: Subscribe errors include InvalidParameterException, errorKind client, httpError 400, awsQueryError code 'InvalidParameter' / httpResponseCode 400 — the model documents only "Indicates that a request parameter does not comply with the associated constraints", so the wording below is recorded behavior only). The recorded Message is "Invalid parameter: SQS endpoint ARN": entirely FIXED text — it names the endpoint FORM the sqs protocol requires, and no part of it is taken from the request (neither the rejected value nor the protocol string appears in it). The refusal creates nothing: ListSubscriptionsByTopic on the topic afterwards shows only the control subscription, and a well-formed queue ARN on the same topic/protocol is still accepted even for a queue that does not exist.
Emulator: the same call is refused 400 InvalidParameter but with Message 'Invalid parameter: Endpoint' — one constant used for every protocol (services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/subscribe.rs, const INVALID_ENDPOINT_REFUSAL, whose doc comment states the sentence is deliberately "the same whichever protocol was asked for"). Replay: AssertionError: unexpected refusal message for a non-ARN sqs endpoint: 'Invalid parameter: Endpoint'.
Why: a client cannot tell WHICH endpoint contract it violated. The sqs/lambda/firehose/application protocols all demand an ARN of a different service, and AWS's message is the only thing that says which one; callers (and CDK/SDK error handling and operator runbooks) that match on the refusal text see a sentence AWS never sends, so the error is misclassified and self-service remediation is lost.
Fix: Subscribe's endpoint validation in the sns crate — the refusal message must be derived from the requested Protocol (the per-protocol endpoint-form sentence) rather than emitted from one protocol-independent constant.
Traces: sns.subscribe-invalid-endpoint-rejected. The same single constant also produces the lambda- and sqs-protocol divergences filed for the lambda__sns and sns__sqs service headings in this same sweep.

Cause 2 of 2: GetTopicAttributes' SubscriptionsDeleted never moves inside any client convergence loop — the emulator deliberately withholds a deletion for 15 minutes.
AWS: GetTopicAttributes reports the three computed counters on every topic (Smithy: GetTopicAttributesResponse$Attributes documents "SubscriptionsConfirmed – The number of confirmed subscriptions for the topic", "SubscriptionsPending – The number of subscriptions pending confirmation for the topic", "SubscriptionsDeleted – The number of deleted subscriptions for the topic"). Recorded behavior: on a fresh topic all three read 0; an auto-confirming sqs Subscribe takes confirmed to 1 with pending 0; a confirmation-requiring email Subscribe takes pending to 1 while confirmed holds at 1; Unsubscribe of the confirmed subscription takes confirmed back to 0, leaves pending at 1, makes GetSubscriptionAttributes on the removed ARN refuse 404 NotFound with Message "Subscription does not exist" (fixed text), and SubscriptionsDeleted reaches at least 1 — the recording observes all of that, polling GetTopicAttributes every 2s, inside the suite's 60s per-check budget.
Emulator: the check is cut off — 'Error: Check timed out after 60.0s'. The counter the last stage waits on cannot move: state.rs defines SUBSCRIPTION_DELETION_SETTLES_AFTER = Duration::from_secs(15 * 60) and TopicRecord::count_deleted_subscription pushes `now + SUBSCRIPTION_DELETION_SETTLES_AFTER`, while TopicRecord::reported_subscriptions_deleted counts only entries already due — so SubscriptionsDeleted stays at its pre-Unsubscribe value for 15 minutes of wall clock, far past any client's polling window, and the check never reaches a verdict.
Why: a client that unsubscribes and then reads the topic's counters to confirm the deletion was accounted for waits for ever (or concludes the unsubscribe was not recorded), and every deployment-verification or drift-detection loop built on SubscriptionsDeleted hangs or reports a stale number. The delay is the emulator's own invention — AWS documents no settling period for this counter, and the recording shows it moving promptly.
Fix: the SNS topic-state counter model — TopicRecord::count_deleted_subscription / reported_subscriptions_deleted in the sns crate's state (and the Unsubscribe path that calls them) must account a deletion into SubscriptionsDeleted on a timescale a client's read actually observes, keeping the confirmed/pending counters immediate as they already are. Open issues 20261110T231031Z-cloudwatch-sns-no-aws-sns-per-topic-counter.md and 20261109T183935Z-cloudwatch-sns-2-net-new-causes-the-aws.md concern AWS/SNS CloudWatch metrics, not these topic attributes, and neither carries this check.
Traces: sns.topic-attributes-subscription-counters-track-state.

## Evidence
Behavioral suite: 452/602 passed. Both checks are reported under '### sns' with no OWNED / ALREADY FILED marker.

[mismatch] sns.subscribe-invalid-endpoint-rejected
  Expected: Subscribe(TopicArn=<topic>, Protocol='sqs', Endpoint='not-an-arn') -> HTTP 400, Error.Code 'InvalidParameter', Error.Message "Invalid parameter: SQS endpoint ARN" (fixed text, no request values); ListSubscriptionsByTopic afterwards contains only the control ARN subscription.
  Actual: HTTP 400, Code 'InvalidParameter', Message 'Invalid parameter: Endpoint'.
  Replay detail: File "check_sns_subscribe_invalid_endpoint_rejected.py", line 58, in run — AssertionError: unexpected refusal message for a non-ARN sqs endpoint: 'Invalid parameter: Endpoint'.
  Source: services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/subscribe.rs — `const INVALID_ENDPOINT_REFUSAL: &str = "Invalid parameter: Endpoint";` (one constant for all protocols).

[mismatch] sns.topic-attributes-subscription-counters-track-state
  Expected: GetTopicAttributes reports SubscriptionsConfirmed/SubscriptionsPending/SubscriptionsDeleted = 0/0/0 on a fresh topic; 1/0/0 after an sqs Subscribe; 1/1/0 after an email Subscribe; after Unsubscribe of the sqs subscription confirmed==0, pending==1, GetSubscriptionAttributes(removed ARN) -> 404 NotFound "Subscription does not exist", and SubscriptionsDeleted >= 1 — all observed by 2s polling within 60s.
  Actual: Check timed out after 60.0s — SubscriptionsDeleted never increments.
  Source: services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/state.rs — `pub const SUBSCRIPTION_DELETION_SETTLES_AFTER: Duration = Duration::from_secs(15 * 60);` with TopicRecord::count_deleted_subscription pushing `now + SUBSCRIPTION_DELETION_SETTLES_AFTER` and TopicRecord::reported_subscriptions_deleted counting only due entries.

## Steps to Reproduce
Cause 1: CreateTopic a standard topic. Subscribe(TopicArn=<topic>, Protocol='sqs', Endpoint='arn:aws:sqs:<region>:<account>:<any-name>') — accepted (SNS does not verify the queue exists). Then Subscribe(TopicArn=<topic>, Protocol='sqs', Endpoint='not-an-arn') and read the ClientError: Code must be 'InvalidParameter' and Message exactly "Invalid parameter: SQS endpoint ARN". Then ListSubscriptionsByTopic and confirm only the first endpoint is present. Replay: sns.subscribe-invalid-endpoint-rejected.

Cause 2: CreateTopic a standard topic and GetTopicAttributes — SubscriptionsConfirmed/Pending/Deleted must all be "0". Subscribe(Protocol='sqs', Endpoint=<a well-formed queue ARN>) and poll GetTopicAttributes until SubscriptionsConfirmed==1 (pending 0, deleted 0). Subscribe(Protocol='email', Endpoint='someone@example.com') and poll until SubscriptionsPending==1 with confirmed still 1. Unsubscribe the sqs subscription and poll until SubscriptionsConfirmed==0 (pending still 1); GetSubscriptionAttributes on that ARN must answer 404 NotFound "Subscription does not exist". Then poll GetTopicAttributes for SubscriptionsDeleted >= 1 — it must be observable within a normal convergence loop (the recording sees it inside 60s at 2s intervals), not after 15 minutes. Replay: sns.topic-attributes-subscription-counters-track-state.
