Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eventbridgev2__iam__sns__sqs.sns-topic-target-publishes-one-event-per-message eventbridgev2__iam__sns__sqs.role-without-sns-publish-dead-letters-and-topic-receives-nothing
Issue-Key: eventbridgev2__iam__sns__sqs|CreateSubscriber|NotImplemented
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# eventbridgev2__iam__sns__sqs: CreateSubscriber answers NotImplemented, so no SNS-topic target delivery or role-denial dead-lettering can be exercised

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSubscriber

## Description
Cause 1 of 1 — the EventBridgeV2 CreateSubscriber handler is an unimplemented scaffold stub, so every subscriber whose invoke target is an SNS topic is unreachable and the whole SNS-target surface of the v2 event bus is ungraded (failing checks: eventbridgev2__iam__sns__sqs.sns-topic-target-publishes-one-event-per-message, eventbridgev2__iam__sns__sqs.role-without-sns-publish-dead-letters-and-topic-receives-nothing).

AWS: Smithy: com.amazonaws.eventbridgev2#CreateSubscriber takes Name (@required), EventBusArn (@required, event-busv2 ARN pattern), InvokeConfiguration (@required, carrying the RoleArn and the TargetArn), plus FilterConfiguration, Transformer, RetryPolicy, OnFailureConfiguration, BatchConfiguration, StartingPosition/PointInTimeConfiguration and ClientToken (@idempotencyToken); it answers 200 with SubscriberArn (pattern ^arn:aws...:subscriber/<name>/<25 lowercase alnum>$), Name, EventBusArn, Type, StartingPosition, State and CreationTime. Its modeled errors are ConcurrentModificationException (409), IdempotentParameterMismatchException (409), InvalidStateException (409), LimitExceededException (400), ResourceAlreadyExistsException (409), ResourceNotFoundException (404), AccessDeniedException (403), InvalidInputException (400), ThrottlingException (429) and InternalException (500). `NotImplemented` is NOT among them. Recorded behavior: once the subscriber exists, PutEvents onto the bus publishes exactly one SNS message per matched event through the subscriber's role, and a role that lacks sns:Publish on the target topic causes the delivery to fail and a dead-letter record naming the access denial to be written to the OnFailureConfiguration queue while the topic receives nothing.

Emulator: CreateSubscriber answers the non-AWS error code `NotImplemented` with message `operation CreateSubscriber is scaffolded but not implemented in this emulator` (handler services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/create_subscriber.rs). No subscriber is ever created, so neither check reaches a verdict.

Why: a client cannot build any EventBridgeV2 -> SNS pipeline against the emulator: one-message-per-event fan-out to a topic, and the IAM-denial path that is supposed to dead-letter instead of delivering, are both unobservable. The returned code is also outside the operation's modeled error set, so SDK error typing and retry classification cannot match it, and a client that catches the modeled exceptions sees an unknown service error instead.

Fix: implement the CreateSubscriber handler and the subscriber state behind it — target resolution for an SNS topic ARN, role-based authorization of the delivery, and the OnFailureConfiguration dead-letter write — in the eventbridgev2 create_subscriber operation handler.

Related open issues that do NOT carry these checks' replay keys: 20261109T163411Z-eventbridgev2-iam-sqs-eventbridge-v2-createsubscriber-answers-notimplemented-so-no-subscriber.md (key eventbridgev2__iam__sqs|CreateSubscriber|NotImplemented — the SQS-target checks only) and 20261104T183027Z-eventbridgev2-createsubscriber-stub.md (the generic stub item). The two checks listed here are the SNS-target ones and were reported unowned by the suite, so they are filed here and must be re-replayed to confirm the fix.

Traces: eventbridgev2__iam__sns__sqs.sns-topic-target-publishes-one-event-per-message, eventbridgev2__iam__sns__sqs.role-without-sns-publish-dead-letters-and-topic-receives-nothing.

## Evidence
Replay of behavioral suite simparity-behavioral-aws-v1 (eventbridgev2__iam__sns__sqs: 0/2 passed, both errored).

[error] eventbridgev2__iam__sns__sqs.sns-topic-target-publishes-one-event-per-message
[error] eventbridgev2__iam__sns__sqs.role-without-sns-publish-dead-letters-and-topic-receives-nothing
Both fail identically on the same call:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateSubscriber operation: operation CreateSubscriber is scaffolded but not implemented in this emulator

Expected: HTTP 200 carrying SubscriberArn / Name / EventBusArn / State / CreationTime for a subscriber whose InvokeConfiguration names an SNS topic TargetArn and a RoleArn, after which PutEvents publishes one message per matched event to the topic (and, with a role lacking sns:Publish, writes an access-denied dead-letter record to the configured SQS queue and publishes nothing).
Actual: the modeled-error-set violation above; no subscriber exists, so nothing downstream is observable.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Create an SNS topic, an SQS dead-letter queue, an IAM role EventBridge can assume, and a v2 event bus (CreateEventBus, wait for ACTIVE).
3. Call CreateSubscriber(Name=..., EventBusArn=<bus arn>, ClientToken=..., InvokeConfiguration={RoleArn: <role>, TargetArn: <topic arn>, ...}) — optionally with OnFailureConfiguration naming the SQS queue.
4. Observe the NotImplemented error instead of a 200 with the subscriber.
5. Re-replay checks eventbridgev2__iam__sns__sqs.sns-topic-target-publishes-one-event-per-message and eventbridgev2__iam__sns__sqs.role-without-sns-publish-dead-letters-and-topic-receives-nothing from suite simparity-behavioral-aws-v1 to confirm a fix (one SNS message per published event; and with a role without sns:Publish, a dead-letter record naming the denial and an empty topic).
