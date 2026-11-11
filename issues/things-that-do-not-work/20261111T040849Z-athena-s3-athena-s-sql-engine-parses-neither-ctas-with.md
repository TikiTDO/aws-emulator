Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__s3.ctas-partitioned-output-layout-matches-show-partitions athena__s3.data-manifest-lists-exactly-the-objects-unload-wrote athena__s3.unload-into-non-empty-s3-prefix-rejected
Issue-Key: athena__s3.ctas-partitioned-output-layout-matches-show-partitions|_run_sql#17|AssertionError|raise AssertionError(
Issue-Key: athena__s3.data-manifest-lists-exactly-the-objects-unload-wrote|_run_sql#18|AssertionError|raise AssertionError(
Issue-Key: athena__s3.unload-into-non-empty-s3-prefix-rejected|run#24|AssertionError|assert reason.startswith("HIVE_PATH_ALREADY_EXISTS: Target directory already exists: "), (
Issue-Commit: c0ced5d18fa4eb624de2042a998eca41dbf9ea10

# Athena's SQL engine parses neither CTAS WITH(...) properties nor UNLOAD, and collapses a qualified table name into a single identifier

> _Note: this issue was identified against the code base at commit `c0ced5d18fa4eb624de2042a998eca41dbf9ea10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
Two net-new root causes in the Athena query engine.

Cause 1 of 2: CREATE TABLE ... WITH (...) AS SELECT (CTAS) is rejected as a syntax error.
AWS: recorded behavior only for the SQL grammar (Smithy models StartQueryExecution/GetQueryExecution but not the dialect). The recording shows a CTAS carrying WITH (format = 'PARQUET', partitioned_by = ARRAY['dt'], external_location = 's3://<bucket>/<prefix>/') reaching SUCCEEDED, with the partitioned output layout under external_location matching SHOW PARTITIONS.
Emulator: the execution reaches FAILED with StateChangeReason 'near "WITH": syntax error in <the statement> at offset 78'. Note the reason also shows the engine re-rendered the qualified name "db"."table" as the single quoted identifier "db.table", so even the echoed statement is not the one submitted.
Why: CTAS is the normal way to materialise a partitioned Parquet table from a query; a client gets a FAILED execution and no output objects, and the failure reason blames its SQL rather than the engine.
Fix: the Athena query-engine statement parser (CTAS/table-property grammar) and its identifier handling, plus the CTAS executor that must write the partitioned output under external_location.
Traces: athena__s3.ctas-partitioned-output-layout-matches-show-partitions.

Cause 2 of 2: UNLOAD (SELECT ...) TO '<s3 uri>' WITH (format = ...) is rejected as a syntax error, so no data objects and no manifest are ever written and the documented refusal for a non-empty destination never happens.
AWS: recorded behavior only. The recording shows UNLOAD succeeding, writing one or more data objects under the TO prefix plus a manifest that lists exactly those objects; and, when the destination prefix already holds objects, the execution fails with a StateChangeReason naming the destination collision.
Emulator: both executions reach FAILED with StateChangeReason 'near "UNLOAD": syntax error in UNLOAD(SELECT ...) TO ... at offset 0' — again with "db"."table" collapsed to "db.table". The non-empty-destination check therefore never sees a collision reason, only a parse error.
Why: a client cannot export query results with UNLOAD at all, and the negative path it relies on (refusal when the prefix is occupied) is indistinguishable from a syntax error, so error handling keyed on the destination collision never fires.
Fix: the Athena query-engine statement parser (UNLOAD grammar) and the UNLOAD executor, including the manifest writer and the destination-not-empty refusal.
Traces: athena__s3.data-manifest-lists-exactly-the-objects-unload-wrote, athena__s3.unload-into-non-empty-s3-prefix-rejected.

## Evidence
Check athena__s3.ctas-partitioned-output-layout-matches-show-partitions [mismatch]:
AssertionError: statement reached FAILED, expected SUCCEEDED: 'CREATE TABLE "<db>"."<tgt>" WITH (format = 'PARQUET', partitioned_by = ARRAY['dt'], external_location = 's3://<bucket>/ctas/<tgt>/') AS SELECT id, name, dt FROM "<db>"."<src>"' -- near "WITH": syntax error ... FROM "<db>.<src>" at offset 78

Check athena__s3.data-manifest-lists-exactly-the-objects-unload-wrote [mismatch]:
AssertionError: statement reached FAILED, expected SUCCEEDED: 'UNLOAD (SELECT id, name FROM "<db>"."<tbl>") TO 's3://<bucket>/unloaded/' WITH (format = 'JSON')' -- near "UNLOAD": syntax error in UNLOAD(SELECT ... FROM "<db>.<tbl>") ... at offset 0

Check athena__s3.unload-into-non-empty-s3-prefix-rejected [mismatch]:
AssertionError: the failure must name the destination collision as its cause, got StateChangeReason 'near "UNLOAD": syntax error in UNLOAD(SELECT ... ) TO 's3://<bucket>/occupied/' WITH (format = 'PARQUET') at offset 0'

In all three the fixed text of the divergence is the parse failure ('near "<keyword>": syntax error ... at offset N'); the database/table/bucket names and the offset are values from the submitted statement.

## Steps to Reproduce
1. Create a Glue database and a source table over an S3 prefix with rows carrying a partition column.
2. StartQueryExecution with CREATE TABLE "db"."tgt" WITH (format='PARQUET', partitioned_by=ARRAY['dt'], external_location='s3://bucket/ctas/tgt/') AS SELECT ... and poll GetQueryExecution: state is FAILED with a 'near "WITH": syntax error' reason.
3. StartQueryExecution with UNLOAD (SELECT ...) TO 's3://bucket/unloaded/' WITH (format='JSON') and poll: state is FAILED with a 'near "UNLOAD": syntax error' reason; no data objects and no manifest appear under the prefix.
4. Put an object under the UNLOAD destination prefix and repeat step 3: the reason is still the parse error, never the destination collision.
