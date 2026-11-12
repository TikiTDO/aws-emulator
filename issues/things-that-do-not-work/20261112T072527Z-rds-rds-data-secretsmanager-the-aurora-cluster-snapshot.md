Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__rds-data__secretsmanager.data-api-rows-survive-cluster-snapshot-restore
Issue-Key: rds__rds-data__secretsmanager.data-api-rows-survive-cluster-snapshot-restore|_wait_until#12|Timeout|return last
Issue-Commit: 63b6e8e2cf3e66c0feda0c3bd64d85b077c87d33

# rds__rds-data__secretsmanager: the Aurora cluster snapshot/restore lifecycle still never converges — the Data API snapshot-restore check is killed by its budget with no verdict

> _Note: this issue was identified against the code base at commit `63b6e8e2cf3e66c0feda0c3bd64d85b077c87d33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RestoreDBClusterFromSnapshot

## Description
NOTE ON OVERLAP: the open issue 20261111T060457Z-rds-rds-data-secretsmanager-aurora-cluster-snapshot-restore-lifecycle-never-converges-a.md describes this same non-convergence and names this same check, but the replay suite did NOT mark this failure as OWNED by it (its recorded Issue-Key `...|_wait_until#10|Timeout|return last` no longer matches the key this run produced), so the failure is re-filed here with the key this run actually produced. Merge/close the two together. Related but covering other checks: 20261109T135215Z-rds-rds-data-secretsmanager-data-api-writes-to.md (RestoreDBClusterFromSnapshot shape) and 20261105T224225Z-rds-2-net-new-causes-every-db-instance.md (snapshot-restored instance endpoints).

Cause 1 of 1: the Aurora cluster snapshot-and-restore lifecycle never reaches the terminal states a client polls for, so a caller that waits on them waits forever.

AWS: Smithy (com-amazonaws-rds-amazonrdsv19): RestoreDBClusterFromSnapshot takes the required DBClusterIdentifier, SnapshotIdentifier and Engine (plus DBSubnetGroupName, VpcSecurityGroupIds, ServerlessV2ScalingConfiguration, DeletionProtection, Tags) and returns a DBCluster; it has no EnableHttpEndpoint parameter, so the Data API is turned on afterwards by EnableHttpEndpoint(ResourceArn=...). The state progression itself is recorded behavior only in its timing but modelled in its fields: DescribeDBClusterSnapshots reports DBClusterSnapshot.Status advancing to "available"; DescribeDBClusters reports the restored cluster's Status advancing to "available"; ModifyDBCluster(ManageMasterUserPassword=true, ApplyImmediately=true) brings DBCluster.MasterUserSecret.SecretStatus to "active"; DescribeDBInstances reports the new writer's DBInstanceStatus advancing to "available"; after EnableHttpEndpoint the rds-data ExecuteStatement surface answers instead of returning the start-up codes DatabaseResumingException / HttpEndpointNotEnabledException / DatabaseNotFoundException / ServiceUnavailableError / DatabaseUnavailableException. Each transition completes in finite time, and rows committed through the Data API before the snapshot are present on the restored cluster through both the Data API and a native session while a row written after the snapshot is absent.

Emulator: the check makes no progress and is cut off by its budget — reported only as `Error: Check timed out after 60.0s`. No call was refused, no non-2xx was seen and no assertion fired, so one of the polled conditions above never becomes true (or a request on that path never returns), even though every loop in the check is far more generous than the budget: 900 s for snapshot availability, 900 s for the restored cluster, 600 s for the managed secret, 1500 s for the restored writer and 600 s for the Data API to answer. Hypothesis: a status the emulator writes for the cluster snapshot or the restored cluster is never advanced to its terminal value (left in a creating/pending status), and/or ModifyDBCluster(ManageMasterUserPassword) never flips MasterUserSecret.SecretStatus to "active", so the wait-for-available loop can never exit. Because the emulator provisions nothing real, every transition should be observable on the first poll.

Why: every real client polls exactly these statuses — botocore's db_cluster_available / db_instance_available waiters, CloudFormation's RDS provider, Terraform, any restore runbook. A status that never reaches its terminal value turns a routine restore into an indefinite hang rather than a visible failure: nothing is refused, so the caller can neither proceed nor diagnose. The data-survival contract of snapshot restore (pre-snapshot rows present through both the Data API and a native session, post-snapshot row absent) is never reachable either.

