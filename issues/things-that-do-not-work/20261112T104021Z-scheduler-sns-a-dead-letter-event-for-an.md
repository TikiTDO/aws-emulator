Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler__sns.scheduler-sns-target-delivers-the-notification-envelope-and-a-denied-role-withholds-it
Issue-Key: scheduler__sns.scheduler-sns-target-delivers-the-notification-envelope-and-a-denied-role-withholds-it|run#28|AssertionError|assert refusal == "AuthorizationError", (
Issue-Commit: 09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77

# scheduler__sns: a dead-letter event for an SNS-target occurrence denied by its Target.RoleArn reports ERROR_CODE='AccessDeniedException' instead of SNS's AuthorizationError

> _Note: this issue was identified against the code base at commit `09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
This issue carries 1 root cause, in the scheduler__sns pairing (EventBridge Scheduler dispatching to an SNS topic target). The failing check is marked neither ALREADY FILED nor OWNED in the replay report.

Related but NOT covering: 20261111T160016Z-scheduler-sns-an-sns-target-occurrence-whose-target.md carries this same check id but is keyed to the earlier assertion in `_await_dlq_event` ("Target.DeadLetterConfig.Arn receives nothing"); in this replay the dead-letter event IS produced and that assertion is passed, and the check now fails one assertion later, on the event's ERROR_CODE attribute. 20261111T011632Z-scheduler-a-dead-letter-event-s-retry-attempts.md concerns dead-letter events whose RETRY_ATTEMPTS/ERROR_CODE attributes are never produced at all (absence of the attributes), not the VALUE stamped into an ERROR_CODE that is produced, and it does not carry this check id. 20261109T141116Z-scheduler-delivery-failure-handling-5-causes-deadletterconfig-never.md and the Lambda/SQS/RDS-target dead-letter issues are grounded in other target types. 20261110T231031Z-sns-2-net-new-causes-publish-s-authorizationerror.md concerns Publish's AuthorizationError on the SNS data plane as seen by a direct caller, not the code Scheduler stamps onto a dead-letter event; if the root cause turns out to be shared (one generic refusal code minted for every denied call), fixing it there fixes this too, but neither issue owns the other's check.

=== Cause 1 of 1: the dead-letter event of an SNS-target occurrence refused for lack of sns:Publish carries the wrong ERROR_CODE ===
Failing check: scheduler__sns.scheduler-sns-target-delivers-the-notification-envelope-and-a-denied-role-withholds-it

AWS: Smithy (com.amazonaws.sns) — Publish's authorization refusal is `AuthorizationErrorException`, awsQueryError code `AuthorizationError`, httpResponseCode 403; SNS's Publish error set contains NO `AccessDeniedException`. Smithy (com.amazonaws.scheduler) — Target.RoleArn is @required (@length 1..1600, @pattern ^arn:aws(-[a-z]+)?:iam::\d{12}:role\/[\w+=,.@\/-]+$), documented as "The Amazon Resource Name (ARN) of the IAM role that EventBridge Scheduler will use for this target when the schedule is invoked", and Target.DeadLetterConfig.Arn (an SQS queue ARN) is where "EventBridge Scheduler delivers failed events that could not be successfully delivered to a target". So each occurrence of a schedule whose Target.Arn is an SNS topic is published AS the assumed Target.RoleArn; when that role grants no sns:Publish on the topic, SNS refuses the Publish with the error code `AuthorizationError` (403) and Scheduler dead-letters the failed event. Recorded behavior only for the dead-letter message's shape (the event body has no modelled contract): the SQS message carries the failed occurrence's payload in its Body and message attributes describing the failure, whose ERROR_CODE is the TARGET SERVICE's own error code for the refusal — here the fixed string `AuthorizationError`, propagated verbatim from SNS, not a generic IAM-shaped code; SCHEDULE_ARN is the failing schedule's own ARN (a value from state); ERROR_MESSAGE names the assumed-role session (a per-invocation value).
Emulator: the positive arm is correct (a Notification envelope with the right Type/TopicArn/MessageId, the substituted <aws.scheduler.scheduled-time>, a Timestamp inside the allowed window), and the denied arm now does produce a dead-letter event on Target.DeadLetterConfig.Arn — but that event's MessageAttributes.ERROR_CODE.StringValue is `AccessDeniedException` instead of `AuthorizationError`. The replay fails with: AssertionError: the denied schedule's dead-letter event reports ERROR_CODE='AccessDeniedException'; expected AuthorizationError, the publish refused for lack of sns:Publish. The emulator is evidently minting a generic cross-service authorization code for the dispatch refusal rather than carrying through the code the refusing service (SNS) actually returns.
Why: ERROR_CODE is the only machine-readable classification a client has for a failed occurrence — there is no caller at occurrence time, CreateSchedule already returned 200, and the control plane keeps reporting State=ENABLED. Consumers of the dead-letter queue branch on ERROR_CODE to tell a permissions refusal of the target service (AuthorizationError from SNS, retry after fixing the role policy) from a throttle, a KMS failure or a not-found; code written against AWS matches on `AuthorizationError` and will fall through to its default/unknown branch here, so refused publishes are misclassified (or silently ignored) by exactly the alarming and triage logic the DLQ exists to feed. It also hides WHICH service refused: `AccessDeniedException` is not a code SNS Publish can ever return.
Fix: the scheduler crate's occurrence/dispatch component (services/service-com-amazonaws-scheduler-awschronosservice/src/, the shared target-invocation path that authorizes Target.RoleArn and writes the dead-letter event) must populate the dead-letter event's ERROR_CODE from the refusing target service's error code — for a denied SNS Publish, SNS's AuthorizationErrorException code `AuthorizationError` (403) — instead of substituting a generic AccessDeniedException; ERROR_MESSAGE should likewise be the refusing service's message naming the assumed-role session, and SCHEDULE_ARN the failing schedule's own ARN.
Traces: scheduler__sns.scheduler-sns-target-delivers-the-notification-envelope-and-a-denied-role-withholds-it (assertion at run line 154, after _await_dlq_event returned an event).

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator, service heading `### scheduler__sns` (0/1 passed, 1 failed). Generated names/ids/timestamps in one run are not requirements.

