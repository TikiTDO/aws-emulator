Issue-Severity: HIGH
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# DynamoDB Kinesis streaming destinations: invalid ARNs are refused synchronously instead of settling to ENABLE_FAILED, a second destination is accepted, and the destination lifecycle never progresses (4 causes)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableKinesisStreamingDestination

## Description
Four net-new root causes in the DynamoDB -> Kinesis streaming-destination feature. The open issue 20261027T001720Z-dynamodb-kinesis-enablekinesisstreamingconfiguration-omits-enablekinesisstreamingconfiguration-from-its-response.md covers the response-shape omission only and carries none of the replay keys below.

=== Cause 1 of 4: a StreamArn that is malformed or points at an unreachable stream is refused synchronously with ValidationException instead of being accepted and settling to ENABLE_FAILED ===
AWS: EnableKinesisStreamingDestination takes StreamArn (Smithy: StreamArn @length min 37 max 1024, required) and models InternalServerError, InvalidEndpointException, LimitExceededException, ResourceInUseException and ResourceNotFoundException — not a synchronous shape refusal for a value that clears the length floor. A value that clears the 37-character floor but does not name a reachable stream (a non-ARN string, or a stream in another region) is ACCEPTED with DestinationStatus ENABLING, and the destination then settles asynchronously to DestinationStatus ENABLE_FAILED, where DescribeKinesisStreamingDestination reports it and the "active destinations" filter excludes it.
Emulator: the call is refused immediately with 400 ValidationException 'Invalid StreamArn: Invalid StreamArn provided as input <value>', so the ENABLE_FAILED terminal state is never reachable and the cross-region case and the active-filter case never get a destination to observe.
Why: a client that enables a destination and then polls for ENABLE_FAILED to report a misconfiguration never sees it; instead it gets an error code AWS does not use here, and the ENABLE_FAILED state is unreachable in the emulator.
Fix: the EnableKinesisStreamingDestination handler must accept any StreamArn satisfying the modeled length constraint, record the destination as ENABLING, and have the destination state machine settle it to ENABLE_FAILED when the stream cannot be reached.
Traces: ddbkds-malformed-stream-arn-settles-to-enable-failed, ddbkds-cross-region-stream-arn-settles-to-enable-failed, ddbkds-enable-failed-destination-is-excluded-by-the-active-filter.

=== Cause 2 of 4: re-enabling an already-ACTIVE destination reports the wrong message ===
AWS: a second Enable for a destination already in ACTIVE is refused with ValidationException whose message is the fixed sentence "Table is not in a valid state to enable Kinesis Streaming Destination: EnableKinesisStreamingDestination must be DISABLED or ENABLE_FAILED to perform ENABLE operation." — it names the state precondition, with no identifiers interpolated.
Emulator: it answers 'Table is not in a valid state to enable Kinesis Streaming Destination: Streaming destination with streamArn: <arn> is already enabled for table with tableName: <table>' — a different sentence that interpolates the stream ARN and table name.
Why: the message is the only statement of the precondition; a client matching AWS's text (or extracting the required state list) fails.
Fix: the state-precondition branch of the EnableKinesisStreamingDestination handler.
Traces: ddbkds-re-enabling-the-active-stream-rejected, and the same divergence under the dynamodb heading as enable-kinesis-streaming-destination-already-active-rejected.

=== Cause 3 of 4: enabling a SECOND, different stream while one destination is ACTIVE is accepted ===
AWS: a table carries at most one Kinesis streaming destination; enabling a different StreamArn while one is ACTIVE is refused.
Emulator: the call succeeds with HTTP 200 and returns DestinationStatus ENABLING for the second stream, so the table ends up with two destinations.
Why: a client can silently fan one table's change data capture into two streams, which AWS will refuse in production, and the table's destination list no longer matches AWS's invariant.
Fix: the EnableKinesisStreamingDestination handler must reject a second destination against the stored destination state.
Traces: ddbkds-second-stream-while-one-is-active-rejected.

