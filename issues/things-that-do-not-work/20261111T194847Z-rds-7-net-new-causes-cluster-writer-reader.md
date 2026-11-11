Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.aurora-cluster-writer-and-reader-endpoints-share-cluster-domain rds.automated-snapshot-listed-by-snapshot-type-for-instance-with-retention rds.create-instance-with-parameter-group-of-another-engine-family-rejected rds.engine-version-catalog-newest-minor-reaches-shipped-floor-and-names-major-family rds.serverless-v2-open-connection-blocks-auto-pause rds.serverless-v2-scale-to-zero-resume-preserves-rows rds.storage-autoscaling-triggers-on-usage-threshold
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.aurora-cluster-writer-and-reader-endpoints-share-cluster-domain|run#12|AssertionError|assert suffix and reader == r_prefix + suffix, (
Issue-Key: rds.automated-snapshot-listed-by-snapshot-type-for-instance-with-retention|_poll#10|AssertionError|raise AssertionError(f"{what} not reached within {cap_s}s; last value {val!r}")
Issue-Key: rds.create-instance-with-parameter-group-of-another-engine-family-rejected|_error_detail#15|AssertionError|raise AssertionError(f"the call that must be refused succeeded instead: {result!r}")
Issue-Key: rds.engine-version-catalog-newest-minor-reaches-shipped-floor-and-names-major-family|run#2|AssertionError|assert minors and max(minors) >= floor, (
Issue-Key: rds.serverless-v2-open-connection-blocks-auto-pause|_poll#18|Timeout|time.sleep(interval_s)
Issue-Key: rds.serverless-v2-scale-to-zero-resume-preserves-rows|_poll#22|Timeout|time.sleep(interval_s)
Issue-Key: rds.storage-autoscaling-triggers-on-usage-threshold|_fill_storage_with_retry#20|Timeout|time.sleep(15)
Issue-Commit: e21bce568e55aa711693c4fe92684e1cee8c7595

# rds: 7 net-new causes — cluster writer/reader endpoints are not cluster-domain hostnames, no automated snapshot is ever taken, a foreign-family parameter group is accepted, the postgres/aurora-postgresql minor catalog is below its shipped floor, Serverless v2 auto-pause/ServerlessDatabaseCapacity never report, and storage autoscaling never fires

> _Note: this issue was identified against the code base at commit `e21bce568e55aa711693c4fe92684e1cee8c7595`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusters, DescribeDBSnapshots, CreateDBInstance, DescribeDBEngineVersions, CreateDBCluster (Serverless v2), ModifyDBInstance (MaxAllocatedStorage)

## Description
Seven net-new root causes in the rds emulator, all found by the behavioral suite. Every other failing rds check in this run was already marked ALREADY FILED or OWNED by an open issue and is excluded here. This issue bundles 7 causes; each must be closed before submitting.

=== Cause 1 of 7: an Aurora cluster's Endpoint and ReaderEndpoint are not cluster-domain hostnames, and ReaderEndpoint embeds a port ===
AWS: DescribeDBClusters returns DBCluster.Endpoint as the writer (primary) connection endpoint and DBCluster.ReaderEndpoint as the load-balancing reader endpoint (Smithy: DBCluster.ReaderEndpoint — "The reader endpoint for the DB cluster. The reader endpoint for a DB cluster load-balances connections across the Aurora Replicas..."). Both members are host names ONLY — the port is carried separately in DBCluster.Port. The two names are derived from the cluster identifier and share one cluster domain suffix: Endpoint is `<db-cluster-identifier>.cluster-<suffix>` and ReaderEndpoint is `<db-cluster-identifier>.cluster-ro-<the same suffix>` (the suffix and the `.<region>.rds.amazonaws.com` tail are environment-specific and are NOT part of the requirement; the `<identifier>.cluster-` / `<identifier>.cluster-ro-` derivation and the shared suffix ARE).
Emulator: for a freshly available Aurora cluster, DescribeDBClusters returns Endpoint='127.0.0.1' and ReaderEndpoint='127.0.0.1:45679' — neither value is derived from the cluster identifier, the two share no `cluster-<suffix>` domain, there is no `cluster-ro-` infix, and the reader value puts `host:port` inside a host-only member.
Why: a client that derives the reader host from the writer host (the documented Aurora pattern), or that feeds ReaderEndpoint straight into a driver's host parameter with DBCluster.Port as the port, connects to the wrong address or fails to parse the host at all; reader/writer split traffic cannot be addressed.
Fix: the Aurora cluster projection that builds Endpoint/ReaderEndpoint in the rds crate's DescribeDBClusters/CreateDBCluster handlers (endpoint construction + state), not the check.
Traces: rds.aurora-cluster-writer-and-reader-endpoints-share-cluster-domain. The open issue 20261111T060457Z-rds-secretsmanager-describedbclusters-returns-a-literal-host-port-in-readerendpoint.md describes the same shape of defect but carries only the rds__secretsmanager replay key; it does not cover this check.

