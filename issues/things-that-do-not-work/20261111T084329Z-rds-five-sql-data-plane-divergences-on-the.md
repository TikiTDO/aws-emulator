Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.aurora-mysql-reader-endpoint-write-refused-read-only rds.deep-rds-sql-truth-mysql-replica-write-rejected-read-only rds.mariadb-require-secure-transport-plaintext-login-rejected rds.mysql-master-user-lacks-super-set-global-rejected rds.replica-session-read-only-override-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.aurora-mysql-reader-endpoint-write-refused-read-only|run#24|AssertionError|assert ro == [(0,)], f"the writer must be writable (@@innodb_read_only 0); got {ro!r}"
Issue-Key: rds.deep-rds-sql-truth-mysql-replica-write-rejected-read-only|run#22|AssertionError|assert rep_host != src_host, f"replica and source must be distinct endpoints; both {rep_host!r}"
Issue-Key: rds.mariadb-require-secure-transport-plaintext-login-rejected|_rows#13|ProgrammingError|cur.execute(sql, params)
Issue-Key: rds.mysql-master-user-lacks-super-set-global-rejected|_rows#5|ProgrammingError|cur.execute(sql, params)
Issue-Key: rds.replica-session-read-only-override-rejected|_sql_rows#14|ReadOnlySqlTransaction|cur.execute(statement)
Issue-Commit: 5f88a7dcadd1e69adbda000c9f17c3a3daf7e228

# rds: five SQL data-plane divergences on the emulated engines (missing system variables, information_schema.processlist, mysql.rds_* procedures, standby SET handling, non-distinct instance endpoints)

> _Note: this issue was identified against the code base at commit `5f88a7dcadd1e69adbda000c9f17c3a3daf7e228`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstanceReadReplica

## Description
5 net-new root causes in the SQL surface the emulator serves on RDS endpoints and in the endpoint addresses it publishes for DB instances. Each has its own failing check. Related but distinct open issues exist for other SQL gaps (20261109T135215Z-rds-10-net-new-causes-the-sql-endpoints.md) and for a literal host in a CLUSTER ReaderEndpoint (20261111T060457Z-rds-secretsmanager-describedbclusters-returns-a-literal-host-port-in-readerendpoint.md); none of the five checks below is OWNED or ALREADY FILED.

Cause 1 of 5: the Aurora MySQL / MySQL engine does not implement the @@innodb_read_only system variable.
AWS: an Aurora MySQL writer answers `SELECT @@innodb_read_only` with the integer 0 (and a reader instance with 1); it is a real server variable, so it never comes back as SQL NULL, and an unknown system variable in MySQL is an error (1193 Unknown system variable), not a NULL.
Emulator: the writer endpoint answers the same query with a single row holding NULL — the variable resolves to nothing rather than to 0, and the engine does not refuse it either.
Why: a client that uses @@innodb_read_only to decide whether a connection it has been handed is a writer or a reader (the standard Aurora writer/reader discrimination) gets NULL, which is neither, so it cannot route reads and writes and will either treat a reader as writable or refuse to use a healthy writer.
Fix: the system-variable table of the emulated MySQL/Aurora-MySQL engine — @@innodb_read_only must be backed by the instance's read-only state.
Traces: rds.aurora-mysql-reader-endpoint-write-refused-read-only.

Cause 2 of 5: the emulated MariaDB/MySQL engine has no information_schema.processlist table.
AWS: RDS for MariaDB and MySQL expose information_schema.processlist; `SELECT count(*) FROM information_schema.processlist WHERE user = '<user>'` is answerable by the admin user and returns the number of live sessions for that user.
Emulator: the query is refused by the engine with MySQL error 1146 and the message `no such table: processlist` — the information_schema view is absent, and the error text is the emulator's underlying store wording rather than MySQL's `Table 'information_schema.processlist' doesn't exist`.
Why: a client that counts or inspects its own sessions — connection-pool health checks, verifying that a refused login left no session, kill/idle-session tooling — cannot run its query at all; an operation AWS answers with a row is a hard error here, and the error it does get does not even name the schema.
Fix: the information_schema surface of the emulated MySQL/MariaDB engine (register a processlist view over live sessions), plus the table-not-found message so an absent table is reported as MySQL words it.
Traces: rds.mariadb-require-secure-transport-plaintext-login-rejected.

Cause 3 of 5: the emulated MySQL engine does not implement the mysql.rds_* stored procedures.
AWS: RDS for MySQL ships the RDS-managed stored procedures in the `mysql` schema; `CALL mysql.rds_show_configuration` is accepted by the master user and returns a result set of configuration rows whose name column includes `binlog retention hours`.
Emulator: the statement is refused with MySQL error 1064 (`You have an error in your SQL syntax; … near 'CALL rds_show_configuration' at line 1`) — CALL of a qualified procedure is not even parsed, and the schema qualifier is dropped from the echoed statement.
Why: every RDS-for-MySQL administrative flow goes through these procedures (binlog retention, replication control, kill, log rotation). A client that reads or sets configuration via mysql.rds_show_configuration / mysql.rds_set_configuration gets a syntax error, so RDS-specific administration is unreachable on the emulated engine, and the error class (syntax) misleads the client into thinking its own SQL is malformed rather than that the procedure is unavailable.
Fix: the CALL/stored-procedure support of the emulated MySQL engine and the mysql.rds_* procedure set it must expose.
Traces: rds.mysql-master-user-lacks-super-set-global-rejected.

