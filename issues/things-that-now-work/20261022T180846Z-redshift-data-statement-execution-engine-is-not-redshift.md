Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift-data.absent-database-fails-async-while-absent-workgroup-is-rejected redshift-data.list-databases-schemas-tables-roundtrip redshift-data.cancel-statement-running-then-finished-rejected redshift-data.get-statement-result-pagination redshift-data.session-persistence-across-statements redshift-data.cancel-running-statement-aborts-and-terminal-cancel-rejected
Issue-Shape-Id: com.amazonaws.redshiftdata#RedshiftData
Issue-Key: redshift-data.absent-database-fails-async-while-absent-workgroup-is-rejected|run#12|AssertionError|assert failed["Status"] == "FAILED", (
Issue-Key: redshift-data.list-databases-schemas-tables-roundtrip|run#6|AssertionError|assert final["Status"] == "FINISHED", final.get("Error")
Issue-Key: redshift-data.cancel-statement-running-then-finished-rejected|run#10|AssertionError|assert holder_final["Status"] == "FINISHED", holder_final.get("Error")
Issue-Key: redshift-data.get-statement-result-pagination|run#8|AssertionError|assert insert_final["Status"] == "FINISHED", insert_final.get("Error")
Issue-Key: redshift-data.session-persistence-across-statements|run#8|AssertionError|assert first_final["Status"] == "FINISHED", first_final.get("Error")
Issue-Key: redshift-data.cancel-running-statement-aborts-and-terminal-cancel-rejected|run#13|AssertionError|assert running["Status"] == "STARTED", (
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# redshift-data statement execution engine is not Redshift-compatible: absent database silently FINISHES, CREATE SCHEMA / LOCK TABLE / :: casts are syntax errors, session temp tables vanish, and no statement is ever observable as STARTED (6 causes)

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Replay of simparity-behavioral-aws-v1 against the redshift-data emulator produced 15 failures that are neither ALREADY FILED (BatchExecuteStatement, ListDatabases stubs) nor OWNED. This issue carries the SIX causes that live in the statement-execution path (the submit/execute/engine side); the other five are filed separately as the "redshift-data Data API response and validation surface" issue. This bundle carries SIX causes; all six must be closed before submitting.

=== Cause 1 of 6: a statement submitted against a database that does not exist runs to FINISHED instead of failing asynchronously ===
AWS: Smithy (com-amazonaws-redshiftdata-redshiftdata): ExecuteStatementInput.Database is an optional plain String, so the model declares nothing here — recorded behavior (check header: MEASURED us-west-2 2026-09-11): ExecuteStatement naming a live workgroup and a database that does not exist is ACCEPTED (HTTP 200 with an Id) and only later reaches DescribeStatement Status `FAILED` with Error exactly 'FATAL: database "<name>" does not exist'; GetStatementResult on it is then refused ResourceNotFoundException. The asynchronous failure is the whole contract: a nonexistent database is NOT a synchronous rejection.
Emulator: the submit is accepted (correct), but the statement reaches Status `FINISHED` and the SELECT appears to have run — the Database member is ignored, never resolved against namespace state, and no Error is ever set.
Why: a caller that typoed its database name is told its query succeeded. Every poll-for-terminal-state client (the documented Data API pattern) reads FINISHED and then reads results from a database it never reached, instead of surfacing the engine's FATAL error.
Fix: ExecuteStatement's handler plus the statement state machine — resolve Database against the target namespace's databases when the statement is dequeued, and drive the statement to FAILED with the engine's 'FATAL: database "<name>" does not exist' error rather than running it anyway.
Traces: redshift-data.absent-database-fails-async-while-absent-workgroup-is-rejected.

