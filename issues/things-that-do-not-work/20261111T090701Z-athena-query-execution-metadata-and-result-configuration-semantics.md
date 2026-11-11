Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.bytes-scanned-cutoff-fails-the-oversized-query athena.result-reuse-respects-query-identity athena.result-reuse-serves-stale-rows-after-the-source-object-changes athena.concurrent-identical-queries-result-reuse-invariant athena.expected-bucket-owner-mismatch-fails-the-query-and-writes-nothing athena.failed-query-error-classification-distinguishes-causes athena.query-statistics-reflect-actual-scan athena.query-results-typed-column-metadata athena.group-by-aggregation-matches-independently-computed-values athena.window-functions-rank-and-partition-total-per-row athena.create-database-ddl-populates-glue-catalog-location athena.query-execution-external-table-s3-roundtrip
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena.result-reuse-respects-query-identity|run#22|AssertionError|assert info2 is not None and info2["ReusedPreviousResult"] is True, (
Issue-Key: athena.result-reuse-serves-stale-rows-after-the-source-object-changes|run#29|AssertionError|assert reused_seen, (
Issue-Key: athena.concurrent-identical-queries-result-reuse-invariant|run#33|AssertionError|assert ninth_info is not None and ninth_info["ReusedPreviousResult"] is True, (
Issue-Key: athena.expected-bucket-owner-mismatch-fails-the-query-and-writes-nothing|run#37|AssertionError|assert bad_state == "FAILED", (
Issue-Key: athena.failed-query-error-classification-distinguishes-causes|run#8|AssertionError|assert isinstance(err.get("ErrorType"), int), (
Issue-Key: athena.query-statistics-reflect-actual-scan|run#10|AssertionError|assert stats["EngineExecutionTimeInMillis"] > 0, (
Issue-Key: athena.query-results-typed-column-metadata|run#11|AssertionError|assert types == {
Issue-Key: athena.group-by-aggregation-matches-independently-computed-values|run#22|AssertionError|assert types == {"category": "varchar", "n": "bigint", "total": "bigint",
Issue-Key: athena.window-functions-rank-and-partition-total-per-row|run#24|AssertionError|assert types == [("cat", "varchar"), ("amount", "bigint"), ("rn", "bigint"),
Issue-Key: athena.create-database-ddl-populates-glue-catalog-location|run#20|AssertionError|assert glue_db["LocationUri"] == location, (
Issue-Key: athena.query-execution-external-table-s3-roundtrip|run#35|AssertionError|assert message_head == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Related: 20261111T090701Z-athena-s-sql-engine-refuses-trino-hive-statements.md
Issue-Commit: c3de02674d16064d9f5c8a9be8af17d3d848d40e

# Athena query-execution metadata and result-configuration semantics diverge (8 causes: BytesScannedCutoffPerQuery, result reuse, ExpectedBucketOwner, AthenaError.ErrorType, EngineExecutionTimeInMillis, ColumnInfo.Type, CREATE DATABASE LOCATION, GetTableMetadata message)

> _Note: this issue was identified against the code base at commit `c3de02674d16064d9f5c8a9be8af17d3d848d40e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryExecution

## Description
Eight net-new root causes in how the emulator reports and honours query-execution configuration and metadata. Related open issues that do NOT cover these checks: 20261104T163600Z-athena-result-and-statistics-representation-is-wrong-boolean.md, 20261111T064644Z-athena-getqueryexecution-responses-omit-recorded-members-queryexecutiondetail-managed.md, 20261108T041854Z-athena-getqueryresults-omits-columninfo-catalog-schema-table-updatecount.md and 20261104T082656Z-athena-glue-gettablemetadata-s-metadataexception-for-a-dropped.md — the checks below are listed under `### athena` and were not marked OWNED by any of them.

