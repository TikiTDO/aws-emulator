Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.session-lifecycle-start-active-terminate emr.session-lifecycle-start-get-terminate emr.session-terminate-then-get-reflects-terminated-state emr.start-session-transitions-active-then-terminate-then-get-fails emr.terminate-session-already-terminated-rejected
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr|TerminateSession|NotImplemented
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# Implement emr TerminateSession: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TerminateSession

## Description
`TerminateSession` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-emr-elasticmapreduce/src/ops/terminate_session.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `TerminateSession` call, so each one unblocks only when `TerminateSession` works: `emr.session-lifecycle-start-active-terminate`, `emr.session-lifecycle-start-get-terminate`, `emr.session-terminate-then-get-reflects-terminated-state`, `emr.start-session-transitions-active-then-terminate-then-get-fails`, `emr.terminate-session-already-terminated-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-emr-elasticmapreduce/src/ops/terminate_session.rs`. Replaying the corpus, the emulator refused the `TerminateSession` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TerminateSession operation: operation TerminateSession is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the emr emulator
Call TerminateSession and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
