Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sns.s3-sns-missing-lambda-invoke-permission-blocks-delivery
Issue-Key: lambda__s3__sns.s3-sns-missing-lambda-invoke-permission-blocks-delivery|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# lambda__s3__sns: an S3->SNS->Lambda chain never delivers, so the missing-invoke-permission block is unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Cause 1 of 1: a bucket notification that publishes to SNS, fanned out to a Lambda subscription, never produces an invocation, so the negative control (delivery blocked when lambda:InvokeFunction is not granted to sns.amazonaws.com) can never be distinguished from the pipeline simply not working; the check is killed at the harness 60 s cap.

AWS: a PutObject matching a TopicConfiguration publishes an s3 event to the topic; SNS then invokes each lambda subscription, but only when the function's resource policy allows sns.amazonaws.com for that topic — without the grant SNS records a delivery failure and the function is never invoked. Recorded behavior only for the delivery delay.

Emulator: nothing is delivered on either arm — the check polls for the function's effect and reports 'Error: Check timed out after 60.0s'. With no positive arm, the absence of an invocation proves nothing about permission enforcement.

Why: event-driven fan-out through SNS silently does nothing, and a client cannot tell a missing permission (which it must fix) from a dead pipeline.

Fix: the S3 notification dispatcher's SNS target publish in the s3 service, SNS delivery to lambda subscriptions, and the invoke-permission check on that delivery path. Related open issue 20261025T002357Z-events-lambda-sns-eventbridge-sns-targets-never-publish-and-an-sns.md covers the EventBridge-fed variant and does not list this check.

Traces: lambda__s3__sns.s3-sns-missing-lambda-invoke-permission-blocks-delivery.

## Evidence
Expected: with the grant, a PutObject invokes the subscribed function once; without it, SNS does not invoke the function and the permission is the only difference.
Actual: Error: Check timed out after 60.0s — neither arm delivered.

## Steps to Reproduce
1. Create a topic, a function, and subscribe the function to the topic.
2. PutBucketNotificationConfiguration with a TopicConfiguration for s3:ObjectCreated:*.
3. PutObject a matching key with and without lambda:InvokeFunction granted to sns.amazonaws.com.
4. Poll for the invocation; observe neither arm delivers.
