Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.athena-workgroup-output-location-removal-makes-a-locationless-query-rejected
Issue-Key: athena__glue__s3.athena-workgroup-output-location-removal-makes-a-locationless-query-rejected|_poll#19|Timeout|time.sleep(interval_s)
Issue-Commit: d6e3ff8b4ffd84f1745388ebeb962e8b130bc98e

# Athena never writes the query-result object to the workgroup's resolved OutputLocation, so a restored workgroup default produces no <prefix>/<QueryExecutionId>.csv in S3

> _Note: this issue was identified against the code base at commit `d6e3ff8b4ffd84f1745388ebeb962e8b130bc98e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
One (1) net-new root cause on Athena's query-results write path, reached through the workgroup output-location removal/restore lifecycle.

No open issue carries this check's replay key. The replay report marked the check neither ALREADY FILED nor OWNED. Three open issues LOOK similar but do not cover it: 20261108T041805Z-athena-glue-s3-athena-startqueryexecution-the-no-output-location-refusal-message.md carries this very check id in Issue-Trace-Ids but keys on an `assert err == ("InvalidRequestException", _NO_LOCATION)` AssertionError that no longer occurs (the check now gets past that refusal-message assertion and hangs later instead); 20261108T043811Z-athena-s3-athena-reports-a-query-succeeded-and-names-a.md and 20261104T163600Z-athena-writes-no-query-result-object-to-s3.md describe a missing result object on other replay units and do not carry this one. Naming them here so a fixer can close them together.

Cause 1 of 1: a query that runs under a workgroup's ResultConfiguration.OutputLocation is reported SUCCEEDED and names a result object, but no object is ever written to S3 at that location.
AWS: Smithy (com.amazonaws.athena#AmazonAthena): StartQueryExecution(QueryString @required, ResultConfiguration, WorkGroup) -> StartQueryExecutionOutput.QueryExecutionId; ResultConfigurationUpdates$OutputLocation is documented as "The location in Amazon S3 where your query and calculation results are stored, such as s3://path/to/query/bucket/", and WorkGroupConfiguration$EnforceWorkGroupConfiguration decides whether the workgroup's location overrides the client's. Recorded behavior: once a query reaches SUCCEEDED, Athena has materialised its results as an S3 object under the resolved output-location prefix, keyed <prefix>/<QueryExecutionId>.csv — the same key GetQueryExecution reports in ResultConfiguration.OutputLocation. The prefix is the request's or the workgroup's configured value and the basename is the QueryExecutionId the submit returned; both are per-run values filling that template, not literals. The object appears for a query submitted after UpdateWorkGroup restored a previously removed OutputLocation under a NEW prefix, i.e. the write follows the workgroup's current configuration and not the one in force when the workgroup was created.
Emulator: the query settles and GetQueryExecution reports ResultConfiguration.OutputLocation as the expected s3://<bucket>/<prefix>/<QueryExecutionId>.csv, but ListObjectsV2 under that prefix never lists the key. The check's final wait — a 60 s poll for the restored query's result object — is never satisfied and the check ends `Check timed out after 60.0s` with no assertion having fired, which places the hang after the control query, after UpdateWorkGroup/RemoveOutputLocation, after the InvalidRequestException refusal and after the OutputLocation restore, all of which the emulator handles. In the checkout, StartQueryExecution stores the resolved ResultConfiguration on its QueryExecutionRecord purely so reads can echo OutputLocation, and then runs the statement through catalog_sql::execute without handing that result configuration to anything: no code path on the submit receives the output location, so no result object can be produced. (Hypothesis, since neither model nor recording states the internal split: the writer was never wired to the resolved result configuration.)
Why: every Athena client reads results by fetching the result object from S3 — the console, `aws athena get-query-results` backed by the CSV, Step Functions/Glue/QuickSight consumers, and any pipeline that lists or copies <prefix>/<id>.csv. Against the emulator such a client sees a SUCCEEDED query that names a location holding nothing, so it either blocks forever polling for the object or fails with NoSuchKey, and the whole "clear the workgroup default, restore it under a new prefix" operation cannot be validated: results silently stop landing while the API keeps claiming success.
Fix: the StartQueryExecution handler's results-materialisation step — the resolved ResultConfiguration must be carried into the execution path so a completed run writes its CSV (and the accompanying metadata object) to the object store at <prefix>/<QueryExecutionId>.csv, under the workgroup configuration in force at submit time. File and line citations are under Traces.
Traces: athena__glue__s3.athena-workgroup-output-location-removal-makes-a-locationless-query-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout. Emulator up (bootstrap_signal false): 338/645 checks passed, 204 mismatched, 103 errored, 0 skipped, 0 unloadable.

Failing check (the only failure in the whole report marked neither ALREADY FILED nor OWNED):

### athena__glue__s3
#### [mismatch] athena__glue__s3.athena-workgroup-output-location-removal-makes-a-locationless-query-rejected
Error: Check timed out after 60.0s

The check body, in order, is:
1. create bucket; create_work_group(Configuration.ResultConfiguration.OutputLocation = s3://<bucket>/athena-results/)
2. start_query_execution('SELECT 1', WorkGroup=<wg>) with no request location; poll to terminal; assert SUCCEEDED and ResultConfiguration.OutputLocation == s3://<bucket>/athena-results/<qid>.csv
3. update_work_group(ConfigurationUpdates.ResultConfigurationUpdates.RemoveOutputLocation=True); assert GetWorkGroup no longer reports OutputLocation
4. assert start_query_execution is refused ('InvalidRequestException', AWS's fixed "No output location provided. You did not provide an output location for  your query results. Either specify an S3 bucket location or enable Athena managed query results in your workgroup settings."); assert list_query_executions still reports only [qid]
5. update_work_group(ResultConfigurationUpdates.OutputLocation = s3://<bucket>/restored/); run 'SELECT 1' again; assert SUCCEEDED
6. poll ListObjectsV2(Prefix='restored/') for 60 s until key 'restored/<qid2>.csv' appears

No AssertionError was raised, so steps 2-5 all passed: the control query succeeded and named the right object key, RemoveOutputLocation was honoured, the refusal carried the right code AND the right fixed message, the execution history was unchanged, and the restore made the identical call run again. The only unsatisfied wait is step 6, whose bound (60.0 s) is exactly the reported timeout: the result object never lands.

Expected: after step 5's SUCCEEDED query, ListObjectsV2(Bucket=<bucket>, Prefix='restored/') lists 'restored/<qid2>.csv', where <qid2> is the QueryExecutionId StartQueryExecution returned.
Actual: the prefix stays empty for the full 60 s while GetQueryExecution reports ResultConfiguration.OutputLocation = s3://<bucket>/restored/<qid2>.csv.

Code in this checkout, consistent with that: services/service-com-amazonaws-athena-amazonathena/src/ops/start_query_execution.rs resolves the result configuration at lines 244-249 (crate::state::resolved_result_configuration, refusing with crate::state::NO_OUTPUT_LOCATION when there is none), stores it on the record at line 303, then at lines 337-348 runs the statement via catalog_sql::execute(&*engine, &prepared, &sql, reading, &query_context, &parameters, now) — the result configuration is not an argument — and finally only records the completion (lines 369-371). Nothing between the submit and the completion is given the output location, so no result object is written.

Not filed as defects here: the per-run values in the above (bucket name, workgroup name, QueryExecutionIds, timestamps) are environment-specific and are not part of the requirement; the requirement is the key template <prefix>/<QueryExecutionId>.csv under the workgroup's current OutputLocation.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay simparity-behavioral-aws-v1 and re-run check athena__glue__s3.athena-workgroup-output-location-removal-makes-a-locationless-query-rejected.
3. Manually, against the emulator:
   a. s3.create_bucket(Bucket=<bucket>)
   b. athena.create_work_group(Name=<wg>, Configuration={'ResultConfiguration': {'OutputLocation': 's3://<bucket>/athena-results/'}})
   c. qid = athena.start_query_execution(QueryString='SELECT 1', WorkGroup=<wg>)['QueryExecutionId']; poll athena.get_query_execution(QueryExecutionId=qid) to a terminal state — it reports SUCCEEDED and ResultConfiguration.OutputLocation = s3://<bucket>/athena-results/<qid>.csv
   d. s3.list_objects_v2(Bucket=<bucket>, Prefix='athena-results/') — EXPECTED to list '<prefix>/<qid>.csv'; ACTUAL: no Contents.
4. Now exercise the removal/restore lifecycle the check covers:
   a. athena.update_work_group(WorkGroup=<wg>, ConfigurationUpdates={'ResultConfigurationUpdates': {'RemoveOutputLocation': True}}); athena.get_work_group(WorkGroup=<wg>) no longer reports OutputLocation.
   b. athena.start_query_execution(QueryString='SELECT 1', WorkGroup=<wg>) is refused InvalidRequestException with AWS's fixed no-output-location sentence (this part is correct).
   c. athena.update_work_group(WorkGroup=<wg>, ConfigurationUpdates={'ResultConfigurationUpdates': {'OutputLocation': 's3://<bucket>/restored/'}})
   d. qid2 = athena.start_query_execution(QueryString='SELECT 1', WorkGroup=<wg>)['QueryExecutionId']; poll to SUCCEEDED.
   e. Poll s3.list_objects_v2(Bucket=<bucket>, Prefix='restored/') for 'restored/<qid2>.csv': on AWS it appears; against the emulator the prefix stays empty indefinitely, which is what exhausts the check's 60 s budget.
