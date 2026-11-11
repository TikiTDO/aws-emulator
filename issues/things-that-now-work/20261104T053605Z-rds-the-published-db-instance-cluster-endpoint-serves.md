Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.aurora-writer-reader-endpoint-query-routing rds.cloudwatch-logs-export-delivers-only-while-enabled rds.cluster-snapshot-lifecycle-and-sharing rds.concurrent-sql-increment-loses-no-update rds.copy-from-stdin-bulk-load-rejects-a-malformed-row rds.deep-rds-1-rds-engine-truth-mysql-constraint-lock-and-charset-violations-rejected rds.deep-rds-sql-truth-iam-token-past-expiry-rejected rds.deep-rds-sql-truth-mysql-replica-write-rejected-read-only rds.deferred-class-modification-stays-pending-while-instance-serves-sql rds.delete-cluster-automated-backup-boundary rds.delete-instance-with-deletion-protection-rejected-rows-still-queryable rds.deploy-db-instance-cfn-connectivity rds.describe-db-log-files-chronological-order rds.e2e-cluster-failover-preserves-writes rds.e2e-parameter-change-affects-runtime-behavior rds.e2e-point-in-time-restore-recovers-data rds.e2e-snapshot-lifecycle rds.engine-dialect-discriminator-query rds.enhanced-monitoring-publishes-os-metric-documents-at-its-interval rds.failover-force-failover-rejected-on-a-single-az-instance-moves-no-write-role rds.failover-moves-the-writer-and-preserves-committed-rows rds.failover-multi-az-instance-keeps-the-endpoint-writable-and-the-rows rds.iam-db-auth-token-connects-and-maps-to-the-granted-role rds.iam-db-auth-token-minted-for-another-user-is-refused rds.instance-automated-backup-lifecycle rds.instance-iam-role-association-lifecycle rds.log-file-portion-contains-the-statement-the-engine-executed rds.max-connections-cap-rejects-the-overflow-session rds.modify-apply-immediately-false-pending-modified-values rds.modify-master-password-below-policy-rejected-live-credential-unaffected rds.multi-az-standby-auto-failover-on-primary-failure rds.mysql-auto-increment-utf8mb4-and-strict-overflow-rejected rds.mysql-master-user-lacks-super-set-global-rejected rds.mysql-plaintext-login-admitted-by-default-and-tls-session-reports-a-cipher rds.mysql-require-secure-transport-plaintext-login-rejected rds.mysql-server-version-matches-the-described-engine-version rds.mysql-wrong-master-password-login-rejected rds.paramgroup-out-of-range-value-rejected-engine-setting-unchanged rds.pg-dialect-numeric-timestamptz-jsonb-arrays-returning-recursive-cte rds.pg-master-user-not-superuser-alter-system-rejected rds.point-in-time-restore-data-query-round-trip rds.postgres-custom-port-is-the-published-and-listening-port rds.postgres-default-parameter-group-plaintext-login-rejected rds.postgres-session-identity-matches-master-username-and-db-name rds.postgres-sslmode-prefer-login-negotiates-tls-in-band rds.postgres-unknown-database-login-rejected rds.rds-sql-connectivity-postgres-mysql rds.read-replica-replication-query-and-promote rds.reboot-severs-open-session-rolls-back-uncommitted-keeps-committed rds.repeatable-read-transaction-sees-a-stable-snapshot rds.replica-session-read-only-override-rejected rds.replica-uncommitted-primary-write-absent-until-commit rds.serializable-write-skew-rejected-repeatable-read-permits rds.snaprestore-final-snapshot-on-delete-restores-rows rds.snaprestore-restore-over-live-instance-rejected rds.snapshot-restore-data-query-round-trip rds.static-parameter-pending-reboot-query-effect rds.stopped-instance-refuses-sql-and-start-keeps-rows
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.aurora-writer-reader-endpoint-query-routing|_sql_connect_with_retry#33|Timeout|time.sleep(2)
Issue-Key: rds.cloudwatch-logs-export-delivers-only-while-enabled|_pg_connect_with_retry#26|AssertionError|raise AssertionError(
Issue-Key: rds.cluster-snapshot-lifecycle-and-sharing|_poll_until#29|Timeout|last = probe()
Issue-Key: rds.concurrent-sql-increment-loses-no-update|_pg_connect_with_retry#15|Timeout|time.sleep(2)
Issue-Key: rds.copy-from-stdin-bulk-load-rejects-a-malformed-row|_pg_connect_with_retry#12|Timeout|time.sleep(2)
Issue-Key: rds.deep-rds-1-rds-engine-truth-mysql-constraint-lock-and-charset-violations-rejected|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: rds.deep-rds-sql-truth-iam-token-past-expiry-rejected|_pg_connect#15|OperationalError|conn = psycopg.connect(host=host, port=int(port), user=user, password=password, dbname=_DB,
Issue-Key: rds.deep-rds-sql-truth-mysql-replica-write-rejected-read-only|_my_connect#15|OperationalError|return pymysql.connect(host=host, port=int(port), user=_ADMIN, password=password,
Issue-Key: rds.deferred-class-modification-stays-pending-while-instance-serves-sql|_pg_connect_retry#9|Timeout|time.sleep(5)
Issue-Key: rds.delete-cluster-automated-backup-boundary|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: rds.delete-instance-with-deletion-protection-rejected-rows-still-queryable|_pg_connect_with_retry#11|Timeout|time.sleep(2)
Issue-Key: rds.deploy-db-instance-cfn-connectivity|_sql_roundtrip_with_retry#14|Timeout|time.sleep(2)
Issue-Key: rds.describe-db-log-files-chronological-order|_wait_multiple_log_files#8|Timeout|last_files = rds.describe_db_log_files(DBInstanceIdentifier=instance_id)[
Issue-Key: rds.e2e-cluster-failover-preserves-writes|_pg_exec_retry#23|Timeout|time.sleep(2)
Issue-Key: rds.e2e-parameter-change-affects-runtime-behavior|_pg_exec_retry#24|TimeoutError|raise TimeoutError(f"SQL against {host}:{port}: the run budget ended first; not graded") from last
Issue-Key: rds.e2e-point-in-time-restore-recovers-data|_pg_exec_retry#17|Timeout|time.sleep(2)
Issue-Key: rds.e2e-snapshot-lifecycle|_sql_connect_with_retry#12|Timeout|time.sleep(2)
Issue-Key: rds.engine-dialect-discriminator-query|_sql_connect_with_retry#11|Timeout|time.sleep(2)
Issue-Key: rds.enhanced-monitoring-publishes-os-metric-documents-at-its-interval|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: rds.failover-force-failover-rejected-on-a-single-az-instance-moves-no-write-role|_pg_connect_with_retry#18|Timeout|time.sleep(interval_s)
Issue-Key: rds.failover-moves-the-writer-and-preserves-committed-rows|_connect_writer#30|Timeout|time.sleep(interval_s)
Issue-Key: rds.failover-multi-az-instance-keeps-the-endpoint-writable-and-the-rows|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: rds.iam-db-auth-token-connects-and-maps-to-the-granted-role|_pg_connect_with_retry#15|Timeout|time.sleep(2)
Issue-Key: rds.iam-db-auth-token-minted-for-another-user-is-refused|run#20|AssertionError|raise AssertionError(
Issue-Key: rds.instance-automated-backup-lifecycle|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: rds.instance-iam-role-association-lifecycle|_add_role_with_retry#8|Timeout|time.sleep(interval_s)
Issue-Key: rds.log-file-portion-contains-the-statement-the-engine-executed|_pg_connect_with_retry#24|TimeoutError|raise TimeoutError(f"SQL connection to {host}:{port}: the run budget ended first; not graded") from last
Issue-Key: rds.max-connections-cap-rejects-the-overflow-session|_pg_connect_with_retry#22|Timeout|time.sleep(2)
Issue-Key: rds.modify-apply-immediately-false-pending-modified-values|_poll#13|Timeout|time.sleep(interval_s)
Issue-Key: rds.modify-master-password-below-policy-rejected-live-credential-unaffected|_login_ok#14|Timeout|time.sleep(2)
Issue-Key: rds.multi-az-standby-auto-failover-on-primary-failure|_poll_until#10|Timeout|value = probe()
Issue-Key: rds.mysql-auto-increment-utf8mb4-and-strict-overflow-rejected|_my_connect_retry#4|Timeout|time.sleep(5)
Issue-Key: rds.mysql-master-user-lacks-super-set-global-rejected|_my_connect_retry#4|Timeout|time.sleep(5)
Issue-Key: rds.mysql-plaintext-login-admitted-by-default-and-tls-session-reports-a-cipher|_my_first#7|Timeout|time.sleep(2)
Issue-Key: rds.mysql-require-secure-transport-plaintext-login-rejected|_my_first#12|Timeout|time.sleep(2)
Issue-Key: rds.mysql-server-version-matches-the-described-engine-version|_my_first#8|Timeout|time.sleep(2)
Issue-Key: rds.mysql-wrong-master-password-login-rejected|_my_first#9|Timeout|time.sleep(2)
Issue-Key: rds.paramgroup-out-of-range-value-rejected-engine-setting-unchanged|_setting#30|TimeoutError|raise TimeoutError(f"SQL connection to {host}:{port}: the run budget ended first; not graded") from last
Issue-Key: rds.pg-dialect-numeric-timestamptz-jsonb-arrays-returning-recursive-cte|_pg_connect_retry#6|Timeout|time.sleep(5)
Issue-Key: rds.pg-master-user-not-superuser-alter-system-rejected|_pg_connect_retry#4|Timeout|time.sleep(5)
Issue-Key: rds.point-in-time-restore-data-query-round-trip|_sql_connect_with_retry#14|Timeout|time.sleep(2)
Issue-Key: rds.postgres-custom-port-is-the-published-and-listening-port|_pg_first#8|Timeout|time.sleep(2)
Issue-Key: rds.postgres-default-parameter-group-plaintext-login-rejected|_pg_first#11|Timeout|time.sleep(2)
Issue-Key: rds.postgres-session-identity-matches-master-username-and-db-name|_pg_first#9|Timeout|time.sleep(2)
Issue-Key: rds.postgres-sslmode-prefer-login-negotiates-tls-in-band|_pg_first#7|Timeout|time.sleep(2)
Issue-Key: rds.postgres-unknown-database-login-rejected|_pg_first#8|Timeout|time.sleep(2)
Issue-Key: rds.rds-sql-connectivity-postgres-mysql|_sql_roundtrip_with_retry#12|Timeout|time.sleep(2)
Issue-Key: rds.read-replica-replication-query-and-promote|_sql_connect_with_retry#14|TimeoutError|raise TimeoutError(f"SQL connection to {host}:{port}: the run budget ended first; not graded") from last
Issue-Key: rds.reboot-severs-open-session-rolls-back-uncommitted-keeps-committed|_pg_connect_retry#5|Timeout|time.sleep(5)
Issue-Key: rds.repeatable-read-transaction-sees-a-stable-snapshot|_pg_connect_with_retry#14|Timeout|time.sleep(2)
Issue-Key: rds.replica-session-read-only-override-rejected|_sql_connect_with_retry#13|Timeout|time.sleep(2)
Issue-Key: rds.replica-uncommitted-primary-write-absent-until-commit|_sql_connect_with_retry#13|Timeout|time.sleep(2)
Issue-Key: rds.serializable-write-skew-rejected-repeatable-read-permits|_pg_connect_with_retry#11|Timeout|time.sleep(2)
Issue-Key: rds.snaprestore-final-snapshot-on-delete-restores-rows|_sql_connect_with_retry#13|Timeout|time.sleep(2)
Issue-Key: rds.snaprestore-restore-over-live-instance-rejected|_sql_connect_with_retry#15|Timeout|time.sleep(2)
Issue-Key: rds.snapshot-restore-data-query-round-trip|_sql_connect_with_retry#13|Timeout|time.sleep(2)
Issue-Key: rds.static-parameter-pending-reboot-query-effect|_sql_connect_with_retry#18|Timeout|time.sleep(2)
Issue-Key: rds.stopped-instance-refuses-sql-and-start-keeps-rows|_pg_connect_retry#6|Timeout|time.sleep(5)
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# rds: the published DB instance/cluster endpoint serves no PostgreSQL/MySQL wire protocol, so 58 engine-behaviour checks never open a session

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
ONE root cause (Cause 1 of 1).

