Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.sagemaker-feature-store-offline-store-writes-to-s3 s3__sagemaker.sagemaker-feature-store-offline-store-missing-s3-permission-negative
Issue-Key: s3__sagemaker.sagemaker-feature-store-offline-store-writes-to-s3|_poll_until#16|TimeoutError|raise TimeoutError("condition: the run budget ended first; not graded")
Issue-Key: s3__sagemaker.sagemaker-feature-store-offline-store-missing-s3-permission-negative|_poll_until#14|TimeoutError|raise TimeoutError("condition: the run budget ended first; not graded")
Issue-Commit: 8fe1b88606c77dfa59d771b1a110885b6dfaed56

# s3__sagemaker: a Feature Store feature group with an OfflineStoreConfig never writes an ingested record to S3 and never moves OfflineStoreStatus (1 cause)

> _Note: this issue was identified against the code base at commit `8fe1b88606c77dfa59d771b1a110885b6dfaed56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
ONE root cause, grounded in TWO failing checks. 1 CAUSE must be closed before this issue is submitted.

NOTE ON PRIOR ART: the open issue 20261023T183034Z-sagemaker-feature-store-data-plane-is-not-event.md names BOTH of these check ids in its Issue-Trace-Ids, but its stored replay keys for them (`_poll_until#16|Timeout` and `_poll_until#14|Timeout`) no longer match: both checks now fail with an explicit `TimeoutError: condition: the run budget ended first; not graded` raised from `_poll_until` (lines 185 and 193 respectively) rather than the harness's 60 s kill. The replay suite therefore did not mark either failure as OWNED, so the fix is un-gated for both. This issue re-grounds the same cause on the current replay keys; it does not cover that issue's other cause (the online store's event-time ordering), which remains that issue's to close.

SERVICE ATTRIBUTION: these check ids carry the compound leading token `s3__sagemaker` (the checks declare SERVICES = ["s3", "sagemaker"]). Both s3 and sagemaker are bootstrapped; the diverging behavior — SageMaker propagating an ingested record into the configured offline-store bucket and deriving OfflineStoreStatus from whether that write is permitted — is SageMaker's, and the issue is filed under the compound token the checks report.

Cause 1 of 1: a record ingested with PutRecord into a feature group that has an OfflineStoreConfig never lands under the configured S3Uri prefix, and DescribeFeatureGroup's OfflineStoreStatus never changes when the feature group's role loses or regains s3:PutObject. Failing checks: s3__sagemaker.sagemaker-feature-store-offline-store-writes-to-s3, s3__sagemaker.sagemaker-feature-store-offline-store-missing-s3-permission-negative.

