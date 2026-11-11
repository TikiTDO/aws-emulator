Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__firehose__s3.cw-metric-stream-zero-buffer-firehose-delivers-while-untrusted-role-delivers-nothing cloudwatch__firehose__s3.metric-stream-statistics-configuration-lifts-total-metric-update-while-stopped-twin-exports-nothing
Issue-Key: cloudwatch__firehose__s3.cw-metric-stream-zero-buffer-firehose-delivers-while-untrusted-role-delivers-nothing|run#31|AssertionError|assert broken_keys == [], (
Issue-Key: cloudwatch__firehose__s3.metric-stream-statistics-configuration-lifts-total-metric-update-while-stopped-twin-exports-nothing|_poll#27|Timeout|time.sleep(_POLL_S)
Issue-Commit: c0ced5d18fa4eb624de2042a998eca41dbf9ea10

# CloudWatch metric streams deliver through a role CloudWatch cannot assume, and a StatisticsConfiguration stream never converges

> _Note: this issue was identified against the code base at commit `c0ced5d18fa4eb624de2042a998eca41dbf9ea10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricStream

## Description
Two net-new root causes.

Cause 1 of 2: a metric stream whose RoleArn names a role that does not trust the metric-streams service principal still exports records to its Firehose destination.
AWS: recorded behavior only for the delivery path (Smithy models PutMetricStream.RoleArn but not the assume-role check). The recording shows that when the role's trust policy does not allow streams.metrics.cloudwatch.amazonaws.com to assume it, CloudWatch cannot assume the role and nothing is delivered — the destination prefix stays empty while the healthy twin stream delivers normally.
Emulator: the stream whose role trusts only ec2.amazonaws.com delivered 2 objects under its own S3 prefix.
Why: the whole point of the RoleArn is that delivery is performed as that role; a client testing least-privilege or diagnosing a broken trust policy sees data flowing that AWS would never deliver, so a misconfiguration is invisible and an authorization boundary is not enforced.
Fix: the metric-stream delivery path — assume the configured role (honouring its trust policy) before publishing to the Firehose destination, and deliver nothing when that assume-role fails.
Traces: cloudwatch__firehose__s3.cw-metric-stream-zero-buffer-firehose-delivers-while-untrusted-role-delivers-nothing.

Cause 2 of 2: a metric stream carrying a StatisticsConfiguration never produces the metric-update/export the check waits for, so the run exhausts its 600 s budget.
AWS: recorded behavior only. The recording shows a running stream with a StatisticsConfiguration raising the total metric-update count it reports, while its stopped twin exports nothing, both observable well inside the budget.
Emulator: 'Error: Check timed out after 600.0s' — the awaited state is never reached, so neither the statistics-configuration effect nor the stopped-twin control can be read.
Why: a client that enables additional statistics on a stream never sees them take effect and has no way to tell a stopped stream from a running one by its output.
Fix: the metric-stream publisher — honour StatisticsConfiguration when assembling exported records and account for them in the stream's metric-update counters, and ensure a running stream converges to delivering within its buffering interval.
Traces: cloudwatch__firehose__s3.metric-stream-statistics-configuration-lifts-total-metric-update-while-stopped-twin-exports-nothing.

## Evidence
Check cloudwatch__firehose__s3.cw-metric-stream-zero-buffer-firehose-delivers-while-untrusted-role-delivers-nothing [mismatch]:
AssertionError: the metric stream whose role trusts 'ec2.amazonaws.com' still exported 2 object(s) to s3://<bucket>/<broken-prefix>/: ['<broken-prefix>/<yyyy/mm/dd/hh>/<stream>-1-...', ...]. CloudWatch cannot assume that role, so nothing may be delivered through it

Check cloudwatch__firehose__s3.metric-stream-statistics-configuration-lifts-total-metric-update-while-stopped-twin-exports-nothing [mismatch]:
Error: Check timed out after 600.0s

Expected: zero objects under the broken stream's prefix; and the statistics-configuration stream's metric-update lift observable within the budget.

## Steps to Reproduce
Cause 1: create two IAM roles, one trusting streams.metrics.cloudwatch.amazonaws.com and one trusting only ec2.amazonaws.com; create two Firehose delivery streams to distinct S3 prefixes; PutMetricStream twice, one per role; publish metrics; list both prefixes — the untrusted-role prefix holds objects where AWS leaves it empty.
Cause 2: PutMetricStream with a StatisticsConfiguration (and a stopped twin stream), publish metrics, and poll the reported total metric update / delivered objects — nothing converges within 600 s.
