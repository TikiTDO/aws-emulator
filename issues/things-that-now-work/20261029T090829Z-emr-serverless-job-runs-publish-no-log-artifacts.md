Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied
Issue-Key: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied|_await_driver_stderr#31|TimeoutError|raise TimeoutError(f"a non-empty {want!r}: the run budget ended first; not graded")
Issue-Commit: f60279ee1b26e62c04ba7bfaecc753f1127e63e1

# EMR Serverless job runs publish no log artifacts to the configured s3MonitoringConfiguration.logUri, so SPARK_DRIVER/stderr.gz never appears and the driver-log read can never complete

> _Note: this issue was identified against the code base at commit `f60279ee1b26e62c04ba7bfaecc753f1127e63e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
One cause.

Attribution note: the failing check's id leads with the four-service combo token `athena__emr-serverless__glue__s3`, which equals no token returned by get_bootstrap_status (neither the bootstrapped nor the pending set), so the per-service gate cannot place the check id as written. The diverging behavior described here is produced by the emr-serverless log-publication path (the application's monitoringConfiguration.s3MonitoringConfiguration.logUri supplied to CreateApplication, and the job run started by StartJobRun), and emr-serverless IS stood up and answering — it passes 34/34 of its own behavioral checks in this same replay — so the issue is filed against `emr-serverless`. It is deliberately NOT attributed to `athena` (which is still pending bootstrap): the check died in its driver-log wait, before reaching any Athena call.

Related but NOT covering: open issue 20261025T015858Z-emr-serverless-publishes-no-job-run-logs-to.md describes the same absence and names this check in its Issue-Trace-Ids, but the replay suite did NOT report this check as owned by it — its recorded Issue-Key is `...|_await_driver_stderr#31|Timeout|time.sleep(15)`, while the check now fails at `_await_driver_stderr` line 266 raising `TimeoutError: a non-empty '<key>': the run budget ended first; not graded` (the helper now polls every 2 s and consults the check's run budget). The key no longer matches, so this check is not carried by that issue and is re-filed here with the current key; a fixer closing either must satisfy this check. Also related and not covering: 20261024T013257Z-emr-serverless-job-runs-never-execute-the-submitted.md (terminal state FAILED instead of SUCCESS) and 20261022T154846Z / 20261026T072046Z, which are about the state machine, not log publication. This cause is independent of the terminal state: in AWS a run that ends FAILED still publishes its driver log, which is precisely how the failure is diagnosed.

Cause 1 of 1: a job run started on an application whose monitoringConfiguration.s3MonitoringConfiguration.logUri points into an S3 bucket never causes any object to appear under that prefix — in particular no `<logUri>applications/<applicationId>/jobs/<jobRunId>/SPARK_DRIVER/stderr.gz`.

AWS: Smithy com.amazonaws.emrserverless#S3MonitoringConfiguration.logUri (target UriString, @length min 1 max 10280) is documented "The Amazon S3 destination URI for log publishing"; com.amazonaws.emrserverless#S3MonitoringConfiguration is "The Amazon S3 configuration for monitoring log publishing. You can configure your jobs to send log information to Amazon S3."; com.amazonaws.emrserverless#MonitoringConfiguration.s3MonitoringConfiguration is "The Amazon S3 configuration for monitoring log publishing." com.amazonaws.emrserverless#JobRun is "Information about a job run. A job run is a unit of work, such as a Spark JAR, Hive query, or SparkSQL query, that you submit to an Amazon EMR Serverless application", with required `state` (JobRunState: SUBMITTED, PENDING, SCHEDULED, RUNNING, SUCCESS, FAILED, CANCELLING, CANCELLED, QUEUED) and required `stateDetails`, and `endedAt` "The date and time when the job was terminated." With that configuration present on the application, a SPARK job run publishes its per-worker-type log artifacts under the configured logUri, keyed by application id and job run id, the driver's stderr written as a gzip stream — and it does so for a run whose terminal state is FAILED as well as SUCCESS. The exact key layout (`applications/<applicationId>/jobs/<jobRunId>/SPARK_DRIVER/stderr.gz`) is recorded behavior only: the Smithy model does not state a key scheme. What the model does make a requirement is that configuring logUri causes log content for the run to exist under it once the run terminates.