Cause 1 of 8: WorkGroupConfiguration.BytesScannedCutoffPerQuery is not enforced.
AWS: Smithy WorkGroupConfiguration.BytesScannedCutoffPerQuery is a long with @range min 10000000. When a query in that workgroup would scan more than the cutoff, Athena aborts it: Status.State CANCELLED (QueryExecutionState enum QUEUED/RUNNING/SUCCEEDED/FAILED/CANCELLED), Status.StateChangeReason exactly `Bytes scanned limit was exceeded` (entirely fixed text), Statistics.DataScannedInBytes exactly equal to the configured cutoff, and GetQueryResults then refused with InvalidRequestException and the message `Could not find results` (fixed text). An under-cutoff query in the same workgroup still SUCCEEDS.
Emulator: the configured cutoff is stored and echoed by GetWorkGroup, but nothing enforces it — the oversized query settles FAILED with the emulator's own 8 MiB read-cap reason (filed as the companion issue's Cause 10) and DataScannedInBytes 0, never CANCELLED and never with the cutoff reason.
Why: a client that caps spend per query with BytesScannedCutoffPerQuery cannot distinguish a cap hit (CANCELLED, retry with a narrower query) from a real query failure, and the reported scanned bytes do not identify the limit that was hit.
Fix: the Athena query-execution engine's scan accounting plus the workgroup-configuration enforcement path, and the terminal-state/StateChangeReason writer in the GetQueryExecution state (services/service-com-amazonaws-athena-amazonathena).
Traces: athena.bytes-scanned-cutoff-fails-the-oversized-query.

Cause 2 of 8: ResultReuseConfiguration is ignored — ReusedPreviousResult is always false.
AWS: Smithy ResultReuseByAgeConfiguration has Enabled (required boolean) and MaxAgeInMinutes (@range 0-10080, default 60); QueryExecutionStatistics.ResultReuseInformation.ReusedPreviousResult is a required boolean that is true when a previous result was reused. Re-running an identical query inside the MaxAgeInMinutes window with reuse enabled reports ReusedPreviousResult true and serves the earlier (possibly stale) rows.
Emulator: every execution reports {'ReusedPreviousResult': False}, including a ninth identical query run after eight identical ones have already SUCCEEDED inside MaxAgeInMinutes=60, and across three sequential identical reruns.
Why: a client relying on result reuse for cost and latency can neither observe nor get reuse; code that branches on ReusedPreviousResult (e.g. to decide whether rows may be stale) always takes the no-reuse branch.
Fix: result-reuse lookup in the Athena query-execution engine (keyed on the query identity and the reuse window) and the ResultReuseInformation serializer on the GetQueryExecution response.
Traces: athena.result-reuse-respects-query-identity, athena.result-reuse-serves-stale-rows-after-the-source-object-changes, athena.concurrent-identical-queries-result-reuse-invariant.

Cause 3 of 8: ResultConfiguration.ExpectedBucketOwner is not consulted.
AWS: Smithy ResultConfiguration.ExpectedBucketOwner is an AwsAccountId (@length 12, pattern ^[0-9]+$); its doc states that if the value does not match the actual owner of the output bucket, the call fails with a permissions error. The query settles FAILED with Status.StateChangeReason `Access denied when writing output to url: <output-location><query-execution-id>.csv . Please ensure you are allowed to access the S3 bucket. If specifying an expected bucket owner, confirm the bucket is owned by the expected account. If you are encrypting query results with KMS key, please ensure you are allowed to access your KMS key` (only the URL is request/state-derived; the rest is fixed text), AthenaError.ErrorCategory 2 (User, per the Smithy doc 1=System/2=User/3=Other), nothing is written under the output prefix for that execution id, and GetQueryResults is refused with InvalidRequestException `Query did not finish successfully. Final query state: FAILED`.
Emulator: with ExpectedBucketOwner set to an account that does not own the output bucket, and a workgroup with EnforceWorkGroupConfiguration=false so the client setting is in force, the identical query settles SUCCEEDED with an empty StateChangeReason.
Why: the guard a client uses to prevent writing results into a bucket that has been taken over by another account is silently inert, so a misdirected write is reported as success.
Fix: output-location ownership validation on the result-writing path of the Athena query-execution engine, and the FAILED-state/AthenaError population it must produce.
Traces: athena.expected-bucket-owner-mismatch-fails-the-query-and-writes-nothing.

Cause 4 of 8: AthenaError.ErrorType is absent on a failed query.
AWS: Smithy AthenaError.ErrorType is an integer (@range 0-9999) that gives the specific Athena error code alongside ErrorCategory; a failed query carries it (e.g. 1100 for an invalid cast argument).
Emulator: Status.AthenaError carries ErrorCategory, Retryable and ErrorMessage but ErrorType is None/absent for every failure class.
Why: ErrorType is the machine-readable discriminator between failure causes within a category; a client that routes on it (retry vs. fix-the-SQL vs. fix-the-data) cannot classify the failure at all.
Fix: the AthenaError construction on the query-failure path and the GetQueryExecution Status serializer.
Traces: athena.failed-query-error-classification-distinguishes-causes.

