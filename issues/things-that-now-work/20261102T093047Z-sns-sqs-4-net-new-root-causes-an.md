Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns__sqs.delivery-policy-on-an-sqs-subscription-rejected sns__sqs.fifo-queue-subscription-to-a-standard-topic-rejected sns__sqs.raw-message-delivery-with-a-non-boolean-value-rejected sns__sqs.resubscribing-with-different-attributes-rejected-and-the-original-is-unchanged
Issue-Key: sns__sqs.delivery-policy-on-an-sqs-subscription-rejected|_error#9|AssertionError|raise AssertionError(f"the call succeeded; real AWS refuses it: {result!r}")
Issue-Key: sns__sqs.fifo-queue-subscription-to-a-standard-topic-rejected|_error#7|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: sns__sqs.raw-message-delivery-with-a-non-boolean-value-rejected|run#6|AssertionError|assert msg == "Invalid parameter: Attributes Reason: RawMessageDelivery: Invalid value [yes]. Must be true or false.", (
Issue-Key: sns__sqs.resubscribing-with-different-attributes-rejected-and-the-original-is-unchanged|_error#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# sns__sqs: 4 net-new root causes — an out-of-range subscription DeliveryPolicy, a FIFO queue subscribing to a standard topic and a conflicting re-subscribe are all accepted, and the RawMessageDelivery refusal message is emulator-worded

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Subscribe

## Description
This issue bundles FOUR net-new root causes of service `sns__sqs`. All 4 must be closed before submitting. None of the four grounding checks was marked ALREADY FILED or OWNED by the replay report, and the one open issue for this service token (20261029T183635Z-sns-sqs-the-delivered-notification-envelope-omits-signatureversion.md) carries none of these four ids. Open SNS-service issues describe neighbouring over-permissiveness (20261025T025317Z-sns-is-over-permissive-ten-validation-rules-aws.md, which owns the `sns`-token twin of Cause 2) but not these.

