Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sqs.metrics-sqs-dlq-redrive-lands-in-dlq-own-series-not-its-sent-count
Issue-Key: cloudwatch__sqs.metrics-sqs-dlq-redrive-lands-in-dlq-own-series-not-its-sent-count|_poll_value#20|Timeout|time.sleep(interval_s)
Issue-Commit: 09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77

# cloudwatch__sqs: the AWS/SQS namespace publishes no datapoints for a redrive, so a DLQ's own depth series and the source's Sent/Received counts never appear

> _Note: this issue was identified against the code base at commit `09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: no AWS/SQS metric datapoints are published for a queue pair exercised through a dead-letter redrive, so GetMetricStatistics returns an empty Datapoints list forever and the check exhausts its 600 s budget.
AWS: recorded behavior only (AWS/SQS is a service-published namespace, not modeled in Smithy; CloudWatch GetMetricStatistics itself is modeled and answers 200 with Datapoints). The recording for this scenario is explicit: after a maxReceiveCount=1 redrive the DLQ publishes ApproximateNumberOfMessagesVisible=1.0 with Unit "Count" under its OWN QueueName dimension while its NumberOfMessagesSent series carries explicit 0.0 datapoints (a move caused by a failed processing attempt is not a send, per the AWS/SQS metric documentation), ApproximateNumberOfMessagesNotVisible=0.0; the source publishes Visible=0.0 and NotVisible=0.0 and NumberOfMessagesSent Sum=1.0, NumberOfMessagesReceived Sum=1.0. A subsequent manual SendMessage to the DLQ moves its NumberOfMessagesSent Sum to 1.0 and its Visible gauge to 2.0. Each series is dimensioned QueueName=<that queue's name>, Period 60.
Emulator: GetMetricStatistics on Namespace=AWS/SQS for either queue returns no datapoints at all, for every metric the scenario reads, so the check's first joint poll never resolves and the run ends with "Check timed out after 600.0s". The queue-attribute side is fine — the message demonstrably reaches the DLQ (ApproximateNumberOfMessages becomes "1"), which is what distinguishes this from an SQS-side defect: the redrive happens, only the metric emission is missing.
Why: a client that sizes consumers, alarms on backlog, or alarms on DLQ arrivals gets a permanently INSUFFICIENT_DATA metric and never fires; and because the absent series cannot be told apart from an explicit 0.0, the documented distinction between a redrive (not counted as a send) and a manual send to the DLQ (counted) is unobservable in either direction.
Fix: SQS's metric emission into the CloudWatch metric store — the per-queue AWS/SQS publisher (depth gauges ApproximateNumberOfMessagesVisible/NotVisible on their 60 s cadence, and the counters NumberOfMessagesSent/NumberOfMessagesReceived) must publish for both the source and the dead-letter queue, dimensioned by that queue's own QueueName, including explicit zero datapoints.
Note: two open issues describe the same dead AWS/SQS namespace from different checks — 20261109T183935Z-cloudwatch-sqs-no-aws-sqs-metric-series-is.md and 20261110T232651Z-cloudwatch-sqs-no-aws-sqs-metric-datapoints-are.md. Neither owns this check (the report lists it as unowned), and this one additionally pins the DLQ-vs-source attribution and the explicit-zero requirement on NumberOfMessagesSent, so it is recorded here as its own work item and should be re-replayed alongside them.
Traces: cloudwatch__sqs.metrics-sqs-dlq-redrive-lands-in-dlq-own-series-not-its-sent-count.

## Evidence
Check cloudwatch__sqs.metrics-sqs-dlq-redrive-lands-in-dlq-own-series-not-its-sent-count [mismatch]:
  Error: Check timed out after 600.0s

What the emulator did answer correctly: the poison message moved to the DLQ — GetQueueAttributes(ApproximateNumberOfMessages) on the DLQ became "1" and the source stopped redelivering past maxReceiveCount=1.

Where it hung: cloudwatch.get_metric_statistics(Namespace='AWS/SQS', MetricName='ApproximateNumberOfMessagesVisible'|'ApproximateNumberOfMessagesNotVisible'|'NumberOfMessagesSent'|'NumberOfMessagesReceived', Dimensions=[{'Name':'QueueName','Value':<dlq or source name>}], Period=60, Statistics=['Maximum'|'Sum']) returns Datapoints=[] indefinitely, so the first joint poll for the DLQ reading Visible=1.0 / NotVisible=0.0 never resolves.

Expected (recorded on AWS):
  DLQ   ApproximateNumberOfMessagesVisible = 1.0, Unit 'Count'; ApproximateNumberOfMessagesNotVisible = 0.0; NumberOfMessagesSent Sum = 0.0 (explicit zero datapoint at/after the gauge timestamp)
  source ApproximateNumberOfMessagesVisible = 0.0; NotVisible = 0.0; NumberOfMessagesSent Sum = 1.0; NumberOfMessagesReceived Sum = 1.0
  after one manual SendMessage to the DLQ: DLQ NumberOfMessagesSent Sum = 1.0 and Visible = 2.0

## Steps to Reproduce
1. CreateQueue a DLQ; read its QueueArn.
2. CreateQueue a source with Attributes RedrivePolicy={"deadLetterTargetArn":<dlq arn>,"maxReceiveCount":"1"}.
3. SendMessage one body to the source; ReceiveMessage it once with VisibilityTimeout=1.
4. Poll ReceiveMessage on the source until the message is gone and GetQueueAttributes on the DLQ reports ApproximateNumberOfMessages="1" (the redrive).
5. Call cloudwatch GetMetricStatistics with Namespace='AWS/SQS', Dimensions=[{'Name':'QueueName','Value':<dlq name>}], Period=60 over a window covering the move, for ApproximateNumberOfMessagesVisible (Maximum) and NumberOfMessagesSent (Sum); repeat for the source name.
6. Every call returns Datapoints=[]. On AWS the DLQ reports Visible=1.0/Unit Count with NumberOfMessagesSent Sum=0.0, and the source reports Visible=0.0, NumberOfMessagesSent Sum=1.0 and NumberOfMessagesReceived Sum=1.0.
7. SendMessage once directly to the DLQ: on AWS its NumberOfMessagesSent Sum becomes 1.0 and Visible becomes 2.0; in the emulator both series are still empty.
