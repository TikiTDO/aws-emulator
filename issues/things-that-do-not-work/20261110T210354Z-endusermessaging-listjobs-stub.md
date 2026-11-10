Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: endusermessaging.list-jobs-rejects-unknown-operation-type
Issue-Shape-Id: com.amazonaws.endusermessaging#EndUserMessaging
Issue-Key: endusermessaging.list-jobs-rejects-unknown-operation-type|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# Implement endusermessaging ListJobs: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListJobs

## Description
`ListJobs` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-endusermessaging-endusermessaging/src/ops/list_jobs.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListJobs` call, so each one unblocks only when `ListJobs` works: `endusermessaging.list-jobs-rejects-unknown-operation-type`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-endusermessaging-endusermessaging/src/ops/list_jobs.rs`. Replaying the corpus, the emulator refused the `ListJobs` call: File "run.py", line 365, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_endusermessaging_list_jobs_rejects_unknown_operation_type.py", line 47, in run
    assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException, got NotImplemented: 'operation ListJobs is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the endusermessaging emulator
Call ListJobs and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
