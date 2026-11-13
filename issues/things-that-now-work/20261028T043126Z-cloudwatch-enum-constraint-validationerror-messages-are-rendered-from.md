Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.get-metric-data-invalid-scan-by-value-rejected cloudwatch.put-metric-stream-invalid-output-format-rejected
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.get-metric-data-invalid-scan-by-value-rejected|run#12|AssertionError|assert message == expected, f"expected the exact message {expected!r}, got {message!r}"
Issue-Key: cloudwatch.put-metric-stream-invalid-output-format-rejected|run#7|AssertionError|assert err.get("Message", "") == expected, f"expected {expected!r}, got {err.get('Message', '')!r}"
Issue-Commit: ea106b18a26aabb2ca599f4e9ecf85c6979f5583

# CloudWatch enum-constraint ValidationError messages are rendered from the Smithy declaration order and echo the offending value inconsistently (2 causes: GetMetricData.ScanBy, PutMetricStream.OutputFormat)

> _Note: this issue was identified against the code base at commit `ea106b18a26aabb2ca599f4e9ecf85c6979f5583`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData / PutMetricStream

## Description
Two net-new root causes, both in CloudWatch's rendering of the common `ValidationError` raised when a request member violates an enum constraint. HTTP status (400) and error code (`ValidationError`) are correct in both; the MESSAGE diverges. These are distinct causes because they are two different operations/members and because they diverge in different ways — one over-reports the offending value, both order the enum set wrongly — so a single ordering fix closes neither completely.

This is NOT covered by the open issue 20261025T024908Z-cloudwatch-four-scaffolded-operations-answer-notimplemented-so-getmetricdata.md (that one is about scaffold stubs answering NotImplemented, and owns the sibling check cloudwatch.get-metric-data-duplicate-query-id-rejected), nor by 20261025T114942Z-cloudwatch-putmetricalarm-refusal-messages-diverge-from-aws-evaluation-horizon.md (PutMetricAlarm horizon/period prose). Both of those operations here DO answer, and answer with a wrong message.

---
Cause 1 of 2: GetMetricData rejects an out-of-enum `ScanBy` with the wrong ValidationError message — the offending value is echoed where AWS omits it, and the enum value set is listed in Smithy declaration order.

AWS:      Smithy: GetMetricData input member `ScanBy` targets com.amazonaws.cloudwatch#ScanBy, an enum whose declared values are [TimestampDescending, TimestampAscending]. `ScanBy` has no modeled per-member error, so an out-of-enum value surfaces as the service-common `ValidationError` at HTTP 400. Recorded behavior only for the message text: AWS returns exactly `1 validation error detected: Value at 'scanBy' failed to satisfy constraint: Member must satisfy enum value set: [TimestampAscending, TimestampDescending]`. Breaking that template down: the fixed text is `1 validation error detected: Value at '<memberPath>' failed to satisfy constraint: Member must satisfy enum value set: [<set>]`; the variable parts are the lowerCamel member path taken from the request (`scanBy`) and the rendered enum set. Two properties of AWS's rendering matter and neither follows from the model: (a) for this member AWS does NOT echo the rejected request value — there is no `Value '<x>' at` segment, only `Value at`; (b) AWS renders the set as TimestampAscending, TimestampDescending, which is the REVERSE of the model's declaration order, i.e. the ordering is a fixed server-side property of the member and cannot be derived by iterating the Smithy enum.

Emulator: returns code `ValidationError` (correct) with message `1 validation error detected: Value 'NotAValidScanByValue' at 'scanBy' failed to satisfy constraint: Member must satisfy enum value set: [TimestampDescending, TimestampAscending]` — it inserts a `Value '<the request's ScanBy string>' at` segment AWS does not emit for this member, and it renders the enum set in the model's declaration order [TimestampDescending, TimestampAscending] instead of AWS's [TimestampAscending, TimestampDescending].

Why:      A client that asserts on, logs, or pattern-matches the validation message for a bad ScanBy (SDK integration tests, user-facing error surfacing, log-based alerting on validation failures) sees a different message than AWS produces. Worse, the emulator reflects the caller's rejected input back into the message where AWS deliberately does not, so a caller that treats the emulator's messages as AWS's will build message parsing and redaction expectations that break against real CloudWatch.

