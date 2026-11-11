Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: arc-zonal-shift.create-practice-run-configuration-duplicate-conflict-rejected arc-zonal-shift.delete-practice-run-configuration-removes-it arc-zonal-shift.delete-practice-run-configuration-while-autoshift-enabled-rejected arc-zonal-shift.list-managed-resources-practice-run-status-reflects-config arc-zonal-shift.practice-run-blocked-by-alarm-in-alarm-state-rejected arc-zonal-shift.practice-run-lifecycle-start-and-cancel arc-zonal-shift.zonal-autoshift-configuration-toggle-reflects-in-managed-resource
Issue-Shape-Id: com.amazonaws.arczonalshift#PercDataPlane
Issue-Key: arc-zonal-shift|CreatePracticeRunConfiguration|NotImplemented
Issue-Commit: fdb8a4e953f01d6c4e0b3bc9b573d323716eabbc

# Implement arc-zonal-shift CreatePracticeRunConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `fdb8a4e953f01d6c4e0b3bc9b573d323716eabbc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePracticeRunConfiguration

## Description
`CreatePracticeRunConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-arczonalshift-percdataplane/src/ops/create_practice_run_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `CreatePracticeRunConfiguration` call, so each one unblocks only when `CreatePracticeRunConfiguration` works: `arc-zonal-shift.create-practice-run-configuration-duplicate-conflict-rejected`, `arc-zonal-shift.delete-practice-run-configuration-removes-it`, `arc-zonal-shift.delete-practice-run-configuration-while-autoshift-enabled-rejected`, `arc-zonal-shift.list-managed-resources-practice-run-status-reflects-config`, `arc-zonal-shift.practice-run-blocked-by-alarm-in-alarm-state-rejected`, `arc-zonal-shift.practice-run-lifecycle-start-and-cancel`, `arc-zonal-shift.zonal-autoshift-configuration-toggle-reflects-in-managed-resource`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-arczonalshift-percdataplane/src/ops/create_practice_run_configuration.rs`. Replaying the corpus, the emulator refused the `CreatePracticeRunConfiguration` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreatePracticeRunConfiguration operation: operation CreatePracticeRunConfiguration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the arc-zonal-shift emulator
Call CreatePracticeRunConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
