Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.ctas-creates-queryable-table athena.columnar-projection-reads-a-fraction-of-the-row-format-bytes athena.ctas-partitioned-by-writes-one-s3-prefix-per-partition athena.get-table-metadata-reflects-ctas-table athena.insert-into-appends-rows-visible-to-subsequent-select athena.ctas-partition-key-not-last-column-rejected athena.unload-parquet-round-trips-through-a-new-external-table athena.schemaevo-add-column-reads-null-from-older-parquet-files athena.schemaevo-cross-family-type-change-fails-instead-of-reinterpreting-bytes athena.schemaevo-dropped-column-is-recovered-by-re-adding-it athena.schemaevo-index-access-reorder-fails-while-name-based-reorder-resolves athena.schemaevo-narrowing-past-a-stored-value-fails-instead-of-truncating athena.schemaevo-widen-int-to-bigint-keeps-int32-files-readable athena.partition-projection-bounds-the-scan-with-no-metastore-partitions athena.partition-pruning-scans-only-the-selected-partition athena.iceberg-time-travel-returns-the-snapshot-before-the-row-level-update athena.runtime-cast-of-non-numeric-text-rejected-with-invalid-cast-argument athena.start-query-execution-malformed-sql-rejected athena.query-execution-syntax-error-rejected-synchronously athena.bytes-scanned-cutoff-fails-the-oversized-query athena.cancelled-scan-writes-no-result-object-and-rejects-results athena.disabled-workgroup-preserves-running-query-and-rejects-new-start-rejected
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena.ctas-creates-queryable-table|run#14|AssertionError|assert ctas_exec["Status"]["State"] == "SUCCEEDED", ctas_exec["Status"]
Issue-Key: athena.columnar-projection-reads-a-fraction-of-the-row-format-bytes|_run_sql#22|AssertionError|raise AssertionError(
Issue-Key: athena.ctas-partitioned-by-writes-one-s3-prefix-per-partition|_run_sql#17|AssertionError|raise AssertionError(
Issue-Key: athena.get-table-metadata-reflects-ctas-table|run#14|AssertionError|assert ctas_exec["Status"]["State"] == "SUCCEEDED", ctas_exec["Status"]
Issue-Key: athena.insert-into-appends-rows-visible-to-subsequent-select|_run_sql#14|AssertionError|raise AssertionError(
Issue-Key: athena.ctas-partition-key-not-last-column-rejected|run#23|AssertionError|assert reason == expected_reason, (
Issue-Key: athena.unload-parquet-round-trips-through-a-new-external-table|_run_sql#16|AssertionError|raise AssertionError(
Issue-Key: athena.schemaevo-add-column-reads-null-from-older-parquet-files|_run_sql#15|AssertionError|raise AssertionError(
Issue-Key: athena.schemaevo-cross-family-type-change-fails-instead-of-reinterpreting-bytes|_run_sql#15|AssertionError|raise AssertionError(
Issue-Key: athena.schemaevo-dropped-column-is-recovered-by-re-adding-it|_run_sql#15|AssertionError|raise AssertionError(
Issue-Key: athena.schemaevo-index-access-reorder-fails-while-name-based-reorder-resolves|_run_sql#15|AssertionError|raise AssertionError(
Issue-Key: athena.schemaevo-narrowing-past-a-stored-value-fails-instead-of-truncating|_run_sql#18|AssertionError|raise AssertionError(
Issue-Key: athena.schemaevo-widen-int-to-bigint-keeps-int32-files-readable|_run_sql#15|AssertionError|raise AssertionError(
Issue-Key: athena.partition-projection-bounds-the-scan-with-no-metastore-partitions|_run_sql#15|AssertionError|raise AssertionError(
Issue-Key: athena.partition-pruning-scans-only-the-selected-partition|_run_sql#15|AssertionError|raise AssertionError(
Issue-Key: athena.iceberg-time-travel-returns-the-snapshot-before-the-row-level-update|_run_sql#13|AssertionError|raise AssertionError(
Issue-Key: athena.runtime-cast-of-non-numeric-text-rejected-with-invalid-cast-argument|run#19|AssertionError|assert bad_state == "FAILED", (
Issue-Key: athena.start-query-execution-malformed-sql-rejected|run#13|AssertionError|assert message == expected_message, f"expected parser message {expected_message!r}, got {message!r}"
Issue-Key: athena.query-execution-syntax-error-rejected-synchronously|run#9|AssertionError|assert message == f"line 1:1: mismatched input 'SELEC'. Expecting: {_EXPECTING}", (
Issue-Key: athena.bytes-scanned-cutoff-fails-the-oversized-query|run#28|AssertionError|assert big_state == "CANCELLED", (
Issue-Key: athena.cancelled-scan-writes-no-result-object-and-rejects-results|_wait_running#19|AssertionError|raise AssertionError(
Issue-Key: athena.disabled-workgroup-preserves-running-query-and-rejects-new-start-rejected|run#31|AssertionError|assert inflight_state == "SUCCEEDED", (
Issue-Commit: c3de02674d16064d9f5c8a9be8af17d3d848d40e

