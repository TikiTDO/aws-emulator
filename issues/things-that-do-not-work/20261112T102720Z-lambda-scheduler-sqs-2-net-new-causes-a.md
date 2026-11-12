Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-deleted-lambda-target-dead-letters-once lambda__scheduler__sqs.xsvc-lambda-3-scheduler-role-without-invoke-permission-denied-to-dlq lambda__scheduler__sqs.xsvc-lambda-3-scheduler-lambda-target-receives-exact-input-dlq-empty lambda__scheduler__sqs.xsvc-lambda-3-scheduler-throwing-lambda-fails-to-onfailure-not-scheduler-dlq
Issue-Key: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-deleted-lambda-target-dead-letters-once|run#38|AssertionError|assert body.get("FunctionName") == doomed["arn"], (
Issue-Key: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-role-without-invoke-permission-denied-to-dlq|run#31|AssertionError|assert body.get("Payload") == denied_input, (
Issue-Key: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-lambda-target-receives-exact-input-dlq-empty|run#25|AssertionError|assert json.loads(twin_dead["Body"]).get("Payload") == twin_input, (
Issue-Key: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-throwing-lambda-fails-to-onfailure-not-scheduler-dlq|run#27|AssertionError|assert b_attrs.get("ERROR_MESSAGE") == f"Function not found: {barrier_fn['arn']}", (
Issue-Commit: 80a4d1672cdd144a0f484dbb402fdb4ec503e852

# lambda__scheduler__sqs: 2 net-new causes — a Scheduler dead-letter message carries the bare Target.Input instead of the Lambda invoke envelope, and Lambda's "Function not found" message appends :$LATEST

> _Note: this issue was identified against the code base at commit `80a4d1672cdd144a0f484dbb402fdb4ec503e852`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
Two net-new root causes on the EventBridge Scheduler -> Lambda -> SQS dead-letter path. The dead-letter event itself now ARRIVES and its message attributes (ERROR_CODE, ERROR_MESSAGE, RETRY_ATTEMPTS, TARGET_ARN, SCHEDULE_ARN) are already correct — every assertion ahead of the ones below passes — so this is not the same defect as the open issues 20261109T141116Z-lambda-scheduler-sqs-scheduler-never-dead-letters-a-failed-lambda-target.md, 20261109T185037Z-lambda-scheduler-sqs-a-schedule-whose-lambda-target-throws-produces-no.md or 20261111T011632Z-scheduler-a-dead-letter-event-s-retry-attempts.md, which are about the event never being produced or its attributes being absent. Neither cause below is covered by any open issue.

