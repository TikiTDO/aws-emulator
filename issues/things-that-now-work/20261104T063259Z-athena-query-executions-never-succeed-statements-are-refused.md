Issue-Source: trace-sweep-divergence
Issue-Trace: athena-batch-query-submission-and-pagination
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-batch-query-submission-and-pagination athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket athena-workgroup-lifecycle-and-query-execution athena-cats-traj-ddl-identifier-syntax-mistakes athena-cats-traj-output-location-mistakes athena-cats-traj-drop-database-cleanup athena-cats-traj-query-execution-polling-reads athena-cats-traj-table-before-database-and-ddl-retries athena-cats-traj-table-location-mistakes athena-kfi-traj-athena-query-context-mistakes athena-kfi-traj-athena-reads-the-iceberg-table
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|StartQueryExecution|2xx->4xx|$.QueryExecutionId
Issue-Key: athena|StartQueryExecution|2xx->4xx|status
Issue-Key: athena|StartQueryExecution|4xx->2xx|$.AthenaErrorCode
Issue-Key: athena|StartQueryExecution|4xx->2xx|$.ErrorCode
Issue-Key: athena|StartQueryExecution|4xx->2xx|$.Message
Issue-Key: athena|StartQueryExecution|4xx->2xx|$.__type
Issue-Key: athena|StartQueryExecution|4xx->2xx|status
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.ResultConfiguration.OutputLocation
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.ResultReuseConfiguration
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Statistics.ResultReuseInformation
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Statistics.ServicePreProcessingTimeInMillis
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Statistics.ServiceProcessingTimeInMillis
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Status.State
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.SubstatementType
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecutionDetail
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Status.AthenaError.ErrorMessage
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Status.AthenaError.ErrorType
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Status.StateChangeReason
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Statistics.DataScannedInBytes
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Statistics.QueryPlanningTimeInMillis
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.QueryExecutionContext.Catalog
Issue-Key: athena|ListQueryExecutions|2xx->2xx|$
Issue-Key: athena|BatchGetQueryExecution|2xx->2xx|$.QueryExecutions[*].ResultConfiguration.OutputLocation
Issue-Key: athena|BatchGetQueryExecution|2xx->2xx|$.QueryExecutions[*].ResultReuseConfiguration
Issue-Key: athena|BatchGetQueryExecution|2xx->2xx|$.QueryExecutions[*].Statistics.ResultReuseInformation
Issue-Key: athena|BatchGetQueryExecution|2xx->2xx|$.QueryExecutions[*].Statistics.ServicePreProcessingTimeInMillis
Issue-Key: athena|BatchGetQueryExecution|2xx->2xx|$.QueryExecutions[*].Statistics.ServiceProcessingTimeInMillis
Issue-Key: athena|BatchGetQueryExecution|2xx->2xx|$.QueryExecutions[*].Status.State
Issue-Key: athena|BatchGetQueryExecution|2xx->2xx|$.QueryExecutions[*].SubstatementType
Issue-Key: athena|BatchGetQueryExecution|2xx->2xx|$.UnprocessedQueryExecutionIds[*].ErrorCode
Issue-Commit: fb394b2a45eb301a39bf46320599094d7846a70d

# Athena query executions never succeed: statements are refused, settle FAILED with SQLite-derived errors, and query reads omit modeled members (10 causes)

