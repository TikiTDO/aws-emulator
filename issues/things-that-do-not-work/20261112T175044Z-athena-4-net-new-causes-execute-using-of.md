Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.execute-prepared-statement-with-parameter athena.prepared-statement-create-and-execute-with-param athena.prepared-statement-execute-with-parameters athena.execute-prepared-statement-from-other-workgroup-rejected athena.op-contract-getPreparedStatement athena.workgroup-scoped-iam-policy-denies-start-query-execution-on-one-workgroup
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena.execute-prepared-statement-with-parameter|run#9|AssertionError|assert execution["Status"]["State"] == "SUCCEEDED", execution["Status"]
Issue-Key: athena.prepared-statement-create-and-execute-with-param|run#11|AssertionError|assert execution["Status"]["State"] == "SUCCEEDED", execution["Status"]
Issue-Key: athena.prepared-statement-execute-with-parameters|run#12|AssertionError|assert state == "SUCCEEDED", f"EXECUTE with a valid parameter must succeed, got {state}"
Issue-Key: athena.execute-prepared-statement-from-other-workgroup-rejected|_error#18|AssertionError|raise AssertionError("the call was expected to raise a ClientError but SUCCEEDED")
Issue-Key: athena.op-contract-getPreparedStatement|run#15|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code}"
Issue-Key: athena.workgroup-scoped-iam-policy-denies-start-query-execution-on-one-workgroup|run#32|KeyError|denied_ids = admin_athena.list_query_executions(WorkGroup=denied_wg)["QueryExecutionIds"]
Issue-Commit: 4561d0722e5c03ef656b5ed57b40071717cb7fdd

# athena: 4 net-new causes — EXECUTE ... USING of a prepared statement is not executed or workgroup-scoped, GetPreparedStatement pattern refusal is an unmodeled ValidationException, and ListQueryExecutions serializes no ids