# Athena's SQL engine refuses Trino/Hive statements and applies SQLite runtime semantics (10 causes: CTAS WITH, UNLOAD, SHOW PARTITIONS, MSCK REPAIR, qualified names, Iceberg, CAST, parser text, 8 MiB read cap)

> _Note: this issue was identified against the code base at commit `c3de02674d16064d9f5c8a9be8af17d3d848d40e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
Ten net-new root causes in the Athena data plane, all reached through StartQueryExecution/GetQueryExecution. Related open issues that do NOT cover these checks: 20261104T163600Z-athena-statements-are-executed-by-a-sqlite-engine.md and 20261111T040849Z-athena-s3-athena-s-sql-engine-parses-neither-ctas-with.md (the latter is filed under service athena__s3; the checks below are the ones the suite lists under `### athena` and were not marked OWNED).

Cause 1 of 10: CTAS `CREATE TABLE ... WITH (...) AS SELECT` is not parsed.
AWS: Athena (Trino engine) accepts `CREATE TABLE <db>.<tbl> WITH (format = 'PARQUET'|'TEXTFILE', external_location = 's3://.../', partitioned_by = ARRAY['<col>']) AS SELECT ...` and settles the execution SUCCEEDED (QueryExecutionState enum: QUEUED/RUNNING/SUCCEEDED/FAILED/CANCELLED), creating a queryable table whose data lands under `external_location`. Recorded behavior only for the exact table-property list; the statement form is Athena's documented CTAS syntax.
Emulator: the execution settles FAILED with Status.StateChangeReason and AthenaError.ErrorMessage of the form `near "WITH": syntax error in <the submitted statement> at offset <n>` (a SQLite parser diagnostic; the fixed text is `near "WITH": syntax error in ` + the echoed statement + ` at offset ` + an offset, the variable parts being the statement and the offset). No table is created.
Why: every client that materialises a table with CTAS — the documented way to convert a dataset to Parquet or to write partitioned output — sees a FAILED query and a parser error that names no fixable mistake in its own SQL, and downstream SELECT/GetTableMetadata over the new table then fails too.
Fix: the SQL parser/translation layer of the Athena query-execution engine in services/service-com-amazonaws-athena-amazonathena (the component StartQueryExecution hands the statement to), plus the table-creation side effect that must register the new table in the catalog and write its data to `external_location`.
Traces: athena.ctas-creates-queryable-table, athena.columnar-projection-reads-a-fraction-of-the-row-format-bytes, athena.ctas-partitioned-by-writes-one-s3-prefix-per-partition, athena.get-table-metadata-reflects-ctas-table, athena.insert-into-appends-rows-visible-to-subsequent-select.

Cause 2 of 10: the CTAS partition-layout rule is not enforced, so its refusal never names the broken rule.
AWS: in a CTAS with `partitioned_by`, the partition columns must be the LAST columns of the projection; `AS SELECT cat, amount ...` with `partitioned_by = ARRAY['cat']` is rejected with a failure whose message names that layout rule so the customer can reorder the SELECT list. Recorded behavior only for the message text; the rule is Athena's documented CTAS requirement.
Emulator: the query settles FAILED with the Cause-1 parser diagnostic (`near "WITH": syntax error in ... at offset <n>`), which says nothing about partition ordering, so a correct CTAS and an incorrectly ordered one are indistinguishable.
Why: a client cannot tell a service-side parse gap from its own layout mistake, and a rule violation that AWS reports as a named semantic error is reported as a syntax error.
Fix: CTAS semantic validation inside the Athena query-execution engine, after the statement parses (same component as Cause 1).
Traces: athena.ctas-partition-key-not-last-column-rejected.

