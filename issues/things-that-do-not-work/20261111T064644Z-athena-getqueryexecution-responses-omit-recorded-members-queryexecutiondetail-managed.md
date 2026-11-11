Issue-Source: trace-sweep-divergence
Issue-Trace: athena-cats-traj-ddl-identifier-syntax-mistakes
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-cats-traj-ddl-identifier-syntax-mistakes athena-cats-traj-drop-database-cleanup athena-cats-traj-output-location-mistakes athena-cats-traj-query-execution-polling-reads athena-cats-traj-table-before-database-and-ddl-retries athena-cats-traj-table-location-mistakes athena-kfi-traj-athena-query-context-mistakes athena-kfi-traj-athena-reads-the-iceberg-table athena-query-execution-inherits-workgroup-output-location athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecutionDetail
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.ManagedQueryResultsConfiguration.TimeToLive
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Status.AthenaError.ErrorType
Issue-Commit: f456950adb04cb9e7696776dd0db02907fd8875a

# Athena GetQueryExecution responses omit recorded members: QueryExecutionDetail, managed-results TimeToLive, AthenaError.ErrorType (3 causes)

> _Note: this issue was identified against the code base at commit `f456950adb04cb9e7696776dd0db02907fd8875a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryExecution

## Description
Three net-new root causes in what the Athena GetQueryExecution serializer emits for a 200 response. All three must be closed before this issue is submitted; each has its own fixture/step below.

Cause 1 of 3 — the whole QueryExecutionDetail sibling member is absent from every GetQueryExecution 200 response (fixture athena-cats-traj-ddl-identifier-syntax-mistakes, step task#8; the same omission clusters across 18 recorded steps in 9 fixtures).
AWS: recorded behavior only — Smithy GetQueryExecutionOutput models exactly one member, QueryExecution, so this member is not in the published model. Every recorded GetQueryExecution 200 carries, alongside QueryExecution, a top-level QueryExecutionDetail structure describing the same execution in a flattened form: QueryExecutionId, Query, QueryExecutionContext, ResultConfiguration{OutputLocation}, a top-level OutputLocation, WorkGroup, Stats{EngineExecutionTimeInMillis, ProcessedBytes, ProcessedInputBytes, ProcessedInputRows, ProcessedRows, ResultReuseInformation{ReusedPreviousResult}} and Status{State, SubmissionDateTime, CompletionDateTime, and StateChangeReason when the execution failed}. Every value in it is derived from the submitted statement and the execution's own state — the id, the query text, the resolved output location, the workgroup name, the counters and the timestamps are per-run values, never fixed text; the requirement is that the member is present and mirrors the execution.
Emulator: the member is missing entirely (expected an object, got None), for both SUCCEEDED and FAILED executions.
Why: a client or console that reads the flattened QueryExecutionDetail view — rather than QueryExecution — sees nothing at all for a query it just submitted: no output location, no statement text, no stats, no state. Polling loops written against that member never observe a terminal state and time out on a query that in fact finished.
Fix: the GetQueryExecution handler's response serializer, which must emit QueryExecutionDetail projected from the stored query-execution state in addition to QueryExecution.
Traces: athena-cats-traj-ddl-identifier-syntax-mistakes#task#8 (representative; cluster also covers athena-cats-traj-drop-database-cleanup#task#7/#10/#14, athena-cats-traj-output-location-mistakes#task#7, athena-cats-traj-query-execution-polling-reads#task#4/#8, athena-cats-traj-table-before-database-and-ddl-retries#task#13/#15/#16/#18, athena-cats-traj-table-location-mistakes#task#9, athena-kfi-traj-athena-query-context-mistakes#task#9/#11, athena-kfi-traj-athena-reads-the-iceberg-table#task#7/#10, athena-query-execution-inherits-workgroup-output-location#task#4).

Cause 2 of 3 — QueryExecution.ManagedQueryResultsConfiguration.TimeToLive is never emitted (fixture athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket, step task#5).
AWS: recorded behavior only for the member itself — Smithy models ManagedQueryResultsConfiguration with Enabled (required, default false) and EncryptionConfiguration, and no TimeToLive member; the recording shows AWS returning ManagedQueryResultsConfiguration.TimeToLive = {Unit: 'DAYS', Value: 1} for an execution in a workgroup whose results are held in Athena-owned storage. Unit and Value are the retention the service applies to managed results, not values from the request.
Emulator: the TimeToLive sub-structure is missing (expected {'Unit': 'DAYS', 'Value': 1}, got None) while the rest of the execution is returned.
Why: a client that reads how long Athena will keep managed query results (to decide whether to re-run or to copy the results out before expiry) gets no retention at all and cannot distinguish 'no expiry' from 'not reported'.
Fix: the GetQueryExecution serializer / the managed-query-results configuration held in query-execution state, which must carry and emit the TimeToLive unit and value.
Traces: athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket#task#5 (cluster also covers the same fixture's task#11).

Cause 3 of 3 — AthenaError.ErrorType is omitted from a failed execution's error, so the failure carries no machine-readable error type (fixture athena-kfi-traj-athena-query-context-mistakes, step task#7).
AWS: Smithy: QueryExecutionStatus.AthenaError -> AthenaError with members ErrorCategory (@range 1-3), ErrorType (@range 0-9999, 'provides specific information about an Athena query error'), Retryable (default false) and ErrorMessage. For a query whose QueryExecutionContext names a catalog that does not exist, the recording shows the execution settling FAILED with StateChangeReason "CATALOG_NOT_FOUND: line 1:22: Catalog '<catalog>' does not exist" and AthenaError.ErrorType = 1006 (the specific type value is recorded behavior; the catalog name and the line/column prefix in the message are taken from the request's statement and context).
Emulator: AthenaError is populated but ErrorType is absent (expected 1006, got None).
Why: AWS's documented failure-handling pattern is to branch on ErrorCategory plus ErrorType rather than parse the message. With ErrorType missing, a client cannot tell a catalog/metadata misconfiguration from any other user error and falls through to a generic retry or abort path.
Fix: the query-execution failure path that builds AthenaError (the component that classifies a failure before GetQueryExecution serializes it) — it must set ErrorType alongside ErrorCategory, Retryable and ErrorMessage.
Traces: athena-kfi-traj-athena-query-context-mistakes#task#7.

Related open issues that do NOT cover these three causes: '20261104T063259Z-athena-workgroup-and-data-catalog-reads-omit-modeled.md' (workgroup/data-catalog reads, not GetQueryExecution), '20261109T161941Z-athena-getworkgroup-omits-systemtablestatuses-and-enforceworkgroupconfiguration-and-over.md' (GetWorkGroup) and '20261110T203524Z-athena-ignores-the-ddl-location-bucket-and-refuses.md' (query outcome/state, not these response members). The replay report attributes none of these three clusters to an open issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (score 45.9%, 41 divergence clusters).

Cause 1 — fixture athena-cats-traj-ddl-identifier-syntax-mistakes, step task#8, GetQueryExecution, status 200 -> 200, 18 affected steps:
  $.QueryExecutionDetail: missing — expected an object carrying {OutputLocation, Query, QueryExecutionContext, QueryExecutionId, ResultConfiguration{OutputLocation}, Stats{EngineExecutionTimeInMillis (shaped, not asserted), ProcessedBytes, ProcessedInputBytes, ProcessedInputRows, ProcessedRows, ResultReuseInformation{ReusedPreviousResult:false}}, Status{State:'SUCCEEDED', SubmissionDateTime, CompletionDateTime}, WorkGroup}, got None.
  Template parts: QueryExecutionId, Query ('CREATE DATABASE `<db>`' in this recording), OutputLocation/ResultConfiguration.OutputLocation, WorkGroup, the Stats counters and the timestamps are all per-run/per-request values; only the member's presence and shape are the requirement.

Cause 2 — fixture athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket, step task#5, GetQueryExecution, status 200 -> 200:
  $.QueryExecution.ManagedQueryResultsConfiguration.TimeToLive: missing — expected {'Unit': 'DAYS', 'Value': 1}, got None.

Cause 3 — fixture athena-kfi-traj-athena-query-context-mistakes, step task#7, GetQueryExecution, status 200 -> 200:
  $.QueryExecution.Status.AthenaError.ErrorType: missing — expected 1006, got None.
  Failing statement: SELECT count(*) FROM "<table>" with QueryExecutionContext {Catalog: '<nonexistent-catalog>', Database: '<db>'}; recorded StateChangeReason "CATALOG_NOT_FOUND: line 1:22: Catalog '<catalog>' does not exist" (fixed text 'CATALOG_NOT_FOUND: ', the line:column prefix and "Catalog '"/"' does not exist"; the catalog name comes from the request).

## Steps to Reproduce
1. git_clone() the project and boot the emulator built from the checkout.
2. replay_wire_corpus() over trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture athena-cats-traj-ddl-identifier-syntax-mistakes to step task#8 — create a workgroup, StartQueryExecution with a CREATE DATABASE statement, then GetQueryExecution on the returned QueryExecutionId. Observe that the 200 body contains only QueryExecution and no top-level QueryExecutionDetail member.
4. Cause 2: replay fixture athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket to step task#5 — submit a CREATE DATABASE through a workgroup that stores results in Athena-owned (managed) storage, then GetQueryExecution. Observe QueryExecution.ManagedQueryResultsConfiguration present without TimeToLive {Unit:'DAYS', Value:1}.
5. Cause 3: replay fixture athena-kfi-traj-athena-query-context-mistakes to step task#7 — StartQueryExecution with QueryExecutionContext.Catalog naming a catalog that does not exist, poll GetQueryExecution to the terminal FAILED state. Observe Status.AthenaError without ErrorType, where AWS reports ErrorType 1006.
