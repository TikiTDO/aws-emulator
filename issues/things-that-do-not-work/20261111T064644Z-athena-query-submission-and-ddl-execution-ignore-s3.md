Issue-Source: trace-sweep-divergence
Issue-Trace: athena-cats-traj-table-location-mistakes
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-cats-traj-table-location-mistakes athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket athena-cats-traj-output-location-mistakes athena-cats-traj-drop-database-cleanup athena-cats-traj-table-before-database-and-ddl-retries
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Status.State
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Status.StateChangeReason
Issue-Key: athena|StartQueryExecution|4xx->2xx|$.AthenaErrorCode
Issue-Key: athena|StartQueryExecution|4xx->2xx|$.ErrorCode
Issue-Key: athena|StartQueryExecution|4xx->2xx|$.Message
Issue-Key: athena|StartQueryExecution|4xx->2xx|$.__type
Issue-Key: athena|StartQueryExecution|4xx->2xx|status
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Statistics.DataScannedInBytes
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Status.AthenaError.ErrorMessage
Issue-Related: 20261110T203524Z-athena-ignores-the-ddl-location-bucket-and-refuses.md
Issue-Related: 20261111T064644Z-athena-getqueryexecution-responses-omit-recorded-members-queryexecutiondetail-managed.md
Issue-Commit: f456950adb04cb9e7696776dd0db02907fd8875a

# Athena query submission and DDL execution ignore S3 location validity and re-word Hive failure text (6 causes)

