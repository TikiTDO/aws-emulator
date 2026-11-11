Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__logs__s3.subscription-filter-to-firehose-decompression-strips-cwl-envelope firehose__logs__s3.cloudwatch-logging-options-emits-the-s3-access-denied-error-while-the-healthy-twin-logs-nothing
Issue-Key: firehose__logs__s3.subscription-filter-to-firehose-decompression-strips-cwl-envelope|run#50|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: firehose__logs__s3.cloudwatch-logging-options-emits-the-s3-access-denied-error-while-the-healthy-twin-logs-nothing|_poll_until#19|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# Firehose/CloudWatch Logs integration produces nothing: subscription-filter data never lands in S3 and CloudWatchLoggingOptions emits no delivery-error log events

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecordBatch

## Description
Two net-new root causes behind the firehose__logs__s3 failures.

Cause 1 of 2: records that reach a Firehose stream (whether put directly or pushed by a CloudWatch Logs subscription filter) are never written to the S3 destination, so the decompressed, envelope-stripped log events never appear.
AWS: Smithy Firehose PutRecord doc: "Firehose buffers records before delivering them to the destination"; BufferingHints.SizeInMBs/IntervalInSeconds govern when the buffer becomes an object under BucketARN+Prefix. A CloudWatch Logs subscription filter with a Firehose destination pushes gzipped CWL records into the stream, and with the CloudWatch Logs decompression processor enabled the delivered object carries the bare log messages rather than the CWL envelope.
Emulator: puts answer 200 with RecordIds and the data is only appended to an in-process buffer (services/service-com-amazonaws-firehose-firehose-20150804/src/state.rs, DeliveryStream.records, drained only by the 24 h RECORD_RETENTION expiry); the crate declares no delivery component (src/lib.rs: ops, scaffolded, service, state) and never writes to S3, so the destination bucket stays empty and the check polls until the suite budget expires ("Error: Check timed out after 60.0s"). Hypothesis: whether the subscription filter itself forwards log events into the stream cannot be told apart from this, because the only observable of either step is the delivered object.
Why: a log-archival pipeline (CloudWatch Logs -> Firehose -> S3) silently archives nothing, and the decompression/envelope-stripping processor semantics are unobservable.
Fix: the Firehose destination-delivery (flush) component (buffered records -> objects in the S3 service's state, honouring BufferingHints, Prefix and ProcessingConfiguration including the CloudWatch Logs decompression processors), plus the CloudWatch Logs subscription-filter forwarder into the Firehose service's state.
Traces: firehose__logs__s3.subscription-filter-to-firehose-decompression-strips-cwl-envelope.

Cause 2 of 2: CloudWatchLoggingOptions never writes delivery-error log events to the configured log group.
AWS: Smithy ExtendedS3DestinationConfiguration.CloudWatchLoggingOptions is "The Amazon CloudWatch logging options for your Firehose stream" (Enabled, LogGroupName, LogStreamName); when delivery to S3 fails because the stream's role cannot write the bucket, Firehose publishes an error event naming the S3 access-denied condition into that log stream, while a healthy twin stream publishes nothing.
Emulator: no log stream or log event is ever created for either stream — there is no delivery attempt at all to succeed or fail (see Cause 1), and the Firehose crate has no CloudWatch Logs writer. The check waits for the error event and times out.
Why: the only channel AWS gives an operator for diagnosing an asynchronous, silent delivery failure is missing, so a misconfigured stream and a healthy one look identical.
Fix: emit delivery-error events through the CloudWatch Logs service's state from the Firehose delivery component when a buffered batch cannot be written to the destination, gated on CloudWatchLoggingOptions.Enabled and keyed by LogGroupName/LogStreamName.
Traces: firehose__logs__s3.cloudwatch-logging-options-emits-the-s3-access-denied-error-while-the-healthy-twin-logs-nothing.

## Evidence
Both checks: "Error: Check timed out after 60.0s".
Expected (real AWS): firehose__logs__s3.subscription-filter-to-firehose-decompression-strips-cwl-envelope — objects appear in the destination bucket containing the raw log messages with the CWL gzip envelope stripped; firehose__logs__s3.cloudwatch-logging-options-emits-the-s3-access-denied-error-while-the-healthy-twin-logs-nothing — the broken stream's configured log stream receives an S3 access-denied delivery-error event while the healthy twin's log stream stays empty.
Actual (emulator): puts return 200 with RecordIds, the destination bucket never receives an object, and no log stream/log event is ever created for either stream.

## Steps to Reproduce
1. Boot the emulator; create an S3 bucket, a log group, and a firehose-trusted role (one variant granting s3:PutObject on the bucket, one variant without it).
2. firehose.create_delivery_stream with ExtendedS3DestinationConfiguration {BucketARN, RoleARN, Prefix, BufferingHints {SizeInMBs:1, IntervalInSeconds:60}, CloudWatchLoggingOptions {Enabled: true, LogGroupName, LogStreamName: 'DestinationDelivery'}}; wait for ACTIVE.
3. logs.put_subscription_filter(logGroupName=..., destinationArn=<stream arn>, roleArn=...) and logs.put_log_events(...) into the source group; also put records directly.
4. Poll s3.list_objects_v2 on the destination bucket (expect objects with the envelope stripped) and logs.get_log_events on LogGroupName/'DestinationDelivery' (expect an S3 access-denied delivery error for the role-less variant, nothing for the healthy one); observe both stay empty.
5. Re-replay the check ids in trace_ids.