=== Cause 2 of 6: CREATE SCHEMA is a syntax error ===
AWS: recorded behavior only (the Data API runs whatever SQL Redshift accepts; the model constrains only Sql's shape). Redshift accepts `CREATE SCHEMA <name>`, the statement reaches FINISHED, and the new schema is then visible to ListSchemas/ListTables.
Emulator: the statement goes terminal with Error 'near "SCHEMA": syntax error in CREATE SCHEMA sim2real_schema_a_<id> at offset 7' — the engine's parser does not implement CREATE SCHEMA at all (the message shape is SQLite's, not Redshift's).
Why: no client can create or organise schemas, and the catalogue operations (ListSchemas/ListTables) can never be exercised against a schema the caller made.
Fix: the SQL engine / statement execution component behind ExecuteStatement — support CREATE SCHEMA (and the schema-qualified names that follow from it).
Traces: redshift-data.list-databases-schemas-tables-roundtrip.

=== Cause 3 of 6: LOCK TABLE ... IN ACCESS EXCLUSIVE MODE is a syntax error ===
AWS: recorded behavior only. Redshift (Postgres-derived) accepts `LOCK TABLE <t> IN ACCESS EXCLUSIVE MODE;` and the statement FINISHES; the check uses it to hold a lock so a second statement is still RUNNING when CancelStatement is issued.
Emulator: Error 'near "LOCK": syntax error in  LOCK TABLE sim2real_lock_<id> IN ACCESS EXCLUSIVE MODE; at offset 1'.
Why: beyond the missing statement type, it makes the whole cancel-while-running contract untestable — a client cannot serialise writers, and lock-based migration scripts fail outright.
Fix: the SQL engine behind ExecuteStatement — accept LOCK TABLE (at minimum as a no-op/serialising statement that FINISHES) instead of failing to parse it.
Traces: redshift-data.cancel-statement-running-then-finished-rejected.

=== Cause 4 of 6: the `::` cast operator is not tokenised ===
AWS: recorded behavior only. Redshift accepts the Postgres cast syntax `<expr>::VARCHAR` (used here inside `INSERT ... WITH RECURSIVE seq(n) AS (...) SELECT ..., LEFT(REPEAT(MD5(((a.n - 1) * 300 + b.n)::VARCHAR), 63), 2000) ...`), and the insert FINISHES with 90000 rows so GetStatementResult pagination can be exercised.
Emulator: Error 'unrecognized token: ":" in INSERT INTO sim2real_page_<id> ... ::VARCHAR ... at offset 202' — the engine rejects the cast operator.
Why: `::` is pervasive in Redshift SQL; every statement using it fails, and here it also blocks the only check that verifies GetStatementResult NextToken paging over a large result set.
Fix: the SQL engine behind ExecuteStatement — support the `::` cast operator (and the Redshift type names it casts to).
Traces: redshift-data.get-statement-result-pagination.

=== Cause 5 of 6: a session's temporary table does not survive to the next statement in the same session ===
AWS: Smithy: ExecuteStatementInput.SessionKeepAliveSeconds (range 0..86400) and SessionId (UUID) exist precisely so consecutive statements share one database session; ExecuteStatementOutput.SessionId returns the session to reuse — recorded behavior: a `CREATE TEMP TABLE` in the first statement is visible to the next statement submitted with that SessionId, for the keep-alive window.
Emulator: the first statement of the session already fails with Error 'no such table: sess_tmp_<id>' — the session's temporary objects are not retained between statements (each statement appears to get a fresh connection/state).
Why: sessions are the Data API's only way to do multi-statement work (temp tables, SET, transactions). A client that opens a session and builds up state gets errors for objects it just created.
Fix: the session/statement state component — bind a statement submitted with SessionId to the session's own persistent connection/state so temporary objects and session settings live for the keep-alive window.
Traces: redshift-data.session-persistence-across-statements.

=== Cause 6 of 6: a deliberately long-running query goes terminal FAILED immediately, so no statement is ever observable as STARTED ===
AWS: Smithy: StatementStatusString enumerates SUBMITTED, PICKED, STARTED, FINISHED, ABORTED, FAILED — recorded behavior: a 5-way cross join over 640 rows (640^5 rows) stays in STARTED long enough for CancelStatement to be issued mid-flight, answers Status true, and the statement then lands ABORTED with Error ending 'cancelled on user's request'; a subsequent CancelStatement on it is refused ValidationException/400 'Could not cancel a query that is already in ABORTED state with ID: <id>'.
Emulator: DescribeStatement on that statement reports `FAILED` at the first poll after SUBMITTED — the query never enters STARTED, so none of the cancel-path behaviour (Status true, ABORTED, the engine's cancellation Error, the double-cancel refusal) can be reached. The statement's own Error was not surfaced by the check, so the immediate failure may share a root cause with Causes 2-4; whether it does or not, the statuses AWS exposes are not produced. Hypothesis: statements are executed synchronously at submit time, which would make STARTED unobservable for any query.
Why: CancelStatement is unusable in practice — a client cannot cancel work because no statement is ever in a cancellable state, and a long query is reported as failed rather than running.
Fix: the statement execution/lifecycle component — run statements asynchronously through SUBMITTED/PICKED/STARTED to a terminal state, and make a long-running query cancellable (CancelStatement -> Status true, terminal ABORTED).
Traces: redshift-data.cancel-running-statement-aborts-and-terminal-cancel-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, redshift-data 6/24 (25%), 18 failed (3 of them ALREADY FILED: BatchExecuteStatement x2, ListDatabases x1).

Cause 1 — check_redshift_data_absent_database_fails_async_while_absent_workgroup_is_rejected.py:204
  expected: DescribeStatement Status 'FAILED', Error 'FATAL: database "sim2real_behav_nodb_<id>" does not exist'
  actual:   AssertionError: a nonexistent database must surface ASYNCHRONOUSLY as FAILED, got 'FINISHED'

Cause 2 — list-databases-schemas-tables-roundtrip
  expected: CREATE SCHEMA sim2real_schema_a_<id> -> Status FINISHED
  actual:   AssertionError: near "SCHEMA": syntax error in CREATE SCHEMA sim2real_schema_a_c38e477bc65d at offset 7

Cause 3 — cancel-statement-running-then-finished-rejected
  expected: LOCK TABLE <t> IN ACCESS EXCLUSIVE MODE; -> Status FINISHED
  actual:   AssertionError: near "LOCK": syntax error in  LOCK TABLE sim2real_lock_b1da73247ecd IN ACCESS EXCLUSIVE MODE; at offset 1

Cause 4 — get-statement-result-pagination
  expected: INSERT ... (expr)::VARCHAR ... -> Status FINISHED
  actual:   AssertionError: unrecognized token: ":" in INSERT INTO sim2real_page_b465521dfa18 (id, payload) WITH RECURSIVE seq(n) AS (...) SELECT (a.n - 1) * 300 + b.n, LEFT(REPEAT(MD5(((a.n - 1) * 300 + b.n)::VARCHAR), 63), 2000) FROM seq a CROSS JOIN seq b at offset 202

Cause 5 — session-persistence-across-statements
  expected: first statement of the session FINISHES and its temp table is visible to the next statement with the same SessionId
  actual:   AssertionError: no such table: sess_tmp_b4a879067125

Cause 6 — check_redshift_data_cancel_running_statement_aborts_and_terminal_cancel_rejected.py:200
  expected: DescribeStatement Status 'STARTED' while the 640^5-row cross join runs; CancelStatement -> Status true; terminal 'ABORTED'
  actual:   AssertionError: the 640^5-row query must still be running when the cancel is issued, but it was 'FAILED'

Model corroboration (services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json): StatementStatusString = [SUBMITTED, PICKED, STARTED, FINISHED, ABORTED, FAILED]; ExecuteStatementInput.Database optional String; .SessionKeepAliveSeconds range 0..86400; .SessionId UUID; ExecuteStatementOutput.SessionId present.

## Steps to Reproduce
1. Clone the repo, boot the emulator, create a Redshift Serverless namespace + workgroup (both are implemented) and point a botocore redshift-data client at the emulator.
2. Cause 1: execute_statement(WorkgroupName=wg, Database='does-not-exist', Sql='select 1') -> 200 with an Id (correct), then poll describe_statement: expect FAILED with Error 'FATAL: database "does-not-exist" does not exist'; observe FINISHED.
3. Cause 2: execute_statement(Sql='CREATE SCHEMA s1') -> expect FINISHED; observe FAILED with 'near "SCHEMA": syntax error'.
4. Cause 3: execute_statement(Sql='LOCK TABLE t IN ACCESS EXCLUSIVE MODE;') -> expect FINISHED; observe 'near "LOCK": syntax error'.
5. Cause 4: execute_statement(Sql="select md5('x')::varchar") -> expect FINISHED; observe 'unrecognized token: ":"'.
6. Cause 5: execute_statement(..., SessionKeepAliveSeconds=90, Sql='create temporary table tmp1(i int)'), take SessionId from the response, then execute_statement(SessionId=..., Sql='insert into tmp1 values (1)') -> expect both FINISHED; observe 'no such table: tmp1'.
7. Cause 6: build a 640-row table and submit 'select count(*) from t a, t b, t c, t d, t e'; poll describe_statement immediately -> expect STARTED (then CancelStatement -> Status true, terminal ABORTED); observe FAILED on the first poll.
8. Re-replay every check id in trace_ids to confirm the fix.
