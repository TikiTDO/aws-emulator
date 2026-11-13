Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission
Issue-Key: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission|_collect_messages#18|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=remaining)
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# events__lambda__sns: a CDK-deployed SNS->Lambda subscription never invokes the function even with the permission granted, so the missing-permission negative is ungradeable

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
One net-new root cause.

Cause 1 of 1: an SNS topic subscription whose endpoint is a Lambda function never invokes that function, so the positive control of the permission-boundary differential never fires and the negative (no lambda:InvokeFunction grant for sns.amazonaws.com) cannot be attributed to the missing permission.
Failing check: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission

AWS: a `lambda` protocol subscription on an SNS topic invokes the subscribed function once per published message, provided the function's resource policy allows `lambda:InvokeFunction` for principal sns.amazonaws.com with SourceArn = the topic ARN. A stack that declares the subscription but omits that permission statement never invokes the function (the publish still succeeds); the same stack WITH the permission does invoke it. Recorded behavior only for the delivery; Smithy (sns) models Subscribe/Publish and (lambda) AddPermission, so both halves of the wiring are modeled operations the emulator accepts.
Emulator: the check ends `Check timed out after 60.0s`. The permitted (positive-control) stack's function is never invoked either, so the negative half is indistinguishable from "SNS never invokes a Lambda subscriber at all".
Why: a client fanning notifications to a function via SNS gets no invocations; a test asserting "no permission => no invocation" passes vacuously on the emulator and fails on AWS, hiding a genuine permission regression.
Fix: the SNS message-delivery path for `lambda`-protocol subscriptions — on Publish, resolve each lambda subscription, authorize against the function's resource policy (principal sns.amazonaws.com, SourceArn = topic ARN) and invoke the function with the SNS record envelope; a missing grant must suppress only that subscriber. Open issues 20261025T002357Z-events-lambda-sns-eventbridge-sns-targets-never-publish-and-an-sns.md and 20261029T182228Z-events-lambda-sns-eventbridge-publishes-to-sns-topics-with-no-topic.md own neighbouring checks in this group but not this replay key.
Traces: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission.

## Evidence
Expected: the stack that grants lambda:InvokeFunction to sns.amazonaws.com for the topic invokes the function on Publish (positive control), while the twin stack omitting the permission never does.
Actual: `Check timed out after 60.0s` — neither variant's function is invoked, so the positive control never lands.

## Steps to Reproduce
1. Create an SNS topic and a Lambda function whose handler reports to an SQS signal queue.
2. Subscribe the function to the topic with protocol `lambda`, WITHOUT adding a lambda resource-policy statement for sns.amazonaws.com.
3. Publish a message and confirm the function is not invoked.
4. Repeat with the statement added (AddPermission: principal sns.amazonaws.com, SourceArn = the topic ARN) — on AWS the function is invoked and the signal queue gets a message; on the emulator it is never invoked and the check times out.
