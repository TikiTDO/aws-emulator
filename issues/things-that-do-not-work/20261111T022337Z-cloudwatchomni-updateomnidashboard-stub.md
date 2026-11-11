Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatchomni.update-dashboard-rejects-invalid-body-and-keeps-stored-body
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni.update-dashboard-rejects-invalid-body-and-keeps-stored-body|run#8|AssertionError|assert (code, message) == ("ValidationException", "dashboard body is not valid JSON at line 1 column 1"), (
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# Implement cloudwatchomni UpdateOmniDashboard: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateOmniDashboard

## Description
`UpdateOmniDashboard` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/update_omni_dashboard.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateOmniDashboard` call, so each one unblocks only when `UpdateOmniDashboard` works: `cloudwatchomni.update-dashboard-rejects-invalid-body-and-keeps-stored-body`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/update_omni_dashboard.rs`. Replaying the corpus, the emulator refused the `UpdateOmniDashboard` call: File "run.py", line 445, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_cloudwatchomni_update_dashboard_rejects_invalid_body_and_keeps_stored_body.py", line 84, in run
    assert (code, message) == ("ValidationException", "dashboard body is not valid JSON at line 1 column 1"), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: unexpected answer to a non-JSON body: NotImplemented: 'operation UpdateOmniDashboard is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the cloudwatchomni emulator
Call UpdateOmniDashboard and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