Cause 5 of 8: Statistics.EngineExecutionTimeInMillis is always 0.
AWS: Smithy QueryExecutionStatistics.EngineExecutionTimeInMillis is a long — the number of milliseconds the query took to execute; a query that really ran reports a value greater than 0, and TotalExecutionTimeInMillis is at least that value.
Emulator: a SUCCEEDED scan over real S3 objects reports EngineExecutionTimeInMillis 0.
Why: any client or dashboard that measures query cost/latency from Statistics reads zero engine time for every query, and the internal consistency AWS guarantees (Total >= Engine) is not observable.
Fix: timing capture in the Athena query-execution engine and the Statistics serializer on GetQueryExecution.
Traces: athena.query-statistics-reflect-actual-scan.

Cause 6 of 8: ColumnInfo.Type reports the storage engine's types instead of Athena's engine type names.
AWS: Smithy ColumnInfo has required Name and Type plus Precision/Scale, and Type is the Athena (Trino) data type of the column. A DECIMAL(10,2) column projects as Type `decimal` with Precision 10 and Scale 2; `count(*)`, `sum(<bigint>)`, `min(<bigint>)`, `max(<bigint>)` project as `bigint`; `row_number()` projects as `bigint`.
Emulator: ColumnInfo.Type is derived from the underlying SQLite column affinity — a DECIMAL(10,2) column is reported as `double`, count/sum/min/max over a bigint column are reported as `integer`, and `row_number()` is reported as `integer`.
Why: ResultSetMetadata is the only type information a JDBC/ODBC client or a typed deserializer has; a `double` where AWS says `decimal` loses the documented precision/scale contract, and an `integer` where AWS says `bigint` overflows for values a bigint column legitimately holds.
Fix: the type-mapping layer that builds ResultSetMetadata.ColumnInfo in the Athena GetQueryResults serializer (it must map engine/aggregate result types to Athena type names, not pass through storage affinities).
Traces: athena.query-results-typed-column-metadata, athena.group-by-aggregation-matches-independently-computed-values, athena.window-functions-rank-and-partition-total-per-row.

Cause 7 of 8: `CREATE DATABASE ... LOCATION 's3://bucket/prefix/'` loses the trailing slash in the Glue LocationUri.
AWS: the LOCATION given in Athena's CREATE DATABASE is stored verbatim as the Glue database's LocationUri, including its trailing slash. Recorded behavior only.
Emulator: the Glue database's LocationUri comes back with the trailing slash stripped — `s3://<bucket>/db` for a submitted `s3://<bucket>/db/`.
Why: a client that composes table locations by appending to the database LocationUri, or that compares the value it submitted with the value stored, gets a path that differs from what it asked for.
Fix: the CREATE DATABASE DDL handler in the Athena query-execution engine where it writes the database location into catalog state (no normalisation of the submitted URI).
Traces: athena.create-database-ddl-populates-glue-catalog-location.

Cause 8 of 8: GetTableMetadata's MetadataException for a dropped table carries emulator-authored text instead of the wrapped AmazonDataCatalog error.
AWS: GetTableMetadata for a table that no longer exists is refused with HTTP 400, error code MetadataException, and a message that wraps the underlying catalog error: `Entity Not Found (Service: AmazonDataCatalog; Status Code: 400; Error Code: EntityNotFoundException; Request ID: <uuid>; Proxy: null)` — the fixed text is everything but the request id, which is per-run.
Emulator: the same call is refused with code MetadataException and HTTP 400, but the message is just `Entity Not Found`, with no `(Service: ...; Status Code: ...; Error Code: ...; Request ID: ...; Proxy: null)` wrapper at all.
Why: the wrapper is what tells a client which underlying catalog raised the error and with which code; a client that parses the wrapped Error Code to distinguish a missing entity from another metadata failure has nothing to parse.
Fix: the GetTableMetadata refusal path in services/service-com-amazonaws-athena-amazonathena (the message must be the wrapped AmazonDataCatalog error, not a bare phrase). Note the per-run request id is not part of the requirement; only the wrapper's fixed text and the Error Code are.
Traces: athena.query-execution-external-table-s3-roundtrip.

## Evidence
Cause 1 — expected: State CANCELLED, StateChangeReason 'Bytes scanned limit was exceeded', DataScannedInBytes == BytesScannedCutoffPerQuery (10000000). Actual: "a scan over 14066670 bytes in a workgroup capped at 10000000 must be aborted as CANCELLED, got 'FAILED' after scanning 0 bytes (reason 'reading <db>.<table> would scan more than the 8388608 bytes this emulator reads for one query')".

