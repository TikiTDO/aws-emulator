Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__s3__sqs.deploy-cdk-bucket-sqs-notification-prefix-filter-and-auto-delete-objects-on-destroy
Issue-Key: cloudformation__lambda__s3__sqs.deploy-cdk-bucket-sqs-notification-prefix-filter-and-auto-delete-objects-on-destroy|_cdk#17|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# The in-Lambda AWS SDK binds no S3 PutBucketNotificationConfiguration, so CDK's BucketNotifications custom resource always fails the deploy

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Cause 1 of 1: the AWS SDK the emulator's Lambda runtime supplies exposes only a six-call subset of Amazon S3, so the CDK-generated Custom::S3BucketNotifications provider cannot configure bucket notifications and the stack fails and rolls back.

AWS: a Lambda function's bundled SDK binds the full S3 API, including PutBucketNotificationConfiguration (and GetBucketNotificationConfiguration), which is exactly what CDK's `bucket.addEventNotification(...)` provider calls to attach the prefix-filtered SQS notification. Recorded behavior: the same CDK app deploys to CREATE_COMPLETE on real AWS and the prefix-filtered notification reaches the queue.
Emulator: the function raises 'the AWS SDK this runtime supplies binds no PutBucketNotificationConfiguration call on Amazon S3; the operations it binds are GetObject, HeadObject, PutObject, CopyObject, DeleteObject, ListObjectsV2', the custom resource reports CREATE_FAILED, and `cdk deploy` exits 1 (it then also fails to delete BucketNotifications8F2E257D on rollback).
Why: the single most common CDK pattern (bucket -> notification -> queue/lambda) cannot be deployed at all, and any in-function code that administers S3 fails with an SDK-shape error rather than an AWS error — a client cannot distinguish 'my IAM is wrong' from 'the SDK has no such call'.
Fix: the in-Lambda SDK surface (the bundled client's S3 operation bindings) must bind the bucket-notification operations the emulator's S3 service already serves.
Traces: cloudformation__lambda__s3__sqs.deploy-cdk-bucket-sqs-notification-prefix-filter-and-auto-delete-objects-on-destroy.

## Evidence
[mismatch] cloudformation__lambda__s3__sqs.deploy-cdk-bucket-sqs-notification-prefix-filter-and-auto-delete-objects-on-destroy
AssertionError: `cdk deploy` rc=1, expected 0: '...CREATE_FAILED Custom::S3BucketNotifications Bucket/Notifications (BucketNotifications8F2E257D) Error: the AWS SDK this runtime supplies binds no PutBucketNotificationConfiguration call on Amazon S3; the operations it binds are GetObject, HeadObject, PutObject, CopyObject, DeleteObject, ListObjectsV2 ... resource(s) failed to delete: [BucketNotifications8F2E257D]'

## Steps to Reproduce
1. `cdk deploy` an app whose bucket adds an SQS event notification with a prefix filter and sets autoDeleteObjects.
2. The CDK-generated Custom::S3BucketNotifications Lambda calls PutBucketNotificationConfiguration through the runtime's SDK.
3. Expect CREATE_COMPLETE; the emulator fails the custom resource with 'binds no PutBucketNotificationConfiguration call on Amazon S3' and rolls back.

## Resolution
The in-Lambda SDK seam now binds the bucket-administration half of Amazon S3 — the
notification and tagging subresources, the version listing and the multi-object delete — so
the provider's call is answered by the emulated S3 service
(`services/service-com-amazonaws-lambda-awsgirapiservice/src/state/execution/aws_s3/bucket.rs`,
with `tests/all/handler_bucket_administration_sdk.rs` and
`core/router/tests/all/cloudformation_cdk_bucket_notifications_custom_resource.rs`, where the
CDK-shaped stack reaches CREATE_COMPLETE and the prefix-filtered notification reaches the
queue). The rollback line of the evidence above has a cause of its own, filed separately as
`20261110T214500Z-cloudformation-a-custom-resources-delete-is-sent-and-its-provider-torn-down.md`:
a teardown sends a custom resource's `Delete` to its provider and then deletes the provider
before it can answer.
