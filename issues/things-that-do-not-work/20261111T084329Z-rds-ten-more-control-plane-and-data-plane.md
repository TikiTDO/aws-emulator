Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.cluster-read-replica-promotion rds.cluster-snapshot-lifecycle-and-sharing rds.delete-cluster-automated-backup-boundary rds.describe-db-log-files-chronological-order rds.enhanced-monitoring-publishes-os-metric-documents-at-its-interval rds.failover-multi-az-instance-keeps-the-endpoint-writable-and-the-rows rds.instance-automated-backup-lifecycle rds.modify-apply-immediately-false-pending-modified-values rds.multi-az-standby-auto-failover-on-primary-failure rds.rds-sql-connectivity-postgres-mysql
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.cluster-read-replica-promotion|_poll_until#20|Timeout|last = probe()
Issue-Key: rds.cluster-snapshot-lifecycle-and-sharing|_poll_until#29|Timeout|last = probe()
Issue-Key: rds.delete-cluster-automated-backup-boundary|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: rds.describe-db-log-files-chronological-order|_wait_multiple_log_files#8|Timeout|last_files = rds.describe_db_log_files(DBInstanceIdentifier=instance_id)[
Issue-Key: rds.enhanced-monitoring-publishes-os-metric-documents-at-its-interval|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: rds.failover-multi-az-instance-keeps-the-endpoint-writable-and-the-rows|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: rds.instance-automated-backup-lifecycle|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: rds.modify-apply-immediately-false-pending-modified-values|_poll#13|Timeout|time.sleep(interval_s)
Issue-Key: rds.multi-az-standby-auto-failover-on-primary-failure|_poll_until#10|Timeout|value = probe()
Issue-Key: rds.rds-sql-connectivity-postgres-mysql|_sql_roundtrip_with_retry#12|Timeout|time.sleep(2)
Issue-Commit: 5f88a7dcadd1e69adbda000c9f17c3a3daf7e228

# rds: ten more control-plane and data-plane flows never reach their terminal state, so every check polling them is killed at its budget

> _Note: this issue was identified against the code base at commit `5f88a7dcadd1e69adbda000c9f17c3a3daf7e228`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBInstances

## Description
10 net-new root causes, all of the same family: the emulator answers every call, but the resource or signal the check polls never reaches the state AWS converges it to, so the polling loop never exits and the check is killed by its budget (60 s for nine of them, 600 s for the Enhanced Monitoring one). None of the ten is marked ALREADY FILED or OWNED.

These are the flows NOT covered by the open issue 20261110T191704Z-rds-six-control-plane-lifecycle-flows-never-reach.md, which names the same mechanism for six different checks; that issue's Hypothesis (status transitions are owed to the clock and only materialised when a later request arrives, so each transition in a chain costs a whole client poll interval) plausibly explains these ten too and is restated once here as a Hypothesis, not as a finding.

Cause 1 of 10: Aurora cluster read-replica promotion never converges.
AWS: a secondary cluster created with ReplicationSourceIdentifier converges to `available` with ReplicationSourceIdentifier set; PromoteReadReplicaDBCluster then clears it and the cluster returns to Status `available`. A cluster that is not a read replica is refused InvalidDBClusterStateFault with the fixed message `You cannot promote a DB cluster that is not a read replica.`
Emulator: the chain primary cluster → writer member → secondary cluster → promotion → promoted/available never gets through its waits; the check is killed at 60 s.
Why: a client cannot establish that a cross-cluster replica was promoted, so it can never learn its secondary is now a standalone writable cluster.
Fix: the status-transition schedule and settling of DB-cluster records in the rds state, and the schedules CreateDBCluster / CreateDBInstance / PromoteReadReplicaDBCluster install.
Traces: rds.cluster-read-replica-promotion.

Cause 2 of 10: the cluster-snapshot lifecycle (creating → available, share, copy, delete) never converges.
AWS: a DB cluster snapshot answers `creating` and converges to `available`; ModifyDBClusterSnapshotAttribute then shares it, CopyDBClusterSnapshot produces an independent snapshot that also converges to `available`, and DeleteDBClusterSnapshot removes it.
Emulator: the chain never gets through its waits; the check is killed at 60 s, so the snapshot is never observed `available`.
Why: a client that waits for a cluster snapshot to be restorable or shareable waits forever, and nothing is refused to tell it otherwise.
Fix: the pending-status schedule and settling of DB-cluster-snapshot records in the rds state and the schedules CreateDBClusterSnapshot / CopyDBClusterSnapshot install.
Traces: rds.cluster-snapshot-lifecycle-and-sharing.

