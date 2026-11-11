Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.sagemaker-feature-store-stale-event-time-write-not-applied-and-newer-record-survives s3__sagemaker.sagemaker-feature-store-offline-store-writes-to-s3 s3__sagemaker.sagemaker-feature-store-offline-store-missing-s3-permission-negative
Issue-Key: sagemaker.sagemaker-feature-store-stale-event-time-write-not-applied-and-newer-record-survives|run#22|AssertionError|assert _record_values(after_stale) == {
Issue-Key: s3__sagemaker.sagemaker-feature-store-offline-store-writes-to-s3|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: s3__sagemaker.sagemaker-feature-store-offline-store-missing-s3-permission-negative|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# SageMaker Feature Store data plane is not event-time ordered and never propagates records to the S3 offline store (2 causes)

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
Two distinct root causes in the SageMaker Feature Store data plane, both grounded in failing behavioral checks. NOTE ON SERVICE ATTRIBUTION: cause 2 is grounded in checks whose id carries the COMPOUND leading token `s3__sagemaker` (the check declares SERVICES = ["s3", "sagemaker"]), so the per-service gate cannot place it on a single service token; both `s3` and `sagemaker` are bootstrapped, and the diverging behavior (SageMaker propagating an ingested record into the configured offline-store bucket) is SageMaker's, so it is filed here under `sagemaker`.

