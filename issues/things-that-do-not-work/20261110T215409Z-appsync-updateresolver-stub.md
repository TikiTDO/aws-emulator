Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.concurrent-update-resolver appsync.op-contract-update-resolver appsync.unit-resolver-success-lifecycle appsync.update-resolver-change-kind-to-pipeline appsync.update-resolver-changes-mapping-template
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.concurrent-update-resolver|_assert_update_race#14|AssertionError|assert successes >= 1, f"expected at least one {op_name} racer to win, got results={results!r}"
Issue-Key: appsync|UpdateResolver|NotImplemented
Issue-Commit: 790c5c5e1c0f240249b840d64996fdcf530cd42d

# Implement appsync UpdateResolver: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `790c5c5e1c0f240249b840d64996fdcf530cd42d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateResolver

## Description
`UpdateResolver` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/update_resolver.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `UpdateResolver` call, so each one unblocks only when `UpdateResolver` works: `appsync.concurrent-update-resolver`, `appsync.op-contract-update-resolver`, `appsync.unit-resolver-success-lifecycle`, `appsync.update-resolver-change-kind-to-pipeline`, `appsync.update-resolver-changes-mapping-template`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/update_resolver.rs`. Replaying the corpus, the emulator refused the `UpdateResolver` call: File "check_appsync_concurrent_update_resolver.py", line 143, in run
    _assert_update_race(results, racers=racers, op_name="UpdateResolver")
  File "check_appsync_concurrent_update_resolver.py", line 76, in _assert_update_race
    assert successes >= 1, f"expected at least one {op_name} racer to win, got results={results!r}"
           ^^^^^^^^^^^^^^
AssertionError: expected at least one UpdateResolver racer to win, got results=[ClientError('An error occurred (NotImplemented) when calling the UpdateResolver operation: operation UpdateResolver is scaffolded but not implemented in this emulator'), ClientError('An error occurred (NotImplemented) when calling the UpdateResolver operation: operation UpdateResolver is scaffolded but not implemented in this emulator'), ClientError('An error occurred (NotImplemented) when calling the UpdateResolver operation: operation UpdateResolver is scaffolded but not implemented in this emulator'), ClientError('An error occurred (NotImplemented) when calling the UpdateResolver operation: operation UpdateResolver is scaffolded but not implemented in this emulator'), ClientError('An error occurred (NotImplemented) when calling the UpdateResolver operation: operation UpdateResolver is scaffolded but not implemented in this emulator')]

## Steps to Reproduce
Boot the appsync emulator
Call UpdateResolver and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