=== Cause 4 of 4: the destination lifecycle and record delivery never progress, and the table a destination is created against is not visible to DescribeTable ===
AWS: once a destination is ACTIVE, item writes are delivered to the stream as records whose creation-time precision follows EnableKinesisStreamingConfiguration.ApproximateCreationDateTimePrecision, a precision update changes the unit of later records only, and deleting the table leaves the Kinesis stream and the already-delivered records in place.
Emulator: these checks produce no observable delivery and hang to the 60 s harness cap, and the e2e lifecycle fails in its deploy phase because DescribeTable answers ResourceNotFoundException 'Requested resource not found: Table: <name> not found' for a table the phase just created.
Why: CDC to Kinesis is entirely unobservable, so nothing downstream of a streaming destination can be developed against the emulator.
Fix: the streaming-destination delivery path (destination state machine plus the record emitter that writes to the Kinesis stream) and the table-visibility/consistency of the control-plane state the destination checks create against.
Traces: ddbkds-deleting-a-streaming-table-leaves-its-stream-and-delivered-records, ddbkds-destination-precision-sets-the-unit-of-each-record-creation-time, ddbkds-precision-update-switches-the-creation-time-unit-of-later-records, e2e-ddbkds-list-streaming-tables-all-regions-lifecycle.

## Evidence
Cause 1 — ddbkds-malformed-stream-arn-settles-to-enable-failed: botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the EnableKinesisStreamingDestination operation: Invalid StreamArn: Invalid StreamArn provided as input this-is-not-an-arn-but-clears-the-length-floor. ddbkds-cross-region-stream-arn-settles-to-enable-failed and ddbkds-enable-failed-destination-is-excluded-by-the-active-filter: Check timed out after 60.0s (no destination ever exists to observe). Related, under the dynamodb heading: enable-kinesis-invalid-arn observed DestinationStatus 'ACTIVE' for a nonexistent stream.

Cause 2 — ddbkds-re-enabling-the-active-stream-rejected: AssertionError: expected message 'Table is not in a valid state to enable Kinesis Streaming Destination: EnableKinesisStreamingDestination must be DISABLED or ENABLE_FAILED to perform ENABLE operation.', got 'Table is not in a valid state to enable Kinesis Streaming Destination: Streaming destination with streamArn: arn:aws:kinesis:us-west-2:<acct>:stream/<name> is already enabled for table with tableName: <table>'.

Cause 3 — ddbkds-second-stream-while-one-is-active-rejected: AssertionError: the call succeeded; real AWS rejects it. It returned {'TableName': '<table>', 'StreamArn': 'arn:aws:kinesis:us-west-2:<acct>:stream/<second>', 'DestinationStatus': 'ENABLING', ... HTTPStatusCode 200}.

Cause 4 — ddbkds-deleting-a-streaming-table-leaves-its-stream-and-delivered-records, ddbkds-destination-precision-sets-the-unit-of-each-record-creation-time, ddbkds-precision-update-switches-the-creation-time-unit-of-later-records: Check timed out after 60.0s. e2e-ddbkds-list-streaming-tables-all-regions-lifecycle: _PhaseError: [phase 1/4 deploy] ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeTable operation: Requested resource not found: Table: <name>-home-streaming not found.

## Steps to Reproduce
Replay the behavioral suite and re-run the dynamodb__kinesis checks in trace_ids. Minimal reproductions: (1) ddb.enable_kinesis_streaming_destination(TableName=t, StreamArn='x'*40) — expect HTTP 200 with DestinationStatus ENABLING then ENABLE_FAILED on describe_kinesis_streaming_destination, observe 400 ValidationException 'Invalid StreamArn: ...'; (2) enable a valid stream, wait for ACTIVE, enable the same stream again and read Error.Message; (3) enable a valid stream, wait for ACTIVE, then enable a DIFFERENT stream — expect a refusal, observe 200 ENABLING; (4) with an ACTIVE destination, put_item and poll the Kinesis stream for records.
