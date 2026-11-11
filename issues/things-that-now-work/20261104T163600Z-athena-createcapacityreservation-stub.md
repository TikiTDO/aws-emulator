Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.athena-capacity-reservation-pending-then-cancelled-transition athena.cancel-capacity-reservation-already-cancelled-fails athena.cancel-capacity-reservation-not-active-fails athena.capacity-assignment-configuration-governs-workgroup-execution athena.capacity-assignment-configuration-put-get-roundtrip athena.capacity-reservation-delete-before-cancel-rejected athena.capacity-reservation-lifecycle-create-get-update-cancel-delete athena.capacity-reservation-lifecycle-create-update-delete athena.capacity-reservation-target-dpus-numeric-precision-preserved athena.capacity-reservation-target-dpus-round-trip athena.capacity-reservation-timestamp-precision-preserved athena.capacity-reservation-update-below-minimum-rejected athena.capacity-reservation-update-while-pending-conflicts athena.concurrent-capacity-reservation-update-race athena.concurrent-capacity-reservation-update-race-resolves-consistently athena.delete-capacity-reservation-while-active-fails athena.delete-capacity-reservation-without-cancel-fails athena.e2e-capacity-reservation-assignment-influences-query-execution athena.e2e-capacity-reservation-workgroup-query-teardown athena.put-capacity-assignment-configuration-invalid-workgroup-fails athena.put-capacity-assignment-configuration-serializes-concurrent-writes athena.update-capacity-reservation-below-minimum-rejected athena.update-capacity-reservation-negative-target-dpus-rejected athena.update-capacity-reservation-unknown-name-rejected athena.update-capacity-reservation-wrong-lifecycle-state-rejected athena.update-capacity-reservation-wrong-state-rejected
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|CreateCapacityReservation|NotImplemented
Issue-Commit: 1b83187822ca52f98e00143d7b0b9c4d0812f404

# Implement athena CreateCapacityReservation: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1b83187822ca52f98e00143d7b0b9c4d0812f404`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityReservation

## Description
`CreateCapacityReservation` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-athena-amazonathena/src/ops/create_capacity_reservation.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

26 recorded unit(s) fail on their `CreateCapacityReservation` call, so each one unblocks only when `CreateCapacityReservation` works: `athena.athena-capacity-reservation-pending-then-cancelled-transition`, `athena.cancel-capacity-reservation-already-cancelled-fails`, `athena.cancel-capacity-reservation-not-active-fails`, `athena.capacity-assignment-configuration-governs-workgroup-execution`, `athena.capacity-assignment-configuration-put-get-roundtrip`, `athena.capacity-reservation-delete-before-cancel-rejected`, `athena.capacity-reservation-lifecycle-create-get-update-cancel-delete`, `athena.capacity-reservation-lifecycle-create-update-delete`, `athena.capacity-reservation-target-dpus-numeric-precision-preserved`, `athena.capacity-reservation-target-dpus-round-trip`, `athena.capacity-reservation-timestamp-precision-preserved`, `athena.capacity-reservation-update-below-minimum-rejected`, `athena.capacity-reservation-update-while-pending-conflicts`, `athena.concurrent-capacity-reservation-update-race`, `athena.concurrent-capacity-reservation-update-race-resolves-consistently`, `athena.delete-capacity-reservation-while-active-fails`, `athena.delete-capacity-reservation-without-cancel-fails`, `athena.e2e-capacity-reservation-assignment-influences-query-execution`, `athena.e2e-capacity-reservation-workgroup-query-teardown`, `athena.put-capacity-assignment-configuration-invalid-workgroup-fails` (+6 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-athena-amazonathena/src/ops/create_capacity_reservation.rs`. Replaying the corpus, the emulator refused the `CreateCapacityReservation` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateCapacityReservation operation: operation CreateCapacityReservation is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the athena emulator
Call CreateCapacityReservation and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