=== Cause 1 of 4: a subscription DeliveryPolicy with a retry count above the documented maximum is accepted ===
AWS: Smithy (sns) SetSubscriptionAttributes declares InvalidParameterException (errorKind client, httpError 400, awsQueryError code `InvalidParameter`). A subscription's DeliveryPolicy is schema-validated the same way a topic's is, independently of the protocol: `healthyRetryPolicy.numRetries` has a documented maximum of 100, and a value above it is refused with code `InvalidParameter` and the message `Invalid parameter: DeliveryPolicy: healthyRetryPolicy.numRetries must be less than or equal to 100` — the fixed text is `Invalid parameter: DeliveryPolicy: healthyRetryPolicy.numRetries must be less than or equal to ` and the limit 100 is the documented bound, not a per-request value. The refusal stores nothing: the previously accepted policy is still readable unchanged. A well-formed policy (numRetries 3) is accepted and round-trips on the same SQS subscription, so the validation is not skipped merely because SQS delivery never consults retries.
Emulator: the out-of-range policy is accepted — HTTP 200, `content-type: text/xml` — and replaces the stored one.
Why: the schema is the only feedback a client gets on a delivery policy; accepting an impossible retry budget means a configuration that a real account refuses ships silently, and the client believes a retry policy is in force that SNS would never have stored.
Fix: the SetSubscriptionAttributes DeliveryPolicy validation (shared with Subscribe's Attributes and the topic DeliveryPolicy path) — validate the healthyRetryPolicy fields against their documented bounds and refuse with the code and message above, leaving the stored value untouched.
Traces: sns__sqs.delivery-policy-on-an-sqs-subscription-rejected.

=== Cause 2 of 4: a FIFO SQS queue is allowed to subscribe to a STANDARD topic ===
AWS: Smithy (sns) Subscribe declares InvalidParameterException (client, httpError 400, awsQueryError `InvalidParameter`). A FIFO queue may subscribe only to a FIFO topic; subscribing one to a standard topic is refused with code `InvalidParameter` and the message `Invalid parameter: Invalid parameter: Endpoint Reason: FIFO SQS Queues can not be subscribed to standard SNS topics` — entirely fixed text (the doubled `Invalid parameter: ` prefix and the `Endpoint Reason: ` segment are AWS's own wording). The refusal is total: the standard topic's subscription list stays empty and its SubscriptionsConfirmed/SubscriptionsPending/SubscriptionsDeleted counters do not move. The identical queue subscribed to a FIFO topic is accepted and moves only that topic's counters.
Emulator: the Subscribe SUCCEEDS and the FIFO queue becomes a subscriber of the standard topic.
Why: the subscription is created and reads back confirmed, but it is a pairing SNS cannot deliver through, so a client builds an ordering guarantee on a wiring the account refuses outright — and because the counters move too, the topic's own view of its subscribers is wrong.
Fix: the Subscribe handler's endpoint/protocol compatibility validation — for Protocol `sqs`, compare the endpoint queue's FIFO-ness with the topic's FifoTopic attribute and refuse the mismatch with the code and message above before any subscription record or counter is written.
Traces: sns__sqs.fifo-queue-subscription-to-a-standard-topic-rejected.

=== Cause 3 of 4: the RawMessageDelivery non-boolean refusal message is the emulator's own wording ===
AWS: Subscribe with `Attributes={"RawMessageDelivery": "yes"}` is refused before any subscription is created, with code `InvalidParameter` (Smithy: InvalidParameterException, httpError 400, awsQueryError `InvalidParameter`) and the message `Invalid parameter: Attributes Reason: RawMessageDelivery: Invalid value [yes]. Must be true or false.` — the fixed text is `Invalid parameter: Attributes Reason: RawMessageDelivery: Invalid value [` … `]. Must be true or false.` and the only variable part is the rejected value, echoed from the request between the brackets. The topic's subscriber list stays empty, and `"true"` is accepted and reads back as `true`.
Emulator: the code and the refusal itself are right, but the message is `Invalid parameter: Attributes Reason: RawMessageDelivery - the value must be true or false` — it drops the `: Invalid value [<value>].` form, never echoes the rejected value, and substitutes its own sentence.
Why: the message is the only place SNS states WHICH value it rejected; a client or test matching AWS's sentence (or parsing the bracketed value out of it to report the bad input) gets neither, so a configuration tool cannot tell the operator what to change.
Fix: the Subscribe/SetSubscriptionAttributes boolean-attribute validator's message construction — emit AWS's `<Attribute>: Invalid value [<value>]. Must be true or false.` form with the submitted value interpolated.
Traces: sns__sqs.raw-message-delivery-with-a-non-boolean-value-rejected.

=== Cause 4 of 4: re-subscribing the same endpoint with DIFFERENT attributes is accepted instead of refused ===
AWS: Subscribe is idempotent only for an IDENTICAL request. Re-subscribing the same topic/protocol/endpoint triple with a different attribute value is refused with code `InvalidParameter` (Smithy: InvalidParameterException, httpError 400, awsQueryError `InvalidParameter`) and the message `Invalid parameter: Attributes Reason: Subscription already exists with different attributes` — entirely fixed text. Nothing changes: the original subscription keeps its attribute (RawMessageDelivery stays `true`), it keeps delivering in that format (the next publish arrives as the bare body, not an envelope), and the topic's SQS subscriber count stays at one. Repeating the request with the IDENTICAL attributes returns the original SubscriptionArn.
Emulator: the conflicting re-subscribe SUCCEEDS, so the refusal never happens and the original subscription's attributes are at the mercy of whatever the second call carried.
Why: this is the rule that keeps a re-run of an idempotent provisioning step from silently flipping a live subscription's delivery format. Accepting the conflicting call means a second `Subscribe` can turn raw delivery off (or on) under a running consumer — a format change the consumer never asked for — and a client gets no signal that its two code paths disagree about the subscription's attributes.
Fix: the Subscribe handler's existing-subscription resolution — match on topic/protocol/endpoint, return the existing SubscriptionArn only when the requested Attributes are identical, and otherwise refuse with the code and message above without touching the stored subscription.
Traces: sns__sqs.resubscribing-with-different-attributes-rejected-and-the-original-is-unchanged.

## Evidence
Cause 1 — sns__sqs.delivery-policy-on-an-sqs-subscription-rejected [mismatch]:
  AssertionError: the call succeeded; real AWS refuses it: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml', 'content-length': '283', …}}}
  expected: ('InvalidParameter', 'Invalid parameter: DeliveryPolicy: healthyRetryPolicy.numRetries must be less than or equal to 100') for healthyRetryPolicy.numRetries = 200, with the previously accepted numRetries = 3 still readable.
  actual: SetSubscriptionAttributes returned 200 for numRetries = 200.

Cause 2 — sns__sqs.fifo-queue-subscription-to-a-standard-topic-rejected [mismatch]:
  AssertionError: the call succeeded; real AWS rejects it, so there is no error code
  (raised from `sns.subscribe(TopicArn=<standard topic>, Protocol='sqs', Endpoint=<fifo queue arn>)`)
  expected: ('InvalidParameter', 'Invalid parameter: Invalid parameter: Endpoint Reason: FIFO SQS Queues can not be subscribed to standard SNS topics'), the standard topic's subscription list still empty and its counters still {confirmed: 0, pending: 0, deleted: 0}.

Cause 3 — sns__sqs.raw-message-delivery-with-a-non-boolean-value-rejected [mismatch]:
  AssertionError: the refusal's message was 'Invalid parameter: Attributes Reason: RawMessageDelivery - the value must be true or false'
  expected message: 'Invalid parameter: Attributes Reason: RawMessageDelivery: Invalid value [yes]. Must be true or false.' (code InvalidParameter matched)

Cause 4 — sns__sqs.resubscribing-with-different-attributes-rejected-and-the-original-is-unchanged [mismatch]:
  AssertionError: the call succeeded; real AWS rejects it, so there is no error code
  (raised from a second `sns.subscribe(TopicArn=<topic>, Protocol='sqs', Endpoint=<same queue arn>, Attributes={'RawMessageDelivery': 'false'})` after the original was created with 'true')
  expected: ('InvalidParameter', 'Invalid parameter: Attributes Reason: Subscription already exists with different attributes'), the original subscription still reading RawMessageDelivery='true', the next publish still delivered as the bare body, and an identical re-subscribe returning the original SubscriptionArn.

## Steps to Reproduce
Boot the emulator and replay the four check ids in trace_ids from suite simparity-behavioral-aws-v1.

By hand:
1. Cause 1: CreateTopic; CreateQueue; Subscribe(Protocol='sqs', Endpoint=<queue arn>); SetSubscriptionAttributes(AttributeName='DeliveryPolicy', AttributeValue=<healthyRetryPolicy with numRetries 3>) — accepted (control); then the same with numRetries = 200 — expect 400 InvalidParameter 'Invalid parameter: DeliveryPolicy: healthyRetryPolicy.numRetries must be less than or equal to 100', observe 200, and observe the stored policy replaced.
2. Cause 2: CreateTopic (standard) and CreateTopic(Attributes={'FifoTopic':'true'}) with a `.fifo` name; CreateQueue with FifoQueue='true'; Subscribe(TopicArn=<standard topic>, Protocol='sqs', Endpoint=<fifo queue arn>) — expect the InvalidParameter refusal above, observe success; check GetTopicAttributes SubscriptionsConfirmed on the standard topic moved from 0. The same queue subscribed to the FIFO topic must stay accepted.
3. Cause 3: Subscribe(TopicArn=<topic>, Protocol='sqs', Endpoint=<queue arn>, Attributes={'RawMessageDelivery':'yes'}) — the code is InvalidParameter but the message is the emulator's sentence instead of 'Invalid parameter: Attributes Reason: RawMessageDelivery: Invalid value [yes]. Must be true or false.'
4. Cause 4: Subscribe the queue with Attributes={'RawMessageDelivery':'true'}; Subscribe the SAME topic/protocol/endpoint with Attributes={'RawMessageDelivery':'false'} — expect 400 InvalidParameter 'Invalid parameter: Attributes Reason: Subscription already exists with different attributes', observe success; then Publish and confirm the queue still receives the bare body, and that an identical re-subscribe (RawMessageDelivery='true') returns the original SubscriptionArn.