Cause 3 of 10: `UNLOAD (SELECT ...) TO '<s3 uri>' WITH (format = '...')` is not parsed.
AWS: Athena accepts UNLOAD and settles it SUCCEEDED, writing the result files in the requested format under the TO prefix. Recorded behavior only.
Emulator: FAILED with `near "UNLOAD": syntax error in UNLOAD(<the rewritten statement>) ... at offset 0` — fixed text `near "UNLOAD": syntax error in ` plus the echoed statement and ` at offset 0`.
Why: UNLOAD is the only way to produce Parquet/ORC output from a SELECT; every staging step built on it fails, and nothing is written to S3.
Fix: the SQL parser/translation layer of the Athena query-execution engine, plus the UNLOAD writer that must emit the objects under the TO prefix.
Traces: athena.unload-parquet-round-trips-through-a-new-external-table, athena.schemaevo-add-column-reads-null-from-older-parquet-files, athena.schemaevo-cross-family-type-change-fails-instead-of-reinterpreting-bytes, athena.schemaevo-dropped-column-is-recovered-by-re-adding-it, athena.schemaevo-index-access-reorder-fails-while-name-based-reorder-resolves, athena.schemaevo-narrowing-past-a-stored-value-fails-instead-of-truncating, athena.schemaevo-widen-int-to-bigint-keeps-int32-files-readable.

Cause 4 of 10: `SHOW PARTITIONS <db>.<tbl>` is not parsed.
AWS: Athena accepts SHOW PARTITIONS and settles it SUCCEEDED, returning one row per partition (and an empty result for a projection-only table). Recorded behavior only.
Emulator: FAILED with `near "SHOW": syntax error in SHOW PARTITIONS <names> at offset 0`.
Why: partition discovery and partition-projection verification are impossible; a client gets a parser error for a statement AWS supports.
Fix: the SQL parser/statement dispatch of the Athena query-execution engine (SHOW-family statements must be served from catalog state, not handed to the row engine).
Traces: athena.partition-projection-bounds-the-scan-with-no-metastore-partitions.

Cause 5 of 10: `MSCK REPAIR TABLE <db>.<tbl>` is not parsed.
AWS: Athena accepts MSCK REPAIR TABLE and settles it SUCCEEDED, adding the partitions discovered under the table location to the catalog. Recorded behavior only.
Emulator: FAILED with `near "MSCK": syntax error in MSCK REPAIR TABLE <names> at offset 0`.
Why: the documented way to register Hive-style partitions after loading S3 data never runs, so every subsequent partition-pruned query reads nothing.
Fix: the SQL parser/statement dispatch of the Athena query-execution engine plus the catalog partition-registration side effect.
Traces: athena.partition-pruning-scans-only-the-selected-partition.

Cause 6 of 10: a qualified table name `"<db>"."<table>"` is collapsed into one quoted identifier `"<db>.<table>"` before execution.
AWS: `"db"."table"` is a two-part name: a quoted schema identifier and a quoted table identifier. Recorded behavior only.
Emulator: the statement the emulator echoes back in its own diagnostics shows the two-part name rewritten as a single quoted identifier — e.g. a submitted `FROM "db"."src"` is reported as `FROM "db.src"` — so even after the parser gaps above are closed the name would resolve against a table literally called `db.src`.
Why: any client that quotes identifiers (the documented way to use names that need quoting) addresses a table that does not exist; the resulting failure is a bogus missing-table error rather than a query over the intended table.
Fix: identifier/qualified-name handling in the Athena SQL translation layer (the same component that rewrites the statement before execution).
Traces: athena.insert-into-appends-rows-visible-to-subsequent-select, athena.ctas-partition-key-not-last-column-rejected, athena.columnar-projection-reads-a-fraction-of-the-row-format-bytes, athena.unload-parquet-round-trips-through-a-new-external-table.

Cause 7 of 10: an Iceberg table created with `CREATE TABLE ... LOCATION ... TBLPROPERTIES ('table_type'='ICEBERG')` is never registered, so the following DML cannot see it.
AWS: the bare-name Iceberg CREATE TABLE settles SUCCEEDED and the table is immediately usable by `INSERT INTO`, row-level `UPDATE`/`DELETE` and `FOR VERSION AS OF` time travel on an engine-version-3 workgroup. Recorded behavior only.
Emulator: the CREATE reports no failure, but the following `INSERT INTO "<db>"."<tbl>" VALUES (...)` settles FAILED with `no such table: <db>.<tbl>` (fixed text `no such table: ` plus the collapsed qualified name from Cause 6).
Why: an Iceberg-based client sees a table it just created reported as nonexistent, and the whole Iceberg feature set (row-level mutation, snapshots, time travel) is unreachable.
Fix: the DDL handling and catalog state of the Athena query-execution engine (Iceberg CREATE TABLE must register the table the subsequent statements resolve against).
Traces: athena.iceberg-time-travel-returns-the-snapshot-before-the-row-level-update.

