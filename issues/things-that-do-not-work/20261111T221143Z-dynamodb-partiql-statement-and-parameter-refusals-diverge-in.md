Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-batch-execute-statement-and-execute-transaction-reject-unconvertible-n-parameter
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-batch-execute-statement-and-execute-transaction-reject-unconvertible-n-parameter dynamodb-execute-transaction-select-partial-key-validation dynamodb-partiql-empty-statement-list-rejected dynamodb-partiql-exists-transaction-only-rejected execute-transaction-mixed-select-and-write-rejected dynamodb-batch-execute-statement-mixed-read-write-rejected dynamodb-execute-statement-partiql-is-type-name-rejected dynamodb-partiql-returning-clause-rejected-in-execute-transaction dynamodb-partiql-select-or-non-key-beside-sort-key-branch-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|ExecuteTransaction|4xx->4xx|$.message
Issue-Key: dynamodb|BatchExecuteStatement|4xx->4xx|$.message
Issue-Key: dynamodb|ExecuteStatement|2xx->4xx|$.Items
Issue-Key: dynamodb|ExecuteStatement|2xx->4xx|status
Issue-Key: dynamodb|ExecuteTransaction|4xx->2xx|$.__type
Issue-Key: dynamodb|ExecuteTransaction|4xx->2xx|$.message
Issue-Key: dynamodb|ExecuteTransaction|4xx->2xx|status
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.attr.S
Issue-Key: dynamodb|ExecuteStatement|4xx->2xx|$.__type
Issue-Key: dynamodb|ExecuteStatement|4xx->2xx|$.message
Issue-Key: dynamodb|ExecuteStatement|4xx->2xx|status
Issue-Commit: 0e3e027770f68a801846f0512bd9649d89ae04a7

# dynamodb: PartiQL statement and parameter refusals diverge in text, verdict and clause support

