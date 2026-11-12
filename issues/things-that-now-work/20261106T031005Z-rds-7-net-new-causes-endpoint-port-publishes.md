Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.deep-rds-1-rds-engine-truth-mysql-constraint-lock-and-charset-violations-rejected rds.deep-rds-sql-truth-iam-token-past-expiry-rejected rds.deep-rds-sql-truth-mysql-replica-write-rejected-read-only rds.deploy-db-instance-cfn-behaves rds.engine-dialect-discriminator-query rds.postgres-custom-port-is-the-published-and-listening-port rds.mysql-master-user-lacks-super-set-global-rejected rds.mysql-require-secure-transport-plaintext-login-rejected rds.postgres-default-parameter-group-plaintext-login-rejected rds.postgres-unknown-database-login-rejected rds.pg-dialect-numeric-timestamptz-jsonb-arrays-returning-recursive-cte rds.pg-master-user-not-superuser-alter-system-rejected rds.copy-from-stdin-bulk-load-rejects-a-malformed-row rds.postgres-session-identity-matches-master-username-and-db-name rds.reboot-severs-open-session-rolls-back-uncommitted-keeps-committed rds.serializable-write-skew-rejected-repeatable-read-permits rds.repeatable-read-transaction-sees-a-stable-snapshot
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.deep-rds-1-rds-engine-truth-mysql-constraint-lock-and-charset-violations-rejected|run#6|AssertionError|assert port == _PORT, f"mysql must listen on {_PORT}; DescribeDBInstances says {port!r}"
Issue-Key: rds.deep-rds-sql-truth-iam-token-past-expiry-rejected|run#13|AssertionError|assert port == _PORT, f"postgres must listen on {_PORT}; DescribeDBInstances says {port!r}"
Issue-Key: rds.deep-rds-sql-truth-mysql-replica-write-rejected-read-only|run#12|AssertionError|assert src_port == _PORT, f"mysql must listen on {_PORT}; DescribeDBInstances says {src_port!r}"
Issue-Key: rds.deploy-db-instance-cfn-behaves|run#11|AssertionError|assert endpoint["Port"] == 3306, f"expected port 3306, got {endpoint['Port']!r}"
Issue-Key: rds.engine-dialect-discriminator-query|run#8|AssertionError|assert inst["Endpoint"]["Port"] == port, (
Issue-Key: rds.postgres-custom-port-is-the-published-and-listening-port|run#6|AssertionError|assert inst["Endpoint"]["Port"] == _PORT, (
Issue-Key: rds.mysql-master-user-lacks-super-set-global-rejected|run#5|AssertionError|assert got == [(1048576,)], f"SET SESSION must apply to the session: {got!r}"
Issue-Key: rds.mysql-require-secure-transport-plaintext-login-rejected|run#13|AssertionError|assert setting == [(1,)], f"require_secure_transport must read ON; got {setting!r}"
Issue-Key: rds.postgres-default-parameter-group-plaintext-login-rejected|run#12|AssertionError|assert stray == [(0,)], f"the refused login must leave no backend; found {stray!r}"
Issue-Key: rds.postgres-unknown-database-login-rejected|run#9|AssertionError|assert listed == [(0,)], f"the refused login must not create {missing!r}; got {listed!r}"
Issue-Key: rds.pg-dialect-numeric-timestamptz-jsonb-arrays-returning-recursive-cte|run#7|AssertionError|assert got == [(Decimal("0.3"), Decimal("0.3333"))], f"numeric arithmetic: {got!r}"
Issue-Key: rds.pg-master-user-not-superuser-alter-system-rejected|run#5|AssertionError|assert got == [(False, True, True, False, True)], (
Issue-Key: rds.copy-from-stdin-bulk-load-rejects-a-malformed-row|_rows#12|SyntaxError|cur.execute(statement)
Issue-Key: rds.postgres-session-identity-matches-master-username-and-db-name|_rows#10|SyntaxError|cur.execute(sql, params)
Issue-Key: rds.reboot-severs-open-session-rolls-back-uncommitted-keeps-committed|run#10|AssertionError|assert seen == [(1,)], f"an uncommitted row must be invisible to another session: {seen!r}"
Issue-Key: rds.serializable-write-skew-rejected-repeatable-read-permits|run#11|AssertionError|assert skewed == [(1, "white"), (2, "black")], (
Issue-Key: rds.repeatable-read-transaction-sees-a-stable-snapshot|run#18|AssertionError|assert b_committed is True, (
Issue-Commit: 1ea8394ddf89938ba9051f7092160f10eda1506a

# rds: 7 net-new causes — Endpoint.Port publishes the emulator's internal listener port, and the engine data plane returns typed columns as text, loses PostgreSQL numeric/role semantics, rejects md5()/pg_get_userbyid(), and leaks uncommitted writes across sessions