AWS: CreateDBInstance/CreateDBCluster return an Endpoint (Address + Port) and, once DBInstanceStatus is 'available', a client that connects to that address:port speaks the engine's native wire protocol — the PostgreSQL frontend/backend protocol on the instance's Port (5432 by default for engine 'postgres') or the MySQL client protocol (3306 by default for engine 'mysql'/'aurora-mysql'). Smithy: CreateDBInstance output DBInstance$Endpoint (Endpoint$Address, Endpoint$Port), DBInstance$DBInstanceStatus, DBInstance$Engine, DBInstance$MasterUsername; the model describes the endpoint as the connection endpoint of the database. The SQL behaviour the checks assert (login refusal codes, transaction isolation, read-only replicas, max_connections caps, engine version strings, TLS negotiation) is engine behaviour AWS exposes only through that listener — recorded behavior only for the individual statements.

Emulator: nothing listens on the advertised endpoint. Every attempt is refused at TCP level or hangs until the harness's 60 s cap: psycopg reports OperationalError('connection failed: connection to server at "127.0.0.1", port 5432 failed: Connection refused'), pymysql reports (2003, "Can't connect to MySQL server on '127.0.0.1' ([Errno 111] Connection refused)"), and the harness records either 'DATAPLANE_UNIMPLEMENTED: could not open a postgres session ... Connection refused' or 'Error: Check timed out after 60.0s'. The instance is simultaneously reported 'available' with a populated Endpoint, so a client has no way to know the data plane is absent. An endpoint the emulator announces but is not reachable on is the emulator's own divergence.