Cause 2 — expected: ReusedPreviousResult True on an identical rerun inside MaxAgeInMinutes=60. Actual: {'ReusedPreviousResult': False} on the ninth identical execution, and on all three sequential reruns: [(False, 65, ...), (False, 65, ...), (False, 65, ...)].

Cause 3 — expected: State FAILED with StateChangeReason 'Access denied when writing output to url: <out><qid>.csv . Please ensure you are allowed to access the S3 bucket. ...', AthenaError.ErrorCategory 2, no object written, GetQueryResults refused with 'Query did not finish successfully. Final query state: FAILED'. Actual: "with ExpectedBucketOwner=111122223333 (which does not own <results-bucket>) the identical query must reach FAILED, got 'SUCCEEDED' (reason '')".

Cause 4 — expected: isinstance(AthenaError['ErrorType'], int). Actual: 'missing-column: AthenaError.ErrorType must be an integer error code, got None'.

Cause 5 — expected: Statistics['EngineExecutionTimeInMillis'] > 0. Actual: 'a query that really ran must report EngineExecutionTimeInMillis > 0, got 0'.

Cause 6 — expected ColumnInfo.Type map {'id': 'integer', 'name': 'varchar', 'amount': 'double', 'active': 'boolean', 'dec_col': 'decimal'}; actual {'id': 'integer', 'name': 'varchar', 'amount': 'double', 'active': 'boolean', 'dec_col': 'double'}. Expected {'category': 'varchar', 'n': 'bigint', 'total': 'bigint', 'lo': 'bigint', 'hi': 'bigint'}; actual {'category': 'varchar', 'n': 'integer', 'total': 'integer', 'lo': 'integer', 'hi': 'integer'}. Expected row_number() 'bigint'; actual [('cat', 'varchar'), ('amount', 'bigint'), ('rn', 'integer'), ('cat_total', 'integer')].

Cause 7 — expected Glue LocationUri 's3://<bucket>/db/'; actual 's3://<bucket>/db'.

Cause 8 — expected message head 'Entity Not Found (Service: AmazonDataCatalog; Status Code: 400; Error Code: EntityNotFoundException'; actual 'Entity Not Found'.

## Steps to Reproduce
1. Build and boot the emulator and point a botocore athena client (plus s3 and sts clients) at it.
2. Cause 1: create a workgroup with Configuration {ResultConfiguration.OutputLocation, EnforceWorkGroupConfiguration: true, BytesScannedCutoffPerQuery: 10000000}; upload ~14 MB of CSV in three objects, define an EXTERNAL TABLE over them, run `SELECT count(*)` in that workgroup and poll GetQueryExecution — observe FAILED with the emulator read-cap reason and DataScannedInBytes 0 instead of CANCELLED / 'Bytes scanned limit was exceeded' / DataScannedInBytes 10000000.
3. Cause 2: run the same `SELECT` twice with ResultReuseConfiguration={'ResultReuseByAgeConfiguration': {'Enabled': True, 'MaxAgeInMinutes': 60}} and read Statistics.ResultReuseInformation on the second execution — ReusedPreviousResult is False.
4. Cause 3: create a workgroup with EnforceWorkGroupConfiguration=false; run a SELECT with ResultConfiguration {'OutputLocation': 's3://<bucket>/results/', 'ExpectedBucketOwner': '111122223333'} where that account does not own the bucket — the query SUCCEEDS.
5. Cause 4: run any failing query (e.g. a SELECT naming a missing column) and read Status.AthenaError — ErrorType is absent.
6. Cause 5: run a SELECT over a real EXTERNAL TABLE and read Statistics.EngineExecutionTimeInMillis — 0.
7. Cause 6: project an INT, STRING, DOUBLE, BOOLEAN and DECIMAL(10,2) column and read ResultSetMetadata.ColumnInfo (decimal reported as double); then run `SELECT category, count(*) AS n, sum(amount) AS total, min(amount) AS lo, max(amount) AS hi ... GROUP BY category` over a bigint column, and a `row_number() OVER (...)` query (aggregates/window reported as integer).
8. Cause 7: run `CREATE DATABASE `db` LOCATION 's3://<bucket>/db/'` through StartQueryExecution, then read the Glue database's LocationUri — the trailing slash is gone.
9. Cause 8: create an EXTERNAL TABLE, drop it, then call GetTableMetadata for it — the MetadataException message is 'Entity Not Found' with no AmazonDataCatalog wrapper.
