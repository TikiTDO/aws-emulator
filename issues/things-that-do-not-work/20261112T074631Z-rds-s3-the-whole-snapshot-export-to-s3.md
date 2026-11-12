Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.cli-rds-describe-export-tasks-status-poll rds__s3.cli-rds-export-missing-permission-negative rds__s3.cli-rds-export-task-s3-object-listing-verify rds__s3.cli-rds-s3-bucket-policy-blocks-export-negative rds__s3.deploy-cfn-rds-s3-bucket-policy-restricts-export-negative rds__s3.deploy-cfn-rds-s3-export-role-stack-positive rds__s3.deploy-rds-export-task-role-stack-positive rds__s3.deploy-s3-bucket-policy-grants-rds-export-negative rds__s3.rds-export-manifest-file-format-positive rds__s3.rds-export-manifest-format-preserved rds__s3.rds-export-manifest-json-format-preserved rds__s3.rds-export-snapshot-s3-key-prefix-positive rds__s3.rds-export-snapshot-to-s3-positive rds__s3.rds-export-task-arn-resolution rds__s3.rds-export-task-bucket-policy-deny-negative rds__s3.rds-export-task-creates-s3-objects-positive rds__s3.rds-export-task-missing-s3-permission-negative rds__s3.rds-export-task-object-key-collision-negative rds__s3.rds-export-task-parquet-format-schema-preserved rds__s3.rds-export-task-partial-table-list-positive rds__s3.rds-export-task-respects-s3-prefix-positive rds__s3.rds-export-task-role-trusts-export-service-positive rds__s3.rds-export-task-s3-bucket-policy-denies-write-negative rds__s3.rds-export-task-s3-bucket-policy-deny-negative rds__s3.rds-export-task-status-transitions-visible-via-describe rds__s3.rds-export-task-writes-to-s3-positive rds__s3.rds-iam-role-trust-s3-service-positive rds__s3.rds-s3-export-manifest-content-positive rds__s3.rds-snapshot-export-missing-iam-role-negative rds__s3.rds-snapshot-export-missing-s3-permission-negative rds__s3.rds-snapshot-export-nonexistent-bucket-negative rds__s3.rds-snapshot-export-task-creates-s3-objects rds__s3.rds-snapshot-export-task-manifest-file-in-s3 rds__s3.rds-snapshot-export-task-respects-s3-prefix rds__s3.rds-snapshot-export-task-writes-to-s3-positive rds__s3.rds-snapshot-export-to-s3-positive rds__s3.rds-start-export-task-writes-objects-to-s3-positive
Issue-Key: rds__s3.cli-rds-describe-export-tasks-status-poll|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.cli-rds-export-missing-permission-negative|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.cli-rds-export-task-s3-object-listing-verify|_poll_until#16|Timeout|return last
Issue-Key: rds__s3.cli-rds-s3-bucket-policy-blocks-export-negative|_poll_until#25|Timeout|return last
Issue-Key: rds__s3.deploy-cfn-rds-s3-bucket-policy-restricts-export-negative|_poll_until#27|Timeout|return last
Issue-Key: rds__s3.deploy-cfn-rds-s3-export-role-stack-positive|_poll_until#24|Timeout|return last
Issue-Key: rds__s3.deploy-rds-export-task-role-stack-positive|_poll_until#28|Timeout|return last
Issue-Key: rds__s3.deploy-s3-bucket-policy-grants-rds-export-negative|_poll_until#33|Timeout|return last
Issue-Key: rds__s3.rds-export-manifest-file-format-positive|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-manifest-format-preserved|_wait_for#15|Timeout|return last
Issue-Key: rds__s3.rds-export-manifest-json-format-preserved|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-snapshot-s3-key-prefix-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-snapshot-to-s3-positive|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-arn-resolution|_wait_for#21|Timeout|return last
Issue-Key: rds__s3.rds-export-task-bucket-policy-deny-negative|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-creates-s3-objects-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-missing-s3-permission-negative|_poll_until#18|Timeout|return last
Issue-Key: rds__s3.rds-export-task-object-key-collision-negative|_poll_until#18|Timeout|return last
Issue-Key: rds__s3.rds-export-task-parquet-format-schema-preserved|_poll_until#21|Timeout|return last
Issue-Key: rds__s3.rds-export-task-partial-table-list-positive|_poll_until#19|Timeout|return last
Issue-Key: rds__s3.rds-export-task-respects-s3-prefix-positive|_poll_until#16|Timeout|return last
Issue-Key: rds__s3.rds-export-task-role-trusts-export-service-positive|_poll_until#13|Timeout|return last
Issue-Key: rds__s3.rds-export-task-s3-bucket-policy-denies-write-negative|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-s3-bucket-policy-deny-negative|_poll_until#24|Timeout|return last
Issue-Key: rds__s3.rds-export-task-status-transitions-visible-via-describe|_poll_until#4|Timeout|last = probe()
Issue-Key: rds__s3.rds-export-task-writes-to-s3-positive|_wait_export_task_terminal#14|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-iam-role-trust-s3-service-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-s3-export-manifest-content-positive|_wait_export_task_terminal#12|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-snapshot-export-missing-iam-role-negative|_poll_until#5|Timeout|last = probe()
Issue-Key: rds__s3.rds-snapshot-export-missing-s3-permission-negative|_poll_until#3|Timeout|last = probe()
Issue-Key: rds__s3.rds-snapshot-export-nonexistent-bucket-negative|_poll_until#4|Timeout|last = probe()
Issue-Key: rds__s3.rds-snapshot-export-task-creates-s3-objects|_poll_until#4|Timeout|last = probe()
Issue-Key: rds__s3.rds-snapshot-export-task-manifest-file-in-s3|_poll_until#11|Timeout|return last
Issue-Key: rds__s3.rds-snapshot-export-task-respects-s3-prefix|_poll_until#4|Timeout|last = probe()
Issue-Key: rds__s3.rds-snapshot-export-task-writes-to-s3-positive|_poll_export_status#17|Timeout|return last
Issue-Key: rds__s3.rds-snapshot-export-to-s3-positive|_wait_for#14|Timeout|return last
Issue-Key: rds__s3.rds-start-export-task-writes-objects-to-s3-positive|_poll_until#14|Timeout|return last
Issue-Commit: 273752272468957bad25b41ef1e283d2e6b76e36

