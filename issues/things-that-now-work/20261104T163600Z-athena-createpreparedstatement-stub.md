Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.athena-prepared-statement-crud-lifecycle athena.athena-prepared-statement-duplicate-name-rejected athena.execute-prepared-statement-with-parameter athena.list-prepared-statements-pagination-stable-order athena.list-prepared-statements-pagination-token-consistent athena.op-contract-batch-get-prepared-statement athena.op-contract-deletePreparedStatement athena.op-contract-getPreparedStatement athena.op-contract-update-prepared-statement-rejected athena.op-contract-updatePreparedStatement athena.prepared-statement-create-and-execute-with-param athena.prepared-statement-delete-then-get-not-found-rejected athena.prepared-statement-empty-vs-absent-description-preserved athena.prepared-statement-get-nonexistent-resource-not-found athena.prepared-statement-lifecycle athena.prepared-statement-lifecycle-create-get-update-delete athena.prepared-statement-list-and-batch-get-partial athena.prepared-statement-update-reflected athena.update-prepared-statement-query-text
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|CreatePreparedStatement|NotImplemented
Issue-Commit: 1b83187822ca52f98e00143d7b0b9c4d0812f404

# Implement athena CreatePreparedStatement: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1b83187822ca52f98e00143d7b0b9c4d0812f404`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePreparedStatement

## Description
`CreatePreparedStatement` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-athena-amazonathena/src/ops/create_prepared_statement.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

19 recorded unit(s) fail on their `CreatePreparedStatement` call, so each one unblocks only when `CreatePreparedStatement` works: `athena.athena-prepared-statement-crud-lifecycle`, `athena.athena-prepared-statement-duplicate-name-rejected`, `athena.execute-prepared-statement-with-parameter`, `athena.list-prepared-statements-pagination-stable-order`, `athena.list-prepared-statements-pagination-token-consistent`, `athena.op-contract-batch-get-prepared-statement`, `athena.op-contract-deletePreparedStatement`, `athena.op-contract-getPreparedStatement`, `athena.op-contract-update-prepared-statement-rejected`, `athena.op-contract-updatePreparedStatement`, `athena.prepared-statement-create-and-execute-with-param`, `athena.prepared-statement-delete-then-get-not-found-rejected`, `athena.prepared-statement-empty-vs-absent-description-preserved`, `athena.prepared-statement-get-nonexistent-resource-not-found`, `athena.prepared-statement-lifecycle`, `athena.prepared-statement-lifecycle-create-get-update-delete`, `athena.prepared-statement-list-and-batch-get-partial`, `athena.prepared-statement-update-reflected`, `athena.update-prepared-statement-query-text`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-athena-amazonathena/src/ops/create_prepared_statement.rs`. Replaying the corpus, the emulator refused the `CreatePreparedStatement` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreatePreparedStatement operation: operation CreatePreparedStatement is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the athena emulator
Call CreatePreparedStatement and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
