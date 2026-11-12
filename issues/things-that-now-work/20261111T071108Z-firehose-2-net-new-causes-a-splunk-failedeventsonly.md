Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.destination-retry-duration-respected firehose.server-side-role-permission-removal-blocks-delivery
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.destination-retry-duration-respected|run#25|AssertionError|raise AssertionError(
Issue-Key: firehose.server-side-role-permission-removal-blocks-delivery|run#21|AssertionError|assert "after-revoke" not in seen, "the blocked record surfaced later instead of being genuinely dropped"
Issue-Commit: d0a04fabcd83cf2b721ac740047fd92b0e777ff8

# firehose: 2 net-new causes — a Splunk FailedEventsOnly stream never parks its undeliverable event in the S3 backup, and a record put while the delivery role lacked s3:PutObject is replayed into S3 once the permission is restored instead of staying undelivered

> _Note: this issue was identified against the code base at commit `d0a04fabcd83cf2b721ac740047fd92b0e777ff8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
Two net-new root causes in the Firehose delivery engine, both reached from an accepted PutRecord on an ACTIVE stream. They are bundled because they belong to one service; they are independent and each must be closed.

Cause 1 of 2: a Splunk destination with S3BackupMode=FailedEventsOnly writes no failed-event object to its S3 backup, so an event that cannot be delivered to the HEC endpoint is unobservable anywhere.
AWS: Smithy (com.amazonaws.firehose) models SplunkDestinationConfiguration with HECEndpoint / HECEndpointType / HECToken, HECAcknowledgmentTimeoutInSeconds (@range 180..600), RetryOptions -> SplunkRetryOptions.DurationInSeconds (@range 0..7200, doc: "The total amount of time that Firehose spends on retries. This duration starts after the initial attempt to send data to Splunk fails.") and S3BackupMode -> enum SplunkS3BackupMode with values [FailedEventsOnly, AllEvents], whose doc states Firehose "writes any data that could not be indexed to the configured Amazon S3 destination". So with DurationInSeconds 0 Firehose makes one attempt against the HEC endpoint, gives up on failure, and — because the backup mode is FailedEventsOnly — parks that event in the bucket S3Configuration names. Recorded behavior only for the shape of the parked record: one JSON document per line, the original bytes base64-encoded under "rawData" and the failure classified by an "errorCode" whose FIXED text is the destination-family prefix "Splunk."; the suffix is the failure class observed on the attempt and the rawData payload are values from the attempt and the request, never part of the template. Nothing is written implausibly early: this stream sets no BufferingHints, so the Splunk default buffer interval elapses before the first attempt and the bucket is still empty 30 s after the put.
Emulator: PutRecord returns 200 and DescribeDeliveryStream echoes RetryOptions.DurationInSeconds=0, HECAcknowledgmentTimeoutInSeconds=180 and S3BackupMode=FailedEventsOnly (those assertions pass), but ListObjectsV2 on the backup bucket returns no Contents for the whole bounded poll window; the check runs to its own deadline and fails with "AssertionError: S3 backup containing the failed Splunk record did not materialize within the bounded poll window; decoded backup records so far: []". No delivered object, no error-prefix object, no backup object is ever written, and no "Splunk."-prefixed errorCode is ever produced.
Why: FailedEventsOnly is the recommended configuration precisely so undeliverable events are recoverable. A client gets a 200 from PutRecord, the event never reaches Splunk, and the backup prefix stays empty, so "delivered" and "lost" are indistinguishable and every runbook that recovers failed events from the backup prefix or classifies them by the Splunk.* errorCode is inexercisable; RetryOptions.DurationInSeconds is echoed but has no observable effect.
Fix: the Splunk arm of the stream's delivery engine (the destination-target selection and the backup-object writer in the delivery module) — give DestinationConfigKind::Splunk a destination attempt analogous to the HTTP-endpoint one, bounded by HECAcknowledgmentTimeoutInSeconds and RetryOptions.DurationInSeconds, and on exhaustion write the failed events into the S3Configuration location as the per-record error envelope (base64 rawData plus the Splunk.-prefixed errorCode and its message) when S3BackupMode is FailedEventsOnly, not only the copy-everything path taken for AllEvents.
Note on an existing issue: open issue 20261102T112723Z-firehose-the-splunk-destination-has-no-delivery-path.md describes the same Splunk gap but was recorded against a different failure key (the check was then killed at the 60 s harness cap, "Timeout|time.sleep(2)"); the check now runs its full poll window and fails on the missing backup record, so it was not reported as owned. Close them together if they share one fix.
Traces: firehose.destination-retry-duration-respected (component: services/service-com-amazonaws-firehose-firehose-20150804/src/state/delivery.rs — s3_targets() Splunk arm, the ALL_RECORDS_BACKUP_MODES set it tests, and backup_object()).

Cause 2 of 2: a record put while the delivery role has no S3 permissions is retained in the buffer and delivered as soon as the permission is restored, instead of never reaching S3.
AWS: recorded behavior only (Smithy models only that S3DestinationConfiguration/ExtendedS3DestinationConfiguration carry a required RoleARN, "the Amazon Resource Name (ARN) of the Amazon Web Services credentials" the delivery is made under; it says nothing about what happens to a record whose delivery was refused). The recording shows: with the role's inline S3 policy intact a put is delivered; after DeleteRolePolicy removes the role's only S3 permissions, a later put never appears in the bucket; after PutRolePolicy restores the identical policy, a further put IS delivered, and the record put during the revoked window is STILL absent — the positive-control record from before the revoke also remains. So authorization is evaluated per delivery attempt and a record whose attempts were refused does not reappear once the role regains the permission.
Emulator: the withheld window matches (the record put after the revoke does not land while the policy is gone), but once the policy is restored the emulator flushes the retained buffer and the blocked record lands in S3 together with the post-restore record: the check fails with "AssertionError: the blocked record surfaced later instead of being genuinely dropped" after the final poll shows both "after-revoke" and "after-restore" lines. The delivery module documents this as intended ("the records stay buffered for a later attempt"), so a refused delivery is a deferral rather than a refusal.
Why: a client cannot use the delivery role to stop a stream from writing. Data a test or an incident response deliberately blocked is resurrected by a later unrelated policy change, so the bucket's contents do not reflect what was authorized at delivery time; a differential test that asserts the blocked payload is absent passes while the role is broken and then fails after any repair.
Fix: the authorization path of the S3 delivery in the stream's delivery engine (withheld_writes and the buffer it returns records to) — a buffer whose write was withheld must not be re-queued indefinitely for replay after the role changes; the refused records must be retired from the buffer (reported through the destination's CloudWatchLoggingOptions as the S3.AccessDenied delivery error) so a later authorized flush delivers only the records put after the repair.
Traces: firehose.server-side-role-permission-removal-blocks-delivery (component: services/service-com-amazonaws-firehose-firehose-20150804/src/state/delivery.rs — withheld_writes() and its destination_logging path, documented at lines 34-39 of that file).

## Evidence
Cause 1 — check firehose.destination-retry-duration-respected (verdict: mismatch).
Expected (AWS): CreateDeliveryStream with SplunkDestinationConfiguration{HECEndpoint=<unroutable TEST-NET-3 https URL>, HECEndpointType=Event, HECToken=<token>, HECAcknowledgmentTimeoutInSeconds=180, RetryOptions={DurationInSeconds:0}, S3BackupMode=FailedEventsOnly, S3Configuration={RoleARN,BucketARN}} -> stream ACTIVE and those values echoed; one PutRecord; the bucket is empty 30 s later; then, within the bounded poll window, an object appears in the backup bucket whose lines are JSON records with base64 "rawData" decoding to the put payload and an "errorCode" starting with the fixed prefix "Splunk.".
Actual (emulator): PutRecord 200; ListObjectsV2(Bucket=<backup bucket>) returns no Contents for the entire window. Failure: AssertionError: S3 backup containing the failed Splunk record did not materialize within the bounded poll window; decoded backup records so far: [].

Cause 2 — check firehose.server-side-role-permission-removal-blocks-delivery (verdict: mismatch).
Expected (AWS): put "before-revoke" -> delivered. DeleteRolePolicy(role, s3-delivery-access) -> put "after-revoke" -> never present in the bucket. PutRolePolicy restoring the identical document -> put "after-restore" -> delivered, and the set of lines read back from the bucket contains "before-revoke" and "after-restore" but NOT "after-revoke".
Actual (emulator): the first two phases match, but after the policy is restored the bucket's lines contain "after-revoke" as well. Failure: AssertionError: the blocked record surfaced later instead of being genuinely dropped.

## Steps to Reproduce
Cause 1 (Splunk FailedEventsOnly backup):
1. Create an S3 bucket and an IAM role trusting firehose.amazonaws.com with s3:PutObject/ListBucket/GetBucketLocation on it.
2. CreateDeliveryStream(DeliveryStreamName=<name>, SplunkDestinationConfiguration={HECEndpoint:"https://203.0.113.7:8088/services/collector/event", HECEndpointType:"Event", HECToken:<token>, HECAcknowledgmentTimeoutInSeconds:180, RetryOptions:{DurationInSeconds:0}, S3BackupMode:"FailedEventsOnly", S3Configuration:{RoleARN:<role arn>, BucketARN:"arn:aws:s3:::<bucket>"}}).
3. Poll DescribeDeliveryStream until ACTIVE; confirm the Splunk settings are echoed.
4. PutRecord(DeliveryStreamName=<name>, Record={Data:b"<marker>\n"}).
5. After 30 s, ListObjectsV2(Bucket=<bucket>) is empty (matches AWS).
6. Keep polling ListObjectsV2/GetObject: AWS yields a backup object with JSON lines carrying base64 "rawData" == "<marker>\n" and an "errorCode" starting "Splunk."; the emulator yields no object at all.

Cause 2 (live delivery-role enforcement):
1. Create an S3 bucket, an IAM role trusting firehose.amazonaws.com, and attach one inline policy granting the S3 delivery actions on that bucket.
2. CreateDeliveryStream with S3/ExtendedS3DestinationConfiguration{RoleARN, BucketARN, BufferingHints:{SizeInMBs:1, IntervalInSeconds:60}}.
3. PutRecord Data=b"before-revoke\n"; poll the bucket until it is delivered.
4. iam.DeleteRolePolicy(RoleName, PolicyName) to remove the role's only S3 permissions.
5. PutRecord Data=b"after-revoke\n"; poll the bucket for ~120 s — it must not appear.
6. iam.PutRolePolicy(RoleName, PolicyName, the identical document).
7. PutRecord Data=b"after-restore\n"; poll until it is delivered, then read every object in the bucket: AWS shows "before-revoke" and "after-restore" only; the emulator also shows "after-revoke".
