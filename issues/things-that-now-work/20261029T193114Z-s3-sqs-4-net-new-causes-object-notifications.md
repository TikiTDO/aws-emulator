Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sqs.a-denied-put-creates-no-object-and-emits-no-notification s3__sqs.s3-notification-disjoint-prefix-routes-only-matching-queue s3__sqs.s3-object-removed-delete-subscription-withholds-the-delete-marker-event s3__sqs.overlapping-object-tagging-wildcard-configuration-rejected s3__sqs.deploy-s3-3-notification-queue-without-policy-rollback-rejected s3__sqs.deploy-s3-3-notification-nonexistent-queue-rollback-rejected
Issue-Key: s3__sqs.a-denied-put-creates-no-object-and-emits-no-notification|_poll_until#26|Timeout|time.sleep(interval_s)
Issue-Key: s3__sqs.s3-notification-disjoint-prefix-routes-only-matching-queue|_collect#15|Timeout|resp = sqs.receive_message(
Issue-Key: s3__sqs.s3-object-removed-delete-subscription-withholds-the-delete-marker-event|_collect#11|Timeout|resp = sqs.receive_message(
Issue-Key: s3__sqs.overlapping-object-tagging-wildcard-configuration-rejected|_client_error#12|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it")
Issue-Key: s3__sqs.deploy-s3-3-notification-queue-without-policy-rollback-rejected|run#17|AssertionError|assert bad["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Related: 20261028T220212Z-s3-sqs-putbucketnotificationconfiguration-accepts-an-sqs-destination-it.md
Issue-Commit: 6712e9b9ed86b1379f14acf29b1637f3b8f9994b

# s3__sqs: 4 net-new causes — object notifications never reach the queue, two notification-destination validations are missing, and a rolled-back stack leaves its bucket behind

> _Note: this issue was identified against the code base at commit `6712e9b9ed86b1379f14acf29b1637f3b8f9994b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Four net-new root causes. Related open issues that do NOT carry these replay keys: 20261028T220212Z-s3-sqs-putbucketnotificationconfiguration-accepts-an-sqs-destination-it.md, 20261029T072224Z-s3-sqs-putbucketnotificationconfiguration-accepts-an-sqs-queue-destination.md.

Cause 1 of 4: configured S3 event notifications never reach their SQS destination.
AWS: recorded behavior only for the payload; the model fixes the surface (s3 PutBucketNotificationConfiguration with QueueConfigurations[].{QueueArn, Events, Filter.Key.FilterRules}). AWS sends one message per matching object event to the matching queue only: a prefix-filtered configuration routes an object to the queue whose prefix matches and to no other; an `s3:ObjectRemoved:Delete`-scoped subscription receives the hard-delete event but withholds the delete-marker event; and a PutObject that policy denies creates no object and emits nothing.
Emulator: no notification is ever observed on any of the configured queues, so the three checks wait for the message (or for a bounded absence window against a working control) and are killed at the 60s harness budget.
Why: S3 event notification is the backbone of object-driven pipelines. With no messages at all, nothing downstream runs, and the "should not be delivered" assertions pass vacuously, so a later prefix-filter or event-type regression is undetectable.
Fix: the S3 object-mutation notification dispatcher — emit the event record to each matching QueueConfiguration, honouring the event-type list and the key filter rules.
Traces: s3__sqs.a-denied-put-creates-no-object-and-emits-no-notification, s3__sqs.s3-notification-disjoint-prefix-routes-only-matching-queue, s3__sqs.s3-object-removed-delete-subscription-withholds-the-delete-marker-event

Cause 2 of 4: PutBucketNotificationConfiguration accepts overlapping object-tagging wildcard filter rules.
AWS: refuses an overlapping/ambiguous filter configuration with HTTP 400 and Error.Code `InvalidArgument`, the message naming the conflicting configurations from the request.
Emulator: `the call succeeded; real AWS rejects it` — the overlap check does not exist, and the ambiguous configuration is persisted.
Why: AWS refuses overlapping rules because delivery would otherwise be non-deterministic; accepting them stores a configuration whose behaviour cannot be reasoned about, and the validation a client relies on to catch the conflict at configuration time is gone.
Fix: the S3 PutBucketNotificationConfiguration handler's filter-overlap validation, before the configuration is stored.
Traces: s3__sqs.overlapping-object-tagging-wildcard-configuration-rejected

Cause 3 of 4: a stack whose bucket notifies a queue with no s3.amazonaws.com grant reaches CREATE_COMPLETE instead of rolling back.
AWS: S3 refuses the notification configuration whose destination grants it nothing (HTTP 400 `InvalidArgument`, fixed text "Unable to validate the following destination configurations"), so the CloudFormation resource reaches CREATE_FAILED and the stack ends ROLLBACK_COMPLETE.
Emulator: the stack ends `'CREATE_COMPLETE' (reason None)`.
Why: a stack that must fail closed is promoted, and the deployed notification path can never deliver — the deploy-time guardrail is simply absent.
Fix: the S3 destination validation named in cause 2's component, plus the CloudFormation provisioning loop that must fail the resource and roll the stack back on a provider refusal.
Traces: s3__sqs.deploy-s3-3-notification-queue-without-policy-rollback-rejected

Cause 4 of 4: after ROLLBACK_COMPLETE, a bucket the stack created still exists.
AWS: recorded behavior only. Rollback deletes the resources the failed create had provisioned, so HeadBucket on that bucket answers HTTP 404 once the stack is ROLLBACK_COMPLETE.
Emulator: HeadBucket on the rolled-back bucket answers HTTP 200 — the resource is orphaned.
Why: rollback is supposed to leave no residue; an orphaned bucket blocks the next deployment on a name conflict and silently retains data and cost that the account believes were never created.
Fix: the CloudFormation rollback path's resource-deletion step for AWS::S3::Bucket.
Traces: s3__sqs.deploy-s3-3-notification-nonexistent-queue-rollback-rejected

## Evidence
Cause 1 — `Error: Check timed out after 60.0s` on all three notification checks; no message arrives on any configured queue.
Cause 2 — `AssertionError: the call succeeded; real AWS rejects it`.
Cause 3 — `AssertionError: a stack whose BucketB notifies a queue with no s3.amazonaws.com grant ended 'CREATE_COMPLETE' (reason None); expected ROLLBACK_COMPLETE`.
Cause 4 — `AssertionError: after ROLLBACK_COMPLETE, HeadBucket on BucketB 'sim2real-s3-3-b-434d6bb3e706' returned '200'; a bucket whose destination failed validation must not exist (expected 404)`.

## Steps to Reproduce
Cause 1: PutBucketNotificationConfiguration with two QueueConfigurations carrying disjoint key prefixes; PutObject under one prefix and poll both queues — neither receives anything. Repeat with an s3:ObjectRemoved:Delete-only subscription on a versioned bucket.
Cause 2: PutBucketNotificationConfiguration with two overlapping object-tagging wildcard filter rules — the call succeeds.
Cause 3: CreateStack whose bucket's NotificationConfiguration names a queue with no s3.amazonaws.com sqs:SendMessage grant — CREATE_COMPLETE.
Cause 4: CreateStack whose bucket notification names a nonexistent queue; after the stack reaches ROLLBACK_COMPLETE, HeadBucket on the bucket it created — 200.
