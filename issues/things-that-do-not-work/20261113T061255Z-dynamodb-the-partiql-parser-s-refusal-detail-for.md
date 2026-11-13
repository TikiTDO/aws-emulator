Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.batch-execute-statement-malformed-slot-siblings-execute dynamodb.execute-transaction-malformed-statement-rejects-whole-call
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.batch-execute-statement-malformed-slot-siblings-execute|run#15|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: dynamodb.execute-transaction-malformed-statement-rejects-whole-call|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: bd012b1aa51096c7ce0d935c21037429ca5dabd2

# dynamodb: the PartiQL parser's refusal detail for a misspelled DML keyword is "Expected data manipulation" where AWS says "Unexpected token after expression"

> _Note: this issue was identified against the code base at commit `bd012b1aa51096c7ce0d935c21037429ca5dabd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchExecuteStatement / ExecuteTransaction

## Description
Cause 1 of 1: a PartiQL statement whose leading DML keyword is misspelled (so the statement parses as a bare expression followed by a token that cannot follow it, e.g. `INSRT INTO "<table>" VALUE {'pk':'x','sk':'s1'}`) is refused with the right HTTP status, the right error class and AWS's fixed message prefix, but the emulator substitutes its own parser diagnostic into the detail position after `can't be processed: `. Checks: dynamodb.batch-execute-statement-malformed-slot-siblings-execute, dynamodb.execute-transaction-malformed-statement-rejects-whole-call.

AWS: 400, and for this statement class the message is `Statement wasn't well formed, can't be processed: Unexpected token after expression`. Fixed text = everything up to and including `can't be processed: `, plus the detail `Unexpected token after expression`, which names the parse position class (an unexpected token following a complete expression) and is itself fixed for this class — AWS echoes no token, offset or table name from the request here. Smithy (services/service-com-amazonaws-dynamodb-dynamodb-20120810): BatchExecuteStatement.Statements -> PartiQLBatchRequest @required @length 1..25; per-slot failures are carried in BatchStatementResponse.Error (BatchStatementError{Code: BatchStatementErrorCodeEnum, Message: String}) with Code = `ValidationError` and the batch call itself answering 200 — the model carries no error prose, so the message text is recorded behavior only. ExecuteTransaction.TransactStatements -> ParameterizedStatements @required @length 1..100; a malformed statement is refused for the WHOLE call through the service-common ValidationException (never TransactionCanceledException, which is modeled for cancellation), and its message is AWS's fixed slot prefix `Validation failed in TransactStatements[<index>]: ` — where `<index>` is the zero-based position of the offending statement in the request — followed by the same statement message.

Emulator: both paths return the correct status, the correct error class (per-slot `ValidationError` for the batch, `ValidationException` for the transaction) and the correct fixed prefix, but the detail is `Expected data manipulation`. Observed: BatchExecuteStatement slot 0 Error.Message = `Statement wasn't well formed, can't be processed: Expected data manipulation` (expected `... can't be processed: Unexpected token after expression`); ExecuteTransaction message = `Validation failed in TransactStatements[0]: Statement wasn't well formed, can't be processed: Expected data manipulation` (expected `Validation failed in TransactStatements[0]: Statement wasn't well formed, can't be processed: Unexpected token after expression`). The `Validation failed in TransactStatements[0]: ` prefix and the index are correct; only the trailing detail diverges, and it diverges identically on both operations, so it is one shared formatter.

Why: a client that branches on the refusal text — a PartiQL console surfacing the parse diagnostic, an SDK wrapper classifying per-slot BatchStatementError.Message, or a test asserting the refusal — sees a string AWS never produces. Exact comparisons fail and substring comparisons keyed on the end of AWS's string fail, so the caller cannot tell this parse class apart from the other PartiQL refusal families, and the emulator's internal parser vocabulary (`Expected data manipulation`) leaks into a client-visible contract.

Note: the open issue 20261106T140930Z-dynamodb-partiql-malformed-statement-refusal-appends-an-emulator.md covers a DIFFERENT member of this message family (the class where AWS leaves the detail after the colon-space EMPTY) and the open issue 20261110T114403Z-dynamodb-2-net-new-validationexception-message-causes-a.md covers the non-DML statement class (`Unsupported operation: ...`). Neither covers this class, where AWS's required detail is the non-empty fixed text `Unexpected token after expression`; the replay report leaves both checks unowned.

Fix: the PartiQL statement parser/validator's message formatter on the shared statement-validation path used by ExecuteStatement, BatchExecuteStatement and ExecuteTransaction — it must emit AWS's detail for the "token cannot follow a complete expression" parse class instead of its own `Expected data manipulation` diagnostic, and the ExecuteTransaction path must keep wrapping it in the `Validation failed in TransactStatements[<index>]: ` prefix it already gets right.

Traces: dynamodb.batch-execute-statement-malformed-slot-siblings-execute (BatchExecuteStatement, slot 0 Error.Message) and dynamodb.execute-transaction-malformed-statement-rejects-whole-call (ExecuteTransaction, ValidationException message).

## Evidence
Check dynamodb.batch-execute-statement-malformed-slot-siblings-execute:
  AssertionError: expected message "Statement wasn't well formed, can't be processed: Unexpected token after expression", got "Statement wasn't well formed, can't be processed: Expected data manipulation"
  (Responses[0].Error.Code == "ValidationError" matched; the batch itself answered 200 and the well-formed sibling slots were not reached by the assertion.)

Check dynamodb.execute-transaction-malformed-statement-rejects-whole-call:
  AssertionError: expected message "Validation failed in TransactStatements[0]: Statement wasn't well formed, can't be processed: Unexpected token after expression", got "Validation failed in TransactStatements[0]: Statement wasn't well formed, can't be processed: Expected data manipulation"
  (Error.Code == "ValidationException" matched, i.e. the whole-call refusal class is already right.)

Message template: fixed text `Statement wasn't well formed, can't be processed: ` + fixed per-class detail `Unexpected token after expression`; for ExecuteTransaction prefixed by the fixed text `Validation failed in TransactStatements[` + the zero-based index of the offending statement taken from the request + `]: `. The table name in the reproducing statement is a per-run value and is not part of the message.

## Steps to Reproduce
1. CreateTable with KeySchema pk (HASH, S) + sk (RANGE, S), BillingMode PAY_PER_REQUEST; wait for ACTIVE.
2. BatchExecuteStatement with Statements = [ `INSRT INTO "<table>" VALUE {'pk':'malformed','sk':'s1'}`, `INSERT INTO "<table>" VALUE {'pk':'sibling-a','sk':'s1'}`, `INSERT INTO "<table>" VALUE {'pk':'sibling-b','sk':'s1'}` ].
   Observe Responses[0].Error.Message = "Statement wasn't well formed, can't be processed: Expected data manipulation" where AWS returns "Statement wasn't well formed, can't be processed: Unexpected token after expression".
3. ExecuteTransaction with TransactStatements = [ the same malformed statement, a well-formed INSERT ].
   Observe the 400 ValidationException message = "Validation failed in TransactStatements[0]: Statement wasn't well formed, can't be processed: Expected data manipulation" where AWS returns the same prefix followed by "Unexpected token after expression".
4. Re-replay checks dynamodb.batch-execute-statement-malformed-slot-siblings-execute and dynamodb.execute-transaction-malformed-statement-rejects-whole-call to confirm the fix.
