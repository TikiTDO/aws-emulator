Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__kinesis.realtime-log-config-delivers-edge-request-record-to-kinesis
Issue-Key: cloudfront__kinesis.realtime-log-config-delivers-edge-request-record-to-kinesis|_poll_until#51|Timeout|last = probe()
Issue-Commit: 3cb64d4082788351a0c76dad9b646eab01e2695e

# cloudfront__kinesis: a distribution's real-time log config delivers no record to the configured Kinesis stream, so the positive real-time-logging check never grades a log line (now times out at the 60 s harness cap)

> _Note: this issue was identified against the code base at commit `3cb64d4082788351a0c76dad9b646eab01e2695e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRealtimeLogConfig

## Description
Cause 1 of 1: the CloudFront real-time logging pipeline produces no observable effect, so the one check that grades the POSITIVE path — one viewer request must yield exactly one real-time log line on the configured Kinesis stream — never reaches a single assertion and is reported as a bare harness timeout. Failing check: cloudfront__kinesis.realtime-log-config-delivers-edge-request-record-to-kinesis.

NOTE ON EXISTING ISSUES: the suite did NOT mark this check as owned on this run, so it is filed here. Three open issues describe neighbouring symptoms of the same pipeline and should be read together with this one, but none carries this run's replay key: 20261110T103113Z-cloudfront-kinesis-real-time-log-delivery-never-completes.md (keyed on an in-check `_poll_until` frame exhausting a 180 s budget — this run instead ends as a bare "Check timed out after 60.0s" with no graded response), 20261109T181319Z-cloudfront-kinesis-real-time-log-delivery-is-unreachable.md (keyed on an immediate PublishedUrlRejected refusal of the published <id>.cloudfront.net DomainName, which is not how this run fails), and 20261025T232706Z-cloudfront-kinesis-cloudfront-real-time-log-configs-never.md. The defect below is the general one: configuring real-time logging yields no record, ever.

AWS: Smithy (cloudfront 2020-05-31) models CreateRealtimeLogConfig as POST /2020-05-31/realtime-log-config -> 201 CreateRealtimeLogConfigResult{RealtimeLogConfig}, with required Name, SamplingRate (long), Fields (FieldList) and EndPoints (EndPointList, EndPoint.StreamType="Kinesis" carrying KinesisStreamConfig{RoleARN, StreamARN}); modeled errors are AccessDenied 403, InvalidArgument 400, RealtimeLogConfigAlreadyExists 409, TooManyRealtimeLogConfigs 400. Neither the role/stream validation at create time nor the delivery itself is modeled — recorded behavior only. The recorded behavior is: the config is accepted and echoes back exactly the requested field set and SamplingRate; a distribution whose DefaultCacheBehavior sets RealtimeLogConfigArn settles to Status=Deployed; a viewer request to the DomainName the distribution itself publishes is answered by the edge (200 with the origin bytes for an OAC-permitted S3 origin under the managed CachingDisabled cache policy); and, at SamplingRate=100, each served viewer request yields exactly one tab-delimited line on the configured Kinesis stream, whose columns follow CloudFront's own canonical ordering of the configured Fields — cs-uri-stem = the requested path, sc-status = the status the edge returned ("200" here), x-edge-response-result-type = "Miss" for a cold request under a caching-disabled behavior, timestamp inside the request's own clock window, c-ip non-empty.

Emulator: the check reports only `Error: Check timed out after 60.0s`; no response is ever graded. The sequence it must complete — wait for GetDistribution Status=Deployed, GET the published distribution domain until it answers 200, then drain GetRecords on a TRIM_HORIZON iterator opened before the request — never converges, so the check is cut at the harness cap. No record naming the requested key ever appears on the Kinesis stream, so none of the line's field count, cs-uri-stem, sc-status, x-edge-response-result-type, timestamp or c-ip assertions is ever reached.

Why: a client that configures CloudFront real-time logging receives no telemetry whatsoever. Dashboards, alarms and stream consumers built on real-time logs see an empty stream and cannot distinguish a working pipeline from a misconfigured one, and a caller that polls for its first log line (as AWS's own docs instruct) hangs until its own deadline instead of seeing a line within seconds of the request.

Fix: the CloudFront distribution data plane (the edge request router that must answer a viewer request on the host and port the emulator actually serves and publishes as Distribution.DomainName) together with the real-time log delivery path that, per served viewer request, renders one tab-delimited line over the configured Fields in CloudFront's canonical order and writes it to the configured Kinesis stream via the configured RoleARN; plus the distribution lifecycle that must settle Status to Deployed so the wait terminates. The log-line renderer and the Kinesis writer belong beside the realtime-log-config handler's state, not in the create handler's validation.

Traces: cloudfront__kinesis.realtime-log-config-delivers-edge-request-record-to-kinesis (positive path: one edge request must produce exactly one log line naming that request's URI stem).

## Evidence
Expected (recorded AWS behavior): CreateRealtimeLogConfig answers 201 and stores exactly the requested field set with SamplingRate=100; the distribution carrying DefaultCacheBehavior.RealtimeLogConfigArn reaches GetDistribution Status=Deployed; GET https://<Distribution.DomainName>/<key> answers 200 with the origin bytes; GetRecords on the configured single-shard stream then returns a record whose Data is exactly one tab-delimited line with cs-uri-stem="/<key>" (the key from the request), sc-status="200", x-edge-response-result-type="Miss", a timestamp inside the caller's own request window and a non-empty c-ip — the column positions being CloudFront's canonical ordering of the Fields the config reports back.

Actual (emulator): the check ends as `Error: Check timed out after 60.0s`. No log line is ever read from the Kinesis stream; the poll for a record naming the requested key never satisfies its predicate, so no status, field ordering, result type, timestamp or viewer-IP value is produced at all. No error code or non-2xx status is returned — the pipeline is silent rather than refusing.

## Steps to Reproduce
1. Kinesis CreateStream (ShardCount=1); wait for StreamStatus=ACTIVE and note StreamARN.
2. IAM CreateRole trusting cloudfront.amazonaws.com, then PutRolePolicy allowing kinesis:DescribeStream, kinesis:DescribeStreamSummary, kinesis:PutRecord and kinesis:PutRecords on that stream ARN.
3. CloudFront CreateRealtimeLogConfig with SamplingRate=100, Fields=[timestamp, c-ip, cs-uri-stem, sc-status, x-edge-response-result-type] and EndPoints=[{StreamType: "Kinesis", KinesisStreamConfig: {RoleARN, StreamARN}}]; keep the Fields order it reports back.
4. Create an S3 bucket holding one small object, create an OriginAccessControl (s3 / always / sigv4), and CreateDistribution over that S3 origin with CachePolicyId=4135ea2d-6df8-44a3-9df3-4b5a84be39ad and DefaultCacheBehavior.RealtimeLogConfigArn set to the config ARN; grant the distribution ARN s3:GetObject in the bucket policy.
5. Wait for GetDistribution Status=Deployed, open a TRIM_HORIZON shard iterator on the stream, then GET the object path on the distribution's published DomainName and confirm the served bytes.
6. Poll GetRecords: no record naming the requested key ever arrives, and the check is cut at the harness timeout instead of grading the single expected log line.
