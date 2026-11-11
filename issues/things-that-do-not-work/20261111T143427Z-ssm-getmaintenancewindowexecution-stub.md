Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.get-maintenance-window-execution-unknown-id-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.get-maintenance-window-execution-unknown-id-rejected|run#10|AssertionError|assert err["Code"] == "DoesNotExistException", f"expected DoesNotExistException, got {err!r}"
Issue-Commit: dcee3c715dd1f76e7da2147c1b920fea18ed74e4

# Implement ssm GetMaintenanceWindowExecution: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `dcee3c715dd1f76e7da2147c1b920fea18ed74e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMaintenanceWindowExecution

## Description
`GetMaintenanceWindowExecution` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ssm-amazonssm/src/ops/get_maintenance_window_execution.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetMaintenanceWindowExecution` call, so each one unblocks only when `GetMaintenanceWindowExecution` works: `ssm.get-maintenance-window-execution-unknown-id-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ssm-amazonssm/src/ops/get_maintenance_window_execution.rs`. Replaying the corpus, the emulator refused the `GetMaintenanceWindowExecution` call: File "run.py", line 445, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_ssm_get_maintenance_window_execution_unknown_id_rejected.py", line 44, in run
    assert err["Code"] == "DoesNotExistException", f"expected DoesNotExistException, got {err!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected DoesNotExistException, got {'Message': 'operation GetMaintenanceWindowExecution is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the ssm emulator
Call GetMaintenanceWindowExecution and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