=== Cause 2 of 7: an instance with a non-zero BackupRetentionPeriod never produces an automated snapshot ===
AWS: when a DB instance has BackupRetentionPeriod >= 1, RDS takes automated backups for it, and DescribeDBSnapshots(DBInstanceIdentifier=<inst>, SnapshotType='automated') lists at least one DBSnapshot whose SnapshotType is 'automated' and whose Status reaches 'available' (Smithy: DescribeDBSnapshots.SnapshotType filters by snapshot type; DBSnapshot.SnapshotType — "Provides the type of the DB snapshot."). The snapshot identifier is service-generated (`rds:<instance>-<timestamp>` form); its literal value is not a requirement, its existence and its 'automated' SnapshotType are.
Emulator: DescribeDBSnapshots with SnapshotType='automated' for such an instance returns an empty DBSnapshots list for the whole 300 s poll — no automated snapshot is ever created.
Why: any client that verifies backups are on, lists restorable automated snapshots, or restores from one sees an instance with retention configured but zero backups.
Fix: instance backup state in the rds crate — create the automated snapshot when retention > 0, and include it in the DescribeDBSnapshots SnapshotType='automated' projection.
Traces: rds.automated-snapshot-listed-by-snapshot-type-for-instance-with-retention.

=== Cause 3 of 7: CreateDBInstance accepts a DB parameter group from another engine family ===
AWS: CreateDBInstance for a postgres instance whose DBParameterGroupName names a group with DBParameterGroupFamily 'mysql8.0' is REFUSED with HTTP 400, error code InvalidParameterCombination and the message "The parameter group <pg> with DBParameterGroupFamily <foreign> can't be used for this instance. Use a parameter group with DBParameterGroupFamily <family>." The sentence text is fixed; <pg> is the group name from the request, <foreign> is that group's family from state, and <family> is the family of the requested engine version. (InvalidParameterCombination is an awsQuery common error and is not in CreateDBInstance's modeled error list, which carries only the typed faults — AuthorizationNotFoundFault, DBParameterGroupNotFoundFault, InvalidSubnet, etc.)
Emulator: the call SUCCEEDS with HTTP 200, returning DBInstance{DBInstanceStatus='creating', Engine='postgres', DBParameterGroups=[{DBParameterGroupName=<the mysql8.0 group>, ParameterApplyStatus='in-sync'}]} — the cross-family combination is never validated and the foreign group is reported in-sync.
Why: a client's negative test passes where AWS refuses, and the instance comes up bound to a parameter group whose parameters cannot apply to its engine, so later parameter behavior silently diverges.
Fix: CreateDBInstance request validation in the rds crate — compare the named parameter group's DBParameterGroupFamily to the family of the requested engine/engine version before provisioning.
Traces: rds.create-instance-with-parameter-group-of-another-engine-family-rejected.

