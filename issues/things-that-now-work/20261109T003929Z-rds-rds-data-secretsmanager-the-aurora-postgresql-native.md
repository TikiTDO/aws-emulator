Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit
Issue-Key: rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit|_rows#2|ProgramLimitExceeded|cur.execute(statement, args)
Issue-Commit: 0d0b5d4835067c793fae3be07582601671bbaf82

# rds__rds-data__secretsmanager: the Aurora PostgreSQL native endpoint now parses SELECT … FOR UPDATE NOWAIT but refuses it with SQLSTATE 54000 (program_limit_exceeded), so no row lock can ever be taken

> _Note: this issue was identified against the code base at commit `0d0b5d4835067c793fae3be07582601671bbaf82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
One net-new root cause in the cross-surface (RDS Data API + the cluster's native engine wire protocol) behaviour of one Aurora PostgreSQL Serverless v2 cluster.

Cause 1 of 1: the cluster's native PostgreSQL endpoint refuses every row-level locking clause with a program-limit error instead of taking the lock.

AWS: recorded behavior only; the row-locking contract is the engine's, documented at https://www.postgresql.org/docs/current/sql-select.html#SQL-FOR-UPDATE-SHARE (no Smithy shape governs the native wire protocol — rds-data's ExecuteStatement is the other door onto the SAME cluster and models nothing about locking). Aurora PostgreSQL ACCEPTS `SELECT <cols> FROM <relation> WHERE <pk> = <value> FOR UPDATE NOWAIT` on an ordinary table and returns the selected rows, taking a row-level exclusive lock on each of them. The statement is refused ONLY when a row it selected is already locked by another transaction, and then with SQLSTATE 55P03 (lock_not_available) and the message `could not obtain lock on row in relation "<relation>"` — fixed text `could not obtain lock on row in relation "` + the relation name from the request + `"`. There is NO row-count ceiling on a locking clause and no "cannot identify the rows" condition: a locking SELECT over a base relation is always identifiable. SQLSTATE 54000 (program_limit_exceeded) is never emitted for a FOR UPDATE clause.

Emulator: the native session's locking SELECT is refused outright, before any contention exists. Replaying the check, the FIRST `SELECT <col> FROM <relation> WHERE <pk> = <value> FOR UPDATE NOWAIT` — issued against an UNLOCKED, uncontended row of a two-row table whose primary key is the predicate — raises `psycopg.errors.ProgramLimitExceeded` (SQLSTATE 54000) with the message `this endpoint serves a row-locking clause over at most 10000 rows of one statement, and only where it can identify them in relation "<relation>"`. That sentence is the emulator's own: its fixed text is `this endpoint serves a row-locking clause over at most 10000 rows of one statement, and only where it can identify them in relation "` + the relation name + `"`, and no PostgreSQL error has that form, that class, or that wording. Because the very first locking statement is refused, the lock is never taken and none of the downstream contract is reachable: the contended NOWAIT cannot produce 55P03, and the post-CommitTransaction NOWAIT cannot be shown to be granted and to read the Data API transaction's value.

Note on an adjacent open issue: 20261108T035337Z-rds-rds-data-secretsmanager-5-net-new-causes.md, Cause 3 of 5, records this endpoint refusing the SAME clause at PARSE time (`psycopg.errors.SyntaxError: syntax error at or near "FOR"`). That is a different refusal with a different replay key — the clause now parses — so this report is net-new and not covered by it; the two should be closed together.

Why: pessimistic row locking is the standard way a client serialises concurrent updates to the same row (SELECT … FOR UPDATE to claim a row, NOWAIT/SKIP LOCKED to fail fast or step over a contended one — the queue/seat-reservation idiom, and what every ORM's `with_for_update()` emits). Against this emulator such code cannot run at all: a correct, uncontended claim is rejected, and the rejection is classed as a resource/program limit, so the client is pointed at an imaginary 10000-row ceiling in its own query rather than at an unimplemented feature. A client that catches 55P03 to retry a contended claim sees 54000 instead and does not retry; it surfaces the refusal as a hard error.

Fix: the RDS cluster's native PostgreSQL endpoint — the emulated engine's SQL executor/planner path that handles the row-locking clause. It must resolve the rows a locking SELECT over a base relation selects (a predicate on the primary key is the ordinary case), take a row-level lock on each, and return the rows; the 10000-row cap and the "can identify them" precondition are emulator inventions and must not refuse a valid statement. A genuinely contended NOWAIT must be refused with the engine's lock_not_available condition (SQLSTATE 55P03) and AWS's message, not under program_limit_exceeded.

Traces: rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit.

## Evidence
Cause 1 — locking clause refused as a program limit:
  check rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit [error]
  at _rows -> cur.execute(statement, args), running the UNCONTENDED neighbour lock
  `SELECT holder FROM <relation> WHERE id = <pk> FOR UPDATE NOWAIT`:

    psycopg.errors.ProgramLimitExceeded: this endpoint serves a row-locking clause over at most
    10000 rows of one statement, and only where it can identify them in relation "<relation>"

  (SQLSTATE 54000, program_limit_exceeded. The relation held exactly two rows; the locked row was
  a DIFFERENT row, held by an open Data API transaction, so no contention applied to this
  statement.)

  Expected (Aurora PostgreSQL):
    - the uncontended `… WHERE id = <other pk> FOR UPDATE NOWAIT` succeeds and returns that row's
      committed value, holding its lock until the native transaction ends;
    - `… WHERE id = <locked pk> FOR UPDATE NOWAIT`, while the Data API transaction holds that row,
      is refused with SQLSTATE 55P03 and message
      `could not obtain lock on row in relation "<relation>"`;
    - a plain (non-locking) read of the locked row still returns its committed pre-transaction
      value;
    - after rds-data CommitTransaction reports 'Transaction Committed', the same NOWAIT lock is
      GRANTED and reads the value the Data API transaction wrote.
  Actual: the first locking statement is refused, so none of the four is observable.

## Steps to Reproduce
Against one Aurora PostgreSQL Serverless v2 cluster with the Data API enabled (EnableHttpEndpoint=true) and its master credential managed in Secrets Manager (ManageMasterUserPassword=true), with a native psycopg session logged in to the cluster's published writer endpoint as the same master user:
1. Through rds-data ExecuteStatement(resourceArn, secretArn, database): `CREATE TABLE <t> (id INT PRIMARY KEY, holder TEXT NOT NULL)` and `INSERT INTO <t> (id, holder) VALUES (1, 'free'), (2, 'free')`.
2. rds-data BeginTransaction, then ExecuteStatement with that transactionId: `UPDATE <t> SET holder = 'data-api' WHERE id = 1` (numberOfRecordsUpdated = 1). Leave the transaction OPEN so row id=1 is locked.
3. From the native psycopg session (autocommit off) run `SELECT holder FROM <t> WHERE id = 2 FOR UPDATE NOWAIT` — an UNCONTENDED row.
   Observed: psycopg.errors.ProgramLimitExceeded (SQLSTATE 54000) 'this endpoint serves a row-locking clause over at most 10000 rows of one statement, and only where it can identify them in relation "<t>"'.
   Expected: the row returns [('free',)] and its lock is held.
4. Then run `SELECT holder FROM <t> WHERE id = 1 FOR UPDATE NOWAIT` — the CONTENDED row.
   Expected: refused with SQLSTATE 55P03 and message 'could not obtain lock on row in relation "<t>"'.
5. rds-data CommitTransaction, then re-run the NOWAIT on id = 1.
   Expected: granted, returning [('data-api',)].
