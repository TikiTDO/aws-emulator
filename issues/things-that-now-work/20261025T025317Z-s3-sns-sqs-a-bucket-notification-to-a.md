Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sns__sqs.s3-notification-without-topic-policy-rejected s3__sns__sqs.s3-object-created-notifies-sns
Issue-Key: s3__sns__sqs.s3-notification-without-topic-policy-rejected|run#11|AssertionError|raise AssertionError(
Issue-Key: s3__sns__sqs.s3-object-created-notifies-sns|_wait_for_object_created#8|Timeout|resp = sqs.receive_message(
Issue-Commit: 5f222c49021a8706c85b6c9c85441ae0b0065824

# s3__sns__sqs: a bucket notification to a topic S3 cannot publish to is accepted, and ObjectCreated events never reach SNS (2 causes)

> _Note: this issue was identified against the code base at commit `5f222c49021a8706c85b6c9c85441ae0b0065824`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Cause 1 of 2: PutBucketNotificationConfiguration accepts a TopicConfiguration whose topic policy does not permit S3 to publish.
AWS: S3 validates the destination at configuration time by test-publishing, and refuses the call (400 InvalidArgument, "Unable to validate the following destination configurations") when the topic policy does not allow s3.amazonaws.com with the bucket as aws:SourceArn; the bucket's notification configuration is left unchanged.
Emulator: the call is accepted, and the bucket is left configured with a destination that can never receive events.
Why: this is an over-permissive emulator — the single pre-flight check that catches the most common S3-notification misconfiguration never fires, so a client learns nothing until events silently disappear in production.
Fix: the destination-validation step of the PutBucketNotificationConfiguration handler.
Traces: s3__sns__sqs.s3-notification-without-topic-policy-rejected.

Cause 2 of 2: an ObjectCreated event is never published to the configured topic.
AWS: PutObject on a bucket with a TopicConfiguration for s3:ObjectCreated:* publishes an s3 event notification to the topic within seconds, which fans out to the topic's SQS subscribers.
Emulator: nothing arrives on the subscribed queue and the check hangs to the 60 s cap. Related to the missing SNS delivery path reported for the sns service in this run, but the S3 side's event emission is equally unproven here.
Why: every S3-event-driven pipeline (ingest, thumbnailing, indexing) is inert against the emulator.
Fix: the S3 event-notification emitter in the PutObject path, and the SNS delivery path it publishes through.
Traces: s3__sns__sqs.s3-object-created-notifies-sns.

## Evidence
- s3__sns__sqs.s3-notification-without-topic-policy-rejected: AssertionError: S3 accepted a notification to a topic S3 cannot publish to (over-permissive)
- s3__sns__sqs.s3-object-created-notifies-sns: Error: Check timed out after 60.0s

## Steps to Reproduce
1. CreateBucket; CreateTopic and leave its default policy (no s3.amazonaws.com grant); PutBucketNotificationConfiguration(TopicConfigurations=[{TopicArn, Events:["s3:ObjectCreated:*"]}]) -> accepted (AWS: 400 InvalidArgument).
2. Now grant the topic policy, subscribe an SQS queue, PutObject, and poll the queue -> no s3 event notification arrives within 60 s.