Emulator: the job run's log prefix stays completely empty for the life of the run and after it terminates. The check's `_await_driver_stderr` helper repeatedly calls ListObjectsV2 under `logs/applications/<app_id>/jobs/<job_run_id>/` looking for `.../SPARK_DRIVER/stderr.gz` with a non-empty body, sleeping 2 s between polls on a 420 s budget; the listing never returns any key, so the wait can only end by exhausting the budget and the suite reports `[error] ... TimeoutError: a non-empty 'logs/applications/0000000000000001/jobs/0000000000000001/SPARK_DRIVER/stderr.gz': the run budget ended first; not graded`. Fixed text of that message is the helper's own; the variable parts are the bucket-relative key built from the configured log prefix (`logs/`), the applicationId and the jobRunId. Nothing in any control-plane response signals the absence: CreateApplication accepts the monitoringConfiguration and returns 200 with an applicationId, StartJobRun returns 200 with a jobRunId. The emulator itself states the underlying reason in the companion check's stateDetails: 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.' — the unpublished log is one of the side effects that absence drops.

Why: every client that uses EMR Serverless logs to diagnose a run breaks, and it breaks by HANGING rather than erroring. The documented pattern after a terminal job run is to fetch the driver log from the configured logUri; against the emulator that read finds nothing, so a poller spins to its own deadline (here 420 s, in a pipeline whatever the orchestrator allows) and the wall-clock is burned on a wait that can never succeed. A caller that reads once instead gets an empty listing and concludes the run emitted no diagnostics, which in AWS would mean a platform fault. It also makes the IAM-denial scenario ungradable end to end: the only place a denied `glue:CreateTable` action is visible is the AccessDeniedException the Spark driver wrote into its own stderr, and with no published stderr that evidence exists nowhere in the emulator, so the negative control cannot be distinguished from a run that was never attempted.

Fix: the emr-serverless job-run lifecycle and its monitoring/log-publication path — the component behind StartJobRun that drives a run to a terminal state must also honour the application's (and the run's configurationOverrides') monitoringConfiguration.s3MonitoringConfiguration by writing the run's log artifacts into the configured S3 logUri under the per-application/per-job-run prefix, including a gzipped SPARK_DRIVER stderr for a run that terminated, so the artifacts exist for a FAILED run as well as a SUCCESS one. Publication must be modelled alongside the state machine, not only on success; otherwise the terminal state remains undiagnosable.

Traces: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied — fails in `_await_driver_stderr` (check file line 266, called from line 424), which waits for `<logUri>applications/<app_id>/jobs/<job_run_id>/SPARK_DRIVER/stderr.gz`.

## Evidence
Replay of simparity-behavioral-ga-prod-v1: 34/36 passed (94.4%); mismatched 1, errored 1. Per-service: athena__emr-serverless__glue__s3 0/2 (0%), emr-serverless 34/34 ✓.