Fix: the RDS snapshot/restore lifecycle in the rds service — the CreateDBClusterSnapshot, RestoreDBClusterFromSnapshot, ModifyDBCluster(ManageMasterUserPassword) and CreateDBInstance handlers plus the cluster/snapshot state that backs the Status, DBInstanceStatus and MasterUserSecret.SecretStatus fields DescribeDBClusterSnapshots / DescribeDBClusters / DescribeDBInstances report — must reach its terminal values promptly, and the EnableHttpEndpoint path must make the restored cluster's rds-data surface answer.

Traces: rds__rds-data__secretsmanager.data-api-rows-survive-cluster-snapshot-restore [mismatch], killed at the 60.0 s budget with no verdict.

## Evidence
Check: rds__rds-data__secretsmanager.data-api-rows-survive-cluster-snapshot-restore [mismatch]

Replay output:
  Error: Check timed out after 60.0s

Awaited conditions, from the check's own helpers:
  _wait_until(describe_db_cluster_snapshots -> Status == "available", timeout_s=900, interval_s=15)
  _wait_until(describe_db_clusters(restored) -> Status == "available", timeout_s=900)
  rds.modify_db_cluster(ManageMasterUserPassword=True, ApplyImmediately=True) then
  _wait_until(... Status == "available" and MasterUserSecret.SecretStatus == "active", timeout_s=600)
  _wait_until(describe_db_instances(restored writer) -> DBInstanceStatus == "available", timeout_s=1500)
  rds.enable_http_endpoint(ResourceArn=<restored cluster arn>) then _wait_data_api_ready (ExecuteStatement "SELECT 1", 600 s, retrying only DatabaseResumingException / HttpEndpointNotEnabledException / DatabaseNotFoundException / ServiceUnavailableError / DatabaseUnavailableException)

Expected (AWS): the check completes — snapshot "available", restored cluster "available", MasterUserSecret.SecretStatus "active", restored writer "available", Data API answering; a Data API SELECT and a native psycopg SELECT on the restored cluster each return exactly the two rows written before the snapshot, and the id=3 row written after the snapshot is absent.
Actual (emulator): no call is refused and no assertion fails — the check never gets past its polling and is killed by the harness budget, so a polled condition never becomes true on the snapshot/restore path. The 60 s figure is the harness budget, not a requirement; cluster/snapshot identifiers, endpoints, passwords and the row token are per-run values.

## Steps to Reproduce
1. Boot the emulator; point rds, rds-data, secretsmanager and ec2 clients at it.
2. CreateSecurityGroup + AuthorizeSecurityGroupIngress on 5432; CreateDBSubnetGroup over the private subnets.
3. CreateDBCluster(Engine="aurora-postgresql", DatabaseName set, MasterUsername set, ManageMasterUserPassword=True, EnableHttpEndpoint=True, ServerlessV2ScalingConfiguration={MinCapacity:0.5,MaxCapacity:1.0}, BackupRetentionPeriod=1) and CreateDBInstance(DBInstanceClass="db.serverless"); poll until both are "available" with MasterUserSecret.SecretStatus "active" and HttpEndpointEnabled true, then rds-data ExecuteStatement "SELECT 1".
4. Through the Data API: CREATE TABLE, INSERT ids 1 and 2.
5. CreateDBClusterSnapshot and poll DescribeDBClusterSnapshots until Status == "available".
6. INSERT id 3 through the Data API (after the snapshot).
7. RestoreDBClusterFromSnapshot into a new cluster identifier; poll DescribeDBClusters until Status == "available"; ModifyDBCluster(ManageMasterUserPassword=True, ApplyImmediately=True) and poll until MasterUserSecret.SecretStatus == "active"; CreateDBInstance(db.serverless) and poll until "available"; EnableHttpEndpoint(ResourceArn=<restored cluster arn>).
8. Observe: one of the polls in steps 5-7 never converges, so the final Data API/native SELECT comparison is never reached and the client hangs indefinitely instead of seeing ids 1 and 2 without id 3.
9. Re-replay rds__rds-data__secretsmanager.data-api-rows-survive-cluster-snapshot-restore to confirm the fix.
