Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.capacity-assignment-configuration-governs-workgroup-execution athena.capacity-assignment-configuration-put-get-roundtrip athena.e2e-capacity-reservation-assignment-influences-query-execution athena.e2e-capacity-reservation-workgroup-query-teardown athena.put-capacity-assignment-configuration-serializes-concurrent-writes
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|PutCapacityAssignmentConfiguration|NotImplemented
Issue-Key: athena.put-capacity-assignment-configuration-serializes-concurrent-writes|run#9|AssertionError|assert not errors, f"unexpected writer failures: {errors}"
Issue-Commit: 115da0938d5a480a5bdac5ab944971a99ada7e73

# Implement athena PutCapacityAssignmentConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `115da0938d5a480a5bdac5ab944971a99ada7e73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutCapacityAssignmentConfiguration

## Description
`PutCapacityAssignmentConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-athena-amazonathena/src/ops/put_capacity_assignment_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `PutCapacityAssignmentConfiguration` call, so each one unblocks only when `PutCapacityAssignmentConfiguration` works: `athena.capacity-assignment-configuration-governs-workgroup-execution`, `athena.capacity-assignment-configuration-put-get-roundtrip`, `athena.e2e-capacity-reservation-assignment-influences-query-execution`, `athena.e2e-capacity-reservation-workgroup-query-teardown`, `athena.put-capacity-assignment-configuration-serializes-concurrent-writes`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-athena-amazonathena/src/ops/put_capacity_assignment_configuration.rs`. Replaying the corpus, the emulator refused the `PutCapacityAssignmentConfiguration` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutCapacityAssignmentConfiguration operation: operation PutCapacityAssignmentConfiguration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the athena emulator
Call PutCapacityAssignmentConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