Cause 3 of 10: the retained CLUSTER automated-backup boundary never becomes observable.
AWS: deleting a cluster while retaining its automated backup leaves a backup DescribeDBClusterAutomatedBackups reports; DeleteDBClusterAutomatedBackup removes it, after which it reads as gone and a repeat delete — or an unknown DbClusterResourceId — is refused DBClusterAutomatedBackupNotFoundFault.
Emulator: the chain cluster available → cluster deleted → retained backup describable → backup deleted never completes within 60 s, so neither the retained backup nor the not-found refusal is ever reached.
Why: a client relying on retained cluster automated backups cannot see one appear or disappear, and never gets the refusal that tells it the backup is gone.
Fix: the cluster deletion/settling schedule and the cluster-automated-backup records in the rds state, plus DescribeDBClusterAutomatedBackups / DeleteDBClusterAutomatedBackup.
Traces: rds.delete-cluster-automated-backup-boundary.

Cause 4 of 10: DescribeDBLogFiles never produces a stable log-file set to compare.
AWS: an available DB instance has describable log files; repeated DescribeDBLogFiles calls return the same files, each file's LastWritten never moves backwards, and paging with a small MaxRecords reproduces the same set as one unpaginated call.
Emulator: the flow instance → `available` → repeated/paged DescribeDBLogFiles does not complete within 60 s, so the repeatability, monotonic-LastWritten and pagination properties are never observed.
Why: a client that enumerates or tails DB logs cannot establish the file set at all, so log download and rotation handling are unusable.
Fix: the DB-instance status settling that gates the flow, and the DescribeDBLogFiles handler's file set / LastWritten / pagination behavior.
Traces: rds.describe-db-log-files-chronological-order.

Cause 5 of 10: Enhanced Monitoring never publishes OS metric documents.
AWS: with MonitoringInterval > 0 and a monitoring role, RDS publishes JSON OS metric documents naming the instance's DbiResourceId into the RDSOSMetrics CloudWatch Logs group about one MonitoringInterval apart; setting the interval to 0 stops them and restoring it resumes them, while an interval of 7 is refused (only 0/1/5/10/15/30/60 are valid).
Emulator: the check is killed at its 600 s budget — the expected OS metric documents never arrive in the log group, so neither the cadence, nor the stop at interval 0, nor the resume is ever observed.
Why: a client that consumes Enhanced Monitoring (dashboards, OS-level alarms, capacity tooling) sees an empty stream forever with no error, and cannot tell a disabled instance from an enabled one.
Fix: the Enhanced Monitoring publisher wired to MonitoringInterval/MonitoringRoleArn in the rds instance state, and its delivery into the CloudWatch Logs RDSOSMetrics group.
Traces: rds.enhanced-monitoring-publishes-os-metric-documents-at-its-interval.

Cause 6 of 10: a forced failover on a Multi-AZ DB INSTANCE never completes, so the endpoint/row guarantees are unobservable.
AWS: RebootDBInstance(ForceFailover=True) on a Multi-AZ instance swaps AvailabilityZone and SecondaryAvailabilityZone, keeps Endpoint.Address unchanged, and the instance returns to `available` serving SQL with every committed row and accepting a new write.
Emulator: the flow create Multi-AZ instance → `available` → forced failover → back to `available` → SQL round trip does not complete within 60 s.
Why: a client cannot establish that failover preserves its endpoint and its data — the central durability promise of Multi-AZ — and a reboot it issued never reports completion.
Fix: the reboot/failover status schedule and settling of DB-instance records in the rds state and the RebootDBInstance handler's AZ swap.
Traces: rds.failover-multi-az-instance-keeps-the-endpoint-writable-and-the-rows.

