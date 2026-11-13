Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kinesis.kinesis-streaming-destination-delivers-item-change-records
Issue-Key: dynamodb__kinesis.kinesis-streaming-destination-delivers-item-change-records|_drain#29|Timeout|time.sleep(3.0)
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# dynamodb__kinesis: an ENABLED Kinesis streaming destination never delivers item-change records to the stream

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableKinesisStreamingDestination

## Description
Cause 1 of 1: a DynamoDB table with an ACTIVE Kinesis streaming destination writes no change-data-capture records to the destination stream.

AWS: EnableKinesisStreamingDestination attaches a Kinesis data stream to a table; once DescribeKinesisStreamingDestination reports DestinationStatus `ACTIVE`, every item change (INSERT/MODIFY/REMOVE) is published to the stream as a CDC record carrying the table name, the event type, the key and — per the configured ApproximateCreationDateTimePrecision — the images, readable through GetShardIterator/GetRecords within seconds. Smithy: the dynamodb service models EnableKinesisStreamingDestination / DescribeKinesisStreamingDestination / DisableKinesisStreamingDestination and the DestinationStatus enum; the record content and the fact that records arrive is AWS documented/recorded behavior.

Emulator: the check exhausts a 600 s budget (`Check timed out after 600.0s`) polling the destination stream — no record ever arrives, so there is no record shape to compare. The destination is reported as attached but the data path is inert.

Why: a client that builds a change-data-capture pipeline on DynamoDB→Kinesis sees a destination that reports healthy and a stream that stays empty, so no downstream consumer (analytics, Firehose, Lambda) ever receives an item change. The failure is silent: nothing is refused, the records simply never appear.

Fix: the Kinesis streaming-destination delivery path in the dynamodb service crate — item writes on a table with an ACTIVE destination must publish a CDC record per change to the attached stream. Related open issues 20261103T071946Z-dynamodb-kinesis-6-net-new-root-causes-in.md, 20261103T022510Z / 20261103T042714Z (invalid-ARN and second-destination handling) and 20261108T092824Z-dynamodb-kinesis-an-enable-failed-kinesis-streaming-destination.md cover the control-plane/status side of this surface; none carries this delivery check's replay key.

Traces: dynamodb__kinesis.kinesis-streaming-destination-delivers-item-change-records.

## Evidence
Expected: after EnableKinesisStreamingDestination reaches DestinationStatus ACTIVE, PutItem/UpdateItem/DeleteItem on the table each produce a CDC record on the destination stream, readable via GetShardIterator + GetRecords.
Actual: `Error: Check timed out after 600.0s` — the stream yields no records for the item changes made.

## Steps to Reproduce
1. CreateStream in Kinesis; CreateTable in DynamoDB and wait ACTIVE.
2. EnableKinesisStreamingDestination(TableName, StreamArn); poll DescribeKinesisStreamingDestination until DestinationStatus == ACTIVE.
3. PutItem, then UpdateItem, then DeleteItem on the table.
4. GetShardIterator(LATEST/TRIM_HORIZON) + GetRecords on the stream and poll for the change records — none arrive.
Replay dynamodb__kinesis.kinesis-streaming-destination-delivers-item-change-records.