> _Note: this issue was identified against the code base at commit `f456950adb04cb9e7696776dd0db02907fd8875a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
Six net-new root causes in the Athena query-submission and query-execution path: S3 locations are never validated (neither the result output location nor a DDL LOCATION), statements that must succeed settle FAILED, and the two Hive DDL failure messages are re-authored. All six must be closed before this issue is submitted; each names its own fixture/step.

Cause 1 of 6 — StartQueryExecution accepts a ResultConfiguration.OutputLocation that is not an S3 path (fixture athena-cats-traj-output-location-mistakes, step task#4).
AWS: StartQueryExecution validates the requested result location before queueing the query and refuses a non-S3 path synchronously with HTTP 400 InvalidRequestException, AthenaErrorCode/ErrorCode 'INVALID_INPUT' and Message 'outputLocation is not a valid S3 path.' — that message is entirely fixed text, it interpolates nothing from the request. Smithy: StartQueryExecution input ResultConfiguration -> ResultConfiguration$OutputLocation ('The location in Amazon S3 where your query and calculation results are stored, such as s3://path/to/query/bucket/'); modeled error InvalidRequestException (httpError 400, members AthenaErrorCode and Message); the ErrorCode duplicate of AthenaErrorCode is recorded behavior.
Emulator: the call is accepted with HTTP 200 and a QueryExecutionId — no __type, no AthenaErrorCode, no ErrorCode, no Message.
Why: a client that typos a result location (or passes a filesystem path/https URL) is told the query was submitted. It then polls an execution whose results can never be written anywhere readable, instead of correcting the request on the synchronous 400 AWS gives it.
Fix: StartQueryExecution input validation for ResultConfiguration.OutputLocation, which must reject a non-s3:// path with InvalidRequestException/INVALID_INPUT before any execution is created.
Traces: athena-cats-traj-output-location-mistakes#task#4.

Cause 2 of 6 — a CREATE [EXTERNAL] TABLE whose LOCATION names a bucket that does not exist settles SUCCEEDED instead of FAILED (fixtures athena-cats-traj-table-location-mistakes step task#10 and athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket step task#8).
AWS: Athena resolves the DDL LOCATION against S3 before committing the statement; when that bucket does not exist the execution settles terminally FAILED with QueryExecution.Status.StateChangeReason 'The bucket specified in the ddl statement does not exist' (wholly fixed text) and QueryExecution.Status.AthenaError = {ErrorCategory 2, ErrorType 1306, Retryable false, ErrorMessage 'The bucket specified in the ddl statement does not exist'}. Smithy: QueryExecutionStatus members State (QueryExecutionState: 'FAILED indicates that the query experienced an error and did not complete processing'), StateChangeReason, AthenaError; AthenaError members ErrorCategory (@range 1-3, 2 = user error), ErrorType (@range 0-9999), Retryable (default false), ErrorMessage. The category/type values are recorded behavior.
Emulator: State 'SUCCEEDED' with no StateChangeReason and no AthenaError member, and the table is committed into the catalog pointing at the unreachable bucket (visible later on the Glue read as StorageDescriptor.Location still naming the absent bucket).
Why: the caller's remediation branch (create the bucket / fix LOCATION) never fires; it proceeds to query a table whose storage cannot be read, and ErrorCategory/Retryable-driven retry logic never sees a user error.
Fix: the StartQueryExecution DDL execution path for CREATE [EXTERNAL] TABLE — resolve the LOCATION bucket against S3 state before the catalog write and settle the execution FAILED with the StateChangeReason and the AthenaError triple.
Traces: athena-cats-traj-table-location-mistakes#task#10 (representative), athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket#task#8 (same cause, second fixture; that cluster also covers the managed-results TimeToLive omission filed separately).

Cause 3 of 6 — a CREATE EXTERNAL TABLE whose LOCATION bucket does exist, submitted with QueryExecutionContext.Database set, settles FAILED instead of SUCCEEDED (fixture athena-cats-traj-table-location-mistakes, step task#13).
AWS: a well-formed Hive CREATE EXTERNAL TABLE naming an unqualified table in the context database, with ROW FORMAT DELIMITED and a LOCATION in an existing bucket, completes: QueryExecution.Status.State 'SUCCEEDED' with no StateChangeReason and no AthenaError. Smithy: StartQueryExecution input QueryExecutionContext ('The database in which the query execution occurred') and QueryString (@length 1-262144); QueryExecutionStatus.State enum ('SUCCEEDED indicates that the query completed without errors').
Emulator: the execution settles State 'FAILED', and the replay's poll aborts because FAILED is terminal. Hypothesis: the emulator's statement engine does not accept this Hive DDL form or does not resolve the unqualified table name against QueryExecutionContext.Database.
Why: the correct-path DDL of the scenario never produces a table, so every subsequent read and SELECT in the client's workflow is operating on catalog state AWS would have created.
Fix: the statement-execution engine behind StartQueryExecution (Hive DDL parsing/execution and QueryExecutionContext.Database name resolution) and the execution state GetQueryExecution reports.
Traces: athena-cats-traj-table-location-mistakes#task#13.

Cause 4 of 6 — a SELECT count(*) over a table whose data objects exist settles FAILED, and its scanned-bytes statistic stays 0 (fixture athena-cats-traj-table-location-mistakes, step task#16).
AWS: the aggregate query completes with QueryExecution.Status.State 'SUCCEEDED' and QueryExecution.Statistics.DataScannedInBytes equal to the number of bytes read from the table's S3 objects (92 in this recording for the fixture's small CSV — a state-derived value, not a constant). Smithy: QueryExecution.Statistics -> QueryExecutionStatistics; QueryExecutionStatus.State enum.
Emulator: State 'FAILED' (poll aborted on the terminal state) and DataScannedInBytes 0.
Why: a read-after-write verification step (row counts, ETL assertions) fails against the emulator for data that is present, and any cost/scan accounting built on DataScannedInBytes reads zero for every query.
Fix: the statement-execution engine for DML over catalog tables, and the statistics it records on the execution.
Traces: athena-cats-traj-table-location-mistakes#task#16.

Cause 5 of 6 — the DROP DATABASE on a database that does not exist reports an emulator-authored AthenaError.ErrorMessage (fixture athena-cats-traj-drop-database-cleanup, step task#15).
AWS: the execution settles FAILED with StateChangeReason 'FAILED: SemanticException [Error 10072]: Database does not exist: <databaseName>' and AthenaError.ErrorMessage 'Cannot drop database <databaseName>, as it does not exist.' — in that ErrorMessage the fixed text is 'Cannot drop database ' + ', as it does not exist.' and the only variable part is the database name taken from the submitted statement. Smithy: AthenaError$ErrorMessage ('Contains a short description of the error that occurred'); the wording itself is recorded behavior.
Emulator: AthenaError.ErrorMessage is 'Database does not exist: <databaseName>' — the Hive StateChangeReason text re-used verbatim instead of Athena's own ErrorMessage wording.
Why: clients and operators that match on the drop-specific wording (or surface it to users) get the generic Hive diagnostic, and the two fields no longer carry the distinct information AWS puts in them.
Fix: the DDL execution path's failure classification for DROP DATABASE, where AthenaError.ErrorMessage is composed (it must not alias StateChangeReason).
Traces: athena-cats-traj-drop-database-cleanup#task#15 (cluster also covers athena-cats-traj-table-before-database-and-ddl-retries#task#5).

Cause 6 of 6 — the CREATE DATABASE on an existing database re-words both failure fields (fixture athena-cats-traj-table-before-database-and-ddl-retries, step task#14).
AWS: the execution settles FAILED with StateChangeReason 'FAILED: Execution Error, return code 1 from org.apache.hadoop.hive.ql.exec.DDLTask. Database <databaseName> already exists' and AthenaError.ErrorMessage 'Database <databaseName> already exists.' — fixed text 'FAILED: Execution Error, return code 1 from org.apache.hadoop.hive.ql.exec.DDLTask. Database ' + ' already exists' for the reason, and 'Database ' + ' already exists.' (with the terminating period) for the ErrorMessage; the database name is the only request-derived part.
Emulator: StateChangeReason wraps the text in a Java exception rendering — '... DDLTask. AlreadyExistsException(message:Database <databaseName> already exists)' — and AthenaError.ErrorMessage drops the terminating period ('Database <databaseName> already exists').
Why: an idempotent create-if-absent client that recognises the already-exists condition from the message text does not match AWS's string and treats a benign re-create as an unknown failure.
Fix: the DDL execution path's failure text for CREATE DATABASE (the StateChangeReason composition and the AthenaError.ErrorMessage wording).
Traces: athena-cats-traj-table-before-database-and-ddl-retries#task#14 (cluster also covers the same fixture's task#17).

Related open issues that do NOT cover these causes: '20261110T203524Z-athena-ignores-the-ddl-location-bucket-and-refuses.md' describes the absent-LOCATION-bucket and GetQueryResults-refusal pair for one fixture but the replay report attributes none of the clusters above to it (the GetQueryExecution clusters here, including the second fixture, come back unowned); '20261104T163600Z-athena-statements-are-executed-by-a-sqlite-engine.md' and '20261104T063259Z-athena-input-validation-returns-validationexception-without-athenaerrorcode-or.md' cover the engine's syntax refusals and input-validation error class generally, not these execution outcomes or these two Hive message texts; '20261108T041805Z-athena-glue-s3-athena-startqueryexecution-the-no-output-location-refusal-message.md' covers the NO output-location refusal, not an invalid S3 path.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — fixture athena-cats-traj-output-location-mistakes, step task#4, StartQueryExecution, expected 400, got 200:
  $.__type: expected 'InvalidRequestException', got None; $.AthenaErrorCode: expected 'INVALID_INPUT', got None; $.ErrorCode: expected 'INVALID_INPUT', got None; $.Message: expected 'outputLocation is not a valid S3 path.' (entirely fixed text), got None.

Cause 2 — fixture athena-cats-traj-table-location-mistakes, step task#10, GetQueryExecution, 200 -> 200:
  $.QueryExecution.Status.State: expected 'FAILED', got 'SUCCEEDED' (poll: 'SUCCEEDED' is in fail_on after 1 attempt)
  $.QueryExecution.Status.StateChangeReason: expected 'The bucket specified in the ddl statement does not exist', got None
  $.QueryExecution.Status.AthenaError: expected {ErrorCategory: 2, ErrorType: 1306, Retryable: false, ErrorMessage: 'The bucket specified in the ddl statement does not exist'}, got None
  Same divergence at athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket#task#8 (CREATE EXTERNAL TABLE IF NOT EXISTS ... LOCATION 's3://<absent-bucket>/'): expected State 'FAILED' + StateChangeReason + AthenaError, got 'SUCCEEDED' after 3 poll attempts.

Cause 3 — fixture athena-cats-traj-table-location-mistakes, step task#13, GetQueryExecution, 200 -> 200:
  $.QueryExecution.Status.State: expected 'SUCCEEDED', got 'FAILED' (poll: 'FAILED' is in fail_on after 3 attempts)
  Statement: CREATE EXTERNAL TABLE items (item_id string, name string, price int) ROW FORMAT DELIMITED FIELDS TERMINATED BY ',' LOCATION 's3://<existing-bucket>/products/' TBLPROPERTIES ('skip.header.line.count'='1'), QueryExecutionContext {Database: '<db>'}.

Cause 4 — fixture athena-cats-traj-table-location-mistakes, step task#16, GetQueryExecution, 200 -> 200:
  $.QueryExecution.Status.State: expected 'SUCCEEDED', got 'FAILED' (poll: 'FAILED' is in fail_on after 3 attempts)
  $.QueryExecution.Statistics.DataScannedInBytes: expected 92 (bytes read from the table's objects in this recording — a state-derived value), got 0
  Statement: SELECT count(*) AS n FROM <db>.items.

Cause 5 — fixture athena-cats-traj-drop-database-cleanup, step task#15, GetQueryExecution, 200 -> 200:
  $.QueryExecution.Status.AthenaError.ErrorMessage: expected 'Cannot drop database <db>, as it does not exist.', got 'Database does not exist: <db>'
  Fixed text of the expected message: 'Cannot drop database ' ... ', as it does not exist.'; variable part: the database name from the DROP DATABASE statement. Recorded StateChangeReason for the same execution: 'FAILED: SemanticException [Error 10072]: Database does not exist: <db>'.

Cause 6 — fixture athena-cats-traj-table-before-database-and-ddl-retries, step task#14, GetQueryExecution, 200 -> 200:
  $.QueryExecution.Status.AthenaError.ErrorMessage: expected 'Database <db> already exists.', got 'Database <db> already exists' (terminating period dropped)
  $.QueryExecution.Status.StateChangeReason: expected 'FAILED: Execution Error, return code 1 from org.apache.hadoop.hive.ql.exec.DDLTask. Database <db> already exists', got 'FAILED: Execution Error, return code 1 from org.apache.hadoop.hive.ql.exec.DDLTask. AlreadyExistsException(message:Database <db> already exists)'
  Fixed text: everything except the database name, which comes from the CREATE DATABASE statement.

## Steps to Reproduce
1. git_clone() the project and boot the emulator built from the checkout.
2. replay_wire_corpus() over trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture athena-cats-traj-output-location-mistakes to step task#4 — StartQueryExecution with ResultConfiguration.OutputLocation set to a value that is not an s3:// path. Observe HTTP 200 with a QueryExecutionId instead of 400 InvalidRequestException / INVALID_INPUT / 'outputLocation is not a valid S3 path.'.
4. Cause 2: replay fixture athena-cats-traj-table-location-mistakes to step task#10 (and athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket to task#8) — submit CREATE EXTERNAL TABLE ... LOCATION 's3://<bucket-that-does-not-exist>/...' and poll GetQueryExecution. Observe State SUCCEEDED with no StateChangeReason/AthenaError.
5. Cause 3: continue athena-cats-traj-table-location-mistakes to step task#13 — submit the same DDL with an unqualified table name, QueryExecutionContext.Database set and a LOCATION in a bucket that does exist, and poll. Observe State FAILED where AWS reports SUCCEEDED.
6. Cause 4: continue to step task#16 — submit SELECT count(*) AS n FROM <db>.items after the data objects are uploaded and poll. Observe State FAILED and Statistics.DataScannedInBytes 0.
7. Cause 5: replay fixture athena-cats-traj-drop-database-cleanup to step task#15 — submit DROP DATABASE for a database that does not exist and read GetQueryExecution. Observe AthenaError.ErrorMessage 'Database does not exist: <db>' instead of 'Cannot drop database <db>, as it does not exist.'.
8. Cause 6: replay fixture athena-cats-traj-table-before-database-and-ddl-retries to step task#14 — submit CREATE DATABASE for a database that already exists and read GetQueryExecution. Observe the AlreadyExistsException(...) wrapper in StateChangeReason and the missing period in AthenaError.ErrorMessage.
