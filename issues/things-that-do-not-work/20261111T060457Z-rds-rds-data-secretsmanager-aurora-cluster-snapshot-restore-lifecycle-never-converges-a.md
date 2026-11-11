Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__rds-data__secretsmanager.data-api-rows-survive-cluster-snapshot-restore
Issue-Key: rds__rds-data__secretsmanager.data-api-rows-survive-cluster-snapshot-restore|_wait_until#10|Timeout|return last
Issue-Commit: 0dd06decd64ca030d29e4dddabe070ef79046880

# Aurora cluster snapshot/restore lifecycle never converges: a Data API snapshot-restore check hangs until its budget expires

> _Note: this issue was identified against the code base at commit `0dd06decd64ca030d29e4dddabe070ef79046880`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RestoreDBClusterFromSnapshot

## Description
Cause 1 of 1: The Aurora cluster snapshot-and-restore lifecycle never reaches the terminal states a client polls for, so a caller that waits for them waits forever.

AWS: recorded behavior only for the timing; the state machine itself is modelled. Smithy (com-amazonaws-rds-amazonrdsv19): CreateDBClusterSnapshot/DescribeDBClusterSnapshots expose DBClusterSnapshot.Status, which advances to "available"; RestoreDBClusterFromSnapshot creates a new DBCluster whose DescribeDBClusters Status advances to "available"; ModifyDBCluster(ManageMasterUserPassword=true, ApplyImmediately=true) makes DBCluster.MasterUserSecret.SecretStatus reach "active"; CreateDBInstance/DescribeDBInstances advance DBInstanceStatus to "available"; EnableHttpEndpoint then makes the rds-data ExecuteStatement surface answer (it stops returning the start-up codes DatabaseResumingException / HttpEndpointNotEnabledException / DatabaseNotFoundException / ServiceUnavailableError / DatabaseUnavailableException). Each of those transitions is reached in finite time, and against a non-provisioning emulator they are expected effectively immediately. Rows committed through the Data API before the snapshot must be present on the restored cluster via both the Data API and a native session, and a row written after the snapshot must be absent.

Emulator: the check makes no progress and is cut off by its budget — "Error: Check timed out after 60.0s". No operation returned an error and no assertion fired, so one of the polled conditions above simply never becomes true (every poll loop in the check is generous: 900 s for snapshot availability, 900 s for the restored cluster, 600 s for the managed secret, 1500 s for the writer, 600 s for the Data API to answer), or a request on this path never returns. Hypothesis: a status the emulator writes on the snapshot or the restored cluster is never advanced to its terminal value (e.g. the snapshot or the restored cluster is left in a "creating"/"pending" status, or the restored cluster's ManageMasterUserPassword modification never flips MasterUserSecret.SecretStatus to "active"), so the client's wait-for-available loop can never exit.

Why: every SDK waiter and every real client polls these statuses — botocore's db_cluster_available / db_instance_available waiters, CloudFormation's RDS provider, Terraform, and any restore script. A status that never reaches its terminal value turns a routine restore into an indefinite hang rather than a visible failure: the client cannot proceed and cannot diagnose, since nothing was refused. Point-in-time/snapshot restore is also unverifiable: the data-survival contract (pre-snapshot rows present, post-snapshot row absent) is never reached.

Fix: the RDS snapshot and restore lifecycle in the rds service — the CreateDBClusterSnapshot / RestoreDBClusterFromSnapshot / ModifyDBCluster(ManageMasterUserPassword) / CreateDBInstance handlers and the cluster/snapshot state that drives the Status, DBInstanceStatus and MasterUserSecret.SecretStatus fields DescribeDBClusterSnapshots / DescribeDBClusters / DescribeDBInstances report, plus the EnableHttpEndpoint path that makes the restored cluster's rds-data surface answer.

Traces: rds__rds-data__secretsmanager.data-api-rows-survive-cluster-snapshot-restore (mismatch, timed out after 60 s).

## Evidence
Check: rds__rds-data__secretsmanager.data-api-rows-survive-cluster-snapshot-restore [mismatch]

Replay output:
  Error: Check timed out after 60.0s

Expected (AWS): the check completes — CreateDBClusterSnapshot's snapshot reaches Status "available"; RestoreDBClusterFromSnapshot's cluster reaches Status "available"; ModifyDBCluster(ManageMasterUserPassword=True, ApplyImmediately=True) brings MasterUserSecret.SecretStatus to "active"; the restored writer reaches DBInstanceStatus "available"; after EnableHttpEndpoint the rds-data ExecuteStatement answers; a Data API SELECT and a native psycopg SELECT on the restored cluster both return exactly the two rows written before the snapshot, and the id=3 row written after the snapshot is absent.

Actual (emulator): no call is refused and no assertion fails — the check never gets past its polling and is killed by the harness budget. Because the emulator provisions nothing, every one of those transitions should be observed on the first poll, so the budget being exhausted shows a polled condition that never becomes true (or a request that never returns) on the snapshot/restore path. The 60 s figure is the harness budget, not a requirement; the divergence is non-convergence of the lifecycle.

## Steps to Reproduce
1. Boot the emulator; point rds, rds-data and secretsmanager clients at it.
2. CreateSecurityGroup + AuthorizeSecurityGroupIngress on 5432, and CreateDBSubnetGroup over the private subnets.
3. CreateDBCluster(Engine="aurora-postgresql", DatabaseName="simparitydb", MasterUsername="simparity", ManageMasterUserPassword=True, EnableHttpEndpoint=True, ServerlessV2ScalingConfiguration={MinCapacity:0.5,MaxCapacity:1.0}, BackupRetentionPeriod=1) + CreateDBInstance(DBInstanceClass="db.serverless").
4. Poll DescribeDBInstances/DescribeDBClusters for "available" with an active MasterUserSecret and HttpEndpointEnabled, then rds-data ExecuteStatement "SELECT 1".
5. Through the Data API: CREATE TABLE sim2real_snap_restore, INSERT ids 1 and 2.
6. CreateDBClusterSnapshot and poll DescribeDBClusterSnapshots until Status == "available".
7. INSERT id 3 through the Data API (after the snapshot).
8. RestoreDBClusterFromSnapshot into a new cluster identifier and poll DescribeDBClusters until Status == "available"; then ModifyDBCluster(ManageMasterUserPassword=True, ApplyImmediately=True) and poll until MasterUserSecret.SecretStatus == "active"; CreateDBInstance(db.serverless) and poll until "available"; EnableHttpEndpoint(ResourceArn=<restored cluster ARN>).
9. Observe: one of the polls in steps 6-8 never converges, so the sequence never reaches the final Data API/native SELECT comparison and the client hangs indefinitely.