Cause 8 of 10: casting non-numeric text to an integer silently succeeds instead of failing the execution.
AWS: a runtime cast of a value that is not a number to INT fails the execution: state FAILED, Status.StateChangeReason `INVALID_CAST_ARGUMENT: Cannot cast '<value>' to INT` (fixed text `INVALID_CAST_ARGUMENT: Cannot cast '` / `' to INT`, the quoted value coming from the offending row), AthenaError.ErrorCategory 2 (User — per the Smithy doc on AthenaError.ErrorCategory: 1=System, 2=User, 3=Other) and AthenaError.ErrorType 1100, and GetQueryResults is then refused.
Emulator: the query settles SUCCEEDED with no StateChangeReason — the underlying engine coerces the text to 0/NULL instead of raising.
Why: a client reading the results gets silently wrong numbers (zeros/nulls) where AWS fails the query, so bad input data is never surfaced.
Fix: cast/coercion semantics in the Athena query-execution engine's expression evaluation, and the failure path that populates Status.StateChangeReason and AthenaError.
Traces: athena.runtime-cast-of-non-numeric-text-rejected-with-invalid-cast-argument.

Cause 9 of 10: the synchronous parser refusal message omits the `Expecting:` token list.
AWS: StartQueryExecution refuses a malformed statement synchronously with InvalidRequestException whose message is `line <l>:<c>: mismatched input '<token>'. Expecting: '<token>', '<token>', ..., <query>` — the fixed part being `mismatched input '...'. Expecting: ` followed by the engine's accepted-token list; only the line/column and the offending token come from the request.
Emulator: the message stops after the offending token, e.g. `line 1:1: mismatched input 'SELEC'`, with no `. Expecting: ...` tail.
Why: the token list is what tells a client (and a SQL tool surfacing the error) what was legal at that position; a client matching or displaying AWS's message sees a truncated diagnostic.
Fix: the parser-error message formatting on the StartQueryExecution refusal path (services/service-com-amazonaws-athena-amazonathena, start_query_execution handler and its SQL front end).
Traces: athena.start-query-execution-malformed-sql-rejected, athena.query-execution-syntax-error-rejected-synchronously.

