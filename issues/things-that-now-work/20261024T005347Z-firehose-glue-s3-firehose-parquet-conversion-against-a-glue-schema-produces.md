Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__glue__s3.firehose-parquet-conversion-rejects-a-schema-mismatched-record-to-the-error-prefix
Issue-Key: firehose__glue__s3.firehose-parquet-conversion-rejects-a-schema-mismatched-record-to-the-error-prefix|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# Firehose Parquet conversion against a Glue schema produces nothing: neither converted objects nor ErrorOutputPrefix records are ever written

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecordBatch

## Description
One net-new root cause behind the firehose__glue__s3 failure.

Cause 1 of 1: with DataFormatConversionConfiguration bound to a Glue table, the emulator writes neither the converted data objects nor the schema-mismatched records that must be diverted to the error prefix, because the Firehose destination-delivery path does not exist.
AWS: Smithy ExtendedS3DestinationConfiguration.DataFormatConversionConfiguration is "The serializer, deserializer, and schema for converting data from the JSON format to the Parquet or ORC format before writing it to Amazon S3" (SchemaConfiguration naming the Glue DatabaseName/TableName/Region/RoleARN); ErrorOutputPrefix is "a prefix that Firehose evaluates and adds to failed records before writing them to S3"; PutRecord doc: "Firehose buffers records before delivering them to the destination". So records that match the Glue schema land as Parquet objects under Prefix, and a record that cannot be converted lands under ErrorOutputPrefix (the !{firehose:error-output-type} expansion) while its well-formed siblings in the same batch still arrive.
Emulator: PutRecordBatch answers 200 with a RecordId per record, but the data is only appended to an in-process buffer (services/service-com-amazonaws-firehose-firehose-20150804/src/state.rs, DeliveryStream.records, drained only by the 24 h RECORD_RETENTION expiry); the crate declares no delivery component at all (src/lib.rs: ops, scaffolded, service, state) and never writes to S3, so neither the main prefix nor the error prefix ever receives an object and the check polls until the suite budget expires ("Error: Check timed out after 60.0s").
Why: a client cannot tell valid from invalid records — a schema-mismatched record is indistinguishable from a converted one because both vanish — and no Parquet/Glue-schema behavior (conversion, ORC/Parquet SerDe options, error-prefix routing) is observable.
Fix: the Firehose destination-delivery (flush) component, including DataFormatConversionConfiguration handling against the Glue service's table state and ErrorOutputPrefix routing for records that fail conversion; the buffered records in services/service-com-amazonaws-firehose-firehose-20150804/src/state.rs currently have no consumer.
Traces: firehose__glue__s3.firehose-parquet-conversion-rejects-a-schema-mismatched-record-to-the-error-prefix.

## Evidence
Check verdict: "Error: Check timed out after 60.0s".
Expected (real AWS): after a PutRecordBatch containing both schema-conforming records and one schema-mismatched record into a stream with DataFormatConversionConfiguration bound to a Glue table, objects appear under the main prefix carrying the converted (Parquet) rows and an object appears under the ErrorOutputPrefix for the unconvertible record.
Actual (emulator): PutRecordBatch returns 200 with RecordIds; both the main prefix and the error prefix of the destination bucket stay empty indefinitely.

## Steps to Reproduce
1. Boot the emulator; create an S3 bucket, a Glue database and table describing the record columns, and an IAM role trusted by firehose.amazonaws.com with s3 write plus Glue read.
2. firehose.create_delivery_stream with ExtendedS3DestinationConfiguration = {BucketARN, RoleARN, Prefix 'main/', ErrorOutputPrefix 'err/!{firehose:error-output-type}/', BufferingHints {SizeInMBs:1, IntervalInSeconds:60}, DataFormatConversionConfiguration {Enabled: true, SchemaConfiguration -> the Glue table, InputFormatConfiguration OpenXJsonSerDe, OutputFormatConfiguration ParquetSerDe}}; wait for ACTIVE.
3. put_record_batch with several schema-conforming JSON records plus one whose types do not match the Glue schema.
4. Poll s3.list_objects_v2 under 'main/' and under 'err/' for several minutes: expect converted objects under 'main/' and a failed-record object under 'err/'; observe both stay empty.
5. Re-replay the check id in trace_ids.