Cause 7 of 10: the retained INSTANCE automated-backup lifecycle never becomes observable.
AWS: DeleteDBInstance with DeleteAutomatedBackups=False leaves a describable retained backup; DeleteDBInstanceAutomatedBackup then removes it, and an unknown DbiResourceId is refused DBInstanceAutomatedBackupNotFound.
Emulator: the chain instance `available` → deleted → retained backup describable → backup deleted never completes within 60 s.
Why: a client cannot see a retained automated backup appear or disappear, so restore-from-retained-backup planning is impossible and the not-found refusal never arrives.
Fix: the DB-instance deletion/settling schedule and the automated-backup records in the rds state, plus DescribeDBInstanceAutomatedBackups / DeleteDBInstanceAutomatedBackup.
Traces: rds.instance-automated-backup-lifecycle.

Cause 8 of 10: ModifyDBInstance(ApplyImmediately=False) parking a class change never converges.
AWS: an ApplyImmediately=False class change is parked in PendingModifiedValues with the live DBInstanceClass unchanged; a no-outage change (CopyTagsToSnapshot) applies at once and never appears pending; re-requesting the current class clears the pending entry.
Emulator: the flow create → `available` → deferred modify → read PendingModifiedValues → no-outage modify → clearing modify does not complete within 60 s.
Why: a client cannot tell a parked change from an applied one, so maintenance-window scheduling and change confirmation are both unavailable.
Fix: the modify/settling schedule for DB-instance records (pending-modified-values parking and clearing) in the rds state and ModifyDBInstance.
Traces: rds.modify-apply-immediately-false-pending-modified-values.

Cause 9 of 10: RebootDBInstance(ForceFailover=True) never swaps the Multi-AZ availability zones.
AWS: a forced-failover reboot on a Multi-AZ instance promotes the standby — AvailabilityZone and SecondaryAvailabilityZone exchange values — and the instance returns to `available`.
Emulator: the flow create Multi-AZ instance → `available` → forced failover → observe the AZ swap does not complete within 60 s, so the swap is never observed.
Why: a client verifying that Multi-AZ failover actually moved the primary (DR drills, failover automation) can never confirm it, and a reboot that merely completes is indistinguishable from one that failed over.
Fix: the AZ-swap effect and status schedule of RebootDBInstance(ForceFailover) on Multi-AZ DB-instance records in the rds state.
Traces: rds.multi-az-standby-auto-failover-on-primary-failure.

Cause 10 of 10: the plain postgres+mysql native SQL connectivity round trip never completes.
AWS: a db.t3.micro postgres instance and a db.t3.micro mysql instance each reach `available`, publish an Endpoint, accept a native SQL connection on it, and round-trip a row.
Emulator: the flow create two instances → both `available` → connect to each endpoint → round-trip a row does not complete within 60 s.
Why: this is the baseline promise of the service — an RDS endpoint that serves SQL. A client cannot provision an instance and use it, and gets no error saying why.
Fix: the DB-instance status settling that gates the flow, and the native SQL listener bound to each instance's published endpoint.
Traces: rds.rds-sql-connectivity-postgres-mysql.

## Evidence
All ten checks fail with the suite's timeout detail and no HTTP error, no error code and no refusal — the emulator keeps answering the polling calls, but the polled state never becomes terminal:

  rds.cluster-read-replica-promotion                                        Error: Check timed out after 60.0s
  rds.cluster-snapshot-lifecycle-and-sharing                                Error: Check timed out after 60.0s
  rds.delete-cluster-automated-backup-boundary                              Error: Check timed out after 60.0s
  rds.describe-db-log-files-chronological-order                             Error: Check timed out after 60.0s
  rds.enhanced-monitoring-publishes-os-metric-documents-at-its-interval     Error: Check timed out after 600.0s
  rds.failover-multi-az-instance-keeps-the-endpoint-writable-and-the-rows   Error: Check timed out after 60.0s
  rds.instance-automated-backup-lifecycle                                   Error: Check timed out after 60.0s
  rds.modify-apply-immediately-false-pending-modified-values                Error: Check timed out after 60.0s
  rds.multi-az-standby-auto-failover-on-primary-failure                     Error: Check timed out after 60.0s
  rds.rds-sql-connectivity-postgres-mysql                                   Error: Check timed out after 60.0s

