Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.topic-delivery-policy-computes-effective-policy
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.topic-delivery-policy-computes-effective-policy|_err#30|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED: {result!r}")
Issue-Commit: 950d97075a2dc28c97155d1939158688cbb186aa

# sns: SetTopicAttributes accepts a DeliveryPolicy with an unknown http.defaultHealthyRetryPolicy.backoffFunction instead of refusing it InvalidParameter

> _Note: this issue was identified against the code base at commit `950d97075a2dc28c97155d1939158688cbb186aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetTopicAttributes

## Description
One net-new root cause, grounded in the single surviving failing sns check the replay report marks neither ALREADY FILED nor OWNED.

Cause 1 of 1: the DeliveryPolicy document submitted to SetTopicAttributes is only partially validated — a `backoffFunction` outside SNS's allowed vocabulary is accepted and stored instead of being refused synchronously.

AWS: SetTopicAttributes validates the DeliveryPolicy JSON document synchronously and refuses an invalid one (Smithy: SetTopicAttributes errors include InvalidParameterException, awsQueryError code `InvalidParameter`, HTTP 400). `http.defaultHealthyRetryPolicy.backoffFunction` is a closed vocabulary — `linear`, `arithmetic`, `geometric`, `exponential` (SNS Developer Guide, sns-message-delivery-retries) — so a value such as `quadratic` is rejected with Error.Code `InvalidParameter` and HTTP status 400, the write has no effect, and the stored DeliveryPolicy and the computed EffectiveDeliveryPolicy are left exactly as they were. Recorded behavior only for the message text: AWS's text for this particular refusal embeds server-side parser class names and a byte offset into the submitted document, so the check deliberately asserts only the code, and this issue requires only the code `InvalidParameter` and the 400 status, not a literal message. (The sibling numeric rule in the same document IS refused correctly by the emulator today, with the fixed text `Invalid parameter: DeliveryPolicy: http.defaultHealthyRetryPolicy.numRetries must be less than or equal to 100`, whose only variable part is the member path and limit — so the validation seam exists and simply does not cover the enum.)

Emulator: `SetTopicAttributes(AttributeName="DeliveryPolicy")` with `http.defaultHealthyRetryPolicy.backoffFunction = "quadratic"` (every other member legal) returns HTTP 200 with an empty SetTopicAttributesResponse — no error at all. The check fails with `AssertionError: expected the call to be refused, but it SUCCEEDED: {... 'HTTPStatusCode': 200 ...}`.

Why: a client that mistypes or mis-cases a backoff function is told its delivery policy was accepted. SNS then has a retry policy it cannot honour, so the topic's actual retry behaviour silently differs from the document the client believes is in force, and the error-handling path the client wrote for InvalidParameter never runs. The computed EffectiveDeliveryPolicy also reports a backoff function that is not a real one.

Fix: the SetTopicAttributes handler's DeliveryPolicy validation — extend the document validator that already enforces the numeric bounds to also check the enumerated members, rejecting an unrecognised `http.defaultHealthyRetryPolicy.backoffFunction` with InvalidParameterException (code `InvalidParameter`, HTTP 400) and leaving both DeliveryPolicy and EffectiveDeliveryPolicy untouched.

Traces: sns.topic-delivery-policy-computes-effective-policy.

## Evidence
Cause 1 — sns.topic-delivery-policy-computes-effective-policy [mismatch].

Detail from the replay report:
```
File "check_sns_topic_delivery_policy_computes_effective_policy.py", line 155, in run
code2, message2 = _err(lambda: sns.set_topic_attributes(
File "check_sns_topic_delivery_policy_computes_effective_policy.py", line 50, in _err
raise AssertionError(f"expected the call to be refused, but it SUCCEEDED: {result!r}")
AssertionError: expected the call to be refused, but it SUCCEEDED: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
```

Expected: SetTopicAttributes with AttributeName=DeliveryPolicy and AttributeValue containing `{"http": {"defaultHealthyRetryPolicy": {..., "backoffFunction": "quadratic"}, "disableSubscriptionOverrides": false}}` is refused — HTTP 400, Error.Code `InvalidParameter` — and GetTopicAttributes still shows the previously accepted DeliveryPolicy (and its EffectiveDeliveryPolicy) unchanged.

Actual: HTTP 200, no Error element, the call succeeded. The preceding negative case in the same check (numRetries=200) IS refused correctly, so only the enumerated member is unvalidated. (The RequestId/date/content-length in the recorded response are per-run values and are not part of the requirement.)

## Steps to Reproduce
1. `CreateTopic(Name=<any>)`.
2. `SetTopicAttributes(TopicArn=<arn>, AttributeName="DeliveryPolicy", AttributeValue=json)` where json is `{"http": {"defaultHealthyRetryPolicy": {"minDelayTarget": 5, "maxDelayTarget": 60, "numRetries": 7, "numMaxDelayRetries": 2, "numNoDelayRetries": 0, "numMinDelayRetries": 1, "backoffFunction": "exponential"}, "disableSubscriptionOverrides": false}}` — accepted, and GetTopicAttributes round-trips it.
3. Repeat step 2 with the SAME document except `"backoffFunction": "quadratic"`.
   Expected: HTTP 400 with Error.Code `InvalidParameter`.
   Actual: HTTP 200, the call succeeds.
4. `GetTopicAttributes(TopicArn=<arn>)` — after the refusal, DeliveryPolicy and EffectiveDeliveryPolicy must still carry the step-2 document (numRetries=7, backoffFunction=exponential).
