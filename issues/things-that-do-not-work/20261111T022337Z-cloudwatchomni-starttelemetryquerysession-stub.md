Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatchomni.completed-query-results-stay-readable-after-stop cloudwatchomni.deleted-view-is-no-longer-a-queryable-table cloudwatchomni.query-over-the-default-dataset-needs-a-bounded-time-range cloudwatchomni.query-without-a-from-clause-is-rejected cloudwatchomni.redefining-a-view-changes-what-a-query-over-it-binds cloudwatchomni.session-name-is-unique-among-open-sessions cloudwatchomni.stopped-session-leaves-the-list-and-refuses-new-queries cloudwatchomni.view-without-a-datastore-is-stored-but-fails-to-bind-in-a-query
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni|StartTelemetryQuerySession|NotImplemented
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# Implement cloudwatchomni StartTelemetryQuerySession: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartTelemetryQuerySession

## Description
`StartTelemetryQuerySession` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/start_telemetry_query_session.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

8 recorded unit(s) fail on their `StartTelemetryQuerySession` call, so each one unblocks only when `StartTelemetryQuerySession` works: `cloudwatchomni.completed-query-results-stay-readable-after-stop`, `cloudwatchomni.deleted-view-is-no-longer-a-queryable-table`, `cloudwatchomni.query-over-the-default-dataset-needs-a-bounded-time-range`, `cloudwatchomni.query-without-a-from-clause-is-rejected`, `cloudwatchomni.redefining-a-view-changes-what-a-query-over-it-binds`, `cloudwatchomni.session-name-is-unique-among-open-sessions`, `cloudwatchomni.stopped-session-leaves-the-list-and-refuses-new-queries`, `cloudwatchomni.view-without-a-datastore-is-stored-but-fails-to-bind-in-a-query`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/start_telemetry_query_session.rs`. Replaying the corpus, the emulator refused the `StartTelemetryQuerySession` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartTelemetryQuerySession operation: operation StartTelemetryQuerySession is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudwatchomni emulator
Call StartTelemetryQuerySession and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
