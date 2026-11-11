Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.queue-policy-deny-and-missing-allow-both-dead-letter-as-no-resource-without-retries events__lambda__sqs.eventbridge-sqs-target-missing-queue-policy-dlq events__s3__sqs.eventbridge-dlq-on-target-failure cloudwatch__events__s3__sqs.xsvc-s3-2-eventbridge-target-without-queue-policy-dead-letters-and-counts-failures-until-fixed
Issue-Key: events.queue-policy-deny-and-missing-allow-both-dead-letter-as-no-resource-without-retries|run#29|AssertionError|assert attrs["ERROR_CODE"] == "NO_RESOURCE", f"an explicit queue-policy Deny dead-lettered as {attrs['ERROR_CODE']!r}"
Issue-Key: events__lambda__sqs.eventbridge-sqs-target-missing-queue-policy-dlq|run#20|AssertionError|assert (error_code, error_message) == ("NO_RESOURCE", expected_message), (
Issue-Key: events__s3__sqs.eventbridge-dlq-on-target-failure|run#19|AssertionError|assert dlq_attrs.get("ERROR_CODE", {}).get("StringValue") == "NO_RESOURCE", (
Issue-Key: cloudwatch__events__s3__sqs.xsvc-s3-2-eventbridge-target-without-queue-policy-dead-letters-and-counts-failures-until-fixed|run#32|AssertionError|assert attrs.get("ERROR_CODE") == "NO_RESOURCE", (
Issue-Commit: f6476a64244d41dc0fd9adea93cb75255e1386be

# EventBridge SQS-target dead letters misclassify an unauthorized send as NO_PERMISSIONS/AccessDenied instead of NO_RESOURCE with SQS's NonExistentQueue refusal