Expected (AWS): each polled resource reaches its terminal state without further client action — DBInstanceStatus/Status `available`, DBInstanceNotFound after a delete, a cluster snapshot `available`, a retained automated backup describable and then DBClusterAutomatedBackupNotFoundFault / DBInstanceAutomatedBackupNotFound, PendingModifiedValues parked then cleared, the Multi-AZ AZ pair swapped with Endpoint.Address unchanged, a stable DescribeDBLogFiles set, RDSOSMetrics documents arriving about one MonitoringInterval apart, and a usable SQL endpoint — so a polling client exits its loop.

Actual (emulator): none of the ten chains reaches that state inside its budget. Same mechanism already documented for six other checks in 20261110T191704Z-rds-six-control-plane-lifecycle-flows-never-reach.md (Hypothesis: Store::settle_due_transitions only materialises transitions already due and PendingStatus::in_a_moment schedules each one into the future, so every transition in a chain costs a whole client poll interval).

## Steps to Reproduce
Replay the ten checks in trace_ids. Each is a poll that never exits:
1. rds.cluster-read-replica-promotion — CreateDBCluster(aurora-mysql, binlog_format=ROW cluster parameter group) → `available` → PromoteReadReplicaDBCluster on the non-replica (expect InvalidDBClusterStateFault) → CreateDBInstance writer → CreateDBCluster(ReplicationSourceIdentifier=<primary ARN>) → poll secondary to `available` → PromoteReadReplicaDBCluster → poll until ReplicationSourceIdentifier is cleared and Status is `available`.
2. rds.cluster-snapshot-lifecycle-and-sharing — CreateDBCluster(aurora-mysql) → `available` → CreateDBClusterSnapshot → poll `creating`→`available` → ModifyDBClusterSnapshotAttribute(restore, add 111122223333) → CopyDBClusterSnapshot → poll the copy `available` → DeleteDBClusterSnapshot.
3. rds.delete-cluster-automated-backup-boundary — cluster `available` → DeleteDBCluster retaining automated backups → poll DescribeDBClusterAutomatedBackups for the retained backup → DeleteDBClusterAutomatedBackup → poll until gone → repeat and with an unknown DbClusterResourceId read DBClusterAutomatedBackupNotFoundFault.
4. rds.describe-db-log-files-chronological-order — CreateDBInstance → `available` → DescribeDBLogFiles twice, then paginate with a small MaxRecords and compare the sets and each file's LastWritten.
5. rds.enhanced-monitoring-publishes-os-metric-documents-at-its-interval — CreateDBInstance(postgres) with MonitoringInterval and MonitoringRoleArn → `available` → poll the RDSOSMetrics CloudWatch Logs group for JSON documents naming the DbiResourceId → ModifyDBInstance(MonitoringInterval=0) → confirm they stop → restore the interval → confirm they resume → ModifyDBInstance(MonitoringInterval=7) and read the refusal.
6. rds.failover-multi-az-instance-keeps-the-endpoint-writable-and-the-rows — CreateDBInstance(postgres, MultiAZ=true) → `available` → SQL seed rows → RebootDBInstance(ForceFailover=True) → poll back to `available` → re-dial the same Endpoint.Address and read/write.
7. rds.instance-automated-backup-lifecycle — instance `available` → DeleteDBInstance(DeleteAutomatedBackups=False) → poll DescribeDBInstanceAutomatedBackups for the retained backup → DeleteDBInstanceAutomatedBackup → poll until gone → unknown DbiResourceId → DBInstanceAutomatedBackupNotFound.
8. rds.modify-apply-immediately-false-pending-modified-values — CreateDBInstance(postgres, db.t3.micro) → `available` → ModifyDBInstance(DBInstanceClass=db.t3.small, ApplyImmediately=False) → read PendingModifiedValues and the live class → ModifyDBInstance(CopyTagsToSnapshot, ApplyImmediately=False) → ModifyDBInstance(current class) and confirm the pending entry clears.
9. rds.multi-az-standby-auto-failover-on-primary-failure — CreateDBInstance(MultiAZ=true) → `available` → record AvailabilityZone/SecondaryAvailabilityZone → RebootDBInstance(ForceFailover=True) → poll until the two have swapped.
10. rds.rds-sql-connectivity-postgres-mysql — CreateDBInstance(postgres, db.t3.micro) and CreateDBInstance(mysql, db.t3.micro) → poll both to `available` → open a native SQL connection to each Endpoint and round-trip a row.