=== Cause 4 of 7: the engine-version catalog's newest minors are below AWS's long-shipped floor ===
AWS: DescribeDBEngineVersions(Engine='postgres') lists 16.x minors whose newest is at or past 16.8, and DescribeDBEngineVersions(Engine='aurora-postgresql') lists 15.x minors whose newest is at or past 15.10 — minors AWS has offered for well over a year (the model places no constraint on the catalog's contents; this is the catalog AWS serves, and the floor, not any one exact version, is the requirement).
Emulator: DescribeDBEngineVersions(Engine='postgres') lists only 16.3 and 16.4 as 16.x minors, so the newest listed 16.x minor is two years of patch releases behind the floor.
Why: a client that pins or requests a current minor (or that reads the newest listed minor and creates with it) is told the version is not available, and version-compatibility logic driven off the catalog picks a version AWS no longer offers as newest.
Fix: the engine-version catalog data the rds crate's DescribeDBEngineVersions serves.
Traces: rds.engine-version-catalog-newest-minor-reaches-shipped-floor-and-names-major-family.

=== Cause 5 of 7: an Aurora Serverless v2 cluster with MinCapacity 0 never reports an auto-pause, so scale-to-zero and resume never happen ===
AWS: a cluster created with ServerlessV2ScalingConfiguration{MinCapacity: 0.0, MaxCapacity: 1.0, SecondsUntilAutoPause: 300} publishes the AWS/RDS ServerlessDatabaseCapacity metric at 1-minute granularity with dimension DBClusterIdentifier, and once the writer has been idle for SecondsUntilAutoPause that metric reports 0.0 ACUs; the next login resumes the instance and every row committed before the pause is still readable and the instance accepts further writes.
Emulator: the check reaches no verdict — it is killed at the 600 s check cap while polling GetMetricStatistics for a ServerlessDatabaseCapacity datapoint of 0.0 after the cluster went idle; no pause is ever reported and the resume leg is never reached. Hypothesis: the emulator neither runs the Serverless v2 auto-pause state machine nor emits ServerlessDatabaseCapacity datapoints, so the poll can never be satisfied. Reported only as 'Check timed out after 600.0s' — no AWS call returned a divergent status or error code here.
Why: a client relying on scale-to-zero cannot observe capacity at all and cannot verify that a paused cluster resumes with its data intact.
Fix: the Serverless v2 capacity/auto-pause state in the rds crate, plus its AWS/RDS ServerlessDatabaseCapacity metric emission.
Traces: rds.serverless-v2-scale-to-zero-resume-preserves-rows.

=== Cause 6 of 7: a held-open session's Serverless v2 capacity samples never arrive, so 'no pause while connected' cannot be observed either ===
AWS: while one session stays connected, ServerlessDatabaseCapacity datapoints keep arriving for the cluster and none of them is 0.0, even well past SecondsUntilAutoPause; after that session closes, the same idle cluster does report 0.0.
Emulator: the check is killed at the 600 s cap while polling for capacity datapoints that cover the hold window; no ServerlessDatabaseCapacity datapoint for the cluster is ever observed, so neither the negative invariant nor its positive control can be evaluated. Reported only as 'Check timed out after 600.0s'. Same missing surface as Cause 5 seen from the connected side; listed separately because it is a distinct check and a distinct invariant (capacity must stay above zero while a session is held).
Why: a client cannot tell whether an open connection keeps a MinCapacity-0 cluster awake — the metric the behavior is read from does not exist.
Fix: same component as Cause 5 — Serverless v2 capacity accounting and metric emission, including the connected/idle distinction.
Traces: rds.serverless-v2-open-connection-blocks-auto-pause.

=== Cause 7 of 7: storage autoscaling never raises AllocatedStorage ===
AWS: with MaxAllocatedStorage set above AllocatedStorage, RDS raises AllocatedStorage on its own once free storage falls past the autoscaling threshold (<=10% free, sustained over the confirmation window), without any ModifyDBInstance(AllocatedStorage=...) call, and the new value stays within the MaxAllocatedStorage bound (recorded behavior only — the Smithy model constrains neither the threshold nor the confirmation window).
Emulator: after a postgres instance is filled past 90% of its 20 GiB AllocatedStorage, the check is killed at the 600 s cap with AllocatedStorage never raised — storage autoscaling does not fire. Reported only as 'Check timed out after 600.0s'.
Why: a client that sets MaxAllocatedStorage instead of pre-provisioning runs the database out of disk; the whole point of the setting is unobservable.
Fix: the instance storage state in the rds crate — the autoscaling trigger that raises AllocatedStorage toward MaxAllocatedStorage from observed usage.
Traces: rds.storage-autoscaling-triggers-on-usage-threshold.

## Evidence
Cause 1 — rds.aurora-cluster-writer-and-reader-endpoints-share-cluster-domain [mismatch]:
  AssertionError: Endpoint must be sim2real-behav-cl-045cd99f3fe6.cluster-<suffix> and ReaderEndpoint sim2real-behav-cl-045cd99f3fe6.cluster-ro-<same suffix>; got Endpoint='127.0.0.1', ReaderEndpoint='127.0.0.1:45679'
  (identifier and port are per-run values; the required shape is <identifier>.cluster-<suffix> / <identifier>.cluster-ro-<same suffix>, host-only, port in DBCluster.Port)

Cause 2 — rds.automated-snapshot-listed-by-snapshot-type-for-instance-with-retention [mismatch]:
  AssertionError: an available automated snapshot of sim2real-behav-inst-1681e971f681 not reached within 300s; last value []
  Expected: DescribeDBSnapshots(SnapshotType='automated') lists one 'available' automated snapshot. Actual: [] for the whole window.

Cause 3 — rds.create-instance-with-parameter-group-of-another-engine-family-rejected [mismatch]:
  Expected: 400 InvalidParameterCombination "The parameter group {pg} with DBParameterGroupFamily {foreign} can't be used for this instance. Use a parameter group with DBParameterGroupFamily {family}." (foreign family in the check: mysql8.0; engine: postgres)
  Actual: HTTP 200 — AssertionError: the call that must be refused succeeded instead: {'DBInstance': {'DBInstanceIdentifier': 'sim2real-behav-inst-9f33a49861f0', 'Engine': 'postgres', 'DBInstanceStatus': 'creating', 'DBParameterGroups': [{'DBParameterGroupName': 'sim2real-behav-pg-63e3a0dc11b7', 'ParameterApplyStatus': 'in-sync'}], ...}}

Cause 4 — rds.engine-version-catalog-newest-minor-reaches-shipped-floor-and-names-major-family [mismatch]:
  AssertionError: DescribeDBEngineVersions(Engine='postgres') must list a 16.x minor at or past 16.8; listed 16.x minors: [3, 4]
  (the check also requires aurora-postgresql 15.x >= 15.10; it failed on the postgres leg first)

Cause 5 — rds.serverless-v2-scale-to-zero-resume-preserves-rows [mismatch]:
  Error: Check timed out after 600.0s — killed while polling GetMetricStatistics(Namespace='AWS/RDS', MetricName='ServerlessDatabaseCapacity', Dimensions=[DBClusterIdentifier]) for a 0.0 Average after the cluster went idle (check's own budget for that poll: 1500 s). No pause datapoint ever observed.

Cause 6 — rds.serverless-v2-open-connection-blocks-auto-pause [mismatch]:
  Error: Check timed out after 600.0s — killed while polling for ServerlessDatabaseCapacity datapoints covering the 360 s hold window with a session held open. No datapoint for the cluster ever observed.

Cause 7 — rds.storage-autoscaling-triggers-on-usage-threshold [mismatch]:
  Error: Check timed out after 600.0s — a postgres instance with MaxAllocatedStorage set, filled to ~92% of its 20 GiB AllocatedStorage, never had AllocatedStorage raised by the service.

Note on Causes 5-7: these three surfaced only as the harness's 600 s check-cap timeout; no AWS call returned a divergent status, error code or message. They are filed because each names a service behavior (ServerlessDatabaseCapacity emission / auto-pause, storage autoscaling) that never materialises in the emulator, and because no open or already-filed issue carries their replay keys.

## Steps to Reproduce
Replay the behavioral suite (simparity-behavioral-aws-v1) against the emulator and re-run exactly these seven checks:

1. rds.aurora-cluster-writer-and-reader-endpoints-share-cluster-domain — create an Aurora cluster, wait for 'available', then DescribeDBClusters and compare Endpoint/ReaderEndpoint: both must be host-only names of the form <cluster-id>.cluster-<suffix> and <cluster-id>.cluster-ro-<same suffix>.
2. rds.automated-snapshot-listed-by-snapshot-type-for-instance-with-retention — create a db.t4g.micro instance with BackupRetentionPeriod >= 1, then poll DescribeDBSnapshots(DBInstanceIdentifier=<inst>, SnapshotType='automated') for an 'available' snapshot.
3. rds.create-instance-with-parameter-group-of-another-engine-family-rejected — CreateDBParameterGroup with DBParameterGroupFamily='mysql8.0', then CreateDBInstance(Engine='postgres', DBParameterGroupName=<that group>, everything else valid); expect 400 InvalidParameterCombination with the cross-family message.
4. rds.engine-version-catalog-newest-minor-reaches-shipped-floor-and-names-major-family — paginate DescribeDBEngineVersions(Engine='postgres') and collect 16.<minor> versions; the newest must be >= 16.8. Repeat for aurora-postgresql 15.x >= 15.10.
5. rds.serverless-v2-scale-to-zero-resume-preserves-rows — CreateDBCluster(Engine='aurora-postgresql', ServerlessV2ScalingConfiguration={MinCapacity:0.0, MaxCapacity:1.0, SecondsUntilAutoPause:300}) plus a db.serverless writer; seed rows, close all sessions, then poll GetMetricStatistics(AWS/RDS, ServerlessDatabaseCapacity, DBClusterIdentifier=<cluster>) for a 0.0 Average, then reconnect and re-read the rows.
6. rds.serverless-v2-open-connection-blocks-auto-pause — same cluster shape; hold one session open past SecondsUntilAutoPause and assert ServerlessDatabaseCapacity datapoints arrive covering the hold window and none is 0.0; then close the session and assert the cluster does report 0.0.
7. rds.storage-autoscaling-triggers-on-usage-threshold — create a postgres instance with AllocatedStorage=20 and MaxAllocatedStorage above it, fill it past ~90% used, and assert AllocatedStorage rises on its own with no ModifyDBInstance(AllocatedStorage=...) call.
