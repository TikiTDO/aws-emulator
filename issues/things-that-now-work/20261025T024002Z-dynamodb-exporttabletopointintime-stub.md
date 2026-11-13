Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__s3.export-to-s3-rejected-when-point-in-time-recovery-disabled
Issue-Key: dynamodb__s3.export-to-s3-rejected-when-point-in-time-recovery-disabled|run#23|AssertionError|assert code == ERR_NO_PITR, (
Issue-Commit: 56c908e9384776713583471da50d0f78dde9fb40

# Implement dynamodb ExportTableToPointInTime: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `56c908e9384776713583471da50d0f78dde9fb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportTableToPointInTime

## Description
`ExportTableToPointInTime` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/export_table_to_point_in_time.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ExportTableToPointInTime` call, so each one unblocks only when `ExportTableToPointInTime` works: `dynamodb__s3.export-to-s3-rejected-when-point-in-time-recovery-disabled`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/export_table_to_point_in_time.rs`. Replaying the corpus, the emulator refused the `ExportTableToPointInTime` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_dynamodb__s3_export_to_s3_rejected_when_point_in_time_recovery_disabled.py", line 102, in run
    assert code == ERR_NO_PITR, (
           ^^^^^^^^^^^^^^^^^^^
AssertionError: expected 'PointInTimeRecoveryUnavailableException' when PITR is disabled, got 'NotImplemented' ('operation ExportTableToPointInTime is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the dynamodb emulator
Call ExportTableToPointInTime and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
