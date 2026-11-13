Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-delivers-length-delimited-protobuf-with-p99
Issue-Key: cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-delivers-length-delimited-protobuf-with-p99|run#21|AssertionError|assert found["name"] == f"amazonaws.com/{namespace}/{name}", f"{found['name']!r}"
Issue-Commit: 3dbcd9c28225866d3017c5499165947b0ec48c10

# cloudwatch__firehose__s3: an opentelemetry1.0 metric stream names each OTLP metric with the bare metric name instead of amazonaws.com/<namespace>/<metric name>

> _Note: this issue was identified against the code base at commit `3dbcd9c28225866d3017c5499165947b0ec48c10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricStream

## Description
ONE net-new root cause in the CloudWatch metric-stream OpenTelemetry serializer, grounded in a failing check the suite marked neither ALREADY FILED nor OWNED.

Relationship to open issues: 20261109T135435Z-cloudwatch-firehose-s3-4-net-new-causes-metric.md (open) lists this same check under its Cause 3, "OutputFormat opentelemetry0.7/opentelemetry1.0 streams deliver nothing" — the check then failed before any object was readable ("the OTel stream did not deliver both 'Latency' and 'Throughput' ... objects [], metrics decoded []"). That is NOT what this replay shows: the opentelemetry1.0 stream now DOES deliver, the objects decode as length-delimited protobuf, and the ExportMetricsServiceRequest messages are parsed far enough to read resource attributes and per-datapoint attributes (Namespace/MetricName matched the check's namespace, so the summaries were found). The divergence that becomes observable once delivery works is the OTLP Metric.name field, and it needs its own fix. The suite's replay key for this check changed accordingly, which is why it is not reported as OWNED.

=== Cause 1 of 1: Metric.name in the delivered OTLP payload is not the AWS-qualified metric name ===
Failing check: cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-delivers-length-delimited-protobuf-with-p99.
AWS: Smithy models only the format selector — PutMetricStreamInput.OutputFormat targets MetricStreamOutputFormat, an enum whose values are json | opentelemetry0.7 | opentelemetry1.0 — the payload layout itself is not in the model, so this is AWS's documented metric-streams OpenTelemetry 1.0.0 format plus recorded behavior (format reference cited by the check: https://docs.aws.amazon.com/AmazonCloudWatch/latest/monitoring/CloudWatch-metric-streams-formats-opentelemetry-100.html). In that format every metric CloudWatch streams is emitted as an OTLP Summary metric whose `name` field is the AWS-qualified name `amazonaws.com/<namespace>/<metric name>` — the fixed parts are the literal `amazonaws.com/` prefix and the `/` separators, the variable parts are the metric's own namespace and metric name taken from the streamed series (so a metric MetricName=M published in namespace NS streams as `amazonaws.com/NS/M`, e.g. `amazonaws.com/AWS/EC2/CPUUtilization`). The unqualified namespace and metric name are additionally carried as the datapoint's `Namespace`/`MetricName` attributes, which is where a consumer reads them back; the `name` field is the qualified form.
Emulator: the delivered Metric carries the bare metric name. The check asserts `found["name"] == f"amazonaws.com/{namespace}/{name}"` and the assertion reports the emulator's value: `AssertionError: 'Latency'` — i.e. `name` is `Latency`, not `amazonaws.com/<Sim2RealCwStreamFmt-...>/Latency`, for a metric published into the check's unique custom namespace. The prefix and namespace qualification are simply absent for every streamed series; the datapoint attributes themselves are right (they are what the check matched the namespace on).
Why: an OTLP consumer (an OpenTelemetry collector, a Prometheus remote-write bridge, any OTLP backend) keys and routes on Metric.name. With the namespace stripped out of the name, series from different CloudWatch namespaces that share a metric name (AWS/ApplicationELB Latency vs a custom Latency) collide into one time series, dashboards and recording rules written against `amazonaws.com/...` names match nothing, and a pipeline validated against the emulator misbehaves the moment it is pointed at real CloudWatch.
Fix: the metric-stream OpenTelemetry serializer — the component that builds the ExportMetricsServiceRequest records for OutputFormat opentelemetry0.7/opentelemetry1.0 before they are published to the destination Firehose stream — must set each Metric's `name` to `amazonaws.com/<namespace>/<metric name>` from the streamed series' namespace and metric name, while leaving the unqualified values in the datapoint attributes. Note the check stops at this assertion, so everything it grades after it is still unverified on this run: `unit == "1"`, the min/max quantiles (0.0 -> 1, 1.0 -> 100), count/sum agreement, the p99 quantile for the metric named in StatisticsConfigurations, and the control metric carrying only the base min/max quantiles. Re-replay the check after the fix rather than assuming the rest passes.
Traces: the check id above.

## Evidence
Replayed suite: simparity-behavioral-aws-v1 — cloudwatch__firehose__s3 6/12 (50%), 6 failing checks; five are marked OWNED by open issues, this one is not.

Failing check: cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-delivers-length-delimited-protobuf-with-p99

Expected (AWS): for a metric MetricName published into namespace NS through a stream with OutputFormat='opentelemetry1.0', each delivered OTLP Summary metric has name == 'amazonaws.com/<NS>/<MetricName>'.

Actual (emulator): the decoded Metric.name is the bare metric name.
  File "check_cloudwatch__firehose__s3_deep_cloudwatch_3_cwstreamfmt_otel_delivers_length_delimited_protobuf_with_p99.py", line 125, in run
    assert found["name"] == f"amazonaws.com/{namespace}/{name}", f"{found['name']!r}"
  AssertionError: 'Latency'

Context from the same replay: the stream was accepted (GetMetricStream reported State 'running', OutputFormat 'opentelemetry1.0'), objects landed under the configured S3 prefix, each object split cleanly as varint-length-delimited protobuf (the check's _split_delimited did not raise), and the per-datapoint attributes carried the check's unique Namespace, which is how the summaries for 'Latency' and 'Throughput' were located. So delivery and framing work; only the Metric.name qualification diverges.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. s3.create_bucket(Bucket=B); firehose.create_delivery_stream for a DirectPut/ExtendedS3 stream S_fh on B under prefix P, with a delivery role that can write B; wait until ACTIVE.
3. iam.create_role(R) trusting streams.metrics.cloudwatch.amazonaws.com with firehose:PutRecord/PutRecordBatch on S_fh.
4. NS = a unique custom namespace. cloudwatch.put_metric_stream(Name=S, FirehoseArn=<S_fh arn>, RoleArn=R, OutputFormat='opentelemetry1.0', IncludeFilters=[{'Namespace': NS}], StatisticsConfigurations=[{'IncludeMetrics': [{'Namespace': NS, 'MetricName': 'Latency'}], 'AdditionalStatistics': ['p99']}]).
5. Repeatedly cloudwatch.put_metric_data(Namespace=NS, MetricData=[{'MetricName': 'Latency'|'Throughput', 'Values': [1..100], 'Counts': [1]*100, 'Timestamp': now}]) until objects appear under s3://B/P.
6. s3.get_object each key and decode the body as varint-length-delimited OTLP ExportMetricsServiceRequest protobuf; for each Summary metric read the Metric.name field.
7. Expect name == 'amazonaws.com/<NS>/Latency' and 'amazonaws.com/<NS>/Throughput'; observe 'Latency' and 'Throughput'.
8. Re-replay cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-delivers-length-delimited-protobuf-with-p99 to confirm the fix and to grade the assertions that follow it (unit '1', min/max quantiles, count/sum, the p99 quantile only on the metric named in StatisticsConfigurations).
