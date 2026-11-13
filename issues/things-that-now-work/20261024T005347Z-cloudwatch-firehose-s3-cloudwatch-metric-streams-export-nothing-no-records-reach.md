Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__firehose__s3.cw-metric-stream-delivers-through-firehose-to-s3-while-untrusted-role-delivers-nothing cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-delivers-length-delimited-protobuf-with-p99 cloudwatch__firehose__s3.mstrx-e2e-exclude-by-metric-name-drops-only-those-names-and-p99-rides-the-kept-one cloudwatch__firehose__s3.obs-cloudwatch-2-statistics-configuration-lifts-total-metric-update-and-stop-quiesces-stream
Issue-Key: cloudwatch__firehose__s3.cw-metric-stream-delivers-through-firehose-to-s3-while-untrusted-role-delivers-nothing|_poll_objects#15|Timeout|time.sleep(15.0)
Issue-Key: cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-delivers-length-delimited-protobuf-with-p99|run#19|Timeout|time.sleep(15.0)
Issue-Key: cloudwatch__firehose__s3.mstrx-e2e-exclude-by-metric-name-drops-only-those-names-and-p99-rides-the-kept-one|_poll#21|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__firehose__s3.obs-cloudwatch-2-statistics-configuration-lifts-total-metric-update-and-stop-quiesces-stream|_poll#13|Timeout|time.sleep(_POLL_S)
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# CloudWatch metric streams export nothing: no records reach the Firehose stream and no object is ever written to the S3 destination

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricStream

## Description
Two net-new root causes behind the cloudwatch__firehose__s3 failures that are not OWNED by 20261022T194351Z-cloudwatch-firehose-s3-cloudwatch-metric-streams-a-cfn-declared-aws-cloudwatch.md (that issue covers the CFN-declared AWS::CloudWatch::MetricStream never being created and Outputs without OutputValue; the four checks below create their metric streams directly through PutMetricStream, so neither of its causes applies).

Cause 1 of 2: a running metric stream never exports published metrics into its Firehose stream.
AWS: PutMetricStream creates a metric stream that continuously exports CloudWatch metric updates to the named FirehoseArn using the named RoleArn; after a PutMetricData into an included namespace, records formatted per OutputFormat (json / opentelemetry0.7 / opentelemetry1.0) are pushed into that Firehose stream, with IncludeFilters/ExcludeFilters and StatisticsConfigurations (e.g. the p99 additional statistic) deciding which metrics and which statistics are exported. Recorded behavior only for the synchronous side: PutMetricStream performs no validation of the role or the target, returns 200 and reads back state 'running', so the only observable consequence of a correct or incorrect wiring is whether records are exported.
Emulator: PutMetricStream and PutMetricData both answer 200 and the stream reads back as running, but nothing is ever exported — no object appears in either destination prefix, so the checks poll their buckets until the suite budget expires ("Error: Check timed out after 60.0s"). Hypothesis: the emulator has no metric-stream exporter at all (the only synchronous surface is implemented; the background export is absent).
Why: a client cannot verify that metrics are being streamed, cannot test filter/statistics configuration semantics, and an untrusted-role or wrong-target misconfiguration is indistinguishable from a working one — on real AWS the difference shows up only in the delivered data, which here is always empty.
Fix: the metric-stream export path in the CloudWatch service crate — a metric update accepted by PutMetricData for a namespace matched by a running stream's Include/Exclude filters must be rendered in the stream's OutputFormat (honouring StatisticsConfigurations) and ingested into the referenced Firehose stream through the Firehose service's state.
Traces: cloudwatch__firehose__s3.cw-metric-stream-delivers-through-firehose-to-s3-while-untrusted-role-delivers-nothing, cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-delivers-length-delimited-protobuf-with-p99, cloudwatch__firehose__s3.mstrx-e2e-exclude-by-metric-name-drops-only-those-names-and-p99-rides-the-kept-one, cloudwatch__firehose__s3.obs-cloudwatch-2-statistics-configuration-lifts-total-metric-update-and-stop-quiesces-stream.

Cause 2 of 2: even if records reached the Firehose stream, Firehose writes no object to its S3 destination.
AWS: Smithy Firehose PutRecord doc: "Firehose buffers records before delivering them to the destination"; BufferingHints.IntervalInSeconds/SizeInMBs govern when the buffered data becomes an object under the destination BucketARN+Prefix with the declared CompressionFormat.
Emulator: records accepted by Firehose are only appended to an in-process buffer (services/service-com-amazonaws-firehose-firehose-20150804/src/state.rs, DeliveryStream.records, dropped only by the 24 h RECORD_RETENTION expiry) and the crate contains no delivery component (src/lib.rs declares only ops, scaffolded, service, state), so the destination bucket stays empty. This is the same defect filed for the firehose service token in this sweep; it is restated here because these four checks cannot pass until both halves of the path exist.
Why: the second half of the metric-streams pipeline is also a silent drop, so repairing only the exporter would leave these checks failing.
Fix: the Firehose destination-delivery component (see the firehose-scoped issue from this sweep).
Traces: the same four check ids listed in trace_ids.

## Evidence
All four checks: "Error: Check timed out after 60.0s" while polling the destination bucket/prefixes for exported metric records.
Expected (real AWS): after PutMetricStream(...state 'running') and a PutMetricData into the filtered namespace, at least two gzipped objects appear under the good prefix of the destination bucket, each record carrying the published namespace, metric name, value, unit and account id (and, for OutputFormat='opentelemetry1.0', length-delimited protobuf including the p99 additional statistic); the differential prefix belonging to the stream with the untrusted Principal.Service stays empty until it is upserted with the trusted role, after which it receives.
Actual (emulator): PutMetricStream and PutMetricData return 200 and the stream reads back 'running', but no object is ever written to any prefix of the destination bucket, so the positive barrier the negatives are graded against never arrives.

## Steps to Reproduce
1. Boot the emulator; create an S3 bucket, an IAM role whose trust policy names streams.metrics.cloudwatch.amazonaws.com with firehose:PutRecord/PutRecordBatch, and a Firehose DirectPut stream whose ExtendedS3DestinationConfiguration points at that bucket with BufferingHints {SizeInMBs:1, IntervalInSeconds:60}.
2. cloudwatch.put_metric_stream(Name=..., FirehoseArn=<stream arn>, RoleArn=<role arn>, OutputFormat='json', IncludeFilters=[{'Namespace': <unique ns>}]) -> 200; get_metric_stream reports State 'running'.
3. cloudwatch.put_metric_data(Namespace=<unique ns>, MetricData=[{'MetricName': 'StreamedMetric', 'Value': 777.0, 'Unit': 'None'}]).
4. Poll s3.list_objects_v2 on the destination bucket for several minutes: expect exported metric records; observe the bucket stays empty.
5. Repeat with OutputFormat='opentelemetry1.0' plus StatisticsConfigurations AdditionalStatistics=['p99'], and with ExcludeFilters, to see that no filter/statistics behavior is observable.
6. Re-replay the check ids in trace_ids.