> _Note: this issue was identified against the code base at commit `4561d0722e5c03ef656b5ed57b40071717cb7fdd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
Four distinct defects on the Athena prepared-statement execution path and its neighbouring reads. 4 causes.

Cause 1 of 4: StartQueryExecution does not implement the `EXECUTE <statement_name> USING <args>` statement at all.
AWS: Smithy: StartQueryExecution.QueryString (QueryString, @length 1..262144, required); errors InvalidRequestException (400), TooManyRequestsException (400), InternalServerException (500). Recorded behavior: a statement created by CreatePreparedStatement whose QueryStatement carries positional `?` placeholders (e.g. `SELECT ? AS val`, `SELECT ? AS n, ? AS s`) is executed by submitting `EXECUTE <stmt> USING 777` / `EXECUTE <stmt> USING 42, 'hi'` through StartQueryExecution in the owning workgroup; the execution reaches Status.State SUCCEEDED and GetQueryResults returns the SUBSTITUTED values in order — the header row holds the statement's column aliases and the first data row holds the bound arguments rendered as VarCharValue ('777'; '42' and 'hi'). Over a real table, `EXECUTE <stmt> USING 2` on `SELECT name FROM <db>.<tbl> WHERE id = ?` selects exactly the id=2 row and reports Statistics.DataScannedInBytes > 0.
Emulator: the submission is accepted (an id is minted) but the execution reaches State FAILED with StateChangeReason / AthenaError.ErrorMessage `near "EXECUTE": syntax error in EXECUTE <stmt> USING <args> at offset 0` — the raw EXECUTE text is handed to the in-process SQL engine, which has no EXECUTE grammar. Fixed text of that message is the emulator's own SQLite-shaped diagnostic; the statement name and argument list in it are the request's QueryString. No prepared statement is looked up and no parameter is bound.
Why: every client that uses Athena prepared statements — the documented way to parameterize SQL and the pattern BI tools and JDBC/ODBC drivers emit — gets a FAILED query instead of rows, and the failure is reported as a syntax error in the user's SQL, so the client is told its own statement is malformed rather than that the feature is missing.
Fix: the athena crate's StartQueryExecution handler (services/service-com-amazonaws-athena-amazonathena/src/ops/start_query_execution.rs) and the statement reader/executor behind it (catalog_sql::statement::read / catalog_sql::execute): EXECUTE must resolve the named prepared statement from the workgroup's prepared-statement store, bind the USING arguments positionally into the stored QueryStatement's `?` placeholders, and run the substituted statement.
Traces: athena.execute-prepared-statement-with-parameter, athena.prepared-statement-create-and-execute-with-param, athena.prepared-statement-execute-with-parameters.

Cause 2 of 4: EXECUTE of a prepared statement from a workgroup that does not own it is accepted instead of refused synchronously.
AWS: Smithy: StartQueryExecution models InvalidRequestException (HTTP 400, "Indicates that something is wrong with the input to the request"); prepared statements are scoped to a workgroup (GetPreparedStatement.WorkGroup is required). Recorded behavior: submitting `EXECUTE <stmt> USING <arg>` with WorkGroup set to a workgroup that does not hold <stmt> is refused by StartQueryExecution itself — no QueryExecutionId is returned — with Code InvalidRequestException and the message, whose text is fixed with no variable part: "PreparedStatement was not found in workGroup". The same EXECUTE in the owning workgroup succeeds, so the refusal is about scope, not about the statement.
Emulator: the call SUCCEEDS and returns a QueryExecutionId (the check's helper reports "the call was expected to raise a ClientError but SUCCEEDED"). StartQueryExecution validates the workgroup's existence and state but never resolves the EXECUTE's statement name against the named workgroup's prepared-statement store.
Why: a client gets a 200 and an execution id for a statement the workgroup cannot run, so a cross-workgroup mistake surfaces later (or not at all) instead of as the documented 400; workgroup isolation of prepared statements is not enforced, which is the boundary multi-tenant setups rely on.
Fix: the athena crate's StartQueryExecution handler (src/ops/start_query_execution.rs) — resolve the EXECUTE target in the request's workgroup before minting an execution and return InvalidRequestException with AWS's message when it is absent; the lookup belongs next to the prepared-statement state read used by src/ops/get_prepared_statement.rs.
Traces: athena.execute-prepared-statement-from-other-workgroup-rejected.

Cause 3 of 4: GetPreparedStatement refuses a StatementName that violates the modeled pattern with an unmodeled ValidationException.
AWS: Smithy: GetPreparedStatement.StatementName (StatementName, required, @length 1..256, @pattern ^[a-zA-Z_][a-zA-Z0-9_@:]{1,256}$); the operation's modeled errors are InternalServerException, InvalidRequestException, ResourceNotFoundException — ValidationException is not among them. Recorded behavior: a name containing hyphens (pattern-violating) is refused with Code InvalidRequestException and the message "1 validation error detected: Value at 'statementName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z_][a-zA-Z0-9_@:]{1,256}" — the fixed text is "N validation error(s) detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", with the member name ('statementName') and the pattern taken from the model and the count from the request.
Emulator: the refusal carries Code ValidationException (an error shape this operation does not model), so the error code a client switches on is wrong. This is the same class as open issue 20261104T163600Z-athena-constraint-refusals-raise-an-unmodeled-validationexception-instead.md, which does not carry this check's replay key; it is listed here because this check is grounded in GetPreparedStatement, whose own constraint refusal is still wrong.
Why: a client that catches InvalidRequestException (the modeled client error) sees an unmodeled code and treats a plain input error as an unknown/retryable failure; SDK error-shape dispatch on a code outside the operation's model falls through to generic handling.
Fix: the athena crate's centralized input-validation layer as it serves GetPreparedStatement (src/ops/get_prepared_statement.rs plus the generated validate step it is driven through) — pattern/length refusals must be emitted as the operation's modeled InvalidRequestException with AWS's validation-error message.
Traces: athena.op-contract-getPreparedStatement.

Cause 4 of 4: ListQueryExecutions returns a 200 with no QueryExecutionIds member.
AWS: Smithy: ListQueryExecutions output QueryExecutionIds (QueryExecutionIdList) and NextToken; input WorkGroup scopes the listing. Recorded behavior: ListQueryExecutions(WorkGroup=W) always serializes QueryExecutionIds — an empty list for a workgroup in which nothing ran, and the submitted ids (most recent first) once queries exist.
Emulator: the 200 response omits QueryExecutionIds entirely, so a client dereferencing it raises KeyError 'QueryExecutionIds'. Reached here after an AccessDeniedException refusal, when the check asserts the refused submission left no execution behind.
Why: query history cannot be enumerated or paged, and the negative assertion a caller needs — that a refused or failed submission created no execution — cannot be made at all.
Fix: the athena crate's ListQueryExecutions handler (src/ops/list_query_executions.rs), its workgroup scoping and the query-execution store it reads; the member must always be serialized, empty list included.
Note: this is the same root cause already described as Cause 5 of open issue 20261104T163600Z-athena-result-and-statistics-representation-is-wrong-boolean.md, which does not carry this check's replay key — re-replay the check named below as well when closing it.
Traces: athena.workgroup-scoped-iam-policy-denies-start-query-execution-on-one-workgroup.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout (service heading: athena).

Cause 1 — athena.execute-prepared-statement-with-parameter: CreatePreparedStatement(StatementName=<stmt>, WorkGroup='primary', QueryStatement='SELECT ? AS val'), then StartQueryExecution(QueryString='EXECUTE <stmt> USING 777'); expected Status.State SUCCEEDED and the data cell '777'. Actual: {'State': 'FAILED', 'StateChangeReason': 'near "EXECUTE": syntax error in EXECUTE <stmt> USING 777 at offset 0', 'AthenaError': {'ErrorCategory': 2, 'Retryable': False, 'ErrorMessage': 'near "EXECUTE": syntax error in EXECUTE <stmt> USING 777 at offset 0'}}.
  athena.prepared-statement-create-and-execute-with-param: QueryStatement='SELECT ? AS n, ? AS s', EXECUTE <stmt> USING 42, 'hi'; expected header ['n','s'] and data ['42','hi']. Actual: State FAILED, 'near "EXECUTE": syntax error in EXECUTE <stmt> USING 42, \'hi\' at offset 0'.
  athena.prepared-statement-execute-with-parameters: 'EXECUTE with a valid parameter must succeed, got FAILED'.

Cause 2 — athena.execute-prepared-statement-from-other-workgroup-rejected: two identical workgroups, the statement created only in wg_owner; StartQueryExecution(QueryString='EXECUTE <stmt> USING 2', WorkGroup=wg_other) must raise InvalidRequestException with Message 'PreparedStatement was not found in workGroup'. Actual: AssertionError 'the call was expected to raise a ClientError but SUCCEEDED' — a QueryExecutionId was returned.

Cause 3 — athena.op-contract-getPreparedStatement: GetPreparedStatement(StatementName='sim2real-missing-stmt', WorkGroup=<wg>) — expected Code InvalidRequestException with Message "1 validation error detected: Value at 'statementName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z_][a-zA-Z0-9_@:]{1,256}". Actual: AssertionError 'expected InvalidRequestException, got ValidationException'.

Cause 4 — athena.workgroup-scoped-iam-policy-denies-start-query-execution-on-one-workgroup: after the scoped-policy refusal, admin ListQueryExecutions(WorkGroup=denied_wg) must return QueryExecutionIds == []. Actual: [error] KeyError: 'QueryExecutionIds' on the ListQueryExecutions response.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay suite simparity-behavioral-aws-v1 and inspect the athena checks listed in trace_ids.
3. Minimal manual repro:
   a. CreateWorkGroup(W) with a ResultConfiguration.OutputLocation; CreatePreparedStatement(StatementName=S, WorkGroup=W, QueryStatement='SELECT ? AS n, ? AS s'); StartQueryExecution(QueryString="EXECUTE S USING 42, 'hi'", WorkGroup=W) — poll GetQueryExecution: State must be SUCCEEDED and GetQueryResults must return header ['n','s'] and row ['42','hi'], not a FAILED query with a 'near "EXECUTE": syntax error' reason.
   b. CreateWorkGroup(W2) (which does not hold S) and StartQueryExecution(QueryString='EXECUTE S USING 2', WorkGroup=W2) — the call itself must be refused with InvalidRequestException / 'PreparedStatement was not found in workGroup' and must mint no QueryExecutionId; the identical call on W must still succeed.
   c. GetPreparedStatement(StatementName='sim2real-missing-stmt', WorkGroup=W) — must be InvalidRequestException with AWS's "1 validation error detected: Value at 'statementName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z_][a-zA-Z0-9_@:]{1,256}", not ValidationException.
   d. ListQueryExecutions(WorkGroup=W3) for a workgroup in which nothing ran — the 200 response must carry QueryExecutionIds as an empty list; after one query runs there it must carry that id.