> _Note: this issue was identified against the code base at commit `fb394b2a45eb301a39bf46320599094d7846a70d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryExecution

## Description
The emulator's Athena query-execution path does not implement Athena's statement surface: valid statements are refused at submission, invalid ones are accepted, and every statement that is accepted terminates FAILED carrying an error produced by the emulator's local SQL engine instead of Athena's. The query-execution record it then serves also omits members AWS always returns. 10 causes.

Cause 1 of 10: StartQueryExecution refuses a valid submission.
AWS: Smithy: StartQueryExecution input QueryString (required, @length 1..262144), output QueryExecutionId (@length 1..128, pattern ^\S+$); modeled errors InvalidRequestException/TooManyRequestsException/InternalServerException. A submission with a resolvable output location and an existing workgroup is accepted with HTTP 200 and a QueryExecutionId.
Emulator: HTTP 400 and no QueryExecutionId member at all.
Why: no query can be submitted, so every downstream poll/read in the client's workflow operates on an id it never received.
Fix: the athena crate's StartQueryExecution handler and its request validation.
Traces: athena-batch-query-submission-and-pagination#task#2 (also task#3, task#4), athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket#task#4, athena-workgroup-lifecycle-and-query-execution#task#3.

Cause 2 of 10: StartQueryExecution accepts a syntactically invalid statement.
AWS: Athena parses the statement at submission and refuses a malformed one with HTTP 400 InvalidRequestException whose AthenaErrorCode and ErrorCode are both MALFORMED_QUERY and whose Message is the parser diagnostic. Fixed form: "line <line>:<column>: mismatched input '<token>'. Expecting: <comma-separated list of accepted tokens>"; the line/column, the offending token and the expectation list are derived from the submitted QueryString (recorded example: "line 1:8: mismatched input 'DATABASE'. Expecting: 'MATERIALIZED', 'MULTI', 'OR', 'PROTECTED', 'ROLE', 'SCHEMA', 'TABLE', 'VIEW'").
Emulator: HTTP 200 with a QueryExecutionId, no __type, no AthenaErrorCode, no ErrorCode, no Message.
Why: a client that relies on submission-time rejection to surface a SQL syntax error instead sees an accepted query, and must mis-diagnose the later failure.
Fix: the athena crate's StartQueryExecution statement parsing/validation.
Traces: athena-cats-traj-ddl-identifier-syntax-mistakes#task#3 (also task#4, task#5), athena-cats-traj-output-location-mistakes#task#4.

Cause 3 of 10: a DDL statement (CREATE DATABASE) terminates FAILED instead of SUCCEEDED.
AWS: Smithy: QueryExecutionStatus.State enum (QUEUED/RUNNING/SUCCEEDED/FAILED/CANCELLED); a successful CREATE DATABASE reaches SUCCEEDED and the record carries SubstatementType (recorded value CREATE_DATABASE for this statement class).
Emulator: State FAILED on every poll (the fixture's poll gives up after 3 attempts because FAILED is terminal), and SubstatementType, ResultReuseConfiguration, Statistics.ResultReuseInformation, Statistics.ServicePreProcessingTimeInMillis and Statistics.ServiceProcessingTimeInMillis are all absent.
Why: a client that waits for SUCCEEDED before reading results never gets there; DDL that AWS performs is reported as a failure.
Fix: the athena crate's query-execution engine (DDL statement handling) and the GetQueryExecution serializer.
Traces: athena-cats-traj-ddl-identifier-syntax-mistakes#task#8 (also task#9), athena-cats-traj-drop-database-cleanup#task#10/#task#14, athena-cats-traj-table-before-database-and-ddl-retries#task#13/#15/#16/#18, athena-cats-traj-table-location-mistakes#task#13.

Cause 4 of 10: a DROP DATABASE failure reports the emulator's SQL engine error, not Athena's.
AWS: Smithy: QueryExecutionStatus.AthenaError (AthenaError: ErrorCategory, ErrorType @range 0..9999, Retryable, ErrorMessage) and StateChangeReason. Dropping a non-empty database fails with AthenaError.ErrorType 1006 and AthenaError.ErrorMessage of the fixed form "Cannot drop database <database> as its not empty.", while StateChangeReason carries the Hive DDL task text "FAILED: Execution Error, return code 1 from org.apache.hadoop.hive.ql.exec.DDLTask. MetaException(message:Unable to drop database: InvalidOperationException(message:Database <database> is not empty.))". Only <database> comes from the request.
Emulator: AthenaError.ErrorType absent, and both AthenaError.ErrorMessage and StateChangeReason carry a SQLite parser diagnostic of the form 'near "DATABASE": syntax error in DROP DATABASE <database> at offset 5' — i.e. the statement was never understood at all. SubstatementType (DROP_DATABASE) is also absent.
Why: the failure is attributed to query syntax instead of to a non-empty database, so a client cannot branch on ErrorType or recognise the real cause.
Fix: the athena crate's query-execution engine (DDL statement handling) and its AthenaError population.
Traces: athena-cats-traj-drop-database-cleanup#task#7 (also task#15), athena-cats-traj-table-before-database-and-ddl-retries#task#5/#14/#17, athena-cats-traj-table-location-mistakes#task#9/#10, athena-kfi-traj-athena-query-context-mistakes#task#7, athena-kfi-traj-athena-reads-the-iceberg-table#task#7.

Cause 5 of 10: a SELECT over a catalog table terminates FAILED and reports no scanned bytes.
AWS: a SELECT over an existing external table reaches State SUCCEEDED, carries SubstatementType SELECT, Statistics.DataScannedInBytes equal to the bytes read, and Statistics.QueryPlanningTimeInMillis / ServicePreProcessingTimeInMillis / ServiceProcessingTimeInMillis (Smithy: QueryExecutionStatistics).
Emulator: State FAILED, DataScannedInBytes 0, and those statistics members plus SubstatementType and ResultReuseConfiguration absent.
Why: analytical reads never complete, so a client sees every query as a failure and never obtains a row count or scan size.
Fix: the athena crate's query-execution engine (DML/SELECT execution against the catalog) and the GetQueryExecution serializer.
Traces: athena-cats-traj-table-location-mistakes#task#16.

Cause 6 of 10: a missing-schema failure reports a table-level SQLite error and the request's catalog spelling is echoed verbatim.
AWS: when the QueryExecutionContext names a database that does not exist, the query fails with AthenaError.ErrorType 1301 and AthenaError.ErrorMessage / StateChangeReason both of the fixed form "SCHEMA_NOT_FOUND: line <line>:<column>: Schema '<database>' does not exist" (<database> from QueryExecutionContext.Database). AWS also normalises the echoed QueryExecutionContext.Catalog to lower case ('awsdatacatalog').
Emulator: ErrorType absent; ErrorMessage and StateChangeReason are 'no such table: <table>' (the emulator's SQL engine blames the table, not the schema), and QueryExecutionContext.Catalog is echoed as submitted ('AwsDataCatalog').
Why: a client cannot distinguish a missing database from a missing table, and a catalog name compared case-sensitively against the response does not match.
Fix: the athena crate's query-execution engine (context/schema resolution) and the QueryExecutionContext serializer.
Traces: athena-kfi-traj-athena-query-context-mistakes#task#9.

Cause 7 of 10: a UTILITY statement (SHOW TABLES) terminates FAILED.
AWS: SHOW TABLES reaches SUCCEEDED with SubstatementType SHOW_TABLES (Smithy: QueryExecution.StatementType UTILITY, QueryExecution.SubstatementType).
Emulator: State FAILED, SubstatementType absent.
Why: catalog-browsing clients that drive Athena with SHOW statements get a failure for every listing.
Fix: the athena crate's query-execution engine (UTILITY statement handling).
Traces: athena-kfi-traj-athena-query-context-mistakes#task#11.

Cause 8 of 10: a SELECT over an Iceberg table terminates FAILED.
AWS: SUCCEEDED with SubstatementType SELECT and the planning/processing statistics populated.
Emulator: FAILED with those members absent.
Why: the Iceberg read path reports failure for data AWS returns.
Fix: the athena crate's query-execution engine (Iceberg/table-format read path).
Traces: athena-kfi-traj-athena-reads-the-iceberg-table#task#10.

Cause 9 of 10: ListQueryExecutions returns an empty body.
AWS: Smithy: ListQueryExecutions output QueryExecutionIds (QueryExecutionIdList, @length 1..50), most-recent-first. After submitting N queries in a workgroup the response lists their ids.
Emulator: HTTP 200 with an empty JSON object — QueryExecutionIds is not serialized at all.
Why: a client cannot enumerate or page query history; pagination over the operation is impossible.
Fix: the athena crate's ListQueryExecutions handler and its query-execution state store.
Traces: athena-batch-query-submission-and-pagination#task#5, athena-workgroup-lifecycle-and-query-execution#task#4.

Cause 10 of 10: BatchGetQueryExecution misreports both halves of its response.
AWS: Smithy: BatchGetQueryExecution output QueryExecutions + UnprocessedQueryExecutionIds (UnprocessedQueryExecutionId: QueryExecutionId, ErrorCode, ErrorMessage). A known id is returned fully populated (State SUCCEEDED, SubstatementType, ResultReuseConfiguration, the ResultReuse/pre-processing/processing statistics, OutputLocation of the result object), and an unknown id appears in UnprocessedQueryExecutionIds with ErrorCode QUERY_EXECUTION_NOT_FOUND.
Emulator: the known entry carries State FAILED, omits SubstatementType/ResultReuseConfiguration/Statistics.ResultReuseInformation/ServicePreProcessingTimeInMillis/ServiceProcessingTimeInMillis and gives the directory prefix as OutputLocation; the unknown entry carries ErrorCode INVALID_INPUT instead of QUERY_EXECUTION_NOT_FOUND.
Why: a client batching status lookups cannot tell 'no such execution' from 'bad request', and sees successful executions as failed.
Fix: the athena crate's BatchGetQueryExecution handler and its unprocessed-id classification.
Traces: athena-cats-traj-query-execution-polling-reads#task#9.

The open issue 20261023T114649Z-athena-startqueryexecution-stub.md covers StartQueryExecution as an unimplemented scaffold; the operation now answers on the wire, so it does not cover any cause above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.
Cause 1 — athena-batch-query-submission-and-pagination#task#2, StartQueryExecution: expected status 200 with $.QueryExecutionId; actual 400, $.QueryExecutionId missing (cluster of 7 steps).
Cause 2 — athena-cats-traj-ddl-identifier-syntax-mistakes#task#3, StartQueryExecution: expected 400 with $.__type=InvalidRequestException, $.AthenaErrorCode=MALFORMED_QUERY, $.ErrorCode=MALFORMED_QUERY, $.Message="line 1:8: mismatched input 'DATABASE'. Expecting: 'MATERIALIZED', 'MULTI', 'OR', 'PROTECTED', 'ROLE', 'SCHEMA', 'TABLE', 'VIEW'"; actual 200 with all four absent (4 steps).
Cause 3 — athena-cats-traj-ddl-identifier-syntax-mistakes#task#8, GetQueryExecution: expected $.QueryExecution.Status.State=SUCCEEDED and SubstatementType=CREATE_DATABASE; actual State=FAILED (poll: 'FAILED' is in fail_on after 3 attempts), SubstatementType/ResultReuseConfiguration/Statistics.ResultReuseInformation/ServicePreProcessingTimeInMillis/ServiceProcessingTimeInMillis missing (12 steps).
Cause 4 — athena-cats-traj-drop-database-cleanup#task#7, GetQueryExecution: expected AthenaError.ErrorType=1006 and AthenaError.ErrorMessage='Cannot drop database <db> as its not empty.' with StateChangeReason='FAILED: Execution Error, return code 1 from org.apache.hadoop.hive.ql.exec.DDLTask. MetaException(message:Unable to drop database: InvalidOperationException(message:Database <db> is not empty.))'; actual ErrorType missing and both texts = 'near "DATABASE": syntax error in DROP DATABASE <db> at offset 5' (9 steps).
Cause 5 — athena-cats-traj-table-location-mistakes#task#16, GetQueryExecution: expected State=SUCCEEDED, SubstatementType=SELECT, Statistics.DataScannedInBytes=92; actual State=FAILED, DataScannedInBytes=0, QueryPlanningTimeInMillis missing.
Cause 6 — athena-kfi-traj-athena-query-context-mistakes#task#9, GetQueryExecution: expected AthenaError.ErrorType=1301 and ErrorMessage/StateChangeReason="SCHEMA_NOT_FOUND: line 1:22: Schema '<db>' does not exist", QueryExecutionContext.Catalog='awsdatacatalog'; actual ErrorType missing, both texts='no such table: <table>', Catalog='AwsDataCatalog'.
Cause 7 — athena-kfi-traj-athena-query-context-mistakes#task#11, GetQueryExecution: expected State=SUCCEEDED, SubstatementType=SHOW_TABLES; actual FAILED, absent.
Cause 8 — athena-kfi-traj-athena-reads-the-iceberg-table#task#10, GetQueryExecution: expected State=SUCCEEDED, SubstatementType=SELECT; actual FAILED, absent.
Cause 9 — athena-batch-query-submission-and-pagination#task#5, ListQueryExecutions: expected 200 {'QueryExecutionIds': [three ids]}; actual 200 {} (empty_body, 2 steps).
Cause 10 — athena-cats-traj-query-execution-polling-reads#task#9, BatchGetQueryExecution: expected QueryExecutions[0].Status.State=SUCCEEDED, SubstatementType=SHOW_DATABASES and UnprocessedQueryExecutionIds[0].ErrorCode=QUERY_EXECUTION_NOT_FOUND; actual State=FAILED, SubstatementType missing, ErrorCode=INVALID_INPUT.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Inspect the listed fixture#step pairs, representative athena-batch-query-submission-and-pagination#task#2.
4. Minimal manual repro: StartQueryExecution with a valid QueryString, a ResultConfiguration.OutputLocation and an existing WorkGroup (expect 200 + QueryExecutionId), then poll GetQueryExecution (expect State SUCCEEDED, SubstatementType set), then ListQueryExecutions for that workgroup (expect the id listed) and BatchGetQueryExecution with that id plus an unknown id (expect the unknown one in UnprocessedQueryExecutionIds with ErrorCode QUERY_EXECUTION_NOT_FOUND).