> _Note: this issue was identified against the code base at commit `1ea8394ddf89938ba9051f7092160f10eda1506a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
This issue bundles 7 net-new root causes in the rds service. Every one is grounded in a failing behavioural check that the suite report marked neither ALREADY FILED nor OWNED. Several of these checks appear in the Issue-Trace-Ids of the open issue 20261105T224225Z-rds-5-net-new-causes-the-postgresql-endpoint.md, but that issue's replay keys are the previous SyntaxError signatures for `::` casts, COPY, role DDL, server functions and `SHOW transaction_isolation`; those statements now parse, and what diverges here is different (port publication, column typing, numeric semantics, role attributes, two remaining function names, and cross-session visibility). A fixer must close all SEVEN causes below before submitting.

=== Cause 1 of 7: Endpoint.Port is the emulator's internal listener port, not the engine port AWS publishes ===
AWS: Smithy: CreateDBInstanceMessage.Port — "The port number on which the database accepts connections. Valid Values: 1150-65535. Default: RDS for MySQL - 3306, RDS for PostgreSQL - 5432, RDS for MariaDB - 3306, RDS for Oracle - 1521, RDS for SQL Server - 1433, RDS for Db2 - 50000"; Endpoint.Port — "Specifies the port that the database engine is listening on." So CreateDBInstance/DescribeDBInstances must report the engine's default port for the engine family when Port is omitted, and the REQUESTED port when Port is supplied; the same value appears in a CloudFormation AWS::RDS::DBInstance Endpoint.Port attribute.
Emulator: Endpoint.Port is a per-instance ephemeral port drawn from the host's dynamic range (observed values in the 38000-47000 band, a different one for every instance), regardless of engine and regardless of a supplied Port. A postgres instance created with Port=5439 publishes a value in that band instead of 5439, and a mysql instance publishes it instead of 3306.
Why: a client that reads Endpoint.Port gets a number that is not AWS's contract: connection strings, security-group rules, DB-proxy/parameter wiring and any assertion that "postgres listens on 5432" are all wrong, and an explicitly requested Port is silently discarded. The published port must be the modelled engine port; the emulator's own listener placement is an implementation detail that must not leak into the response.
Fix: the DB-instance endpoint projection in the CreateDBInstance/DescribeDBInstances serializers and the instance state that records Port (default per engine family, else the requested value), decoupled from whatever local socket the data plane actually binds.
Traces: rds.deep-rds-1-rds-engine-truth-mysql-constraint-lock-and-charset-violations-rejected (3306 expected, 41499 published), rds.deep-rds-sql-truth-iam-token-past-expiry-rejected (5432 vs 46491), rds.deep-rds-sql-truth-mysql-replica-write-rejected-read-only (3306 vs 44331), rds.deploy-db-instance-cfn-behaves (3306 vs 38953), rds.engine-dialect-discriminator-query (5432 vs 39897), rds.postgres-custom-port-is-the-published-and-listening-port (requested 5439 vs 40351).

=== Cause 2 of 7: every result column is sent as text, so integer and boolean columns decode to strings ===
AWS: recorded behavior only (the engine surface is a PostgreSQL / MySQL wire contract, not a Smithy one). A column's declared type travels with the result, so a driver decodes an integer column to a Python int and a boolean column to a bool: `SELECT @@session.max_heap_table_size` yields (1048576,), `SELECT @@global.require_secure_transport` yields (1,), `SELECT count(*) FROM pg_stat_activity ...` yields (0,).
Emulator: the same queries answer with the value stringified — [('1048576',)], [('1',)], [('0',)] — i.e. the row description advertises a text type for columns whose values are integral.
Why: client-side arithmetic, comparisons and ORM mapping silently break on an endpoint advertised as PostgreSQL/MySQL; `row[0] + 1` raises, `if row[0]:` is true for the string '0', and no error is ever reported so the client cannot detect it.
Fix: the engine data plane's result-set encoder / row-description metadata — carry each column's real type (integer, boolean, numeric) into the wire row description instead of emitting everything as text.
Traces: rds.mysql-master-user-lacks-super-set-global-rejected, rds.mysql-require-secure-transport-plaintext-login-rejected, rds.postgres-default-parameter-group-plaintext-login-rejected, rds.postgres-unknown-database-login-rejected.

=== Cause 3 of 7: PostgreSQL numeric is evaluated as binary floating point and numeric division truncates to an integer ===
AWS: recorded behavior only. PostgreSQL's `numeric` is exact decimal: `SELECT 0.1::numeric + 0.2::numeric` returns exactly 0.3 and `SELECT (1::numeric / 3)::numeric(10,4)` returns 0.3333; both decode to Python Decimal values.
Emulator: the cast now parses, but the arithmetic is done in binary floating point and the division is integral — the same two expressions answer 0.30000000000000004 and 0, decoded as float/int rather than Decimal.
Why: monetary and quantity arithmetic is wrong, not merely imprecise: a `numeric` division returns 0 instead of a fraction, and sums of decimal literals carry float error. A client relying on exact decimal semantics gets silently corrupted values with no error.
Fix: the engine data plane's type system and expression evaluator — implement `numeric`/`decimal` as exact decimal with scale/precision propagation (including `/` between numerics) and encode it as the numeric wire type.
Traces: rds.pg-dialect-numeric-timestamptz-jsonb-arrays-returning-recursive-cte.

=== Cause 4 of 7: the PostgreSQL master role's attributes are wrong — it is not CREATEDB ===
AWS: recorded behavior only. On an RDS for PostgreSQL instance the master user created by CreateDBInstance's MasterUsername is a non-superuser role with rolsuper=false, rolcreaterole=true, rolcreatedb=true, rolreplication=false, rolinherit=true (this is how RDS withholds superuser while leaving the master able to create roles and databases).
Emulator: pg_roles for the master user reports (false, true, false, false, true) — rolcreatedb is false, so the master user cannot create a database.
Why: a client that provisions additional databases as the master user (the documented RDS pattern, since superuser is unavailable) is refused, and privilege-scoped behaviour keyed on the master role's attribute set reads wrongly.
Fix: the master-role bootstrap in the engine data plane's PostgreSQL catalog (pg_roles/pg_authid attribute flags) performed when CreateDBInstance provisions the instance.
Traces: rds.pg-master-user-not-superuser-alter-system-rejected.

=== Cause 5 of 7: md5() and pg_get_userbyid() are still refused at parse time as syntax errors ===
AWS: recorded behavior only. PostgreSQL evaluates `md5(<text>)` (used to checksum loaded rows) and `pg_get_userbyid(<oid>)` (used to resolve a database/role owner, e.g. `SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname = current_database()`), returning one row with the value and no error.
Emulator: both are rejected by the statement parser. The client sees psycopg.errors.SyntaxError: 'syntax error at or near "md5"' and 'syntax error at or near "pg_get_userbyid"'. The fixed text is PostgreSQL's `syntax error at or near "…"` with the offending token from the submitted statement quoted — but AWS emits no error here at all, and the wrong error CLASS (42601 syntax_error) is reported for what should be a successful evaluation.
Why: a bulk-load verification path and the standard owner/identity query both fail outright on an endpoint advertised as PostgreSQL, and a client cannot tell "this dialect is unimplemented" from "my statement is malformed".
Fix: the engine data plane's PostgreSQL built-in function surface and its function-call parsing (string/hash functions and the pg_* catalog-accessor functions), backed by the catalogs the control plane already keeps.
Traces: rds.copy-from-stdin-bulk-load-rejects-a-malformed-row, rds.postgres-session-identity-matches-master-username-and-db-name.

=== Cause 6 of 7: an UNCOMMITTED write is visible to other sessions — there is no transaction isolation between connections ===
AWS: recorded behavior only. Under PostgreSQL's default READ COMMITTED (and MySQL's REPEATABLE READ), a row inserted or updated inside an open, uncommitted transaction is invisible to every other session until that transaction commits; a concurrent write-skew pair at REPEATABLE READ leaves each transaction's own row value in place ((1,'white'),(2,'black')), because each transaction reads its own snapshot.
Emulator: a second session reads the uncommitted row — `SELECT id FROM t ORDER BY id` returns [(1,), (2,)] while only row 1 is committed — and the concurrent pair ends with BOTH rows holding the same value [(1,'black'),(2,'black')], i.e. one transaction observed and overwrote the other's in-flight state. No snapshot is taken per transaction; writes land in shared state immediately.
Why: this is a silent correctness failure of the strongest guarantee a database makes. A client reads data that may never be committed, a rollback cannot undo what other sessions already consumed, and the whole isolation-level contract (including the SERIALIZABLE refusal that is supposed to prevent write skew) is unreachable because even REPEATABLE READ does not isolate.
Fix: the engine data plane's transaction manager and storage — per-transaction snapshots with writes buffered until COMMIT, and per-session visibility derived from that snapshot rather than from shared mutable state.
Traces: rds.reboot-severs-open-session-rolls-back-uncommitted-keeps-committed, rds.serializable-write-skew-rejected-repeatable-read-permits.

=== Cause 7 of 7: a COMMITTED update from a second session is not visible to a later session — the commit is lost ===
AWS: recorded behavior only. Once a session commits `UPDATE t SET v = '<new>' WHERE id = <k>`, a freshly opened session reads the new value; a commit is durable and globally visible.
Emulator: after the second session commits its update, a fresh session still reads the pre-update value ([('first',)]) — the committed write never becomes visible, so the update is lost rather than merely delayed.
Why: committed data is dropped. This is unrecoverable data loss from the client's point of view: the COMMIT returned success, and the value is gone.
Fix: the engine data plane's transaction commit path — apply a committing transaction's buffered writes to the shared store before acknowledging COMMIT, and make them visible to sessions opened afterwards.
Traces: rds.repeatable-read-transaction-sees-a-stable-snapshot.

## Evidence
Cause 1: AssertionError: mysql must listen on 3306; DescribeDBInstances says 41499 | postgres must listen on 5432; DescribeDBInstances says 46491 | mysql must listen on 3306; DescribeDBInstances says 44331 | expected port 3306, got 38953 | postgres must be reachable on its default port 5432; the endpoint advertises 39897 | Endpoint.Port must be the requested 5439; got 40351. Expected: 3306 for mysql/mariadb, 5432 for postgres, 1433 SQL Server, 1521 Oracle, 50000 Db2, or the requested Port when supplied.
Cause 2: AssertionError: SET SESSION must apply to the session: [('1048576',)] (expected [(1048576,)]); require_secure_transport must read ON; got [('1',)] (expected [(1,)]); the refused login must leave no backend; found [('0',)] (expected [(0,)]); the refused login must not create 'nosuch_<id>'; got [('0',)] (expected [(0,)]).
Cause 3: AssertionError: numeric arithmetic: [(0.30000000000000004, 0)]; expected [(Decimal('0.3'), Decimal('0.3333'))] for SELECT 0.1::numeric + 0.2::numeric, (1::numeric / 3)::numeric(10,4).
Cause 4: AssertionError: the master user must be NOSUPERUSER CREATEROLE CREATEDB NOREPLICATION INHERIT: [(False, True, False, False, True)] — rolcreatedb came back false.
Cause 5: psycopg.errors.SyntaxError: syntax error at or near "md5"; psycopg.errors.SyntaxError: syntax error at or near "pg_get_userbyid". AWS returns a row and no error for both.
Cause 6: AssertionError: an uncommitted row must be invisible to another session: [(1,), (2,)]; AssertionError: the write skew must actually have HAPPENED under REPEATABLE READ -- both rows swapped colour ... An independent session reads [(1, 'black'), (2, 'black')] (expected [(1, 'white'), (2, 'black')]).
Cause 7: AssertionError: the second session did not actually commit its update — a fresh session still reads [('first',)].

## Steps to Reproduce
Cause 1: CreateDBInstance(Engine=mysql, DBInstanceClass=db.t3.micro, AllocatedStorage=20, MasterUsername/Password set) with no Port, poll to available, then DescribeDBInstances and read Endpoint.Port — it must be 3306. Repeat with Engine=postgres (must be 5432) and with Engine=postgres, Port=5439 (must be 5439). Also deploy an AWS::RDS::DBInstance with Engine=mysql via CloudFormation and read the stack's Endpoint.Port output.
Cause 2: on an available mysql instance, open a pymysql session as the master user and run `SET SESSION max_heap_table_size = 1048576; SELECT @@session.max_heap_table_size` and `SELECT @@global.require_secure_transport` — the values must decode as ints. On a postgres instance run `SELECT count(*) FROM pg_stat_activity WHERE datname = '<db>'` via psycopg — it must decode as an int.
Cause 3: on a postgres instance run `SELECT 0.1::numeric + 0.2::numeric, (1::numeric / 3)::numeric(10,4)` — expect (Decimal('0.3'), Decimal('0.3333')).
Cause 4: on a postgres instance run `SELECT rolsuper, rolcreaterole, rolcreatedb, rolreplication, rolinherit FROM pg_roles WHERE rolname = '<MasterUsername>'` — expect (false, true, true, false, true).
Cause 5: on a postgres instance run `SELECT md5('abc')` and `SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname = current_database()`.
Cause 6: on a postgres instance, session A: BEGIN; INSERT a second row; do not commit. Session B: SELECT id FROM t ORDER BY id — must see only the committed row. Then run the classic write-skew pair at REPEATABLE READ (A sets row 1 to the colour of row 2, B sets row 2 to the colour of row 1, both commit) and read the table from a third session — expect both rows swapped, not both equal.
Cause 7: on a postgres instance, session B: UPDATE t SET v='second' WHERE id=1; COMMIT. Open a fresh session and SELECT v FROM t WHERE id=1 — it must read 'second'.
