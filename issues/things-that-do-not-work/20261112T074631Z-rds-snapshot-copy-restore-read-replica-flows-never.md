Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.copy-db-snapshot-round-trip rds.copy-db-cluster-snapshot-round-trip rds.automated-snapshot-listed-by-snapshot-type-for-instance-with-retention rds.e2e-read-replica-promotion-independent-writes rds.snapshot-restore-data-query-round-trip rds.proxy-default-endpoint-postgres-session-reaches-target rds.proxy-require-tls-rejects-plaintext-login
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.copy-db-snapshot-round-trip|_wait_until#14|Timeout|last = probe()
Issue-Key: rds.copy-db-cluster-snapshot-round-trip|_wait_until#17|Timeout|last = probe()
Issue-Key: rds.automated-snapshot-listed-by-snapshot-type-for-instance-with-retention|_poll#10|Timeout|raise AssertionError(f"{what} not reached within {cap_s}s; last value {val!r}")
Issue-Key: rds.e2e-read-replica-promotion-independent-writes|_pg_exec#24|Timeout|conn.close()
Issue-Key: rds.snapshot-restore-data-query-round-trip|_poll#43|Timeout|time.sleep(interval_s)
Issue-Key: rds.proxy-default-endpoint-postgres-session-reaches-target|run#22|PublishedUrlRejected|proxy_host, proxy_port = ctx.endpoint_host(proxy["Endpoint"], 5432)
Issue-Key: rds.proxy-require-tls-rejects-plaintext-login|run#23|PublishedUrlRejected|proxy_host, proxy_port = ctx.endpoint_host(proxy["Endpoint"], 5432)
Issue-Commit: 273752272468957bad25b41ef1e283d2e6b76e36

# RDS: snapshot copy / restore / read-replica flows never reach `available`, and DB proxy endpoints are published on rds.amazonaws.com hosts the emulator does not serve

