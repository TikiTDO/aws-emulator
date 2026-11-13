Issue-Severity: HIGH
Issue-Commit: 62084d168200d993886f3f9bcbdaeb992dfa6fb2

# dynamodb__kinesis: 6 net-new root causes in the DynamoDB→Kinesis change-data-capture surface — invalid ARNs refused synchronously, a second destination accepted, the re-enable message invented, no record ever delivered, and a CFN-deployed streaming table does not exist

> _Note: this issue was identified against the code base at commit `62084d168200d993886f3f9bcbdaeb992dfa6fb2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableKinesisStreamingDestination

## Description
Six root causes in the DynamoDB → Kinesis streaming-destination surface, all grounded in failing checks the replay report lists under `### dynamodb__kinesis` and marks NEITHER "ALREADY FILED" nor "OWNED by".

Open issues 20261103T042714Z-dynamodb-kinesis-streaming-destinations-invalid-arns-are-refused-929137.md, 20261103T022510Z-dynamodb-kinesis-streaming-destinations-invalid-arns-are-refused.md and 20261027T001720Z-dynamodb-kinesis-enablekinesisstreamingdestination-omits-enablekinesisstreamingconfiguration-from-its-response.md describe the same or neighbouring ground, but the suite did not bind any of the checks below to them (no OWNED marker), so they are re-filed here with their replay keys attached in trace_ids. A fixer should treat those issues and this one as one body of work.