> _Note: this issue was identified against the code base at commit `f6476a64244d41dc0fd9adea93cb75255e1386be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One net-new root cause, grounded in four failing checks. The checks are listed by the suite under four headings that all include EventBridge (`events`, `events__lambda__sqs`, `events__s3__sqs`, `cloudwatch__events__s3__sqs`); the defect is single and lives in EventBridge's target-delivery failure classifier plus the SQS refusal it classifies, so it is filed once, under `events`, with every failing check id in trace_ids.

Cause 1 of 1: a rule's SQS target whose queue exists but whose resource policy does not authorize events.amazonaws.com (no policy at all, or an explicit Deny) is dead-lettered with ERROR_CODE='NO_PERMISSIONS' and an invented AccessDenied ERROR_MESSAGE, where AWS dead-letters it with ERROR_CODE='NO_RESOURCE' carrying SQS's own NonExistentQueue refusal.

AWS: EventBridge assumes no role for an SQS target — the send is authorized solely by the destination queue's resource policy, and the refusal the queue produces is what EventBridge classifies. Smithy (sqs): SendMessage's modeled errors are InvalidAddress, InvalidMessageContents, InvalidSecurity, KMS.* (KmsAccessDenied/KmsDisabled/KmsInvalidKeyUsage/KmsInvalidState/KmsNotFound/KmsOptInRequired/KmsThrottled), QueueDoesNotExist, RequestThrottled and UnsupportedOperation — there is NO AccessDenied/AccessDeniedException on SendMessage at all. QueueDoesNotExist is httpError 400 with awsQueryError code `AWS.SimpleQueueService.NonExistentQueue`, i.e. SQS hides a queue it will not let the caller write to behind the same not-found class. The EventBridge DLQ message attributes themselves are not modelled, so their values are recorded behavior: ERROR_CODE='NO_RESOURCE' (a fixed enumerated token, nothing in it comes from the request) and ERROR_MESSAGE='The specified queue does not exist or you do not have access to it. (Service: AmazonSQS; Status Code: 400; Error Code: AWS.SimpleQueueService.NonExistentQueue; Request ID: <id>; Proxy: null)' — every word of that sentence is fixed text except the Request ID, which is the per-invocation id SQS minted. The sibling attributes RULE_ARN and TARGET_ARN name the rule and the target of the failed delivery (values from state), and the delivery is dead-lettered once with no retries.

Emulator: the event IS dead-lettered, with the right body, RULE_ARN and TARGET_ARN, but ERROR_CODE='NO_PERMISSIONS' and ERROR_MESSAGE='User: events.amazonaws.com is not authorized to perform: sqs:sendmessage on resource: <queue ARN> because no resource-based policy allows the sqs:sendmessage action (Service: AmazonSQS; Status Code: 403; Error Code: AccessDenied; Request ID: <id>; Proxy: null)'. That is a 403 AccessDenied in SQS's name for an error SQS does not model on SendMessage, with a lower-cased action token ('sqs:sendmessage'), and it drives the wrong ERROR_CODE class.

Why: a client that triages, routes or alarms on the EventBridge DLQ taxonomy reads the wrong class for the commonest EventBridge→SQS misconfiguration, and any consumer branch keyed on NO_RESOURCE (or on SQS's NonExistentQueue sentence, which operators grep for) is dead code against this emulator. Replay tooling that re-drives only NO_RESOURCE letters re-drives nothing.

Fix: the EventBridge target-delivery failure classifier that fills the dead-letter ERROR_CODE/ERROR_MESSAGE attributes, together with the SQS SendMessage authorization refusal it consumes — an unauthorized send must surface as SQS's QueueDoesNotExist (400 / AWS.SimpleQueueService.NonExistentQueue / that fixed sentence) and be classified NO_RESOURCE. CAUTION for the fixer: do not flip the classifier globally. Open issue 20261103T130707Z-redshift-sqs-eventbridge-dead-letters-a-delivery-its.md and the RESOLVED issue 20261107T205257Z-events-4-net-new-causes-an-oauth-connection.md (its cause 2) ask for NO_PERMISSIONS on an authorization failure; those keys are not these, and the flip they produced is what these four checks now fail on. The discriminator is which refusal the target service actually returns: SQS answers NonExistentQueue for a queue-policy failure -> NO_RESOURCE, while a target whose service really answers AccessDenied (a role-assumed target, or KMS on an encrypted queue) stays NO_PERMISSIONS.

Traces: events.queue-policy-deny-and-missing-allow-both-dead-letter-as-no-resource-without-retries; events__lambda__sqs.eventbridge-sqs-target-missing-queue-policy-dlq; events__s3__sqs.eventbridge-dlq-on-target-failure; cloudwatch__events__s3__sqs.xsvc-s3-2-eventbridge-target-without-queue-policy-dead-letters-and-counts-failures-until-fixed.

## Evidence
events.queue-policy-deny-and-missing-allow-both-dead-letter-as-no-resource-without-retries (line 67):
  assert attrs["ERROR_CODE"] == "NO_RESOURCE"
  AssertionError: an explicit queue-policy Deny dead-lettered as 'NO_PERMISSIONS'

events__lambda__sqs.eventbridge-sqs-target-missing-queue-policy-dlq (line 89):
  AssertionError: expected DLQ message attributes ERROR_CODE=NO_RESOURCE / ERROR_MESSAGE='The specified queue does not exist or you do not have access to it. (Service: AmazonSQS; Status Code: 400; Error Code: AWS.SimpleQueueService.NonExistentQueue; Request ID: <id>; Proxy: null)' for a target queue with no resource policy, got 'NO_PERMISSIONS' / 'User: events.amazonaws.com is not authorized to perform: sqs:sendmessage on resource: arn:aws:sqs:us-west-2:<acct>:<queue> because no resource-based policy allows the sqs:sendmessage action (Service: AmazonSQS; Status Code: 403; Error Code: AccessDenied; Request ID: <id>; Proxy: null)'

events__s3__sqs.eventbridge-dlq-on-target-failure (line 112):
  AssertionError: expected ERROR_CODE=NO_RESOURCE on the DLQ message, got {'ERROR_CODE': 'NO_PERMISSIONS', 'ERROR_MESSAGE': 'User: events.amazonaws.com is not authorized to perform: sqs:sendmessage ... Status Code: 403; Error Code: AccessDenied ...', 'RULE_ARN': <rule arn>, 'TARGET_ARN': <queue arn>}

cloudwatch__events__s3__sqs.xsvc-s3-2-eventbridge-target-without-queue-policy-dead-letters-and-counts-failures-until-fixed (line 123):
  AssertionError: dead letter carries ERROR_CODE='NO_PERMISSIONS', expected 'NO_RESOURCE' (all other attributes — ERROR_MESSAGE, TARGET_ARN, RULE_ARN — were present; the check stops here, before its FailedInvocations-metric and post-fix-delivery assertions).

Expected vs actual, per dead-letter message attribute:
  expected ERROR_CODE   = "NO_RESOURCE"                      actual "NO_PERMISSIONS"
  expected ERROR_MESSAGE= SQS's fixed NonExistentQueue sentence (Status Code: 400; Error Code: AWS.SimpleQueueService.NonExistentQueue), Request ID the only variable part
  actual   ERROR_MESSAGE= an emulator-authored AccessDenied sentence (Status Code: 403; Error Code: AccessDenied) naming action 'sqs:sendmessage'
RULE_ARN/TARGET_ARN are per-run values and are not part of the requirement beyond naming the failed delivery's rule and target.

## Steps to Reproduce
1. CreateQueue twice: one dead-letter queue, one rule-target queue. Grant the DLQ a policy allowing events.amazonaws.com sqs:SendMessage. Leave the TARGET queue with no resource policy (repeat with a policy whose Statement explicitly Denies events.amazonaws.com — both must behave identically).
2. CreateEventBus (or use default), PutRule with an event pattern, PutTargets with the target queue's ARN plus DeadLetterConfig.Arn = the DLQ ARN and RetryPolicy {MaximumRetryAttempts: 185, MaximumEventAgeInSeconds: 86400}.
3. PutEvents with one matching entry.
4. ReceiveMessage on the DLQ with MessageAttributeNames=['All'].
   Expected: exactly one message, ERROR_CODE='NO_RESOURCE', ERROR_MESSAGE='The specified queue does not exist or you do not have access to it. (Service: AmazonSQS; Status Code: 400; Error Code: AWS.SimpleQueueService.NonExistentQueue; Request ID: <id>; Proxy: null)', RULE_ARN=the rule, TARGET_ARN=the target queue, and no further attempts.
   Emulator: ERROR_CODE='NO_PERMISSIONS' with a 403 AccessDenied ERROR_MESSAGE.
5. SetQueueAttributes to grant the target queue events.amazonaws.com sqs:SendMessage and PutEvents again — delivery succeeds, proving the queue existed all along and only authorization had failed.
