Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sqs.sqs-service-metrics-sum-exactly-the-driven-traffic-per-queue
Issue-Key: cloudwatch__sqs.sqs-service-metrics-sum-exactly-the-driven-traffic-per-queue|_poll_until_stable#18|AssertionError|raise AssertionError(
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# cloudwatch__sqs: no AWS/SQS metric datapoints are published for a driven queue — the NumberOfMessagesSent/Received, NumberOfEmptyReceives and SentMessageSize series never produce a reading

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
One net-new root cause of service `cloudwatch__sqs`, grounded in one failing behavioral check.

Cause 1 of 1 — SQS publishes no queue metrics to CloudWatch, so the per-queue counters cannot be read at all.
AWS: recorded behavior only (metric emission is not modelled in Smithy; see the Amazon SQS CloudWatch metrics documentation). SQS pushes queue metrics to CloudWatch at one-minute intervals in namespace `AWS/SQS`, dimensioned by `QueueName` only. For driven traffic the Sums are exact: `NumberOfMessagesSent` counts each send, `NumberOfMessagesReceived` counts each message returned (a ReceiveMessage returning k messages counts k, not 1), `NumberOfEmptyReceives` counts each receive that returned nothing, and `SentMessageSize` (unit Bytes) sums the exact UTF-8 byte total of the bodies with no attribute or envelope overhead; the three counters carry unit `Count`. A queue created in the same instant but never used reports no datapoint (or an explicit Sum 0.0) under its own QueueName — never the busy queue's totals.
Emulator: the four counters for the driven queue never yield a complete reading: after 360 s of polling, the last reading was `None` — there is no AWS/SQS datapoint for the queue at all.
Why: every SQS observability path is dark — a client cannot measure traffic, size queue consumers, or alarm on queue activity, and an alarm over AWS/SQS can never evaluate.
Fix: the SQS data plane must publish AWS/SQS, QueueName-dimensioned datapoints (on send, receive and empty receive) into the CloudWatch metric store that ListMetrics/GetMetricData read, at the documented one-minute cadence. Open issue 20261109T183935Z-cloudwatch-sqs-no-aws-sqs-metric-series-is.md covers other cloudwatch__sqs checks in this family and does not carry this check's replay key.
Traces: cloudwatch__sqs.sqs-service-metrics-sum-exactly-the-driven-traffic-per-queue

## Evidence
Check cloudwatch__sqs.sqs-service-metrics-sum-exactly-the-driven-traffic-per-queue
AssertionError: the four AWS/SQS counters for QueueName=<busy queue> settling on one value (SQS publishes at one-minute intervals) never held one complete value for 120.0s within 360.0s; last reading was None
Expected for the driven traffic of this check: NumberOfMessagesSent Sum == 7, NumberOfMessagesReceived Sum == 7, NumberOfEmptyReceives Sum == 3, SentMessageSize Sum == 357.0 Bytes, dimension set exactly {('QueueName',)}; the silent twin reports no datapoint or Sum 0.0. (Those totals are this run's driven traffic; the requirement is that the series exist and sum the traffic exactly.)

## Steps to Reproduce
1. Boot the emulator and replay cloudwatch__sqs.sqs-service-metrics-sum-exactly-the-driven-traffic-per-queue.
2. Create two queues; send 7 messages to one and leave the other silent; receive all 7 and perform 3 WaitTimeSeconds=0 receives against the drained queue.
3. ListMetrics(Namespace='AWS/SQS') and GetMetricStatistics/GetMetricData for NumberOfMessagesSent, NumberOfMessagesReceived, NumberOfEmptyReceives and SentMessageSize with dimension QueueName=<busy queue>: expect datapoints with the exact Sums above and units Count/Bytes. The emulator returns no datapoints.