Cause 4 of 5: a PostgreSQL read replica refuses `SET default_transaction_read_only = off` instead of accepting it.
AWS: on a PostgreSQL standby, `SET default_transaction_read_only = off` is ACCEPTED as a session setting (SHOW default_transaction_read_only then reads `off`); it simply changes nothing that matters, because recovery forces transaction_read_only = on, and the subsequent write is the statement that is refused — with SQLSTATE 25006 (read_only_sql_transaction). The error `cannot set transaction read-write mode during recovery` is what PostgreSQL raises for `SET TRANSACTION READ WRITE` / `SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE`, not for default_transaction_read_only.
Emulator: the replica endpoint refuses the SET itself, raising SQLSTATE 25006 `cannot set transaction read-write mode during recovery` on the `SET default_transaction_read_only = off` statement, so the session never gets past it.
Why: the read-only-ness of a standby is reported as a refusal of a benign session GUC rather than as a property of recovery enforced at write time. A client or framework that unconditionally normalises session settings on connect (ORMs and poolers commonly SET default_transaction_read_only) cannot open a usable session against a read replica at all, where against AWS it connects and only its writes are refused.
Fix: the SET/GUC handling of the emulated PostgreSQL engine on a replica — accept default_transaction_read_only as a session setting, keep transaction_read_only forced on during recovery, and raise 25006 on the write.
Traces: rds.replica-session-read-only-override-rejected.

Cause 5 of 5: a read replica and its source publish the same Endpoint.Address, so DB-instance endpoints are not distinct per instance.
AWS: every DB instance gets its own Endpoint.Address — a per-instance DNS name derived from its DBInstanceIdentifier — so a read replica's address is never equal to its source's, and a client can hold two connections, one to each, and tell them apart.
Emulator: DescribeDBInstances reports the identical literal host for the source instance and for the replica created from it (the recorded run shows the same single loopback literal for both). The requirement is not any particular hostname — the host and port of an emulator are environment-specific — it is that each instance's Endpoint.Address is its own, derived from its identifier, and that two distinct instances never share one.
Why: reader/writer separation is unobservable and unroutable. A client that resolves the source and the replica and expects two endpoints gets one, so replica-only read traffic is silently sent to whatever the shared address resolves to, and the read-replica topology AWS exposes through DescribeDBInstances cannot be reconstructed.
Fix: endpoint-address generation for DB-instance records in the rds state (and the DescribeDBInstances/CreateDBInstanceReadReplica serializers that publish Endpoint), so each instance publishes its own address.
Traces: rds.deep-rds-sql-truth-mysql-replica-write-rejected-read-only.

## Evidence
Cause 1 — rds.aurora-mysql-reader-endpoint-write-refused-read-only:
  AssertionError: the writer must be writable (@@innodb_read_only 0); got [(None,)]
  Expected rows for `SELECT @@innodb_read_only` on the writer: [(0,)]. Actual: [(None,)].

Cause 2 — rds.mariadb-require-secure-transport-plaintext-login-rejected:
  pymysql.err.ProgrammingError: (1146, 'no such table: processlist')
  raised by `SELECT count(*) FROM information_schema.processlist WHERE user = %s`. AWS answers this with one row; MySQL's own 1146 text names the schema-qualified table.

Cause 3 — rds.mysql-master-user-lacks-super-set-global-rejected:
  pymysql.err.ProgrammingError: (1064, "You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use near 'CALL rds_show_configuration' at line 1")
  raised by `CALL mysql.rds_show_configuration`, which on AWS returns rows whose name column contains 'binlog retention hours'.

Cause 4 — rds.replica-session-read-only-override-rejected:
  psycopg.errors.ReadOnlySqlTransaction: cannot set transaction read-write mode during recovery
  raised by `SET default_transaction_read_only = off` on the replica. AWS accepts that statement (SHOW then reads 'off'), keeps transaction_read_only = 'on', and refuses the following INSERT with SQLSTATE 25006.

Cause 5 — rds.deep-rds-sql-truth-mysql-replica-write-rejected-read-only:
  AssertionError: replica and source must be distinct endpoints; both '127.0.0.1'
  src['Endpoint']['Address'] == rep['Endpoint']['Address'] from DescribeDBInstances for the source and its read replica.

## Steps to Reproduce
Cause 1: CreateDBCluster(aurora-mysql) with a writer member, wait for `available`, connect to the cluster writer endpoint with the master user and run `SELECT @@innodb_read_only`.

Cause 2: CreateDBInstance(mariadb, db.t3.micro) with a parameter group setting require_secure_transport=1, wait for `available`, connect over TLS as the admin user and run `SELECT count(*) FROM information_schema.processlist WHERE user = '<admin>'`.

Cause 3: CreateDBInstance(mysql, db.t3.micro), wait for `available`, connect as the master user and run `CALL mysql.rds_show_configuration`.

Cause 4: CreateDBInstance(postgres, db.t3.micro, BackupRetentionPeriod=1), wait for `available`; CreateDBInstanceReadReplica from it, wait for `available`; connect to the replica endpoint and run `SELECT pg_is_in_recovery()` then `SET default_transaction_read_only = off`.

Cause 5: CreateDBInstance(mysql, db.t3.micro, BackupRetentionPeriod=1), wait for `available`; CreateDBInstanceReadReplica(SourceDBInstanceIdentifier=<source>), wait for `available`; DescribeDBInstances for both and compare Endpoint.Address.