=== Cause 1 of 6: a StreamArn that clears the length floor but names no reachable stream is refused SYNCHRONOUSLY instead of settling to ENABLE_FAILED ===
AWS: Smithy models EnableKinesisStreamingDestination.StreamArn as StreamArn with @length min 37 max 1024 (required), and the operation's modeled errors are only InternalServerError, InvalidEndpointException (421), LimitExceededException, ResourceInUseException and ResourceNotFoundException — ValidationException is NOT a modeled error of this operation. Recorded behavior: a StreamArn that clears the 37-character floor is ACCEPTED at the door (HTTP 200, DestinationStatus `ENABLING`, StreamArn echoed verbatim) and the destination then settles ASYNCHRONOUSLY to `ENABLE_FAILED`, which DescribeKinesisStreamingDestination reports together with a non-empty DestinationStatusDescription. This holds for a string that is not an ARN at all, for a well-formed ARN naming no existing stream, and for a stream ARN in another region.
Emulator: EnableKinesisStreamingDestination refuses the call synchronously with `ValidationException` and the message `Invalid StreamArn: Invalid StreamArn provided as input <the StreamArn from the request>` (fixed text `Invalid StreamArn: Invalid StreamArn provided as input `, variable part the request's StreamArn). No destination row is ever created, so `ENABLE_FAILED` is unreachable: the two checks that poll for that terminal state, and the ACTIVE-filter check that needs an ENABLE_FAILED sibling, hang to the 60 s harness cap.
Why: a client that enables a destination and polls DescribeKinesisStreamingDestination for a terminal status never observes ENABLE_FAILED and never sees DestinationStatusDescription; and it receives an error code AWS does not model for this operation, so its error handling for the modeled set never fires.
Fix: the EnableKinesisStreamingDestination handler must accept any length-valid StreamArn, record the destination as ENABLING, and drive it to ENABLE_FAILED (with a DestinationStatusDescription) in the destination state machine, rather than refusing at the door.
Traces: ddbkds-malformed-stream-arn-settles-to-enable-failed, ddbkds-cross-region-stream-arn-settles-to-enable-failed, ddbkds-enable-failed-destination-is-excluded-by-the-active-filter.

=== Cause 2 of 6: the Enable state-precondition refusal names the stream and table instead of the state rule, and does not fire at all for a DIFFERENT stream ===
AWS: recorded behavior — a table carries at most one Kinesis streaming destination, and Enable is only permitted from DISABLED or ENABLE_FAILED. Enabling while a destination is ACTIVE is refused with `ValidationException` and the fixed sentence `Table is not in a valid state to enable Kinesis Streaming Destination: EnableKinesisStreamingDestination must be DISABLED or ENABLE_FAILED to perform ENABLE operation.` — the message is entirely fixed text; it names the STATE PRECONDITION and carries no identifiers. The same refusal fires whether the ARN offered is the already-ACTIVE stream or a different stream; after the first destination is disabled, the identical call is accepted (200, ENABLING).
Emulator: the precondition is implemented as an "is this same stream already enabled" test rather than a destination-state test. Re-enabling the SAME stream is refused with ValidationException but with the invented message `Table is not in a valid state to enable Kinesis Streaming Destination: Streaming destination with streamArn: <arn> is already enabled for table with tableName: <table>` (variable parts: the stream ARN and the table name — identifiers AWS withholds here), and enabling a DIFFERENT stream while one is ACTIVE is not refused at all: it answers HTTP 200 with the second StreamArn and DestinationStatus `ENABLING`.
Why: the message is part of the error contract and the emulator's version omits the precondition the caller must satisfy; worse, the missing cardinality guard lets a table reach a destination topology AWS cannot produce (two destinations), after which DescribeKinesisStreamingDestination output and record delivery diverge from AWS for the life of the table.
Fix: the state-precondition/cardinality check in the EnableKinesisStreamingDestination handler and its message — gate on the table's current destination state (DISABLED or ENABLE_FAILED), not on the submitted stream ARN.
Traces: ddbkds-re-enabling-the-active-stream-rejected, ddbkds-second-stream-while-one-is-active-rejected.

=== Cause 3 of 6: no change record is ever delivered to an ACTIVE Kinesis destination ===
AWS: recorded behavior — with a destination ACTIVE, every item-level write on the table is published to the Kinesis stream as one change record carrying eventID, the table name and the item image, readable with GetShardIterator/GetRecords.
Emulator: after the destination settles ACTIVE, items written with PutItem produce no record on the destination stream; every check that reads a delivered record hangs to the 60 s harness cap.
Why: the entire point of a Kinesis streaming destination is unusable — a CDC consumer cannot be developed or tested at all against the emulator.
Fix: the DynamoDB → Kinesis change-record publisher invoked from the item-write path, plus the destination state that binds a table to its stream.
Traces: ddbkds-destination-precision-sets-the-unit-of-each-record-creation-time, ddbkds-precision-update-switches-the-creation-time-unit-of-later-records, ddbkds-deleting-a-streaming-table-leaves-its-stream-and-delivered-records.

=== Cause 4 of 6: ApproximateCreationDateTimePrecision does not govern the unit of a delivered record's creation time ===
AWS: EnableKinesisStreamingConfiguration.ApproximateCreationDateTimePrecision (MILLISECOND default, MICROSECOND) sets the UNIT of ApproximateCreationDateTime on every record the table publishes, and an UpdateKinesisStreamingDestination that changes it switches the unit for records written AFTER the change while leaving earlier records stamped in the earlier unit.
Emulator: unobservable — no record reaches the stream (Cause 3), so neither the initial unit nor the post-update switch can be read; both checks hang to the 60 s cap.
Why: a consumer that must read ApproximateCreationDateTime in the configured unit (the two differ by 1000x) has no way to verify its parsing, and a precision change cannot be validated.
Fix: the ApproximateCreationDateTime rendering in the change-record publisher, driven by the destination's stored precision at publish time.
Traces: ddbkds-destination-precision-sets-the-unit-of-each-record-creation-time, ddbkds-precision-update-switches-the-creation-time-unit-of-later-records.

=== Cause 5 of 6: deleting a streaming table does not leave the stream and its already-delivered records behind ===
AWS: recorded behavior — DeleteTable on a table with an ACTIVE Kinesis destination succeeds; DescribeKinesisStreamingDestination for the gone table then answers ResourceNotFoundException with `Requested resource not found: Table: <table name> not found` (fixed text around the table name from the request), while the customer-owned Kinesis stream stays ACTIVE and still holds every record delivered before the delete (same eventID on re-read).
Emulator: the check never reaches a verdict, hanging to the 60 s cap — with nothing ever delivered (Cause 3) the pre-delete read of the record never completes, so the orphan-stream guarantee is unverifiable.
Why: an operator cannot confirm that CDC data survives its source table, which is the reason to stream to a customer-owned stream in the first place.
Fix: the change-record publisher (Cause 3) plus the DeleteTable path's handling of an attached destination (drop the destination view, leave the stream and its records untouched).
Traces: ddbkds-deleting-a-streaming-table-leaves-its-stream-and-delivered-records.

=== Cause 6 of 6: a CloudFormation-deployed AWS::DynamoDB::Table does not exist after CREATE_COMPLETE ===
AWS: recorded behavior — a stack declaring AWS::Kinesis::Stream plus an AWS::DynamoDB::Table with ProvisionedThroughput and KinesisStreamSpecification reaches CREATE_COMPLETE only once those resources exist; DescribeTable then answers for the table (with the declared 5/5 provisioned throughput) and its KinesisStreamSpecification materialises one destination that settles ACTIVE on the stack's stream.
Emulator: the stack reaches CREATE_COMPLETE and its Outputs resolve (Ref of the table returns the declared name, Fn::GetAtt of the stream returns its ARN — those assertions pass), but DescribeTable on that name is refused `ResourceNotFoundException: Requested resource not found: Table: <stack table name> not found`. The resource was reported complete without ever being provisioned, so the lifecycle fails in phase 1/4 (deploy).
Why: every deploy-shaped workflow over a CloudFormation-declared DynamoDB table is unreachable, and CREATE_COMPLETE is not a usable signal that the stack's resources exist.
Fix: the CloudFormation provisioner for AWS::DynamoDB::Table must call the native CreateTable (and apply KinesisStreamSpecification) and only then report the resource complete. Open issue 20261025T015306Z-cloudformation-cannot-deploy-large-parts-of-the-resource.md describes the same CloudFormation gap for other resource types but does not carry this replay key.
Traces: e2e-ddbkds-list-streaming-tables-all-regions-lifecycle.

## Evidence
Cause 1 — ddbkds-malformed-stream-arn-settles-to-enable-failed: botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the EnableKinesisStreamingDestination operation: Invalid StreamArn: Invalid StreamArn provided as input this-is-not-an-arn-but-clears-the-length-floor. Expected HTTP 200 with (StreamArn echoed, DestinationStatus 'ENABLING') and a later DescribeKinesisStreamingDestination showing ENABLE_FAILED with a non-empty DestinationStatusDescription. ddbkds-cross-region-stream-arn-settles-to-enable-failed and ddbkds-enable-failed-destination-is-excluded-by-the-active-filter: 'Check timed out after 60.0s' while polling for ENABLE_FAILED.

Cause 2 — ddbkds-re-enabling-the-active-stream-rejected: AssertionError: expected message 'Table is not in a valid state to enable Kinesis Streaming Destination: EnableKinesisStreamingDestination must be DISABLED or ENABLE_FAILED to perform ENABLE operation.', got 'Table is not in a valid state to enable Kinesis Streaming Destination: Streaming destination with streamArn: arn:aws:kinesis:<region>:<account>:stream/<name>-cdc is already enabled for table with tableName: <name>-table'. ddbkds-second-stream-while-one-is-active-rejected: AssertionError: the call succeeded; real AWS rejects it. It returned {'TableName': '<name>-table', 'StreamArn': 'arn:aws:kinesis:<region>:<account>:stream/<name>-second', 'DestinationStatus': 'ENABLING', ... 'HTTPStatusCode': 200}.

Causes 3 and 4 — ddbkds-destination-precision-sets-the-unit-of-each-record-creation-time, ddbkds-precision-update-switches-the-creation-time-unit-of-later-records: 'Check timed out after 60.0s' while waiting for the record for (table, item key) to appear on the destination shard.

Cause 5 — ddbkds-deleting-a-streaming-table-leaves-its-stream-and-delivered-records: 'Check timed out after 60.0s'.

Cause 6 — e2e-ddbkds-list-streaming-tables-all-regions-lifecycle: _PhaseError: [phase 1/4 deploy] ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeTable operation: Requested resource not found: Table: <stack>-home-streaming not found. The preceding assertions (stack status CREATE_COMPLETE, Outputs = [PlainTableName, StreamArn, StreamTableName], StreamTableName == '<stack>-home-streaming') all passed.

## Steps to Reproduce
1. Create an on-demand DynamoDB table T and a 1-shard Kinesis stream S (ARN A); wait for both ACTIVE.
2. Cause 1: ddb.enable_kinesis_streaming_destination(TableName=T, StreamArn='this-is-not-an-arn-but-clears-the-length-floor') — expect HTTP 200, DestinationStatus 'ENABLING', StreamArn echoed; then poll describe_kinesis_streaming_destination(TableName=T) until that entry is ENABLE_FAILED with a non-empty DestinationStatusDescription. Repeat with a well-formed ARN naming no stream, and with a stream ARN from another region.
3. Cause 2: enable A on T, wait ACTIVE, then (a) enable A again and read Error.Code/Error.Message — expect ValidationException with the fixed 'must be DISABLED or ENABLE_FAILED to perform ENABLE operation.' sentence; (b) enable a SECOND stream's ARN on T — expect the same refusal, observe 200/ENABLING; (c) disable A, wait DISABLED, enable the second stream — expect 200/ENABLING.
4. Causes 3/4: with the destination ACTIVE, put_item on T, then GetShardIterator/GetRecords on S and look for the change record; enable a second table with EnableKinesisStreamingConfiguration ApproximateCreationDateTimePrecision=MICROSECOND and compare the ApproximateCreationDateTime units; then update_kinesis_streaming_destination to the other precision and re-read later records.
5. Cause 5: delete_table(T), wait until it is gone, assert describe_kinesis_streaming_destination(TableName=T) is ResourceNotFoundException 'Requested resource not found: Table: T not found', then confirm S is still ACTIVE and still returns the record delivered before the delete.
6. Cause 6: create a CloudFormation stack with AWS::Kinesis::Stream + AWS::DynamoDB::Table (ProvisionedThroughput 5/5, KinesisStreamSpecification -> the stream ARN) + a plain table; wait for CREATE_COMPLETE, read Outputs, then call describe_table on the StreamTableName output.
Then replay the dynamodb__kinesis checks listed in trace_ids.