[error] athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied
  File "check_athena__emr_serverless__glue__s3_spark_saveastable_without_glue_createtable_permission_is_denied.py", line 424, in run
    log_key, log_gz = _await_driver_stderr(
  File "...", line 266, in _await_driver_stderr
    raise TimeoutError(f"a non-empty {want!r}: the run budget ended first; not graded")
  TimeoutError: a non-empty 'logs/applications/0000000000000001/jobs/0000000000000001/SPARK_DRIVER/stderr.gz': the run budget ended first; not graded

The check creates the application with:
  monitoringConfiguration={"s3MonitoringConfiguration": {"logUri": f"s3://{bucket}/logs/"}}
starts a sparkSubmit job run with an execution role that grants every Glue table/partition action EXCEPT glue:CreateTable, waits for the run to go terminal, asserts the Glue table is absent, and then waits for the driver log:

    def _await_driver_stderr(s3, bucket, log_prefix, app_id, job_run_id, budget_s, ctx=None):
        want = f"{log_prefix}applications/{app_id}/jobs/{job_run_id}/SPARK_DRIVER/stderr.gz"
        deadline = time.monotonic() + budget_s           # budget_s = _LOG_BUDGET_S = 420.0
        seen = []
        while time.monotonic() < deadline:
            seen = _list_keys(s3, bucket, f"{log_prefix}applications/{app_id}/jobs/{job_run_id}/")
            if want in seen:
                body = s3.get_object(Bucket=bucket, Key=want)["Body"].read()
                if body:
                    return want, body
            time.sleep(2)
        ...

Expected (AWS): once the job run reaches a terminal state, ListObjectsV2 under
  logs/applications/<applicationId>/jobs/<jobRunId>/
returns the run's published log objects, including a non-empty gzip object
  logs/applications/<applicationId>/jobs/<jobRunId>/SPARK_DRIVER/stderr.gz
(body starts with the gzip magic \x1f\x8b) whose decompressed text is the driver's stderr — for this scenario naming AccessDeniedException and the denied action glue:CreateTable. This holds for a FAILED terminal state as well as SUCCESS.

Actual (emulator): that prefix returns no Contents at any point, before or after the run terminates, so the wait exhausts its budget and the check raises TimeoutError without ever reaching its graded assertions (gzip magic, 'AccessDeniedException' in the stderr text, 'glue:CreateTable' in the stderr text). CreateApplication returned 200 having accepted the monitoringConfiguration and StartJobRun returned 200 with jobRunId, so no control-plane response indicates logs will never be published.

Scope note: the check's later steps (the positive-control run that must reach SUCCESS, the Glue PartitionKeys assertions and the Athena read-back) were never reached; the SUCCESS half is the separately-open cause 20261024T013257Z and is not re-filed here.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-behavioral-ga-prod-v1 behavioral suite; check athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied errors with TimeoutError in _await_driver_stderr.
2. By hand against the emulator endpoint:
   a. s3 CreateBucket <bucket>; PutObject <bucket>/input/facts.csv (a header'd CSV id,region,amount,dt) and <bucket>/job/ddl.py (a PySpark script doing spark.read.csv(...).write.partitionBy('dt').option('path', warehouse).saveAsTable(db + '.' + tbl)).
   b. glue CreateDatabase for <db>.
   c. iam CreateRole trusted by emr-serverless.amazonaws.com + PutRolePolicy granting the S3 object/bucket actions on <bucket> and the Glue table/partition actions (glue:GetDatabase, glue:GetTable, glue:GetTables, glue:UpdateTable, glue:DeleteTable, glue:CreatePartition, glue:BatchCreatePartition, glue:GetPartition, glue:GetPartitions, glue:UpdatePartition, glue:BatchGetPartition, glue:DeletePartition) but NOT glue:CreateTable.
   d. emr-serverless CreateApplication with releaseLabel 'emr-7.9.0', type 'SPARK', and monitoringConfiguration={'s3MonitoringConfiguration': {'logUri': 's3://<bucket>/logs/'}} -> note the returned applicationId.
   e. emr-serverless StartJobRun with jobDriver.sparkSubmit.entryPoint = s3://<bucket>/job/ddl.py, entryPointArguments = [s3://<bucket>/input/facts.csv, <db>, <tbl>, s3://<bucket>/warehouse/<tbl>/], and sparkSubmitParameters setting spark.sql.catalogImplementation=hive plus the AWSGlueDataCatalogHiveClientFactory metastore factory -> note the returned jobRunId.
   f. Poll emr-serverless GetJobRun until jobRun.state is terminal (SUCCESS / FAILED / CANCELLED).
3. s3 ListObjectsV2 --bucket <bucket> --prefix logs/applications/<applicationId>/jobs/<jobRunId>/ , repeatedly for several minutes after the run went terminal.
4. Observe the listing stays empty: no SPARK_DRIVER/stderr.gz and no log object at all ever appears under the configured logUri, so any client polling for the driver log runs to its own deadline. In AWS the prefix is populated once the run terminates, and GetObject on .../SPARK_DRIVER/stderr.gz returns a non-empty gzip stream whose decompressed text is the driver's stderr (here naming AccessDeniedException and glue:CreateTable).
5. Repeat (b)-(f) with glue:CreateTable granted to confirm the absence is independent of the terminal state: the log prefix is still empty.
