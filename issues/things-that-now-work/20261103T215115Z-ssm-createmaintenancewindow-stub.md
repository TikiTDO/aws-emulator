Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.create-maintenance-window-and-get ssm.create-maintenance-window-rejects-idempotency-mismatch ssm.create-time-tags-list-exactly-across-resource-types ssm.delete-maintenance-window-then-get-fails ssm.deregister-target-in-use-rejected ssm.deregister-target-rejects-when-task-references-it ssm.get-maintenance-window-execution-task-nonexistent-rejected ssm.get-maintenance-window-execution-unknown-id-rejected ssm.get-nonexistent-maintenance-window-fails ssm.maintenance-window-dates-echo-and-invalid-schedule-rejected ssm.maintenance-window-execution-lookup-nonexistent-rejected ssm.maintenance-window-execution-task-invocation-lifecycle ssm.maintenance-window-next-execution-time-recomputed ssm.maintenance-window-target-update-and-describe ssm.maintenance-window-task-concurrent-registration-consistent ssm.maintenance-window-task-full-lifecycle ssm.maintenance-window-task-update-and-schedule ssm.maintenance-window-update-and-schedule-describe ssm.register-deregister-maintenance-window-target ssm.tag-calls-reject-missing-window-and-mismatched-resource-type
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|CreateMaintenanceWindow|NotImplemented
Issue-Commit: 69e3918bc598d03b2c669434388419738bf97909

# Implement ssm CreateMaintenanceWindow: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `69e3918bc598d03b2c669434388419738bf97909`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMaintenanceWindow

## Description
`CreateMaintenanceWindow` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ssm-amazonssm/src/ops/create_maintenance_window.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

20 recorded unit(s) fail on their `CreateMaintenanceWindow` call, so each one unblocks only when `CreateMaintenanceWindow` works: `ssm.create-maintenance-window-and-get`, `ssm.create-maintenance-window-rejects-idempotency-mismatch`, `ssm.create-time-tags-list-exactly-across-resource-types`, `ssm.delete-maintenance-window-then-get-fails`, `ssm.deregister-target-in-use-rejected`, `ssm.deregister-target-rejects-when-task-references-it`, `ssm.get-maintenance-window-execution-task-nonexistent-rejected`, `ssm.get-maintenance-window-execution-unknown-id-rejected`, `ssm.get-nonexistent-maintenance-window-fails`, `ssm.maintenance-window-dates-echo-and-invalid-schedule-rejected`, `ssm.maintenance-window-execution-lookup-nonexistent-rejected`, `ssm.maintenance-window-execution-task-invocation-lifecycle`, `ssm.maintenance-window-next-execution-time-recomputed`, `ssm.maintenance-window-target-update-and-describe`, `ssm.maintenance-window-task-concurrent-registration-consistent`, `ssm.maintenance-window-task-full-lifecycle`, `ssm.maintenance-window-task-update-and-schedule`, `ssm.maintenance-window-update-and-schedule-describe`, `ssm.register-deregister-maintenance-window-target`, `ssm.tag-calls-reject-missing-window-and-mismatched-resource-type`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ssm-amazonssm/src/ops/create_maintenance_window.rs`. Replaying the corpus, the emulator refused the `CreateMaintenanceWindow` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateMaintenanceWindow operation: operation CreateMaintenanceWindow is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ssm emulator
Call CreateMaintenanceWindow and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
