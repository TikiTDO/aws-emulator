Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.data-api-session-transaction-hides-uncommitted-rows-from-a-second-session redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected
Issue-Key: redshift.data-api-session-transaction-hides-uncommitted-rows-from-a-second-session|run#22|AssertionError|assert b_sees == 0, (
Issue-Key: redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected|run#18|AssertionError|assert status == 400, f"expected HTTP 400, got {status!r}"
Issue-Commit: df0e92648e2e716917662ffab5a7edf9e8dd8fc2

# Redshift Data API on a provisioned cluster: SessionId does not isolate a connection (uncommitted rows leak across sessions), and the no-result GetStatementResult refusal is served at HTTP 404 instead of 400 (2 causes)

> _Note: this issue was identified against the code base at commit `df0e92648e2e716917662ffab5a7edf9e8dd8fc2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Two net-new root causes in the Redshift Data API as exercised against a provisioned Redshift cluster. Both are served by the redshift-data crate (services/service-com-amazonaws-redshiftdata-redshiftdata), but they are reported under the `redshift` suite heading, so they are filed here; the same no-result status defect is filed separately under the `redshift-data` heading because an issue carries one service token.

Cause 1 of 2: ExecuteStatement's SessionId is cosmetic — every statement runs on one shared auto-committing connection, so an UNCOMMITTED row written inside session A's open transaction is immediately visible to a second session.
AWS: ExecuteStatement models `SessionKeepAliveSeconds` (range 0..86400) and `SessionId` (UUID) and returns `SessionId` in its output; a session is a database connection kept open across statements. Statements submitted on DIFFERENT SessionIds run on DIFFERENT connections, so a transaction opened on one session (BEGIN ... INSERT with no COMMIT) is invisible to any other session until it commits — standard PostgreSQL/Redshift read-committed isolation. Smithy: ExecuteStatement input members SessionId/SessionKeepAliveSeconds, output member SessionId. Isolation itself is recorded behavior only (the model does not state it).
Emulator: session B's SELECT count(*) returns 1 for a row session A inserted inside a still-open transaction (expected 0). The emulator therefore runs both sessions' statements on a single shared, auto-committing connection, and DescribeStatement reports ExecutionMode AUTO_COMMIT for session work.
Why: a client that uses sessions for transactional work gets no isolation and no atomicity: another session (another tenant, another worker) reads dirty, uncommitted data, and a ROLLBACK cannot undo what other readers already saw. Any read-your-own-writes/transaction test that passes here fails on real Redshift and vice-versa.
Fix: the session and connection management in the redshift-data state store (services/service-com-amazonaws-redshiftdata-redshiftdata/src/state.rs) plus the submit path in src/ops/execute_statement.rs — a statement carrying a SessionId must execute on that session's own connection, with transaction control (BEGIN/COMMIT/ROLLBACK) scoped to it, instead of on one shared auto-commit connection.
Traces: redshift.data-api-session-transaction-hides-uncommitted-rows-from-a-second-session.

Cause 2 of 2: GetStatementResult for a terminal statement that produced no result set is refused ResourceNotFoundException at HTTP 404; the service answers this refusal at HTTP 400.
AWS: reading the result of an ABORTED (or FAILED, or FINISHED-without-a-result-set) statement is refused with error code `ResourceNotFoundException` and the fixed message "Query does not have result. Please check query status with DescribeStatement." (no variable part), at HTTP status 400. Recorded behavior: the replayed check asserts status 400 for exactly this error code and message. Note the Smithy model declares `ResourceNotFoundException` with @httpError(404) — that is the shape's default rendering; on this path the service answers the client-input status 400, which is why the emulator already has a status-override seam for the sibling "Query does not exist." refusal (state.rs UNKNOWN_QUERY_STATUS = 400, served via Scaffolded::modeled_with_status).
Emulator: the error code and the message match, but the response status is 404. Replay: "AssertionError: expected HTTP 400, got 404" after the code assertion for ResourceNotFoundException passed.
Why: clients and SDK retry/classification layers branch on the HTTP status: a 404 is treated as a resource-addressing error (often non-retryable "gone"/routing failure, and in some HTTP stacks surfaced before the JSON error body is parsed), while AWS's 400 marks it a client-input failure for a statement that exists but has no result. Error-handling code and proxies calibrated against AWS take the wrong branch.
Fix: the GetStatementResult handler's no-result refusal (services/service-com-amazonaws-redshiftdata-redshiftdata/src/ops/get_statement_result.rs, the `!statement.has_fetchable_result()` terminal branch around lines 215-233) must serve the modeled error at the status the service uses, the way the unknown-query path in the same file already does (no_such_query/UNKNOWN_QUERY_STATUS); the error code and message must not change.
Traces: redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected.

## Evidence
Cause 1 — check redshift.data-api-session-transaction-hides-uncommitted-rows-from-a-second-session:
  AssertionError: read 1 of 3: session B must NOT see session A's UNCOMMITTED row: expected 0, got 1. A non-zero count means SessionId is cosmetic and every statement is running on one shared auto-committing connection
  Expected: session B's SELECT count(*) == 0 while session A's transaction is open. Actual: 1.

Cause 2 — check redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected:
  rs_data.get_statement_result(Id=<aborted statement id>) ->
  Expected: code ResourceNotFoundException, HTTP 400, message "Query does not have result. Please check query status with DescribeStatement."
  Actual:   code ResourceNotFoundException, HTTP 404 (same message).
  AssertionError: expected HTTP 400, got 404
  Emulator source: src/ops/get_statement_result.rs returns the modeled ResourceNotFoundException with no status override, so it renders at the shape's @httpError(404); the sibling "Query does not exist." refusal in the same file is served at 400 via Scaffolded::modeled_with_status (state.rs UNKNOWN_QUERY_STATUS).

## Steps to Reproduce
Cause 1:
1. CreateCluster a single-node Redshift cluster and wait for `available`.
2. redshift-data ExecuteStatement with SessionKeepAliveSeconds=900 and Sql="CREATE TABLE t (n INT)"; wait FINISHED. Note SessionId (call it A).
3. ExecuteStatement with SessionId=A, Sql="BEGIN; INSERT INTO t VALUES (1);" (no COMMIT); wait FINISHED.
4. ExecuteStatement WITHOUT SessionId (or with a different SessionId) on the same cluster, Sql="SELECT count(*) FROM t"; wait FINISHED and GetStatementResult.
5. AWS returns 0 (the row is uncommitted and invisible to the other session); the emulator returns 1.

Cause 2:
1. On the same cluster, submit a long-running INSERT via ExecuteStatement and wait until DescribeStatement reports STARTED.
2. CancelStatement(Id=<that id>); poll DescribeStatement until Status == ABORTED.
3. GetStatementResult(Id=<that id>).
4. AWS: HTTP 400, ResourceNotFoundException, "Query does not have result. Please check query status with DescribeStatement." Emulator: HTTP 404 with the same code and message.
