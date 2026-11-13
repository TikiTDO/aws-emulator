Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eventbridgev2__iam__sns__sqs.sns-topic-target-publishes-one-event-per-message eventbridgev2__iam__sns__sqs.role-without-sns-publish-dead-letters-and-topic-receives-nothing
Issue-Key: eventbridgev2__iam__sns__sqs.sns-topic-target-publishes-one-event-per-message|_warm_up#7|TimeoutError|raise TimeoutError("the run budget ended during warm-up; not graded")
Issue-Key: eventbridgev2__iam__sns__sqs.role-without-sns-publish-dead-letters-and-topic-receives-nothing|run#14|TimeoutError|raise TimeoutError("the run budget ended before a dead-letter record; not graded")
Issue-Commit: 0275b87193e8729c4e6e84626c64cbadccfa7c1a

# eventbridgev2__iam__sns__sqs: an EventBridge v2 subscriber with an SNS-topic target never delivers, and no dead-letter record is written when its delivery role lacks sns:Publish

> _Note: this issue was identified against the code base at commit `0275b87193e8729c4e6e84626c64cbadccfa7c1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One root cause, grounded in both failing checks of this service.

=== Cause 1 of 1: the SNS-topic target of an EventBridge v2 subscriber is inert ===
AWS: recorded behaviour only. A subscriber created on an ACTIVE event bus with an SNS topic as its TargetArn publishes one SNS message per matched event, so an SQS queue subscribed to that topic receives one message per published event. When the subscriber's delivery role is NOT allowed sns:Publish on the topic, EventBridge instead writes a dead-letter record to the subscriber's OnFailureConfiguration target (errorCode ACCESS_DENIED naming the target ARN) and the topic receives nothing. Both checks establish liveness first with a warm-up loop that republishes a marker every 5 s until it is observed downstream; on AWS that loop completes.
Emulator: nothing is ever delivered through the SNS target and no dead-letter record is ever written. The warm-up loop never observes a message ("the run budget ended during warm-up; not graded") and the dead-letter poll never sees a record ("the run budget ended before a dead-letter record; not graded"). PutEvents itself answers FailedEntryCount 0, so the publisher sees success while nothing moves.
Why: an EventBridge-to-SNS fan-out is silently dead — the publisher is told the event was accepted, no subscriber of the topic ever receives it, and the misconfiguration case produces no dead-letter record either, so there is nothing for an operator to diagnose. Any client that fans an event bus out through SNS gets no data and no error.
Fix: the eventbridgev2 subscriber delivery dispatcher — the SNS-topic target type, and its retry / OnFailureConfiguration dead-letter path.
Traces: eventbridgev2__iam__sns__sqs.sns-topic-target-publishes-one-event-per-message, eventbridgev2__iam__sns__sqs.role-without-sns-publish-dead-letters-and-topic-receives-nothing

Note: the open issue 20261110T210822Z-eventbridgev2-iam-sns-sqs-createsubscriber-answers-notimplemented-so.md describes this service's SNS-target gap as caused by CreateSubscriber answering NotImplemented. That is no longer the observed failure — CreateSubscriber now succeeds and the checks proceed to the delivery poll — so this is the next, distinct cause and neither check is bound to that issue by the suite.

## Evidence
sns-topic-target-publishes-one-event-per-message:
  File "check_eventbridgev2__iam__sns__sqs_sns_topic_target_publishes_one_event_per_message.py", line 265, in run
    _warm_up(ctx, _put_events_warm(ebv2, bus_arn), sqs, [fanout_url])
  TimeoutError: the run budget ended during warm-up; not graded
  Expected: each PutEvents entry yields exactly one message on the SQS queue subscribed to the subscriber's SNS topic.

role-without-sns-publish-dead-letters-and-topic-receives-nothing:
  File "check_eventbridgev2__iam__sns__sqs_role_without_sns_publish_dead_letters_and_topic_receives_nothing.py", line 301, in run
    raise TimeoutError("the run budget ended before a dead-letter record; not graded")
  TimeoutError: the run budget ended before a dead-letter record; not graded
  Expected: one dead-letter record on the OnFailureConfiguration queue with errorCode ACCESS_DENIED naming the target ARN, and an empty topic-subscribed queue.

In both cases PutEvents answered FailedEntryCount 0 — the publish is accepted and nothing is delivered.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. CreateEventBus and wait for State ACTIVE.
3. Create an SNS topic, an SQS queue, and subscribe the queue to the topic.
4. Create an IAM role EventBridge can assume, allowed sns:Publish on the topic.
5. CreateSubscriber on the bus with InvokeConfiguration {RoleArn: <role>, TargetArn: <topic arn>}.
6. PutEvents one entry on the bus and poll the SQS queue — no message arrives.
7. Repeat with a delivery role that is NOT allowed sns:Publish and an OnFailureConfiguration pointing at a second queue; poll that queue — no dead-letter record arrives.
