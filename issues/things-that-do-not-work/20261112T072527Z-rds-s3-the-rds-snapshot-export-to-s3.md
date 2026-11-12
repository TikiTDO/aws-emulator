Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.cli-rds-export-task-s3-object-listing-verify rds__s3.rds-export-manifest-json-format-preserved rds__s3.rds-export-task-creates-s3-objects-positive rds__s3.rds-snapshot-export-task-manifest-file-in-s3
Issue-Key: rds__s3.cli-rds-export-task-s3-object-listing-verify|_poll_until#16|Timeout|return last
Issue-Key: rds__s3.rds-export-manifest-json-format-preserved|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-creates-s3-objects-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-snapshot-export-task-manifest-file-in-s3|_poll_until#11|Timeout|return last
Issue-Commit: 63b6e8e2cf3e66c0feda0c3bd64d85b077c87d33

# rds__s3: the RDS snapshot-export-to-S3 pipeline still never reaches a terminal state — all four seeded-snapshot export checks are killed by the run budget with no verdict

> _Note: this issue was identified against the code base at commit `63b6e8e2cf3e66c0feda0c3bd64d85b077c87d33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExportTask

## Description
NOTE ON OVERLAP: the open issue 20261110T153718Z-rds-s3-the-snapshot-export-to-s3-path.md describes the same non-convergence and lists these four check ids among its 36, but the replay suite did NOT mark any of these four as OWNED by it (its recorded Issue-Keys, e.g. `rds__s3.cli-rds-export-task-s3-object-listing-verify|_poll_until#10|Timeout|time.sleep(interval_s)`, no longer match the keys this run produced), so they are re-filed here with the keys this run actually produced. Also related, but covering OTHER checks that DO get an answer from StartExportTask and then assert on its shape: 20261109T135215Z-rds-s3-9-net-new-causes-export-task.md, 20261107T185637Z-rds-s3-rds-snapshot-export-to-s3-7-causes-no.md, 20261029T195009Z-rds-s3-5-net-new-root-causes-startexporttask.md, 20261103T040901Z-rds-s3-rds-startexporttask-answers-501-notimplemented-so.md, 20261104T053605Z-rds-startexporttask-stub.md. Merge/close them together.

Cause 1 of 1: the Aurora/RDS snapshot -> S3 export pipeline never progresses to a state a polling client can observe, so every check that provisions a seeded cluster snapshot and then waits on DescribeExportTasks is cut off by the suite's per-check budget without ever producing an assertion.

AWS: Smithy (com-amazonaws-rds-amazonrdsv19): StartExportTask takes the required ExportTaskIdentifier, SourceArn, S3BucketName, IamRoleArn, KmsKeyId (plus optional S3Prefix/ExportOnly) and returns the ExportTask shape carrying Status, PercentProgress, TaskStartTime/TaskEndTime, TotalExtractedDataInGB and FailureCause; DescribeExportTasks returns that same ExportTask list and is paginated on Marker. The modelled terminal values and the object layout are recorded behavior only: an accepted task advances STARTING -> IN_PROGRESS -> COMPLETE (or FAILED/CANCELED) with PercentProgress reaching 100 and no FailureCause, and on COMPLETE the destination bucket holds non-empty Parquet table files plus an export manifest nested under <S3Prefix>/<ExportTaskIdentifier>/ (the identifier and prefix are request values, not fixed text). The upstream legs these checks drive are equally modelled and equally terminal: CreateDBCluster/CreateDBInstance -> DescribeDBInstances reports DBInstanceStatus "available", and CreateDBClusterSnapshot -> DescribeDBClusterSnapshots reports Status "available".

Emulator: none of the four checks produces any assertion at all. Each is reported only as `Error: Check timed out after 60.0s`, i.e. the harness killed it while one of its poll loops was still spinning — and every loop in these checks is far more generous than the budget (instance available 1200 s at 20 s intervals, snapshot available 900 s at 15 s, export task terminal 1800 s at 30 s). No call was refused and no non-2xx was observed, so a polled predicate simply never becomes true. Hypothesis: the export task is accepted and recorded but its Status/PercentProgress are never advanced past the initial non-terminal value, and/or the provisioned Aurora writer or its cluster snapshot never reports "available", so each wait spins to the deadline. Because the emulator provisions nothing real, every one of these transitions should be observable on the first poll.

