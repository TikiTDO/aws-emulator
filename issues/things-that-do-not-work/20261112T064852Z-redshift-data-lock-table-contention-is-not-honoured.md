Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift-data.cancel-statement-running-then-finished-rejected
Issue-Shape-Id: com.amazonaws.redshiftdata#RedshiftData
Issue-Key: redshift-data.cancel-statement-running-then-finished-rejected|_poll_until#11|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last={last!r})")
Issue-Commit: 63b6e8e2cf3e66c0feda0c3bd64d85b077c87d33

# redshift-data: LOCK TABLE contention is not honoured — a SELECT blocked by another session's ACCESS EXCLUSIVE lock FINISHES immediately, so no statement is ever observable in a running state and the CancelStatement lifecycle (cancel -> ABORTED, second cancel refused) is unreachable (1 cause)

> _Note: this issue was identified against the code base at commit `63b6e8e2cf3e66c0feda0c3bd64d85b077c87d33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelStatement

## Description
One net-new root cause in the redshift-data (Redshift Data API) emulator, exercised against a self-provisioned Redshift Serverless namespace + workgroup.

Cause 1 of 1: a statement that contends with another session's ACCESS EXCLUSIVE table lock is not made to wait — it is executed and reported terminal (FINISHED) on the first DescribeStatement poll. Because no statement is ever observable in a non-terminal state, the whole CancelStatement contract behind it (cancel a running statement -> Status true -> ABORTED, then a second cancel on the now-terminal statement refused) can never be reached by a client.

AWS: a session opened with ExecuteStatement(SessionKeepAliveSeconds=...) that runs `BEGIN; LOCK TABLE <t> IN ACCESS EXCLUSIVE MODE;` holds that lock for the life of its open transaction. A `SELECT * FROM <t>` submitted on a different (default/new) session must then block on the lock: DescribeStatement reports Status `STARTED` or `PICKED` (both modeled StatusString values) and keeps reporting a non-terminal status until the statement is cancelled or the holder commits/rolls back. CancelStatement on that running statement answers `Status: true` (Smithy: CancelStatement output member Status, Boolean) and the statement then lands Status `ABORTED`. A second CancelStatement on the now-terminal statement is refused with error code `ValidationException` at HTTP 400 (Smithy: CancelStatement errors include ValidationException, errorKind client, httpError 400) and the message "Could not cancel a query that is already in ABORTED state with ID: <Id>" — the fixed text is "Could not cancel a query that is already in ", " state with ID: "; the variable parts are the statement's current terminal state name taken from emulator/service state (ABORTED here) and the statement `Id` taken from the request. The rejected cancel has no effect: DescribeStatement still reports ABORTED. Recorded behavior supplies the lifecycle and that message; Smithy supplies the error's code/status and the Status/StatusString members.

Emulator: the contending SELECT is never non-terminal. On polling DescribeStatement for up to 30 s with a 1 s interval, every response is already terminal: Status `FINISHED`, HasResultSet true, Duration ~1.7e5 ns, ExecutionMode `AUTO_COMMIT`, HTTP 200 — i.e. the lock the holder session took is not held across calls and the reader is not blocked by it. The check therefore fails at its first gate ("condition not met within 30s") and never gets to issue CancelStatement at all, so the cancel/abort/second-cancel assertions below it are ungraded.

Why: a client relying on AWS's behavior gets two things wrong. (a) Concurrency is unsafe: a reader that AWS makes wait for an exclusive writer instead runs immediately against the locked table, so the emulator cannot reproduce lock waits, lock timeouts, or any workload whose correctness depends on ACCESS EXCLUSIVE serialisation — code that is correct on AWS silently reads through a lock here. (b) The cancel contract is unobservable: because statements are always already terminal by the time a caller can look, there is no way to verify that a running statement is cancellable, that a cancel drives it to ABORTED, or that re-cancelling a terminal statement is refused ValidationException/400 with AWS's sentence. Retry/cancel logic calibrated against AWS cannot be exercised or validated.

Fix: statement lifecycle and lock accounting in the redshift-data state/execution layer — the submit and transition path that decides when a submission becomes terminal (services/service-com-amazonaws-redshiftdata-redshiftdata/src/state.rs, the execution-scheduling / due-transition materialisation, together with src/ops/execute_statement.rs) must keep a statement that cannot acquire its required table lock in a non-terminal status (STARTED/PICKED) rather than executing it, and a session's open transaction must retain its locks between calls instead of auto-committing. The already-terminal cancel refusal belongs in src/ops/cancel_statement.rs, which must serve ValidationException at HTTP 400 with AWS's wording and leave the statement's status untouched.

Traces: redshift-data.cancel-statement-running-then-finished-rejected.

Related open issue, which does NOT cover this check (the suite does not mark this check as owned by it; its recorded key for this check is a 60 s harness timeout with no verdict, whereas the failure now reproduces as a concrete terminal-status mismatch inside the 30 s poll): 20261111T072724Z-redshift-data-4-net-new-causes-no-result.md. Other related open issues, none of which cover lock contention or the cancel lifecycle: 20261111T072724Z-redshift-data-api-on-a-provisioned-cluster-sessionid.md (SessionId isolation on a provisioned cluster), 20261109T193330Z-redshift-data-api-serverless-workgroup-statement-responses-diverge.md (statement response members), 20261108T070550Z-redshift-data-the-result-reader-format-mismatch-validationexception.md (result-reader message).

## Evidence
Cause 1 — redshift-data.cancel-statement-running-then-finished-rejected

Expected (AWS): after session 1 runs `BEGIN; LOCK TABLE <t> IN ACCESS EXCLUSIVE MODE;` with SessionKeepAliveSeconds=120 and reaches FINISHED, a `SELECT * FROM <t>` submitted on a NEW default session is observable with DescribeStatement Status in (STARTED, PICKED); CancelStatement(Id) -> {"Status": true}; DescribeStatement -> Status ABORTED; a second CancelStatement(Id) -> HTTP 400, Error.Code `ValidationException`, Error.Message "Could not cancel a query that is already in ABORTED state with ID: <Id>"; DescribeStatement still ABORTED.

Actual (emulator): the SELECT is terminal on every poll, so the lifecycle is never entered —
  AssertionError: condition not met within 30s (last={'Id': '<uuid>', 'Database': 'dev', 'Duration': 169056, 'Status': 'FINISHED', 'RedshiftPid': 1000, 'HasResultSet': True, 'QueryString': 'SELECT * FROM sim2real_lock_<suffix>', 'ResultRows': 0, 'ResultSize': 0, 'RedshiftQueryId': 3, 'WorkgroupName': 'sim2real-rswg-<suffix>', 'ResultFormat': 'json', 'ExecutionMode': 'AUTO_COMMIT', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}})
(Ids, names, timestamps and durations above are per-run values; the requirement is that the status is non-terminal, not these literals.)

Note the holder statement itself reports FINISHED and is accepted, so the divergence is not a parse/refusal of LOCK TABLE — it is that the lock has no effect on a concurrent reader and the contending statement is executed and completed instead of being made to wait.

Consequently ungraded by this failure (never reached): CancelStatement -> Status true; transition to ABORTED; the second-cancel ValidationException/400 message; and the no-effect assertion that the rejected cancel leaves the status ABORTED.

## Steps to Reproduce
Setup: create a Redshift Serverless namespace (dbName "dev") and a workgroup (baseCapacity 8) in it and wait for status AVAILABLE; use that workgroup name and database "dev" for every redshift-data call below.

1. ExecuteStatement(WorkgroupName=<wg>, Database="dev", Sql="CREATE TABLE t (id INT)"); poll DescribeStatement until FINISHED.
2. ExecuteStatement(WorkgroupName=<wg>, Database="dev", Sql="BEGIN; LOCK TABLE t IN ACCESS EXCLUSIVE MODE;", SessionKeepAliveSeconds=120); keep the returned SessionId; poll DescribeStatement until FINISHED. The holder session stays open with the lock held.
3. ExecuteStatement(WorkgroupName=<wg>, Database="dev", Sql="SELECT * FROM t") with NO SessionId (a new, default session). Poll DescribeStatement(Id) every 1 s for 30 s.
   - AWS: Status is STARTED or PICKED for the whole window (the statement is blocked on the lock).
   - Emulator: the first poll already returns Status FINISHED, HasResultSet true, ExecutionMode AUTO_COMMIT, HTTP 200.
4. (AWS only, unreachable on the emulator) CancelStatement(Id=<blocked id>) -> {"Status": true}; poll DescribeStatement until Status == ABORTED.
5. (AWS only, unreachable on the emulator) CancelStatement(Id=<same id>) again -> HTTP 400, ValidationException, "Could not cancel a query that is already in ABORTED state with ID: <Id>"; DescribeStatement still reports ABORTED.
6. Cleanup: ExecuteStatement(Sql="ROLLBACK;", SessionId=<holder session>), then delete the workgroup and namespace.
