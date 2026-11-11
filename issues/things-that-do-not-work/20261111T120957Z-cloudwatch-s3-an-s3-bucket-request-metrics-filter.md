Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__s3.obs-s3-1-request-metrics-filter-counts-each-verb-exactly
Issue-Key: cloudwatch__s3.obs-s3-1-request-metrics-filter-counts-each-verb-exactly|run#50|AssertionError|assert dps, f"no {metric} datapoints in the measured window for FilterId {_FILTER_ID!r}"
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# cloudwatch__s3: an S3 bucket request-metrics filter publishes no AWS/S3 FirstByteLatency datapoints, so per-verb request metrics cannot be read

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: with a bucket metrics configuration in place and traffic driven under its prefix, the AWS/S3 request-metric series for that FilterId is empty — FirstByteLatency has no datapoints in the measured window.

AWS: recorded behavior only (metric emission is not expressible in the Smithy model). Once PutBucketMetricsConfiguration registers a metrics configuration with Id '<filter-id>' scoped to a prefix, S3 publishes request metrics for traffic matching that filter into the AWS/S3 namespace under the {BucketName, FilterId} dimension set — AllRequests, GetRequests, PutRequests, HeadRequests, DeleteRequests, 4xxErrors, BytesUploaded/BytesDownloaded and the latency metrics FirstByteLatency and TotalRequestLatency — within roughly a minute of the requests. Each verb is counted exactly once per request, and FirstByteLatency carries one sample per request that returned a body.

Emulator: after PutObject/GetObject/HeadObject/DeleteObject traffic under the filter's prefix, GetMetricStatistics for AWS/S3 FirstByteLatency with Dimensions [{BucketName},{FilterId}] returns an empty Datapoints list in the measured window ('no FirstByteLatency datapoints in the measured window for FilterId <id>'). No call is refused; the series simply does not exist.

Why: nothing that reads S3 request behaviour through CloudWatch works — per-prefix request dashboards, 4xxErrors alarms, latency SLOs — and a client cannot distinguish 'no matching traffic' from 'metrics not emitted'. Any reconciliation between the requests a client made and the metric is unverifiable, and alarms over these series never leave INSUFFICIENT_DATA.

Fix: the S3 request-metrics emission path — the bucket metrics configuration must cause S3's request handling to publish AWS/S3 datapoints under the exact {BucketName, FilterId} dimension set (including the latency families), not CloudWatch's read path, which serves other namespaces correctly.

Traces: cloudwatch__s3.obs-s3-1-request-metrics-filter-counts-each-verb-exactly.

Related open issues that do NOT carry this check's replay key and therefore do not cover it; a fixer should merge with them: 20261025T062946Z-cloudwatch-s3-s3-request-metrics-configurations-publish-no.md and 20261109T085123Z-cloudwatch-s3-deleting-one-bucket-metrics-configuration-cannot.md.

## Evidence
Check cloudwatch__s3.obs-s3-1-request-metrics-filter-counts-each-verb-exactly [mismatch]:

AssertionError: no FirstByteLatency datapoints in the measured window for FilterId 'sim2realobsverbs'

Expected: after 3 PutObjects (131, 977, 2053 bytes), 5 GetObjects, 2 HeadObjects, 1 DeleteObject and 2 missing-key GETs under the filter's prefix, AWS/S3 metrics for Dimensions [{BucketName},{FilterId}] carry the exact per-verb counts and a FirstByteLatency series with one sample per body-returning request.
Actual: GetMetricStatistics for FirstByteLatency returns an empty Datapoints list for that FilterId in the measured window.

## Steps to Reproduce
1. Create a bucket and call s3 PutBucketMetricsConfiguration with Id='<filter-id>' and a filter scoped to prefix 'obs/'.
2. Under that prefix, PutObject 3 bodies of distinct sizes, GetObject 5 times, HeadObject twice, DeleteObject once, and GET 2 nonexistent keys.
3. cloudwatch GetMetricStatistics Namespace='AWS/S3', MetricName='FirstByteLatency', Dimensions=[{'Name':'BucketName','Value':<bucket>},{'Name':'FilterId','Value':'<filter-id>'}], Period=60 over a window covering the traffic.
4. Expect datapoints (one sample per body-returning request); observe an empty Datapoints list. Repeat for GetRequests/PutRequests/HeadRequests/DeleteRequests/4xxErrors/BytesUploaded.