Cause 1 of 2: the dead-letter message Body is the schedule's raw Target.Input document, not the Lambda invocation request that failed.
  AWS: when a templated Lambda target invocation fails permanently, EventBridge Scheduler writes one message to Target.DeadLetterConfig.Arn whose Body is the TARGET INVOCATION Scheduler attempted — a JSON object naming the call, with members FunctionName (the target function's unqualified function ARN, taken from Target.Arn), InvocationType ("Event", because Scheduler invokes a Lambda target asynchronously) and Payload (the schedule's Target.Input, verbatim, as a string). The schedule's Input is a MEMBER of that envelope, it is not the envelope. recorded behavior only: the Smithy model for scheduler describes Target.DeadLetterConfig.Arn and Target.Input but does not model the dead-letter body; the shape above is what the recording shows and what AWS documents at scheduler/latest/UserGuide/configuring-schedule-dlq.html.
  Emulator: the Body is exactly the Target.Input string and nothing else — for Input '{"marker": "<marker>"}' the delivered Body is '{"marker": "<marker>"}', so json.loads(Body).get("FunctionName"), .get("InvocationType") and .get("Payload") are all None. The envelope is never constructed; the Input is forwarded in its place.
  Why: a client reading its Scheduler DLQ to find out WHICH target failed and to replay it cannot. It has no FunctionName to re-target, no InvocationType to know the call was asynchronous, and it cannot tell the failed invocation's payload apart from the envelope, because the body it receives is shaped like the payload. Replay tooling that does json.loads(Body)["Payload"] raises KeyError on every message.
  Fix: the scheduler delivery-failure path that composes the dead-letter message for a templated Lambda target — the dead-letter message builder/serializer, which must wrap the attempted invocation (FunctionName/InvocationType/Payload) rather than pass Target.Input through as the body. The message attributes it already sets are correct and must not change.
  Traces: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-deleted-lambda-target-dead-letters-once (body FunctionName is None, expected the target function ARN); lambda__scheduler__sqs.xsvc-lambda-3-scheduler-role-without-invoke-permission-denied-to-dlq (body Payload is None, expected the schedule's Input); lambda__scheduler__sqs.xsvc-lambda-3-scheduler-lambda-target-receives-exact-input-dlq-empty (body has no Payload at all; the delivered Body is the Input itself).

Cause 2 of 2: Lambda's ResourceNotFoundException message for an invoke of a deleted function appends the :$LATEST qualifier.
  AWS: an Invoke (or GetFunction) naming a function that does not exist answers HTTP 404 with error code ResourceNotFoundException (Smithy: com.amazonaws.lambda#ResourceNotFoundException, errorKind client, httpError 404, members Type and Message) and the message 'Function not found: <function ARN>'. The fixed text is 'Function not found: '; the only variable part is the function ARN, and it is the UNQUALIFIED function ARN built from the request/state (arn:aws:lambda:<region>:<account>:function:<name>) — no version or alias qualifier is appended when the caller supplied none. The same check's GetFunction assertion against that exact unqualified wording passes, so only the invoke path diverges.
  Emulator: the invoke path produces 'Function not found: arn:aws:lambda:<region>:<account>:function:<name>:$LATEST' — the right code and status, but the message names a qualified ARN the caller never asked for. It surfaces verbatim in the Scheduler dead-letter event's ERROR_MESSAGE attribute, which is where the check observes it.
  Why: a client that matches on the error message, or that parses the ARN out of it to decide which function to recreate or re-target, gets an ARN with a qualifier appended and either fails to match or resolves the wrong resource identity. The same divergence reaches anything that logs or alarms on the dead-letter ERROR_MESSAGE.
  Fix: the Lambda Invoke handler's not-found refusal — the place that formats the ResourceNotFoundException message must render the unqualified function ARN when the request carried no qualifier, matching the wording GetFunction already uses.
  Traces: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-throwing-lambda-fails-to-onfailure-not-scheduler-dlq.

## Evidence
Cause 1 of 2 — check lambda__scheduler__sqs.xsvc-lambda-3-scheduler-deleted-lambda-target-dead-letters-once:
  AssertionError: dead-letter body FunctionName is None, expected 'arn:aws:lambda:<region>:<account>:function:<fn>'
  (assertion: json.loads(dead["Body"]).get("FunctionName") == target function ARN; the two assertions that follow it require InvocationType == "Event" and Payload == the schedule's Target.Input)

Cause 1 of 2 — check lambda__scheduler__sqs.xsvc-lambda-3-scheduler-role-without-invoke-permission-denied-to-dlq:
  AssertionError: dead-letter body Payload None is not the Input '{"marker": "<marker>"}'

Cause 1 of 2 — check lambda__scheduler__sqs.xsvc-lambda-3-scheduler-lambda-target-receives-exact-input-dlq-empty:
  AssertionError: the twin's dead-letter body does not carry its Input '{"marker": "<marker>"}': '{"marker": "<marker>"}'
  Expected Body: a JSON object {"FunctionName": "<target function ARN>", "InvocationType": "Event", "Payload": "<Target.Input>"}.
  Actual Body: the Target.Input document itself — which is why the two reprs in the message are identical and .get("Payload") is None.
  Note: the preceding assertions on ERROR_CODE, ERROR_MESSAGE, RETRY_ATTEMPTS, TARGET_ARN and SCHEDULE_ARN all PASS, so the divergence is confined to the message body.

Cause 2 of 2 — check lambda__scheduler__sqs.xsvc-lambda-3-scheduler-throwing-lambda-fails-to-onfailure-not-scheduler-dlq:
  AssertionError: barrier ERROR_MESSAGE is 'Function not found: arn:aws:lambda:<region>:<account>:function:<fn>:$LATEST'
  Expected (AWS wording): 'Function not found: arn:aws:lambda:<region>:<account>:function:<fn>'  — status 404, code ResourceNotFoundException.
  Actual: same status and code, message with ':$LATEST' appended to the ARN.
  Fixed text: 'Function not found: '. Variable part: the unqualified function ARN taken from the request/state. ':$LATEST' is not part of the template.

## Steps to Reproduce
Cause 1 of 2:
1. Create an SQS queue to act as a dead-letter queue, and a role trusted by scheduler.amazonaws.com that may lambda:InvokeFunction the target and sqs:SendMessage to that queue.
2. Create a Lambda function, then CreateSchedule with FlexibleTimeWindow Mode=OFF, a one-time at() expression, and Target = {Arn: <function ARN>, RoleArn: <scheduler role>, Input: '{"marker":"x"}', RetryPolicy: {MaximumRetryAttempts: 1, MaximumEventAgeInSeconds: 60}, DeadLetterConfig: {Arn: <dlq ARN>}}.
3. Make the invocation fail permanently before the nominal instant — either DeleteFunction on the target, or create the schedule with a RoleArn that lacks lambda:InvokeFunction on it.
4. After the schedule fires, ReceiveMessage from the dead-letter queue and json.loads the Body.
5. Observe the Body is the Target.Input document; FunctionName, InvocationType and Payload are absent. AWS returns an envelope carrying all three, with Payload holding the Input.

Cause 2 of 2:
1. Create a Lambda function, note its unqualified ARN, then DeleteFunction it.
2. Invoke it by that ARN (directly, or via a Scheduler schedule whose Target.Arn names it, then read the dead-letter event's ERROR_MESSAGE message attribute).
3. Observe the message 'Function not found: <ARN>:$LATEST'. AWS answers 'Function not found: <ARN>' with no qualifier, as the same emulator's GetFunction already does.
