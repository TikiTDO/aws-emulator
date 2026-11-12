Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sqs.s3-object-removed-delete-subscription-withholds-the-delete-marker-event
Issue-Key: s3__sqs.s3-object-removed-delete-subscription-withholds-the-delete-marker-event|_collect#36|Timeout|resp = sqs.receive_message(
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# s3__sqs: an ObjectRemoved:DeleteMarkerCreated / ObjectRemoved:Delete notification pair on a versioned bucket never delivers, so the delete-marker event is unobservable

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration (ObjectRemoved event delivery to SQS)

## Description
One net-new root cause in the s3__sqs suite. It is NOT covered by the open s3__sqs issues (20261028T220212Z-s3-sqs-putbucketnotificationconfiguration-accepts-an-sqs-destination-it.md, 20261103T235649Z-s3-sqs-a-bucket-whose-aws-s3-bucket.md, 20261029T193114Z-s3-sqs-4-net-new-causes-object-notifications.md), which carry other checks' replay keys for destination validation and object-notification configuration; this check carries none.

Cause 1 of 1: on a versioned bucket, neither the delete-marker event nor the permanent-version-delete event reaches its subscribed queue, so the per-event-type split S3 documents cannot be observed at all.
AWS: recorded behavior only for the delivery; the event taxonomy is documented (notification-how-to-event-types-and-destinations). On a bucket with versioning Enabled and two QueueConfigurations — one for s3:ObjectRemoved:DeleteMarkerCreated, one for s3:ObjectRemoved:Delete — a DeleteObject without a versionId creates a delete marker and delivers exactly one ObjectRemoved:DeleteMarkerCreated record to the first queue, carrying the new delete marker's versionId and no object size; a DeleteObject naming a concrete versionId permanently removes that version and delivers exactly one ObjectRemoved:Delete record to the second queue, carrying that versionId and no size. Each queue stays empty for the other's event type, and ListObjectVersions agrees with what each record said happened.
Emulator: the check is reported "Error: Check timed out after 60.0s" with no assertion reached — the poll for the first expected record never completes, so neither the per-queue routing, nor the versionId on the record, nor the absence of a size field was ever compared.
Why: a client that reacts to deletions — tombstone replication, audit of permanent removals, undelete tooling — receives nothing, and cannot tell a delete marker from a permanent version delete, which are operationally opposite events. Because both queues stay empty, a configuration that routes the wrong event type is indistinguishable from a correct one.
Fix: the S3 notification dispatcher's ObjectRemoved path — a versioned DeleteObject must emit ObjectRemoved:DeleteMarkerCreated for a marker and ObjectRemoved:Delete for a version delete, match each against the per-queue event-type filter, and deliver the record (with object.versionId set and object.size omitted) to the matching QueueConfiguration only.
Traces: s3__sqs.s3-object-removed-delete-subscription-withholds-the-delete-marker-event.

## Evidence
Check s3__sqs.s3-object-removed-delete-subscription-withholds-the-delete-marker-event, verdict [mismatch]:
  Error: Check timed out after 60.0s
with no assertion reached.

The awaited requirement, from the check's own docstring: "On a versioned bucket, a delete marker reaches only the ObjectRemoved:DeleteMarkerCreated queue and a permanent version delete only the ObjectRemoved:Delete queue, each carrying its versionId and no size; each queue stays quiet for the other's event, and ListObjectVersions agrees." Reference cited by the check: https://docs.aws.amazon.com/AmazonS3/latest/userguide/notification-how-to-event-types-and-destinations.html

Expected: one record per deletion in the matching queue, with eventName s3:ObjectRemoved:DeleteMarkerCreated / s3:ObjectRemoved:Delete, object.versionId present and object.size absent, and the sibling queue empty. Actual: no message is ever received, so the check is killed with nothing compared. Bucket names, queue URLs, keys and version ids are per-run values.

## Steps to Reproduce
1. Create a bucket, PutBucketVersioning Status=Enabled, and create two SQS queues with policies allowing s3.amazonaws.com to SendMessage.
2. PutBucketNotificationConfiguration with two QueueConfigurations: queue A on ["s3:ObjectRemoved:DeleteMarkerCreated"], queue B on ["s3:ObjectRemoved:Delete"].
3. PutObject a key, then DeleteObject without a VersionId — AWS delivers one ObjectRemoved:DeleteMarkerCreated record to queue A (versionId of the marker, no size) and nothing to queue B.
4. DeleteObject naming a concrete VersionId — AWS delivers one ObjectRemoved:Delete record to queue B and nothing to queue A; ListObjectVersions agrees.
5. The emulator delivers no message to either queue. Re-replay s3__sqs.s3-object-removed-delete-subscription-withholds-the-delete-marker-event to confirm the fix.