[mismatch] scheduler__sns.scheduler-sns-target-delivers-the-notification-envelope-and-a-denied-role-withholds-it
  File "run.py", line 445, in <lambda> -> body = calls.in_phase(calls.BODY, lambda: fn(ctx))
  File "check_scheduler__sns_scheduler_sns_target_delivers_the_notification_envelope_and_a_denied_role_withholds_it.py", line 154, in run
    refusal = dlq_event["MessageAttributes"]["ERROR_CODE"]["StringValue"]
    assert refusal == "AuthorizationError"
  AssertionError: the denied schedule's dead-letter event reports ERROR_CODE='AccessDeniedException'; expected AuthorizationError, the publish refused for lack of sns:Publish

Expected (AWS): the dead-letter message for the denied occurrence carries MessageAttributes.ERROR_CODE.StringValue = 'AuthorizationError' — the whole string is fixed text, the code SNS returns for a Publish it refuses (Smithy: Publish error AuthorizationErrorException, awsQueryError code 'AuthorizationError', httpResponseCode 403) — alongside SCHEDULE_ARN = the denied schedule's own ARN (a value from state) and ERROR_MESSAGE naming the assumed-role session (a per-invocation value the check deliberately does not assert on).
Actual (emulator): ERROR_CODE = 'AccessDeniedException' — a code SNS Publish does not define.

Positive control passed first, so this is not a dead dispatcher: the sibling schedule with the identical cadence (rate(1 minute), FlexibleTimeWindow Mode=OFF, State=ENABLED) and a role carrying sns:Publish delivered an envelope the check accepted (Type == 'Notification', TopicArn == the created topic, non-empty MessageId, Message marker intact, no unsubstituted '<aws.scheduler' token, Timestamp not >5 s early nor >300 s late). The earlier dead-letter-existence assertion in _await_dlq_event also passed this time, so the event IS being written; only its ERROR_CODE diverges.

Blocked downstream by this assertion (untested here, they hold on AWS): SCHEDULE_ARN equals the denied schedule's ARN and is never crossed with the permitted arm's event; the denied marker never reaches the subscriber during a further 60 s absence window while the permitted one keeps arriving; and after iam:PutRolePolicy grants sns:Publish on the topic and nothing else changes, the previously withheld marker starts arriving within 240 s.

## Steps to Reproduce
All calls via boto3 against the emulator, as the check does.

1. sns.create_topic(Name=<topic>) -> topic_arn.
2. sqs.create_queue twice: an `inbox` subscriber queue and a `dlq`; get_queue_attributes(AttributeNames=['QueueArn']) for each.
3. sqs.set_queue_attributes on `inbox` with a Policy allowing Principal {"Service": "sns.amazonaws.com"} sqs:SendMessage on the queue ARN with Condition ArnEquals aws:SourceArn = topic_arn, then sns.subscribe(TopicArn=topic_arn, Protocol='sqs', Endpoint=<inbox arn>).
4. iam.create_role twice, both trusting {"Service": "scheduler.amazonaws.com"} for sts:AssumeRole:
   - ALLOWED role: inline policy Allow sns:Publish on topic_arn + Allow sqs:SendMessage on <dlq arn>.
   - DENIED role: inline policy Allow sqs:SendMessage on <dlq arn> ONLY (no sns:Publish anywhere).
5. scheduler.create_schedule twice (retrying ValidationException while IAM propagates), both with ScheduleExpression='rate(1 minute)', ScheduleExpressionTimezone='UTC', FlexibleTimeWindow={'Mode':'OFF'}, State='ENABLED', Target={'Arn': topic_arn, 'RoleArn': <that arm's role>, 'Input': json.dumps({'marker': <per-arm marker>, 'at': '<aws.scheduler.scheduled-time>'}), 'DeadLetterConfig': {'Arn': <dlq arn>}}.
6. Poll sqs.receive_message(QueueUrl=<inbox url>, MaxNumberOfMessages=10, WaitTimeSeconds=5) until the PERMITTED marker appears (<=240 s) — positive control, passes.
7. Poll sqs.receive_message(QueueUrl=<dlq url>, MaxNumberOfMessages=10, WaitTimeSeconds=20, MessageAttributeNames=['All']) until the DENIED arm's dead-letter event arrives (<=240 s). It arrives.
8. Read event['MessageAttributes']['ERROR_CODE']['StringValue']. AWS: 'AuthorizationError' (the code SNS returns for the refused Publish). Emulator: 'AccessDeniedException' — this is the divergence.
9. (Blocked by step 8, holds on AWS:) assert MessageAttributes['SCHEDULE_ARN']['StringValue'] == the denied schedule's ARN; watch `inbox` 60 s more and see the denied marker absent while the permitted one keeps arriving; then iam.put_role_policy adding Allow sns:Publish on topic_arn to the denied role and see the denied marker arrive within 240 s.