Why: every client that does the only thing an RDS instance exists for — connect and run SQL — fails. No login/credential behaviour, no isolation level, no replica read-only enforcement, no parameter-group effect, no snapshot/PITR data round trip, and no engine-version/charset reporting can be observed. Control-plane calls that only matter because of their SQL consequence (failover preserving committed rows, reboot rolling back uncommitted work, stopped instance refusing SQL) silently cannot be verified.

Fix: the RDS instance/cluster data plane — a PostgreSQL and MySQL wire-protocol listener bound to the address and port the emulator publishes in DBInstance$Endpoint / DBCluster$Endpoint / DBCluster$ReaderEndpoint, with the instance's MasterUsername/MasterUserPassword, DBName, parameter-group settings and engine version wired to it, in the rds service crate's data-plane component. Related open issue human-rds-the-published-db-instance-endpoint-refuses-every-engine.md describes the same absence for a smaller set of connection-semantics checks; it does not carry these 58 replay keys.

Traces: the 58 failing check ids in trace_ids, each either 'Check timed out after 60.0s' at the connect step or an explicit 'Connection refused' from psycopg/pymysql.

## Evidence
Expected: a TCP connect to the Address/Port reported in DBInstance$Endpoint completes and the server speaks the PostgreSQL or MySQL wire protocol, admitting MasterUsername with MasterUserPassword.
Actual: `psycopg.OperationalError: connection failed: connection to server at "127.0.0.1", port 5432 failed: Connection refused` (rds.deep-rds-sql-truth-iam-token-past-expiry-rejected), `pymysql.err.OperationalError: (2003, "Can't connect to MySQL server on '127.0.0.1' ([Errno 111] Connection refused)")` (rds.deep-rds-sql-truth-mysql-replica-write-rejected-read-only), `AssertionError: DATAPLANE_UNIMPLEMENTED: could not open a postgres session at 127.0.0.1:5432 within 32s` (rds.cloudwatch-logs-export-delivers-only-while-enabled), and `Error: Check timed out after 60.0s` for the remaining 55 checks. DescribeDBInstances concurrently reports DBInstanceStatus 'available' with Endpoint.Address '<id>.<account>.us-west-2.rds.amazonaws.com' and Port 5432/3306.

## Steps to Reproduce
1. rds.create_db_instance(DBInstanceIdentifier=<id>, Engine='postgres', DBInstanceClass='db.t3.micro', MasterUsername=<u>, MasterUserPassword=<p>, AllocatedStorage=20, DBSubnetGroupName=<sng>).
2. Wait for DescribeDBInstances to report DBInstanceStatus='available' and read Endpoint.Address/Endpoint.Port.
3. Open a PostgreSQL connection to that address:port with the master credentials (psycopg) — the connection is refused. Repeat with Engine='mysql' and pymysql on port 3306 for the same result.