Fix:      The CloudWatch request-validation / ValidationError message renderer for enum-constraint violations (the component that builds the `1 validation error detected: ...` string for GetMetricData's input), not the GetMetricData handler's query logic. It needs the AWS-observed rendering per member — whether the offending value is echoed, and the set ordering — rather than deriving both from the Smithy enum definition.

Traces:   cloudwatch.get-metric-data-invalid-scan-by-value-rejected (mismatch; the identical query with ScanBy=TimestampAscending is the check's positive control and is not implicated).

---
Cause 2 of 2: PutMetricStream rejects an out-of-enum `OutputFormat` with the enum value set listed in Smithy declaration order instead of AWS's order.

AWS:      Smithy: PutMetricStream input member `OutputFormat` targets com.amazonaws.cloudwatch#MetricStreamOutputFormat, an enum with @length min 1 max 255 whose declared values are [json, opentelemetry0.7, opentelemetry1.0]. An out-of-enum value is refused with the service-common `ValidationError` at HTTP 400 and the stream is NOT created (a subsequent GetMetricStream answers ResourceNotFoundException). Recorded behavior only for the message text: AWS returns exactly `1 validation error detected: Value 'not-a-real-format' at 'outputFormat' failed to satisfy constraint: Member must satisfy enum value set: [opentelemetry1.0, json, opentelemetry0.7]`. Fixed text: `1 validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must satisfy enum value set: [<set>]`; the variable parts are the rejected value taken from the request (here the caller's OutputFormat string) and the member path `outputFormat`. The set ordering AWS emits — opentelemetry1.0, json, opentelemetry0.7 — is again NOT the model's declaration order, confirming the ordering is a per-member server-side constant and not an iteration of the Smithy enum. Note the contrast with Cause 1: for this member AWS DOES echo the rejected value, so the echo is a per-member property too.

Emulator: returns code `ValidationError` (correct) and the correct `Value '<rejected value>' at 'outputFormat'` prefix, but renders the enum set as `[json, opentelemetry0.7, opentelemetry1.0]` — the Smithy declaration order — instead of AWS's `[opentelemetry1.0, json, opentelemetry0.7]`.

Why:      Same client impact as Cause 1 for the control plane: any consumer that compares the full validation message, or that parses the advertised allowed-value list out of it to present remediation to a user, gets a string CloudWatch never returns. Because the divergence here is ordering only, it is invisible to a loose substring check and will silently fail an exact-message contract.

Fix:      The same CloudWatch enum-constraint ValidationError message renderer, applied to PutMetricStream's input validation. The allowed-value list must be emitted in AWS's per-member order, which has to be recorded per member rather than taken from the Smithy enum ordering.

Traces:   cloudwatch.put-metric-stream-invalid-output-format-rejected (mismatch; the check's later arms — GetMetricStream answering ResourceNotFoundException with an empty message, and the positive control with OutputFormat=json succeeding and round-tripping — were not reached and are not implicated).

## Evidence
Cause 1 — cloudwatch.get-metric-data-invalid-scan-by-value-rejected (verdict: mismatch)
  Request: GetMetricData(MetricDataQueries=[{Id:'m1',MetricStat:{Metric:{Namespace:<unique>,MetricName:'RequestCount'},Period:60,Stat:'Sum'}}], StartTime=now-10m, EndTime=now+10m, ScanBy='NotAValidScanByValue')
  Error code: ValidationError — MATCHES AWS.
  Expected message (AWS): "1 validation error detected: Value at 'scanBy' failed to satisfy constraint: Member must satisfy enum value set: [TimestampAscending, TimestampDescending]"
  Actual message (emulator): "1 validation error detected: Value 'NotAValidScanByValue' at 'scanBy' failed to satisfy constraint: Member must satisfy enum value set: [TimestampDescending, TimestampAscending]"
  Two deltas: (1) the emulator adds a "Value '<request's ScanBy>' at" segment AWS omits for this member; (2) the enum set order is reversed relative to AWS.
  Smithy cross-check: com.amazonaws.cloudwatch#ScanBy declares values in the order [TimestampDescending, TimestampAscending] — the emulator's order — proving the renderer iterates the model rather than reproducing AWS's fixed per-member order.
  Assertion site: check_cloudwatch_get_metric_data_invalid_scan_by_value_rejected.py line 52.

Cause 2 — cloudwatch.put-metric-stream-invalid-output-format-rejected (verdict: mismatch)
  Request: PutMetricStream(Name=<unique>, FirehoseArn=<created stream>, RoleArn=<created role>, OutputFormat='not-a-real-format')
  Error code: ValidationError — MATCHES AWS.
  Expected message (AWS): "1 validation error detected: Value 'not-a-real-format' at 'outputFormat' failed to satisfy constraint: Member must satisfy enum value set: [opentelemetry1.0, json, opentelemetry0.7]"
  Actual message (emulator): "1 validation error detected: Value 'not-a-real-format' at 'outputFormat' failed to satisfy constraint: Member must satisfy enum value set: [json, opentelemetry0.7, opentelemetry1.0]"
  Delta: enum-set ordering only. The value-echo and member path are correct here.
  Smithy cross-check: com.amazonaws.cloudwatch#MetricStreamOutputFormat declares values in the order [json, opentelemetry0.7, opentelemetry1.0] — again exactly the emulator's output, and again not AWS's.
  Assertion site: check_cloudwatch_put_metric_stream_invalid_output_format_rejected.py line 33.

Joint evidence for the single underlying mechanism: in BOTH causes the emulator's rendered allowed-value list is byte-for-byte the Smithy declaration order of the target enum, and in both cases AWS's is a different, fixed order. The value-echo behaviour differs between the two members on AWS (omitted for scanBy, present for outputFormat) while the emulator echoes unconditionally, which is why the two must be fixed as two causes.

## Steps to Reproduce
Cause 1 (GetMetricData.ScanBy):
1. Point a botocore/AWS SDK CloudWatch client at the emulator.
2. PutMetricData into a fresh unique namespace: Namespace=<unique>, MetricData=[{MetricName:'RequestCount', Value:3.0, Timestamp:now}].
3. Call GetMetricData with MetricDataQueries=[{Id:'m1', MetricStat:{Metric:{Namespace:<that namespace>, MetricName:'RequestCount'}, Period:60, Stat:'Sum'}, ReturnData:true}], StartTime=now-10m, EndTime=now+10m, and ScanBy set to any string outside the ScanBy enum (e.g. 'NotAValidScanByValue').
4. Catch the ClientError and read Error.Code and Error.Message.
   Observed: Code='ValidationError' (correct); Message="1 validation error detected: Value 'NotAValidScanByValue' at 'scanBy' failed to satisfy constraint: Member must satisfy enum value set: [TimestampDescending, TimestampAscending]".
   Expected (AWS): "1 validation error detected: Value at 'scanBy' failed to satisfy constraint: Member must satisfy enum value set: [TimestampAscending, TimestampDescending]" — no echoed value, reversed set order.
5. Re-issue the identical query with ScanBy='TimestampAscending': it must return HTTP 200 with MetricDataResults[0].Id == 'm1' (this arm already passes and must keep passing).

Cause 2 (PutMetricStream.OutputFormat):
1. Stand up the metric-stream prerequisites: an S3 bucket, an IAM role trusted by firehose.amazonaws.com with s3:PutObject/GetBucketLocation/ListBucket on that bucket, a DirectPut Firehose delivery stream with that S3 destination (wait for ACTIVE), and an IAM role trusted by streams.metrics.cloudwatch.amazonaws.com with firehose:PutRecord/PutRecordBatch on the delivery stream.
2. Call PutMetricStream(Name=<unique>, FirehoseArn=<delivery stream ARN>, RoleArn=<metric-stream role ARN>, OutputFormat='not-a-real-format').
3. Catch the ClientError and read Error.Code and Error.Message.
   Observed: Code='ValidationError' (correct); Message ends with the allowed set rendered as "[json, opentelemetry0.7, opentelemetry1.0]".
   Expected (AWS): the same message with the set rendered as "[opentelemetry1.0, json, opentelemetry0.7]".
4. Confirm the remaining arms: GetMetricStream(Name=<that name>) must answer ResourceNotFoundException with an empty message, and the identical PutMetricStream with OutputFormat='json' must succeed, return an Arn, and GetMetricStream must report OutputFormat=='json'.

Replay shortcut: re-run the two checks named in trace_ids.