AWS: Smithy doc for PutRecord (com.amazonaws.sagemakerfeaturestoreruntime#PutRecord) states "If a new record's EventTime is greater, the new record is written to both the OnlineStore and OfflineStore. Otherwise, the record is a historic record and it is written only to the OfflineStore" — so an ingested record reaching the offline store is modeled behavior, not an optional extra. The delivery timing and the status surface are recorded behavior only: after PutRecord into a feature group whose OfflineStoreConfig.S3StorageConfig.S3Uri is s3://<bucket>/<prefix>, at least one NON-EMPTY object appears under that prefix (ListObjectsV2 then HeadObject ContentLength > 0), and DescribeFeatureGroup reports OfflineStoreStatus.Status "Active" while the write is permitted, moving to "Blocked" with a BlockedReason once the feature group's RoleArn loses s3:PutObject on that prefix and back to "Active" when the permission is restored. The bucket, prefix, role ARN, feature-group name and object keys are per-run values; the requirements are that an object is written at all, that it is non-empty, and the Active/Blocked status transition.

Emulator: no object of any kind ever appears under the configured offline-store prefix after PutRecord, and OfflineStoreStatus never leaves its initial value. Both checks exhaust their poll budgets and raise `TimeoutError: condition: the run budget ended first; not graded` — the happy-path check from run() line 92 -> `_poll_until` line 185 (polling for a non-empty object under the prefix) and the negative check from run() line 56 -> `_poll_until` line 193 (polling for the Blocked/Active transition). Feature-group creation and PutRecord themselves are accepted, so the missing piece is the offline-store side effect of ingestion and the status it must drive.

Why: any pipeline that builds its training set from the offline store reads an empty bucket while ingestion reports success — data is silently dropped with no error anywhere. Monitoring that watches OfflineStoreStatus/BlockedReason to detect a broken delivery role sees a permanently healthy feature group, so a genuinely broken offline store is indistinguishable from a working one, and the remediation path (fix the role, wait for Active) can never be exercised.

Fix: the offline-store side effect of the PutRecord handler plus the feature-group state behind DescribeFeatureGroup in the sagemaker crate — on ingestion into a feature group carrying OfflineStoreConfig, write a non-empty object under the configured S3Uri prefix using the feature group's RoleArn, and derive OfflineStoreStatus (Active vs Blocked + BlockedReason) from whether that write is permitted.

Traces: s3__sagemaker.sagemaker-feature-store-offline-store-writes-to-s3 (run() line 92 -> `_poll_until` line 185), s3__sagemaker.sagemaker-feature-store-offline-store-missing-s3-permission-negative (run() line 56 -> `_poll_until` line 193).

## Evidence
Expected (AWS): after PutRecord into a feature group with OfflineStoreConfig.S3StorageConfig.S3Uri = s3://<bucket>/offline, ListObjectsV2 under that prefix eventually returns at least one key and HeadObject on it reports ContentLength > 0; DescribeFeatureGroup.OfflineStoreStatus.Status is "Active", becomes "Blocked" with a BlockedReason after s3:PutObject is revoked from the feature group's RoleArn, and returns to "Active" when it is restored.

Actual (emulator), both checks:
  [error] s3__sagemaker.sagemaker-feature-store-offline-store-writes-to-s3
  File "check_s3__sagemaker_sagemaker_feature_store_offline_store_writes_to_s3.py", line 92, in run -> sizes = _poll_until(...)
  File "...", line 185, in _poll_until -> raise TimeoutError("condition: the run budget ended first; not graded")
  TimeoutError: condition: the run budget ended first; not graded

  [error] s3__sagemaker.sagemaker-feature-store-offline-store-missing-s3-permission-negative
  File "check_s3__sagemaker_sagemaker_feature_store_offline_store_missing_s3_permission_negative.py", line 56, in run -> good_keys = _poll_until(...)
  File "...", line 193, in _poll_until -> raise TimeoutError("condition: the run budget ended first; not graded")
  TimeoutError: condition: the run budget ended first; not graded

Zero objects under the offline prefix in both runs and no OfflineStoreStatus transition. Smithy: PutRecord doc — "the new record is written to both the OnlineStore and OfflineStore ... Otherwise ... written only to the OfflineStore"; the sibling UpdateRecord models ConflictException for the same ordering contract.

## Steps to Reproduce
1. Boot the emulator from mainline.
2. Create an S3 bucket and an IAM role trusted by sagemaker.amazonaws.com with s3:PutObject/GetObject/ListBucket/DeleteObject/GetBucketAcl on it.
3. CreateFeatureGroup with RecordIdentifierFeatureName=rid, EventTimeFeatureName=et, OfflineStoreConfig.S3StorageConfig.S3Uri=s3://<bucket>/offline, DisableGlueTableCreation=true, RoleArn=<role>; wait for FeatureGroupStatus=Created.
4. PutRecord one record, then ListObjectsV2 under s3://<bucket>/offline and HeadObject each key: no object ever appears (expected at least one with ContentLength > 0). Replay: s3__sagemaker.sagemaker-feature-store-offline-store-writes-to-s3.
5. Revoke s3:PutObject from the role, PutRecord again, and poll DescribeFeatureGroup: OfflineStoreStatus.Status never becomes Blocked (with a BlockedReason) and never returns to Active after the permission is restored. Replay: s3__sagemaker.sagemaker-feature-store-offline-store-missing-s3-permission-negative.