> _Note: this issue was identified against the code base at commit `273752272468957bad25b41ef1e283d2e6b76e36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CopyDBSnapshot

## Description
2 net-new root causes in the rds service. None of these checks is marked OWNED or ALREADY FILED in the replay report.

Cause 1 of 2: a derived snapshot/restore/replica resource never advances to a terminal `available` state, so every flow that depends on one exhausts its budget polling Describe*.
AWS: CopyDBSnapshot returns a DBSnapshot and CopyDBClusterSnapshot returns a DBClusterSnapshot; both come back in a non-terminal state and then, autonomously and promptly, reach Status="available", at which point the copy is an independent resource with its own identifier and its own ARN that survives deletion of the source. Smithy: CopyDBSnapshot output DBSnapshot (errors DBSnapshotAlreadyExists 400, DBSnapshotNotFound 404, InvalidDBSnapshotState 400, KMSKeyNotAccessibleFault 400, SnapshotQuotaExceeded 400); CopyDBClusterSnapshot output DBClusterSnapshot (errors DBClusterSnapshotAlreadyExistsFault 400, DBClusterSnapshotNotFoundFault 404, InvalidDBClusterSnapshotStateFault 400, InvalidDBClusterStateFault 400, KMSKeyNotAccessibleFault 400, SnapshotQuotaExceeded 400). Likewise an instance with BackupRetentionPeriod > 0 eventually has an `automated` snapshot that DescribeDBSnapshots surfaces under SnapshotType, a restore produces an instance that reaches `available` and serves the restored rows, and a read replica reaches `available` and replicates. Every one of these is an advance-to-terminal-state contract, not an error contract: AWS never parks the resource.
Emulator: each of these checks ends as "Check timed out after 60.0s" while sitting in a Describe* poll for the terminal state. Nothing is refused - no error code and no non-2xx is returned - the state simply never arrives, so the poll runs until the harness budget is gone. The checks poll at 2-30 s intervals with their own multi-minute deadlines, so the harness budget, not the check's own deadline, is what ends them; the shared signature across five independent flows is that the derived resource stays non-terminal.
Why: a client cannot copy a snapshot, cannot observe its automated backups, cannot restore from a snapshot and cannot build a read replica: the standard wait-until-available idiom (every AWS waiter and every CDK/CloudFormation provisioning step uses it) never returns, so the caller hangs rather than failing fast. Code that is correct against AWS deadlocks against the emulator.
Fix: the state progression in the RDS service state for derived resources - the CopyDBSnapshot and CopyDBClusterSnapshot handlers and the automated-backup, restore and create-read-replica paths - must drive the new resource to its terminal `available` Status (and populate the automated SnapshotType entry) the way the create paths that do pass already do.
Traces: rds.copy-db-snapshot-round-trip, rds.copy-db-cluster-snapshot-round-trip, rds.automated-snapshot-listed-by-snapshot-type-for-instance-with-retention, rds.e2e-read-replica-promotion-independent-writes, rds.snapshot-restore-data-query-round-trip.

Cause 2 of 2: a DB proxy's published Endpoint is a real-AWS hostname the emulator does not answer on, so no session can be opened through the proxy.
AWS: Smithy DBProxy.Endpoint is documented as "The endpoint that you can use to connect to the DB proxy. You include the endpoint value in the connection string for a database client application." Against AWS that host resolves and completes the engine handshake. Recorded behavior only for the host's shape; the requirement is that the address the emulator publishes be an address the emulator itself serves - the endpoint host it is reached on, a documented own host, or a loopback address - because a customer dials exactly the string it is handed.
Emulator: the proxy endpoint is published as '<proxy-name>.proxy-<hex>.<region>.rds.amazonaws.com' and the session is refused before a byte is sent: PublishedUrlRejected - "its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]". This is the emulator's own divergence: it announced an address other than the one it is reached on.
Why: the DB proxy is unusable - the one field whose entire purpose is to be dialled points elsewhere - so proxy routing to a registered target and RequireTLS enforcement of a plaintext login can never be exercised.
Fix: the DB proxy endpoint publisher in the RDS service (the Endpoint member of CreateDBProxy / DescribeDBProxies and of the proxy's endpoints) must announce the emulator's own listener.
Note: open issue 20261109T135215Z-lambda-rds-secretsmanager-db-proxy-endpoints-are-published.md describes the same shape under the lambda__rds__secretsmanager service with the MYSQL engine family on port 3306 and does not carry these two rds checks' replay keys; these two are the POSTGRESQL family on port 5432 under the rds service and were left unowned by the report, so they are filed here. One fix in the endpoint publisher should close both.
Traces: rds.proxy-default-endpoint-postgres-session-reaches-target, rds.proxy-require-tls-rejects-plaintext-login.

## Evidence
Cause 1: all five checks end with "Error: Check timed out after 60.0s" with no AWS error returned at any point.
- rds.copy-db-snapshot-round-trip and rds.copy-db-cluster-snapshot-round-trip poll describe_db_snapshots / describe_db_cluster_snapshots for Status == "available" on the COPY (their own deadlines are 1200 s at a 20 s interval) and then assert the copy has a distinct identifier and ARN from the source and the same Engine, and that it stays available after the source is deleted. Expected: the copy reaches Status="available". Actual: the terminal status never arrives within the run budget.
- rds.automated-snapshot-listed-by-snapshot-type-for-instance-with-retention waits for the `automated` SnapshotType entry of an instance created with BackupRetentionPeriod > 0. Expected: DescribeDBSnapshots lists it. Actual: budget exhausted.
- rds.snapshot-restore-data-query-round-trip and rds.e2e-read-replica-promotion-independent-writes wait for the restored instance / the read replica to reach "available" before querying it. Actual: budget exhausted.

Cause 2: simparity.published.PublishedUrlRejected: endpoint_host was given sim2real-behav-proxy-<hex>.proxy-<hex>.us-west-2.rds.amazonaws.com:5432 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address. The proxy name, the hex account/proxy segment and the region in that string are per-run values; the defect is the domain being one the emulator does not serve.

## Steps to Reproduce
Cause 1: (a) create an aurora-mysql DB cluster plus one writer instance, wait for the cluster to be available, CreateDBClusterSnapshot, wait for it to be available, then CopyDBClusterSnapshot to a new identifier and poll DescribeDBClusterSnapshots for the copy's Status; it never becomes "available". (b) Same with CreateDBSnapshot + CopyDBSnapshot on a single-instance mysql DB. (c) Create a DB instance with BackupRetentionPeriod > 0 and poll DescribeDBSnapshots for its automated snapshot. (d) RestoreDBInstanceFromDBSnapshot, or CreateDBInstanceReadReplica, and poll DescribeDBInstances for DBInstanceStatus == "available".

Cause 2: CreateDBProxy with EngineFamily=POSTGRESQL (once with RequireTLS=true), wait for Status=available, read DescribeDBProxies' Endpoint and open a PostgreSQL session to it on port 5432.
