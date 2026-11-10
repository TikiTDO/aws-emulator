Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.update-broker-storage-triggers-operation kafka.update-rebalancing-server-enforced
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.update-broker-storage-triggers-operation|run#6|AssertionError|assert listed == _NOT_FOUND, f"ListClusterOperationsV2: expected {_NOT_FOUND!r}, got {listed!r}"
Issue-Key: kafka|ListClusterOperationsV2|NotImplemented
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# Implement kafka ListClusterOperationsV2: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListClusterOperationsV2

## Description
`ListClusterOperationsV2` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kafka-kafka/src/ops/list_cluster_operations_v2.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ListClusterOperationsV2` call, so each one unblocks only when `ListClusterOperationsV2` works: `kafka.update-broker-storage-triggers-operation`, `kafka.update-rebalancing-server-enforced`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kafka-kafka/src/ops/list_cluster_operations_v2.rs`. Replaying the corpus, the emulator refused the `ListClusterOperationsV2` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_kafka_update_broker_storage_triggers_operation.py", line 69, in run
    assert listed == _NOT_FOUND, f"ListClusterOperationsV2: expected {_NOT_FOUND!r}, got {listed!r}"
           ^^^^^^^^^^^^^^^^^^^^
AssertionError: ListClusterOperationsV2: expected ('NotFoundException', 'The requested resource doesn’t exist.'), got ('NotImplemented', 'operation ListClusterOperationsV2 is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the kafka emulator
Call ListClusterOperationsV2 and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
