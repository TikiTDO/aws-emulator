Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-delivers-put-metric-to-s3
Issue-Key: cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-delivers-put-metric-to-s3|_poll_until#35|Timeout|time.sleep(interval_s)
Issue-Commit: 271fde8cc1a42675cd9f2105c7c7bd008dd933ab

# cloudwatch__firehose__s3: a CloudFormation-deployed metric stream delivers no PutMetricData record to S3 (1 cause)

> _Note: this issue was identified against the code base at commit `271fde8cc1a42675cd9f2105c7c7bd008dd933ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricData

## Description
One root cause.

Cause 1 of 1: a CloudFormation-deployed AWS::CloudWatch::MetricStream wired to a Firehose delivery stream with an S3 destination delivers nothing — a PutMetricData datapoint never materialises as an object in the destination bucket.
AWS: once the metric stream is in the running state, CloudWatch exports datapoints for the configured namespaces continuously through the Firehose delivery stream, and the records land as S3 objects under the destination prefix within the stream's buffering interval.
Emulator: recorded behavior only — the check polls the destination bucket and hangs to the 60 s cap, so no object is ever created.
Why: a client that puts a metric and waits for its exported record waits forever; a metric-stream export pipeline deployed exactly as documented reports healthy while moving no data, so the absence of exported metrics is indistinguishable from a quiet namespace.
Fix: the CloudWatch metric-stream export path and its Firehose record production (and the Firehose-to-S3 delivery it feeds). Related open issues name the non-CloudFormation legs of this surface — 20261024T005347Z-cloudwatch-firehose-s3-cloudwatch-metric-streams-export-nothing-no-records-reach.md and 20261025T002604Z-cloudwatch-firehose-s3-cloudformation-deployed-metric-stream-pipeline-the-stack-reports.md — but neither carries this check's replay key.
Traces: cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-delivers-put-metric-to-s3

## Evidence
`Error: Check timed out after 60.0s` for cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-delivers-put-metric-to-s3. Expected: at least one S3 object under the destination prefix containing the metric datapoint that was put, within the delivery stream's buffering interval.

## Steps to Reproduce
Boot the emulator and replay the suite, re-running cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-delivers-put-metric-to-s3. Manual: deploy a CloudFormation stack with an AWS::KinesisFirehose::DeliveryStream to an S3 bucket plus an AWS::CloudWatch::MetricStream over a namespace, call PutMetricData in that namespace, then list the destination bucket.