> _Note: this issue was identified against the code base at commit `0e3e027770f68a801846f0512bd9649d89ae04a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
The PartiQL surface authors its own refusal text where AWS has fixed wording, refuses a statement AWS accepts, and accepts two statement forms AWS refuses — one of which mutates an item. 6 causes.

Cause 1 of 6: ExecuteTransaction's unconvertible numeric parameter is reported with emulator prose.
AWS: recorded behavior — status 400 ValidationException, message 'The parameter cannot be converted to a numeric value: <parameter>'. Fixed text 'The parameter cannot be converted to a numeric value: '; the substituted part is the offending parameter's literal value from the request (here a number literal whose exponent exceeds DynamoDB's range).
Emulator: message 'Number overflow. Attempting to store a number with magnitude larger than supported range' — its own sentence, which also mis-describes the fault as a storage overflow rather than a parameter-conversion failure, and does not echo the offending parameter.
Why: a client that matches AWS's sentence to point the user at the bad parameter cannot, and the parameter value is not reported at all.
Fix: ExecuteTransaction's parameter binding/validation — raise the conversion failure with AWS's sentence and the request's parameter value.
Traces: dynamodb-batch-execute-statement-and-execute-transaction-reject-unconvertible-n-parameter#task#8.

Cause 2 of 6: BatchExecuteStatement's mixed read/write refusal is reworded.
AWS: recorded behavior — status 400 ValidationException, message 'Read and write requests together in the same batch is not supported.' (literal wording including the trailing period; no variable part).
Emulator: message 'The entire batch must consist of either read statements or write statements, you cannot mix both in one batch'.
Why: the diagnosis is the same but the text is not AWS's, so message-matching assertions and user-facing error surfaces diverge.
Fix: BatchExecuteStatement's batch-composition validation message.
Traces: dynamodb-batch-execute-statement-mixed-read-write-rejected#task#5.

Cause 3 of 6: ExecuteStatement refuses a statement AWS serves.
AWS: recorded behavior — status 200 with $.Items = the matching rows; a PartiQL statement whose predicate uses IS <typeName> is valid.
Emulator: status 400 — the statement is refused and no Items are returned.
Why: a valid PartiQL query cannot be run at all, so any client using the IS-type predicate is broken outright rather than degraded.
Fix: the PartiQL statement parser/planner — accept the IS <typeName> predicate form.
Traces: dynamodb-execute-statement-partiql-is-type-name-rejected#task#4.

Cause 4 of 6: ExecuteTransaction accepts a RETURNING clause AWS refuses in a transaction.
AWS: recorded behavior — status 400, __type 'com.amazon.coral.validate#ValidationException', message 'Validation failed in TransactStatements[<index>]: RETURNING clause is not supported in ExecuteTransaction.' Fixed text 'Validation failed in TransactStatements[' / ']: RETURNING clause is not supported in ExecuteTransaction.'; the substituted part is the zero-based index of the offending statement in the request's TransactStatements list.
Emulator: status 200 — the transaction is executed, no __type and no message.
Why: the write AWS refuses is committed, so the item is mutated by a statement the client would have had to correct.
Fix: ExecuteTransaction's per-statement validation — reject a RETURNING clause before executing, reporting the statement index.
Traces: dynamodb-partiql-returning-clause-rejected-in-execute-transaction#task#4.

Cause 5 of 6: the mutation from cause 4 is observable.
AWS: recorded behavior — after the refusal the item still holds its pre-transaction attribute value.
Emulator: GetItem returns the post-transaction value.
Why: this is cause 4's data mutation, visible on read.
Fix: follows from cause 4 — a refused ExecuteTransaction must commit nothing.
Traces: dynamodb-partiql-returning-clause-rejected-in-execute-transaction#task#5.

Cause 6 of 6: ExecuteStatement accepts overlapping sort-key conditions in a WHERE clause.
AWS: recorded behavior — status 400, __type 'com.amazon.coral.validate#ValidationException', message 'Overlapping conditions with range keys are not supported in where clause' (fixed text, no variable part).
Emulator: status 200 — the statement runs.
Why: a client gets a result set from a query AWS will not plan, so the rows it reads do not match what the real service would return and the query bug is never surfaced.
Fix: the PartiQL WHERE-clause planner's key-condition validation.
Traces: dynamodb-partiql-select-or-non-key-beside-sort-key-branch-rejected#task#5.

Related but not covering: open issues 20261111T170613Z-dynamodb-partiql-statement-parser-rejects-aws-s-grammar.md, 20261111T170613Z-dynamodb-partiql-semantic-validation-index-qualified-select-update.md and 20261110T114403Z-dynamodb-2-net-new-validationexception-message-causes-a.md cover other PartiQL causes; none carries these clusters' replay keys.

## Evidence
Cause 1: ExecuteTransaction 400 — $.message expected 'The parameter cannot be converted to a numeric value: <parameter>', got 'Number overflow. Attempting to store a number with magnitude larger than supported range'.
Cause 2: BatchExecuteStatement 400 — $.message expected 'Read and write requests together in the same batch is not supported.', got 'The entire batch must consist of either read statements or write statements, you cannot mix both in one batch'.
Cause 3: ExecuteStatement — status expected 200, got 400; $.Items expected the matching rows, missing.
Cause 4: ExecuteTransaction — status expected 400, got 200; $.__type expected 'com.amazon.coral.validate#ValidationException' (missing); $.message expected 'Validation failed in TransactStatements[<index>]: RETURNING clause is not supported in ExecuteTransaction.' (missing).
Cause 5: GetItem 200 — $.Item.<attr>.S expected the pre-transaction value, got the post-transaction value.
Cause 6: ExecuteStatement — status expected 400, got 200; $.__type and $.message expected 'com.amazon.coral.validate#ValidationException' / 'Overlapping conditions with range keys are not supported in where clause', both missing.

## Steps to Reproduce
1. Replay dynamodb-batch-execute-statement-and-execute-transaction-reject-unconvertible-n-parameter#task#8 and dynamodb-batch-execute-statement-mixed-read-write-rejected#task#5 and compare the ValidationException messages to AWS's fixed sentences.
2. Replay dynamodb-execute-statement-partiql-is-type-name-rejected#task#4: must answer 200 with the matching Items.
3. Replay dynamodb-partiql-returning-clause-rejected-in-execute-transaction#task#4 (must be refused 400 naming TransactStatements[<index>]) and #task#5 (the item must still hold its pre-transaction value).
4. Replay dynamodb-partiql-select-or-non-key-beside-sort-key-branch-rejected#task#5: must be refused 400 with 'Overlapping conditions with range keys are not supported in where clause'.
