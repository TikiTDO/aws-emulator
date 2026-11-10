Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-destination-echoes-configuration-and-refuses-short-freshness
Issue-Key: glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-destination-echoes-configuration-and-refuses-short-freshness|_create_channel_error#3|AssertionError|raise AssertionError(f"CreateChannel accepted {channel_name!r}: {resp['ChannelDescription']!r}")
Issue-Commit: 46b1ef78a2de21b94439ce78901879d208363fd1

# Kinesis CreateChannel accepts an S3 Tables DataFreshnessInSeconds below the 300 s minimum instead of refusing it with ValidationException

> _Note: this issue was identified against the code base at commit `46b1ef78a2de21b94439ce78901879d208363fd1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
Cause 1 of 1: CreateChannel with an S3TablesDestinationConfiguration whose DataFreshnessInSeconds is below the documented minimum (299 s, one second under 300) is accepted and a channel is created, instead of being refused.

AWS: the Smithy model does not state the rule — com.amazonaws.kinesis#S3TablesDestinationConfiguration.DataFreshnessInSeconds targets a plain integer with no @range, and its doc only says "The maximum age, in seconds, of undelivered data before the channel delivers it to the destination. The default value is 300 seconds." The documented delivery range for a streaming-table destination is 300-900 seconds (https://docs.aws.amazon.com/streams/latest/dev/data-delivery-st-about.html), and the recorded behavior is that the service refuses an out-of-range value: HTTP 400 with error code ValidationException (Smithy: CreateChannel errors include ValidationException, httpError 400) and the message "DataFreshnessInSeconds must be between 300 and 900." In that message the fixed text is "DataFreshnessInSeconds must be between " ... " and " ... "."; the two variable parts are the documented bounds of the member (300 and 900), not values from this one run. No channel is created by the refused request, so ListChannels for the source stream does not list the refused ChannelName.
Emulator: the call succeeds with HTTP 200 and returns a full ChannelDescription — ChannelStatus CREATING, a generated ChannelARN/ChannelId, and an S3TablesDestinationConfiguration echoing DataFreshnessInSeconds 299 verbatim (replay: "AssertionError: CreateChannel accepted '<channel-name>': {'ChannelName': ..., 'ChannelStatus': 'CREATING', ..., 'S3TablesDestinationConfiguration': {'DataFreshnessInSeconds': 299, ...}}"). The out-of-range value is persisted and the channel is registered on the stream; the ARNs, ids, names and timestamps in that output are per-run values.
Why: a client that relies on the service to reject an unsupported freshness window gets a channel the real service would never have created, so its error path keyed on ValidationException / "DataFreshnessInSeconds must be between 300 and 900." never runs, and the misconfiguration is only discovered (or never discovered) much later at delivery time. Code that validates by round-tripping a deliberately bad value against the API also sees the wrong verdict, and the accepted channel leaves residue on the stream.
Fix: CreateChannel's request validation for the S3 Tables destination (range check on S3TablesDestinationConfiguration.DataFreshnessInSeconds, refusing with ValidationException before any channel is persisted) in the Kinesis CreateChannel handler.
Traces: glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-destination-echoes-configuration-and-refuses-short-freshness.

Related but distinct: open issue 20261109T141301Z-glue-iam-kinesis-s3-s3tables-kinesis-createchannel-with-an-s3-tables-destination-never.md covers the missing destination-table provisioning and the missing PartitionSpec requirement on the same operation; neither covers the DataFreshnessInSeconds range, which is a separate unenforced validation and is not carried by that issue's replay keys.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout.
[mismatch] glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-destination-echoes-configuration-and-refuses-short-freshness, line 37 `got = _create_channel_error(ctx, infra, bad_name, _FRESHNESS_MIN_S - 1)` -> line 107 `raise AssertionError(f"CreateChannel accepted {channel_name!r}: {resp['ChannelDescription']!r}")`.
Expected: HTTP 400, (Code, Message) == ("ValidationException", "DataFreshnessInSeconds must be between 300 and 900."), and the refused ChannelName absent from ListChannels on the source stream.
Actual: HTTP 200 with a ChannelDescription — ChannelStatus 'CREATING', S3TablesDestinationConfiguration {'DataFreshnessInSeconds': 299, 'DeadLetterQueueS3Configuration': {...}, 'S3TablesConfigurationList': [{'CompressionType': 'ZSTD', 'PartitionSpec': {...}, ...}]} — i.e. the sub-minimum freshness was stored and echoed. (The channel/stream/role/table ARNs, ChannelId and ChannelCreationTimestamp in the recorded output are per-run values and are not part of the requirement.)

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create the prerequisites: a Kinesis data stream, a Glue schema, an S3 Tables table bucket + namespace + table, an S3 dead-letter bucket, and an IAM service-execution role.
3. Call kinesis.create_channel(ChannelName=<name>, ServiceExecutionRoleARN=<role-arn>, StreamConfigurationList=[{"StreamARN": <stream-arn>, "RecordConfiguration": {"RecordFormatType": "JSON", "GSRSchemaARN": <schema-arn>}}], S3TablesDestinationConfiguration={"DataFreshnessInSeconds": 299, "DeadLetterQueueS3Configuration": {"BucketARN": <dlq-bucket-arn>, "ExpectedBucketOwner": <account>}, "S3TablesConfigurationList": [{"TableBucketARN": <table-bucket-arn>, "Namespace": <ns>, "TableName": <table>, "CompressionType": "ZSTD", "PartitionSpec": {"PartitionFields": [{"Transform": "TIME_HOUR", "SourceName": "creation_ts"}]}}]}).
4. Expect HTTP 400 ValidationException "DataFreshnessInSeconds must be between 300 and 900." and no such channel in ListChannels(StreamARN=<stream-arn>); the emulator instead returns 200 and a CREATING channel echoing DataFreshnessInSeconds 299.
5. Repeat with DataFreshnessInSeconds=300 for the positive control: the channel is expected to be created and reach ACTIVE.