Cause 1 of 2: the online store applies a stale PutRecord instead of ignoring it (the store is last-write-wins, not event-time ordered).
Failing check ids: sagemaker.sagemaker-feature-store-stale-event-time-write-not-applied-and-newer-record-survives
AWS: Smithy doc for PutRecord (com.amazonaws.sagemakerfeaturestoreruntime#PutRecord): "If a new record's EventTime is greater, the new record is written to both the OnlineStore and OfflineStore. Otherwise, the record is a historic record and it is written only to the OfflineStore." The sibling UpdateRecord's modeled ConflictException states the same ordering contract: "The service rejected the update because the provided EventTime is older than the record's current EventTime ... resubmit the request with an EventTime that is equal to or newer than the current value." Recorded behavior (per the check's measurement notes): the stale PutRecord is NOT refused — it returns HTTP 200 with an empty body — and is then simply not applied, so a later GetRecord still returns the newer record's values AND the newer EventTime; a DeleteRecord tombstone participates in the same ordering (a re-write at an older EventTime does not resurrect the record).
Emulator: the stale PutRecord (EventTime 2026-01-01T00:00:00Z against a record stored at 2026-06-01T00:00:00Z) is applied — GetRecord afterwards returns {'rid': 'R1', 'et': '2026-01-01T00:00:00Z', 'payload': 'stale', 'score': '1.0'} where AWS still returns the T2 record unchanged. The store overwrites unconditionally on PutRecord.
Why: a client that ingests out-of-order events (replays, backfills, multiple producers) silently corrupts its online features — an older event clobbers the current feature values, and the stored EventTime moves backwards, which no AWS-conformant client guards against because AWS guarantees the ordering.
Fix: the record-ordering decision in PutRecord's handler and in the feature-store record state: compare the incoming EventTime against the stored record's EventTime (and against any delete tombstone's event time) and leave the online record untouched when the incoming value is not greater, still answering an empty HTTP 200.
Traces: sagemaker.sagemaker-feature-store-stale-event-time-write-not-applied-and-newer-record-survives (assertion at run():214).

Cause 2 of 2: a record ingested into a feature group that has an OfflineStoreConfig never lands in the offline-store S3 location, and OfflineStoreStatus never moves.
Failing check ids: s3__sagemaker.sagemaker-feature-store-offline-store-writes-to-s3, s3__sagemaker.sagemaker-feature-store-offline-store-missing-s3-permission-negative
AWS: PutRecord's modeled doc (above) states a record is written to the OfflineStore; recorded behavior only for the delivery timing — the recording notes a bare marker key appearing under the offline prefix and then a non-empty data object under the configured S3Uri prefix, and DescribeFeatureGroup's OfflineStoreStatus reporting Blocked with a BlockedReason once the feature group's role loses s3:PutObject, returning to Active when the permission is restored.
Emulator: no object of any kind ever appears under the configured offline-store prefix after PutRecord, and OfflineStoreStatus never leaves its initial value, so both checks poll their delivery/status predicates indefinitely and are killed by the harness ("Check timed out after 60.0s"). Nothing is delivered and no status transition is produced; the feature group itself reaches Created (the online-only check above gets past creation and PutRecord), so the missing piece is offline-store propagation and its status reporting, not feature-group creation.
Why: any pipeline that reads its training set from the offline store, or that monitors OfflineStoreStatus/BlockedReason to detect a broken delivery role, sees an empty bucket and a permanently healthy status — ingestion appears to succeed while no data is retained and no failure is surfaced.
Fix: the offline-store side effect of PutRecord's handler plus the feature-group state that backs DescribeFeatureGroup: on ingestion into a feature group with OfflineStoreConfig, write a non-empty object under the configured S3Uri prefix using the feature group's RoleArn, and derive OfflineStoreStatus (Active vs Blocked + BlockedReason) from whether that write is permitted.
Traces: s3__sagemaker.sagemaker-feature-store-offline-store-writes-to-s3 (polls for a non-empty object under s3://<bucket>/offline), s3__sagemaker.sagemaker-feature-store-offline-store-missing-s3-permission-negative (polls for OfflineStoreStatus Blocked, then Active again after the permission is restored).

## Evidence
Cause 1 — expected (AWS): GetRecord after a stale PutRecord returns the T2 record unchanged, {'rid': 'R1', 'et': '2026-06-01T00:00:00Z', 'payload': 'newer', 'score': '2.0'}. Actual (emulator): {'rid': 'R1', 'et': '2026-01-01T00:00:00Z', 'payload': 'stale', 'score': '1.0'} — AssertionError: "a PutRecord with an OLDER EventTime overwrote the stored record -- the online store is not event-time ordered".

Cause 2 — expected (AWS): after PutRecord into a feature group with OfflineStoreConfig, at least one non-empty object under the configured offline prefix (list_objects_v2 + head_object ContentLength > 0), and DescribeFeatureGroup.OfflineStoreStatus.Status moving Active -> Blocked (with BlockedReason) -> Active as s3:PutObject is revoked and restored. Actual (emulator): zero objects under the prefix and no OfflineStoreStatus transition; both checks fail with "Check timed out after 60.0s" while polling those predicates.

## Steps to Reproduce
Cause 1: create an online-only feature group (RecordIdentifierFeatureName=rid, EventTimeFeatureName=et, String EventTime) and wait for FeatureGroupStatus=Created. PutRecord rid=R1 at et=2026-06-01T00:00:00Z with payload=newer, score=2.0. PutRecord rid=R1 again at et=2026-01-01T00:00:00Z with payload=stale, score=1.0 (expect an empty HTTP 200). GetRecord rid=R1: AWS returns the 2026-06-01 record untouched; the emulator returns the stale values. Then DeleteRecord at 2026-09-05T00:00:00Z and re-PutRecord at 2026-09-01T00:00:00Z: AWS does not resurrect the record.

Cause 2: create a bucket and an IAM role trusted by sagemaker.amazonaws.com with s3:PutObject/GetObject/ListBucket/DeleteObject/GetBucketAcl on it. CreateFeatureGroup with OfflineStoreConfig.S3StorageConfig.S3Uri=s3://<bucket>/offline and DisableGlueTableCreation=true, wait for Created, then PutRecord one record. List s3://<bucket>/offline and head each key: AWS eventually shows a non-empty object; the emulator shows none. Then revoke s3:PutObject from the role, PutRecord again, and poll DescribeFeatureGroup: AWS reports OfflineStoreStatus.Status=Blocked with a BlockedReason and returns to Active when the permission is restored; the emulator never changes the status.
