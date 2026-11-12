Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sqs.metrics-sqs-delayed-message-counted-only-in-delayed-series
Issue-Key: cloudwatch__sqs.metrics-sqs-delayed-message-counted-only-in-delayed-series|_poll_joint#10|Timeout|time.sleep(interval_s)
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# SQS publishes no AWS/SQS delayed-message metric series, so a delayed message is never counted in ApproximateNumberOfMessagesDelayed

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: a message sent with a delay mints no AWS/SQS CloudWatch datapoints, so the delayed-vs-visible split is unobservable.
AWS: recorded behavior only (the sqs Smithy model describes DelaySeconds and the queue attributes, not the emitted metrics). A message sent with DelaySeconds > 0 is counted ONLY in the AWS/SQS delayed series (ApproximateNumberOfMessagesDelayed) under {QueueName} while the delay is in force — not in the visible series — and moves to the visible series when the delay expires.
Emulator: the check exhausts its 60s budget polling CloudWatch for the AWS/SQS series for the queue; no datapoint appears at all, so neither the delayed count nor its exclusivity can be observed.
Why: queue-depth dashboards and backlog alarms (the primary SQS operational signal) are empty, and a customer cannot tell a delayed backlog from a visible one.
Fix: the SQS queue state must publish its AWS/SQS series (ApproximateNumberOfMessagesDelayed / -Visible / -NotVisible under {QueueName}) into the CloudWatch metric store.
Traces: cloudwatch__sqs.metrics-sqs-delayed-message-counted-only-in-delayed-series.

## Evidence
cloudwatch__sqs.metrics-sqs-delayed-message-counted-only-in-delayed-series — `Error: Check timed out after 60.0s`. The check creates a queue, sends a message with DelaySeconds set, then polls CloudWatch for the AWS/SQS delayed and visible series under {QueueName}; neither series ever yields a datapoint.

## Steps to Reproduce
Replay cloudwatch__sqs.metrics-sqs-delayed-message-counted-only-in-delayed-series: create a queue, SendMessage with DelaySeconds > 0, then poll CloudWatch GetMetricStatistics for AWS/SQS ApproximateNumberOfMessagesDelayed and ApproximateNumberOfMessagesVisible dimensioned by QueueName.
