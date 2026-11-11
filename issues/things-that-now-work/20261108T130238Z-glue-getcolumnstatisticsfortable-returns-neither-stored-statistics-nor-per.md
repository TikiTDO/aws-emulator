Issue-Source: trace-sweep-divergence
Issue-Trace: glue-table-versioning-and-column-statistics
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-table-versioning-and-column-statistics
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetColumnStatisticsForTable|2xx->2xx|$.ColumnStatisticsList[*]
Issue-Key: glue|GetColumnStatisticsForTable|2xx->2xx|$.Errors[*]
Issue-Commit: c90916eed99ad246a7420e440fa460596401284c

# Glue GetColumnStatisticsForTable returns neither stored statistics nor per-column error entries

> _Note: this issue was identified against the code base at commit `c90916eed99ad246a7420e440fa460596401284c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetColumnStatisticsForTable

## Description
Two net-new root causes in the Glue Data Catalog's table-level column-statistics read path. Both surface on GetColumnStatisticsForTable, which answers HTTP 200 with BOTH of its output members empty regardless of catalog state: it neither returns statistics that were successfully persisted, nor reports the columns for which no statistics exist. Neither cause is marked ALREADY FILED and neither is OWNED by an open issue in the replay report.

Adjacency note: the open issue 20261025T170315Z-glue-2-net-new-root-causes-column-statistics.md ("column statistics task runs always settle to FAILED and compute no statistics...") covers the ASYNCHRONOUS producer path — StartColumnStatisticsTaskRun / GetColumnStatisticsTaskRun / CreateColumnStatisticsTaskSettings. It does NOT cover the causes below: these two replayed steps never start a task run. They write statistics synchronously with UpdateColumnStatisticsForTable and then read them back, so the defect is in GetColumnStatisticsForTable's own read of persisted catalog state and in its ColumnErrors reporting. Fixing the task-run state machine would not make either of these steps pass.

=== Cause 1 of 2: statistics persisted for a column are not returned — ColumnStatisticsList comes back empty ===
Fixture glue-table-versioning-and-column-statistics, step task#11.
AWS: Smithy (com.amazonaws.glue) — GetColumnStatisticsForTable takes DatabaseName/TableName/ColumnNames (all @required; ColumnNames is a GetColumnNamesList of 0..100 entries) and returns ColumnStatisticsList (list of ColumnStatistics) plus Errors (ColumnErrors). ColumnStatistics requires ColumnName, ColumnType, AnalyzedTime and StatisticsData; StatisticsData is a ColumnStatisticsData union whose Type discriminator selects the per-kind payload (LONG -> LongColumnStatisticsData with MinimumValue, MaximumValue, NumberOfNulls, NumberOfDistinctValues). Statistics written for a table's column by UpdateColumnStatisticsForTable (same @required DatabaseName/TableName, ColumnStatisticsList of 0..25 entries) are persisted against that catalog/database/table/column, so a subsequent GetColumnStatisticsForTable naming that column MUST return exactly one ColumnStatisticsList entry for it, echoing the ColumnName, the ColumnType, the AnalyzedTime and the StatisticsData that were stored — and that column MUST NOT appear in Errors. The recording confirms the round-trip: one entry for column 'id' with ColumnType 'int' and StatisticsData {Type: 'LONG', LongColumnStatisticsData: {MinimumValue, MaximumValue, NumberOfDistinctValues, NumberOfNulls}}.
Emulator: answers HTTP 200 with ColumnStatisticsList empty — ColumnStatisticsList[0] is missing entirely (expected the stored entry for column 'id', got None). The write was accepted, but nothing is read back.
Why: a client that stores statistics and reads them back sees them silently vanish, with a success status and no error of any kind. There is no way to distinguish "never written" from "written and lost", because both answer 200 with an empty list and an empty Errors. Every consumer of catalog statistics — cost-based query planning in Athena/Spark/Redshift Spectrum, data-quality baselines, ETL jobs that branch on cardinality or null counts, operator tooling that displays table profiles — reads an unpopulated table and plans as if the table had never been analyzed. Code that writes statistics then asserts the round-trip (a very common idempotency check before a scheduled refresh) rewrites them on every pass, forever.
Fix: the GetColumnStatisticsForTable handler and the catalog's column-statistics state — the read must look up the persisted per-column statistics for the resolved catalog/database/table and serialize one ColumnStatisticsList entry per requested column that has them, carrying the stored ColumnName/ColumnType/AnalyzedTime/StatisticsData (with the correct ColumnStatisticsData union discriminator). If the statistics are not being persisted at all by UpdateColumnStatisticsForTable, the state write is the defect and the serializer change alone will not fix it.
Traces: glue-table-versioning-and-column-statistics#task#11.

=== Cause 2 of 2: a column with no statistics is omitted from Errors instead of reported as EntityNotFoundException ===
Fixture glue-table-versioning-and-column-statistics, step task#13.
AWS: Smithy (com.amazonaws.glue) — GetColumnStatisticsForTable is a partial-success operation: it does NOT fail the whole call when a requested column has no statistics. Instead it reports that column in the Errors member (ColumnErrors, a list of ColumnError). ColumnError has ColumnName ("The name of the column that failed") and Error, an ErrorDetail carrying ErrorCode ("The code associated with this error") and ErrorMessage ("A message describing the error"); ErrorDetail also carries the __type discriminator this service's awsJson1.1 serializer writes onto the nested structure. So for every requested column that exists on the table but has no stored statistics, Errors MUST contain one entry whose ColumnName is that column's name (a value taken from the request), whose Error.__type is 'ErrorDetail', whose Error.ErrorCode is the fixed string 'EntityNotFoundException' and whose Error.ErrorMessage is the fixed string 'Statistics do not exist for this column' — the message has no variable parts, so it is reproduced verbatim; the only request-derived value in the entry is ColumnName. The HTTP status stays 200. The recording confirms exactly this shape.
Emulator: answers HTTP 200 with Errors empty — Errors[0] is missing entirely (expected the ColumnError entry for column 'id' with Error.__type 'ErrorDetail', Error.ErrorCode 'EntityNotFoundException', Error.ErrorMessage 'Statistics do not exist for this column'; got None). The absent-statistics condition is reported nowhere: not as a top-level error, not as a ColumnError.
Why: Errors is the ONLY channel this operation has for telling a client that a requested column was not satisfied, and it is the channel SDKs and tooling check, because the call returns 200. With it empty, a caller cannot tell a column that genuinely has no statistics from a column whose statistics the service failed to return, and cannot tell either from a fully satisfied request. A client that iterates ColumnNames and pairs results by position or by presence-in-Errors silently mis-attributes or drops columns; a client that retries only the columns named in Errors retries nothing and concludes the table is fully analyzed. Combined with Cause 1 the operation is indistinguishable from a no-op: every call answers 200 with two empty lists whatever the catalog contains.
Fix: the GetColumnStatisticsForTable handler's result assembly — after resolving which requested columns have persisted statistics, emit one ColumnError into Errors for each requested column that does not, with ColumnName from the request and an ErrorDetail whose ErrorCode is 'EntityNotFoundException' and whose ErrorMessage is the fixed text 'Statistics do not exist for this column', and ensure the nested-structure serializer writes the ErrorDetail __type tag. The status must remain 200.
Traces: glue-table-versioning-and-column-statistics#task#13.

## Evidence
Cause 1 of 2 — fixture glue-table-versioning-and-column-statistics, step task#11, operation GetColumnStatisticsForTable
Expected status 200, actual status 200 (the status agrees; the body does not).
Expected: ColumnStatisticsList[0] = {'ColumnName': 'id', 'ColumnType': 'int', 'AnalyzedTime': <the AnalyzedTime that was stored>, 'StatisticsData': {'Type': 'LONG', 'LongColumnStatisticsData': {'MinimumValue': <stored min>, 'MaximumValue': <stored max>, 'NumberOfDistinctValues': <stored distinct count>, 'NumberOfNulls': <stored null count>}}} — i.e. the entry previously persisted by UpdateColumnStatisticsForTable for that column, echoed back.
Actual (emulator): $.ColumnStatisticsList[0]: missing — got None. ColumnStatisticsList is empty.
(The recorded run's concrete values — AnalyzedTime 1786092726.0, MinimumValue 1, MaximumValue 1000, NumberOfDistinctValues 100, NumberOfNulls 0 — are the values that run happened to write. They fill the template; the requirement is that whatever was stored is what comes back, with ColumnName and ColumnType echoed and the StatisticsData union tagged for the column's kind.)

Cause 2 of 2 — fixture glue-table-versioning-and-column-statistics, step task#13, operation GetColumnStatisticsForTable
Expected status 200, actual status 200 (again the status agrees; the body does not).
Expected: Errors[0] = {'ColumnName': 'id', 'Error': {'__type': 'ErrorDetail', 'ErrorCode': 'EntityNotFoundException', 'ErrorMessage': 'Statistics do not exist for this column'}}.
Message breakdown: 'Statistics do not exist for this column' is entirely FIXED text — it interpolates nothing. 'EntityNotFoundException' and 'ErrorDetail' are likewise fixed strings. The ONLY request-derived value in the entry is ColumnName ('id' in this run, whatever column the caller asked for in general).
Actual (emulator): $.Errors[0]: missing — got None. Errors is empty, so the absent-statistics condition is reported through no channel at all while the call still answers 200.

## Steps to Reproduce
Cause 1 of 2 (fixture glue-table-versioning-and-column-statistics, step task#11):
1. glue.create_database(DatabaseInput={'Name': <db>}).
2. glue.create_table(DatabaseName=<db>, TableInput with Name=<table> and StorageDescriptor.Columns including a column 'id' of type 'int').
3. glue.update_column_statistics_for_table(DatabaseName=<db>, TableName=<table>, ColumnStatisticsList=[{'ColumnName': 'id', 'ColumnType': 'int', 'AnalyzedTime': <a timestamp>, 'StatisticsData': {'Type': 'LONG', 'LongColumnStatisticsData': {'MinimumValue': 1, 'MaximumValue': 1000, 'NumberOfDistinctValues': 100, 'NumberOfNulls': 0}}}]) and confirm Errors is empty.
4. glue.get_column_statistics_for_table(DatabaseName=<db>, TableName=<table>, ColumnNames=['id']).
   -> AWS: 200 with exactly one ColumnStatisticsList entry echoing the stored ColumnName/ColumnType/AnalyzedTime/StatisticsData, and Errors empty.
   -> Emulator: 200 with ColumnStatisticsList empty.
Replay: glue-table-versioning-and-column-statistics#task#11

Cause 2 of 2 (fixture glue-table-versioning-and-column-statistics, step task#13):
1. Reach a state where the table exists and column 'id' has NO stored statistics (either never written, or written and then removed with glue.delete_column_statistics_for_table(DatabaseName=<db>, TableName=<table>, ColumnName='id')).
2. glue.get_column_statistics_for_table(DatabaseName=<db>, TableName=<table>, ColumnNames=['id']).
   -> AWS: HTTP 200, ColumnStatisticsList empty, and Errors = [{'ColumnName': 'id', 'Error': {'__type': 'ErrorDetail', 'ErrorCode': 'EntityNotFoundException', 'ErrorMessage': 'Statistics do not exist for this column'}}].
   -> Emulator: HTTP 200 with Errors empty — the unsatisfied column is reported nowhere.
Replay: glue-table-versioning-and-column-statistics#task#13