Why: a client cannot complete, observe or verify a single RDS snapshot export to S3. The happy paths (COMPLETE plus .parquet objects under the prefix, manifest JSON content/format, object listing via `aws s3 ls --recursive`) are unreachable, and so are the authorization negatives, because the task never terminates either way. Every SDK/CLI waiter that polls DescribeExportTasks hangs instead of returning or failing, which is indistinguishable from a stuck service and gives the caller nothing to diagnose.

Fix: the RDS export-task state machine — the StartExportTask handler and whatever state DescribeExportTasks reads for Status/PercentProgress/TaskEndTime/FailureCause — must drive an accepted task promptly to COMPLETE, writing the Parquet data objects and the export manifest under <S3Prefix>/<ExportTaskIdentifier>/ in the named bucket, or to FAILED with a FailureCause; and the cluster/instance/cluster-snapshot lifecycle these exports rest on must report its terminal "available" statuses in the rds service state rather than remaining in a creating/pending status.

Traces: rds__s3.cli-rds-export-task-s3-object-listing-verify, rds__s3.rds-export-manifest-json-format-preserved, rds__s3.rds-export-task-creates-s3-objects-positive, rds__s3.rds-snapshot-export-task-manifest-file-in-s3 — all four [mismatch], each killed at the 60.0 s budget with no verdict.

## Evidence
All four rds__s3 checks in this run report exactly:
  Error: Check timed out after 60.0s
No assertion was reached, so no status code, error code or response shape was compared — the divergence is non-convergence, not a wrong field.

The awaited conditions, from the checks' own helpers (cli-rds-export-task-s3-object-listing-verify shown):
  _poll_until(describe_db_instances -> DBInstanceStatus == "available", timeout_s=1200.0, interval_s=20.0)
  _poll_until(_try(_seed) is True, timeout_s=180.0, interval_s=10.0)   # rds-data CREATE TABLE + INSERT
  _poll_until(describe_db_cluster_snapshots -> Status == "available", timeout_s=900.0, interval_s=15.0)
  _poll_until(describe_export_tasks -> Status in ("COMPLETE","FAILED","CANCELED"), timeout_s=1800.0, interval_s=30.0)
and then, on COMPLETE:
  assert task["Status"] == "COMPLETE"
  assert lines, "aws s3 ls returned no objects under s3://<bucket>/<prefix>/"
  assert any(export_id in ln for ln in lines)
  assert any(ln.lower().endswith(".parquet") for ln in lines)

Expected (AWS): each poll is satisfied on its first or an early iteration — "available", "available", then a terminal export Status "COMPLETE" with PercentProgress 100, no FailureCause, and non-empty .parquet objects plus the export manifest under <S3Prefix>/<ExportTaskIdentifier>/.
Actual (emulator): no poll ever satisfies its predicate and the check is killed at the harness budget. By contrast, the rds__s3 checks that fail with an assertion (owned by the other open rds__s3 issues) do get an answer from StartExportTask, which places this cause after acceptance, in the progression of the task and of the cluster/snapshot lifecycle it rests on. Bucket names, prefixes, export identifiers, hosts and the 60 s budget itself are per-run values, not requirements.

## Steps to Reproduce
1. Boot the emulator; point rds, rds-data, s3, kms and iam clients at it.
2. CreateBucket for the export destination; kms CreateKey; iam CreateRole trusting export.rds.amazonaws.com with s3:PutObject*/GetObject*/ListBucket/DeleteObject*/GetBucketLocation on the bucket and kms:CreateGrant/DescribeKey/Decrypt/GenerateDataKey* on the key.
3. CreateDBSubnetGroup over the shared network's subnets; CreateDBCluster(Engine="aurora-mysql", EnableHttpEndpoint=True, ServerlessV2ScalingConfiguration={0.5,2.0}, DatabaseName set) and CreateDBInstance(DBInstanceClass="db.serverless"); poll DescribeDBInstances for DBInstanceStatus "available".
4. Seed one table through rds-data ExecuteStatement, then CreateDBClusterSnapshot and poll DescribeDBClusterSnapshots for Status "available".
5. StartExportTask(ExportTaskIdentifier=..., SourceArn=<cluster snapshot arn>, S3BucketName=<bucket>, S3Prefix=<prefix>, IamRoleArn=<role>, KmsKeyId=<key>) and poll DescribeExportTasks.
6. Observe: the export Status never becomes COMPLETE/FAILED/CANCELED (and/or an upstream status never becomes "available"), so the poll never exits; `aws s3 ls s3://<bucket>/<prefix>/ --recursive` is never reached. AWS reaches COMPLETE and the listing shows .parquet files under <prefix>/<export id>/.
7. Re-replay the four check ids in trace_ids to confirm the fix.