# rds-s3: the whole snapshot-export-to-S3 path hangs — StartExportTask/DescribeExportTasks never reach a terminal Status, no S3 objects or manifest appear, and the negative variants never produce their refusal

> _Note: this issue was identified against the code base at commit `273752272468957bad25b41ef1e283d2e6b76e36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExportTask

## Description
1 net-new root cause, evidenced by 37 failing checks that all share one signature. None of them is marked OWNED or ALREADY FILED in the replay report.

Cause 1 of 1: the snapshot-export-to-S3 flow never terminates - neither by completing nor by refusing - so every caller is left polling.
AWS: StartExportTask takes ExportTaskIdentifier, SourceArn, S3BucketName, IamRoleArn and KmsKeyId (all @required in Smithy) plus optional S3Prefix and ExportOnly, and returns an ExportTask with Status and PercentProgress. DescribeExportTasks then surfaces that task advancing through a coarse, never-regressing sequence - STARTING -> IN_PROGRESS -> COMPLETE - ending in exactly one terminal value from {COMPLETE, FAILED, CANCELED}, with PercentProgress == 100 at COMPLETE. On success the export writes objects into the named bucket under the task's prefix, including the export manifest JSON, with per-table Parquet data laid out under the requested S3Prefix and restricted to ExportOnly when it is given. On a bad request AWS refuses promptly and terminally rather than hanging, with the operation's modelled errors: IamRoleMissingPermissions (400, awsQueryError code IamRoleMissingPermissions), IamRoleNotFound (404, IamRoleNotFound), InvalidS3BucketFault (400, InvalidS3BucketFault), KMSKeyNotAccessibleFault (400, KMSKeyNotAccessibleFault), InvalidExportOnly (400, InvalidExportOnly), InvalidExportSourceState (400, InvalidExportSourceState), ExportTaskAlreadyExists (400, ExportTaskAlreadyExists), DBSnapshotNotFound (404, DBSnapshotNotFound), DBClusterSnapshotNotFoundFault (404), DBClusterNotFoundFault (404). Those codes and statuses are the requirement; the bucket, role, key, task identifier and snapshot ARN named in each message are values taken from the request and the emulator's state, never from one recording.
Emulator: all 37 checks - positive and negative alike - end as "Error: Check timed out after 60.0s". The flow produces no terminal outcome of either kind. The positive checks never see DescribeExportTasks reach COMPLETE with PercentProgress 100 and never see an object (or the manifest) appear under ListObjectsV2 for the task prefix. The negative checks - nonexistent bucket, missing IAM role, role lacking s3:PutObject, role not trusting export.rds.amazonaws.com, a bucket policy denying the write, an export-task object key collision - never get their modelled 400/404 refusal either; the check's StartExportTask retry helper, which is only supposed to absorb short-lived IAM propagation (retrying on IamRoleMissingPermissions, InvalidS3BucketFault and KMSKeyNotAccessibleFault for 150 s at a 15 s interval) and the DescribeExportTasks poll that follows both run until the harness budget is gone. Several of these checks only poll RDS and S3 after the snapshot is already available, so the budget drains inside the export path itself, not in snapshot setup.
Why: snapshot export to S3 is unusable and, worse, un-diagnosable. A client that follows the documented idiom - StartExportTask, then poll DescribeExportTasks to a terminal Status - hangs forever instead of either finding its Parquet/manifest objects in the bucket or being told, with a specific code, which of role trust, role policy, bucket, bucket policy or KMS key is wrong. Error handling written against AWS's modelled codes is dead code here, and a CloudFormation/CDK stack that provisions an export role and drives an export never settles.
Fix: the StartExportTask and DescribeExportTasks handlers in the RDS service and the export-task state they drive. The handler must (a) validate the request up front and refuse terminally with the modelled error for each failure class instead of leaving the task pending, and (b) for an accepted request, advance the task's Status through at least one intermediate value to COMPLETE with PercentProgress 100 while its serializer/state writes the per-table data objects and the export manifest into the target bucket under S3Prefix/ExportTaskIdentifier, honouring ExportOnly.
Traces: all 37 checks listed in trace_ids, e.g. rds__s3.rds-export-task-status-transitions-visible-via-describe (status sequence + populated bucket), rds__s3.rds-export-task-creates-s3-objects-positive and rds__s3.rds-snapshot-export-task-manifest-file-in-s3 (objects/manifest in S3), rds__s3.rds-export-task-parquet-format-schema-preserved (Parquet layout), rds__s3.rds-export-task-respects-s3-prefix-positive and rds__s3.rds-export-task-partial-table-list-positive (S3Prefix/ExportOnly honoured), rds__s3.rds-snapshot-export-nonexistent-bucket-negative, rds__s3.rds-snapshot-export-missing-iam-role-negative, rds__s3.rds-export-task-missing-s3-permission-negative and rds__s3.rds-export-task-s3-bucket-policy-denies-write-negative (the refusals that never arrive).

## Evidence
Every one of the 37 checks reports exactly "Error: Check timed out after 60.0s"; no AWS error code and no non-2xx response is observed at any point in any of them.

Representative expectations that are never reached (from rds__s3.rds-export-task-status-transitions-visible-via-describe, which polls DescribeExportTasks on its own 2400 s deadline at a 2 s interval after StartExportTask returns):
- assert task["Status"] == "COMPLETE" (terminal set {COMPLETE, FAILED, CANCELED}; the status must never regress to an earlier stage)
- assert task["PercentProgress"] == 100 at COMPLETE
- assert at least one intermediate status was observed first, the first being STARTING or IN_PROGRESS
- assert s3.list_objects_v2(Bucket=<bucket>, Prefix=<ExportTaskIdentifier>)["Contents"] is non-empty at COMPLETE
Actual: the poll never leaves the loop and the 60 s harness budget expires.

Negative variants: expected a terminal refusal from StartExportTask with the modelled code for the injected fault - InvalidS3BucketFault (400) for a nonexistent or wrong bucket, IamRoleNotFound (404) for a missing role, IamRoleMissingPermissions (400) for a role lacking s3:PutObject/ListBucket or not trusting export.rds.amazonaws.com or blocked by a bucket policy, KMSKeyNotAccessibleFault (400) for an unusable key, ExportTaskAlreadyExists (400) for a task identifier collision - with the bucket name, role ARN, KMS key id and task identifier in the message taken from the request. Actual: no refusal is ever returned; the call is retried/polled until the budget expires. The bucket names, role ARNs and task identifiers in any single run are per-run values that fill those messages, not part of the requirement.

## Steps to Reproduce
1. Create an S3 bucket, a KMS key, and an IAM role trusting export.rds.amazonaws.com with s3:PutObject*/GetObject*/ListBucket/DeleteObject*/GetBucketLocation on that bucket.
2. Create a mysql DB instance in the shared network, wait for it to be available, CreateDBSnapshot and wait for the snapshot to be available.
3. Call StartExportTask with ExportTaskIdentifier, the snapshot ARN as SourceArn, S3BucketName, IamRoleArn and KmsKeyId (optionally S3Prefix and ExportOnly), then poll DescribeExportTasks(ExportTaskIdentifier=...) for a terminal Status. It never reaches COMPLETE/FAILED/CANCELED, and ListObjectsV2 on the bucket under the task prefix stays empty — no data objects and no export manifest.
4. Repeat step 3 with each fault injected in turn — a nonexistent bucket, no IamRoleArn, a role whose policy omits s3:PutObject, a role trusting ec2.amazonaws.com instead of export.rds.amazonaws.com, a bucket policy denying the export principal, and a duplicate ExportTaskIdentifier — and observe that no modelled 400/404 refusal is returned for any of them.
