Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler__sqs.scheduler-unauthorized-target-role-routes-invocation-to-the-dead-letter-queue
Issue-Key: scheduler__sqs.scheduler-unauthorized-target-role-routes-invocation-to-the-dead-letter-queue|run#22|KeyError|assert payload["MessageBody"] == denied_input, (
Issue-Commit: 09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77

# scheduler__sqs: a Scheduler dead-letter event for an SQS target carries the raw Target.Input as its Body instead of the attempted SendMessage request ({QueueUrl, MessageBody})

> _Note: this issue was identified against the code base at commit `09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Cause 1 of 1: the dead-letter message Body for a denied SQS-target occurrence is the schedule's Target.Input verbatim instead of the SQS request Scheduler attempted.
AWS: recorded behavior only — the Smithy model (scheduler) documents DeadLetterConfig as "An object that contains information about an Amazon SQS queue that EventBridge Scheduler uses as a dead-letter queue for your schedule. If specified, EventBridge Scheduler delivers failed events that could not be successfully delivered to a target to the queue" and models nothing about the event's shape. The recording shows the dead-letter SQS message Body is the JSON document of the target API call Scheduler tried to make; for an SQS (templated) target the keys are QueueUrl = the target queue's URL and MessageBody = the schedule's Target.Input as a string. The failure metadata rides in the message ATTRIBUTES — ERROR_CODE (here the fixed value 'AccessDenied'), ERROR_MESSAGE, TARGET_ARN = the target queue ARN, SCHEDULE_ARN = the failing schedule's own ARN. The ERROR_MESSAGE is AWS's IAM denial wording: fixed text "User: ", the assumed-role session ARN arn:aws:sts::<account>:assumed-role/<role name>/<session token>, then " is not authorized to perform: sqs:sendmessage on resource: ", the target queue ARN, then " because no identity-based policy allows the sqs:sendmessage action" — the account, role name, session token and queue ARN are values from the request and state, the rest is fixed text.
Emulator: the Body is the Target.Input object itself, so neither MessageBody nor QueueUrl exists; reading Body["MessageBody"] raises KeyError: 'MessageBody'.
Why: a DLQ consumer cannot tell which queue the occurrence was destined for, and cannot recover the original payload from the documented Body.MessageBody key or replay the failed send; every failure-triage and redrive path built on a Scheduler DLQ for an SQS target breaks.
Fix: Scheduler's dead-letter composition in the schedule occurrence/delivery path — the component that writes the DeadLetterConfig message must serialize the attempted target request (QueueUrl/MessageBody for an SQS target) as the message Body rather than forwarding Target.Input.
Note: this is NOT the already-open 20261109T141116Z-scheduler-sqs-scheduler-never-routes-an-invocation-denied-by-its.md ("never routes an invocation denied by its Target.RoleArn to the configured dead-letter queue"): the event now DOES arrive on the DLQ, and the replay gets past that wait before failing on the body's shape. The report lists this check as unowned.
Traces: scheduler__sqs.scheduler-unauthorized-target-role-routes-invocation-to-the-dead-letter-queue.

## Evidence
Check scheduler__sqs.scheduler-unauthorized-target-role-routes-invocation-to-the-dead-letter-queue [error]:
  expected json.loads(Body) == {"QueueUrl": "<target queue URL>", "MessageBody": "<the schedule's Target.Input>"}
  actual   KeyError: 'MessageBody' at `payload["MessageBody"] == denied_input` — the Body parses to the Target.Input object itself, so it has neither MessageBody nor QueueUrl.
  The dead-letter event itself was delivered to the configured DLQ (the preceding wait succeeded), so only the body's shape diverges.

## Steps to Reproduce
1. Create four SQS queues: a target, its DLQ, a control target and a control DLQ.
2. Create a role trusted by scheduler.amazonaws.com granting sqs:SendMessage on the DLQ, the control target and the control DLQ — but NOT on the target queue.
3. CreateSchedule with FlexibleTimeWindow Mode=OFF, an at() instant ~30 s out, and Target={Arn: <target queue ARN>, RoleArn: <role>, Input: '{"marker":"x"}', RetryPolicy:{MaximumRetryAttempts:0,MaximumEventAgeInSeconds:900}, DeadLetterConfig:{Arn: <dlq ARN>}}.
4. After the occurrence, ReceiveMessage on the DLQ with MessageAttributeNames=['All'].
5. json.loads(Body) has no "MessageBody" and no "QueueUrl" key — it is the Target.Input object. AWS returns {"QueueUrl":"<target queue URL>","MessageBody":"<Input>"}.
