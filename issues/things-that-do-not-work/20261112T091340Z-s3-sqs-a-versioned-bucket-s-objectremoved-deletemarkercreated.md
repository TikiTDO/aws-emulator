Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sqs.s3-object-removed-delete-subscription-withholds-the-delete-marker-event
Issue-Key: s3__sqs.s3-object-removed-delete-subscription-withholds-the-delete-marker-event|_collect#36|Timeout|resp = sqs.receive_message(
Issue-Commit: 40d5826a48481a0d3e9807fb6b86c6b32d9a493b

# s3__sqs: a versioned bucket's ObjectRemoved:DeleteMarkerCreated / ObjectRemoved:Delete notifications never reach their subscribed SQS queues, so the removal-subscription scenario reaches no verdict

> _Note: this issue was identified against the code base at commit `40d5826a48481a0d3e9807fb6b86c6b32d9a493b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
One cause.

Cause 1 of 1: on a versioning-enabled bucket with two disjoint single-event removal subscriptions, no removal notification is observed on its queue, so the check cannot progress and is killed at the harness's 60 s cap.

AWS: Smithy (s3): PutBucketNotificationConfiguration takes NotificationConfiguration with QueueConfigurations[].{QueueArn, Events, Filter.Key.FilterRules}, and its input models SkipDestinationValidation (header x-amz-skip-destination-validation, documented as "Skips validation of Amazon SQS, Amazon SNS, and Lambda destinations") — i.e. by default S3 validates each destination at configuration time (by publishing the s3:TestEvent message to it) and then publishes notifications to it. Per the event-types reference the check cites (notification-how-to-event-types-and-destinations), on a versioned bucket a DeleteObject WITHOUT a VersionId creates a delete marker and publishes exactly one record with eventName "ObjectRemoved:DeleteMarkerCreated" whose s3.object.versionId is the new marker's version id and which carries NO s3.object.size, while a DeleteObject WITH a VersionId permanently removes that version and publishes exactly one record with eventName "ObjectRemoved:Delete" whose s3.object.versionId is the deleted version; each record goes only to the queue subscribed to that event type. Recorded behavior: both records arrive on their own queue within seconds, so the whole two-leg scenario finishes inside the 60 s budget.

Emulator: the check reaches no verdict at all — `Check timed out after 60.0s`. Before that point the check only waits on (a) the configuration-time s3:TestEvent drain on each of the two queues (30 s each) and (b) the up-to-120 s wait for the DeleteMarkerCreated record on the marker queue; the later legs (the permanent-delete record, the two quiet windows, ListObjectVersions) are never reached. So at least one of those waits consumes the entire budget with nothing delivered. Hypothesis: the S3 object-mutation notification dispatcher publishes nothing for a versioned-bucket removal, so the DeleteMarkerCreated wait burns the remaining budget (a sibling s3-service check shows the s3:TestEvent itself IS delivered, which points the stall at the removal record rather than at destination validation).

Why: version-aware removal events are how object-driven pipelines react to deletes (tombstone propagation, index invalidation, replication bookkeeping). With no record delivered, nothing downstream runs; worse, every "this queue must stay quiet" assertion in such a pipeline passes vacuously, so a later regression that collapses DeleteMarkerCreated into Delete (or fans a record to both subscriptions) cannot be detected.

Fix: the S3 object-removal notification dispatcher — a version-less DeleteObject on a versioned bucket must emit one ObjectRemoved:DeleteMarkerCreated record (versionId = the marker, no size) to each matching QueueConfiguration, and a version-qualified DeleteObject one ObjectRemoved:Delete record (versionId = the deleted version), honouring the configuration's Events list and key FilterRules so neither subscription receives the other's event.

Traces: s3__sqs.s3-object-removed-delete-subscription-withholds-the-delete-marker-event.

Overlap to merge, not a separate defect: the open issue 20261029T193114Z-s3-sqs-4-net-new-causes-object-notifications.md names this same check id under its cause 1, but the replay runner did not match this run's failure to it (that issue's replay key for the check is `_collect#11|Timeout|resp = sqs.receive_message(`, while this run produced a bare harness timeout with no frame), so the failure is filed here; a fixer should treat it as the same dispatcher defect. Also related, under a different service heading: 20261110T153718Z-events-s3-sqs-a-two-destination-s3-notification.md (a two-destination configuration delivers neither ObjectCreated nor ObjectRemoved).

## Evidence
Expected (AWS/recorded): after `delete_object(Bucket, Key)` on a versioned bucket, the queue subscribed to s3:ObjectRemoved:DeleteMarkerCreated receives one message whose Records[0].eventName == "ObjectRemoved:DeleteMarkerCreated", Records[0].s3.object.versionId == the VersionId the DeleteObject response returned, and with no `size` member; after `delete_object(Bucket, Key, VersionId=<original>)`, the queue subscribed to s3:ObjectRemoved:Delete receives one message with eventName "ObjectRemoved:Delete" and versionId == the deleted version. Each queue stays empty for the other's event. (The version ids, queue names and bucket name are per-run values; the eventName strings, the versionId correspondence and the absent size are the requirements.)
Actual (emulator): `Error: Check timed out after 60.0s` — the check is killed inside its notification waits, having observed no removal record; no assertion is ever reached.

## Steps to Reproduce
1. Create a versioning-enabled S3 bucket and two SQS queues, each with a queue policy granting s3.amazonaws.com sqs:SendMessage scoped by aws:SourceArn to the bucket.
2. PutBucketNotificationConfiguration with two QueueConfigurations over prefix 'v/': one Events=["s3:ObjectRemoved:Delete"] -> queue A, one Events=["s3:ObjectRemoved:DeleteMarkerCreated"] -> queue B. (GetBucketNotificationConfiguration echoes both correctly.)
3. Drain the s3:TestEvent from both queues.
4. put_object(Bucket, Key='v/subject.bin', Body=...) and note the VersionId.
5. delete_object(Bucket, Key) (no VersionId) and long-poll queue B for up to 120 s -> no record for the key arrives; the check is killed at the 60 s harness cap before its later legs (version-qualified delete on queue A, the two quiet windows, ListObjectVersions) run.
