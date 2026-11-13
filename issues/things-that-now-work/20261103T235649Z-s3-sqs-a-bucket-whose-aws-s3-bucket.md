Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sqs.deploy-s3-3-notification-queue-without-policy-rollback-rejected
Issue-Key: s3__sqs.deploy-s3-3-notification-queue-without-policy-rollback-rejected|run#25|AssertionError|assert code == "404", (
Issue-Commit: a84989a361e8c7c49cb35cc41e86e09526be932e

# s3__sqs: a bucket whose AWS::S3::Bucket resource ended CREATE_FAILED on notification-destination validation still exists after the stack reaches ROLLBACK_COMPLETE

> _Note: this issue was identified against the code base at commit `a84989a361e8c7c49cb35cc41e86e09526be932e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
ONE net-new root cause of service `s3__sqs`. The grounding check was not marked ALREADY FILED or OWNED by the replay report.

=== Cause 1 of 1: the rollback does not delete the physical bucket of a resource that transitioned to CREATE_FAILED ===
AWS: recorded behavior only (CloudFormation's rollback contract; no Smithy error is involved, and the check's refusal arm is already correct here). When a stack create fails, CloudFormation rolls back and deletes every resource the create had physically provisioned — including the resource whose own creation then failed. After the stack reaches `ROLLBACK_COMPLETE`, HeadBucket on EITHER bucket of the template answers HTTP 404 with no body. The recording shows the emulator already gets the rest of this sequence right, so none of it is in question: the stack ends ROLLBACK_COMPLETE; BucketB — the AWS::S3::Bucket whose NotificationConfiguration names an SQS queue whose resource policy does not grant s3.amazonaws.com sqs:SendMessage — has exactly one CREATE_FAILED event whose ResourceStatusReason contains `Unable to validate the following destination configurations`; BucketA is CREATE_COMPLETE and then DELETE_COMPLETE; and HeadBucket on BucketA answers 404.
Emulator: HeadBucket on BucketB answers HTTP 200 — the bucket that was physically created before notification-destination validation refused the resource is left behind by the rollback.
Why: the account is left holding a bucket no stack tracks, under a name the template owns. S3 bucket names are globally unique, so a retry of the same deploy, or any later CreateBucket of that name, is refused as already existing, and a client that trusts ROLLBACK_COMPLETE to mean "nothing this stack created exists" leaks storage and a reserved name with no API answer pointing at it. The orphan also keeps whatever notification state the failed create had begun to install.
Fix: the rollback/cleanup path of the cloudformation crate — the set of resources a rollback deletes must include a resource that was physically created and then transitioned to CREATE_FAILED, not only those that reached CREATE_COMPLETE; for AWS::S3::Bucket that means deleting the bucket it had already created before destination validation refused the resource. The resolved-queue (no-policy) and absent-queue arms reach the same code path.
Note on neighbouring issues: the open issue 20261028T220212Z-s3-sqs-putbucketnotificationconfiguration-accepts-an-sqs-destination-it.md describes the same root cause in its Cause 2, but it is grounded in the sibling check `s3__sqs.deploy-s3-3-notification-nonexistent-queue-rollback-rejected` (destination queue does NOT exist) and carries only that check's replay key; this check drives the same leak through the EXISTING-queue-without-a-policy arm, which the replay report did not mark as owned, so it is filed here with its own key. 20261029T193114Z-s3-sqs-4-net-new-causes-object-notifications.md and 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back-failed.md also look related but do not cover it: here the rollback does run and reports its events correctly, and only the failed resource's physical bucket survives.
Traces: s3__sqs.deploy-s3-3-notification-queue-without-policy-rollback-rejected.

## Evidence
s3__sqs.deploy-s3-3-notification-queue-without-policy-rollback-rejected [mismatch]:
  AssertionError: after ROLLBACK_COMPLETE, HeadBucket on BucketB 'sim2real-s3-3-b-<generated>' returned '200'; expected 404
  expected: after the terminal status ROLLBACK_COMPLETE, HeadBucket on BucketB answers HTTP 404 (the bucket name is per-run and is not part of the requirement).
  actual: HeadBucket on BucketB answers HTTP 200.
  Assertions in the same check that PASSED and therefore bound the divergence to this one cause: StackStatus == 'ROLLBACK_COMPLETE'; exactly one CREATE_FAILED event for LogicalResourceId 'BucketB'; that event's ResourceStatusReason contains 'Unable to validate the following destination configurations'; BucketA's events include both CREATE_COMPLETE and DELETE_COMPLETE; HeadBucket on BucketA answers 404. The freshly created queue also correctly carried no Policy attribute.

## Steps to Reproduce
Boot the emulator and replay s3__sqs.deploy-s3-3-notification-queue-without-policy-rollback-rejected from suite simparity-behavioral-aws-v1.

By hand:
1. CreateQueue and read its QueueArn; do NOT give it a resource policy (confirm GetQueueAttributes reports no 'Policy').
2. CreateStack with a template declaring BucketA (plain AWS::S3::Bucket) and BucketB (AWS::S3::Bucket whose NotificationConfiguration names that queue ARN for s3:ObjectCreated:*), and no AWS::SQS::QueuePolicy.
3. Wait for the terminal stack status: ROLLBACK_COMPLETE (correct), with one BucketB CREATE_FAILED event whose reason names 'Unable to validate the following destination configurations' (correct) and BucketA CREATE_COMPLETE then DELETE_COMPLETE (correct).
4. HeadBucket BucketA — 404 (correct). HeadBucket BucketB — it answers 200 instead of 404: the bucket created for the CREATE_FAILED resource survived the rollback.
5. As the positive control, redeploy the same template plus an AWS::SQS::QueuePolicy granting s3.amazonaws.com sqs:SendMessage on that queue; the stack reaches CREATE_COMPLETE.