Cause 10 of 10: a hard-coded ~8 MiB per-query read cap aborts any larger scan as FAILED with an emulator-authored reason.
AWS: Athena imposes no such per-query read ceiling; a scan of a multi-megabyte table settles SUCCEEDED (or, where a workgroup cutoff applies, CANCELLED with that cutoff's reason — see the companion issue for BytesScannedCutoffPerQuery). Recorded behavior only.
Emulator: any query whose source exceeds 8388608 bytes settles FAILED with StateChangeReason `reading <db>.<table> would scan more than the 8388608 bytes this emulator reads for one query` — text that exists in no AWS response, naming the emulator itself. DataScannedInBytes is reported as 0. This also kills a query that was already admitted and running, and makes it impossible to exercise cancellation of a long scan.
Why: a client's query over an ordinary-sized dataset fails for a reason AWS never produces, and any behavior that depends on a scan lasting longer than an instant (StopQueryExecution, workgroup disable while a query is in flight) cannot be reached at all.
Fix: remove the fixed read ceiling from the Athena query-execution engine's scan path; large scans must complete, and the only size-driven abort must be the workgroup's configured BytesScannedCutoffPerQuery.
Traces: athena.bytes-scanned-cutoff-fails-the-oversized-query, athena.cancelled-scan-writes-no-result-object-and-rejects-results, athena.disabled-workgroup-preserves-running-query-and-rejects-new-start-rejected.

## Evidence
Cause 1 — expected: CTAS SUCCEEDED and a queryable table. Actual (athena.ctas-creates-queryable-table): {'State': 'FAILED', 'StateChangeReason': 'near "WITH": syntax error in CREATE TABLE <db>.<tbl> WITH (format=\'PARQUET\', external_location=\'s3://<bucket>/ctas/<tbl>/\') AS SELECT 42 AS num, \'hello\' AS txt at offset 82', 'AthenaError': {'ErrorCategory': 2, 'Retryable': False, 'ErrorMessage': <same text>}}.

Cause 2 — expected: a failure naming the partition-layout rule. Actual: 'near "WITH": syntax error in CREATE TABLE ... partitioned_by = ARRAY[\'cat\']) AS SELECT cat, amount FROM ... at offset 80'.

Cause 3 — expected: UNLOAD SUCCEEDED. Actual: FAILED, 'near "UNLOAD": syntax error in UNLOAD(SELECT id, name FROM "<db>.<src>" ORDER BY id) TO \'s3://<bucket>/unloaded/\' WITH (format = \'PARQUET\') at offset 0'.

Cause 4 — expected: SHOW PARTITIONS SUCCEEDED. Actual: FAILED, 'near "SHOW": syntax error in SHOW PARTITIONS `<db>`.`<tbl>` at offset 0'.

Cause 5 — expected: MSCK REPAIR TABLE SUCCEEDED. Actual: FAILED, 'near "MSCK": syntax error in MSCK REPAIR TABLE `<db>`.`<tbl>` at offset 0'.

Cause 6 — submitted: FROM "<db>"."<src>". Echoed by the emulator: FROM "<db>.<src>" (one identifier).

Cause 7 — expected: INSERT INTO the just-created Iceberg table SUCCEEDED. Actual: FAILED, 'no such table: <db>.<tbl>'.

Cause 8 — expected: State FAILED, StateChangeReason 'INVALID_CAST_ARGUMENT: Cannot cast '<value>' to INT', AthenaError.ErrorCategory 2, ErrorType 1100, GetQueryResults refused. Actual: State 'SUCCEEDED', StateChangeReason None.

Cause 9 — expected message: "line 1:1: mismatched input 'SELEC'. Expecting: 'ALTER', 'ANALYZE', 'CALL', 'COMMENT', 'COMMIT', 'CREATE', 'DEALLOCATE', 'DELETE', 'DENY', 'DESC', 'DESCRIBE', 'DROP', 'EXECUTE', 'EXPLAIN', 'GRANT', 'INSERT', 'MERGE', 'PREPARE', 'REFRESH', 'RESET', 'REVOKE', 'ROLLBACK', 'SET', 'SHOW', 'START', 'TRUNCATE', 'UNLOAD', 'UPDATE', 'USE', <query>". Actual: "line 1:1: mismatched input 'SELEC'".

Cause 10 — expected: the oversized scan is not aborted by the emulator. Actual: State 'FAILED' after scanning 0 bytes, reason 'reading <db>.<table> would scan more than the 8388608 bytes this emulator reads for one query'.

## Steps to Reproduce
1. Build and boot the emulator and point a botocore athena client at it.
2. Create a results bucket and a data bucket in the emulator's S3, upload a small CSV, and run `CREATE DATABASE `db`` and `CREATE EXTERNAL TABLE `db`.`src` (...) ROW FORMAT DELIMITED FIELDS TERMINATED BY ',' STORED AS TEXTFILE LOCATION 's3://<data-bucket>/src/'` via StartQueryExecution.
3. Submit `CREATE TABLE "db"."dest" WITH (format = 'PARQUET', external_location = 's3://<data-bucket>/dest/') AS SELECT id, name FROM "db"."src"` and poll GetQueryExecution: observe State FAILED with `near "WITH": syntax error ...` (Cause 1), and that the echoed statement shows `"db.src"` (Cause 6).
4. Repeat with `partitioned_by = ARRAY['cat']` and a SELECT whose partition column is first (Cause 2), with `UNLOAD (SELECT ...) TO 's3://.../out/' WITH (format = 'PARQUET')` (Cause 3), with `SHOW PARTITIONS "db"."src"` (Cause 4) and with `MSCK REPAIR TABLE "db"."src"` (Cause 5).
5. Create an Iceberg table with the bare-name `CREATE TABLE ... LOCATION ... TBLPROPERTIES ('table_type'='ICEBERG')` form on an engine-version-3 workgroup, then `INSERT INTO` it: observe `no such table` (Cause 7).
6. Query `SELECT CAST(name AS INT) FROM "db"."src"` over a row whose name is not numeric: observe SUCCEEDED instead of FAILED/INVALID_CAST_ARGUMENT (Cause 8).
7. Call StartQueryExecution with QueryString 'SELEC 1' and read the InvalidRequestException message: no `. Expecting: ...` tail (Cause 9).
8. Upload ~14 MB of CSV across three objects, define an EXTERNAL TABLE over them and run `SELECT count(*)`: observe FAILED with the `this emulator reads for one query` reason and DataScannedInBytes 0 (Cause 10).
