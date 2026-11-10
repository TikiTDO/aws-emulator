Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__sqs.invocations-sent-to-dlq-counts-only-the-rule-whose-target-failed
Issue-Key: cloudwatch__events__sqs.invocations-sent-to-dlq-counts-only-the-rule-whose-target-failed|_poll_until#52|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# cloudwatch__events__sqs: EventBridge publishes no AWS/Events InvocationsSentToDlq datapoint when a rule's target invocation fails and the event is dead-lettered

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
One net-new root cause of service `cloudwatch__events__sqs`, grounded in one failing behavioral check.

Cause 1 of 1 — the dead-letter attribution metric for a rule is never published.
AWS: recorded behavior only (CloudWatch metric emission is not modelled in Smithy; see the EventBridge monitoring documentation). When a rule's target invocation fails and EventBridge writes the event to that rule's dead-letter queue, EventBridge publishes a datapoint in namespace `AWS/Events` with dimension `RuleName` = the failing rule, under the documented metric name (`InvocationsSentToDlq`, also spelled `InvocationsSentToDLQ` in parts of the documentation), with a Sum of at least 1. A sibling rule whose target succeeded gets no such datapoint — the metric is attributed per rule.
Emulator: the dead-letter delivery happens or not, but neither spelling ever reports a datapoint: after 300 s of polling, both `InvocationsSentToDlq` and `InvocationsSentToDLQ` read 0 for the failing rule.
Why: a client that alarms on dead-lettered EventBridge deliveries, or that attributes a dead-letter to the rule that caused it, sees a permanently-zero series and can never detect or attribute delivery failures.
Fix: the EventBridge target-delivery failure path (the component that writes to a rule's DeadLetterConfig) must publish the AWS/Events RuleName-dimensioned datapoint into the CloudWatch metric store that GetMetricData/GetMetricStatistics read.
Traces: cloudwatch__events__sqs.invocations-sent-to-dlq-counts-only-the-rule-whose-target-failed

## Evidence
Check cloudwatch__events__sqs.invocations-sent-to-dlq-counts-only-the-rule-whose-target-failed
AssertionError: AWS/Events recorded a sent-to-DLQ invocation for the failing rule <rule-name> under at least one of its two documented metric spellings not met within 300s. Last result: {'InvocationsSentToDlq': 0, 'InvocationsSentToDLQ': 0}
(The rule name is a per-run value; the requirement is a non-zero Sum under dimension RuleName for the rule whose target failed, and none for the sibling rule.)

## Steps to Reproduce
1. Boot the emulator and replay cloudwatch__events__sqs.invocations-sent-to-dlq-counts-only-the-rule-whose-target-failed.
2. Create two rules on the same bus, each with its own target and DeadLetterConfig queue; make one rule's target fail (e.g. a target the rule's role cannot invoke) and leave the other succeeding.
3. PutEvents matching both rules and confirm the dead-letter queue of the failing rule receives the event.
4. GetMetricStatistics/GetMetricData for namespace AWS/Events, dimension RuleName=<failing rule>, metric InvocationsSentToDlq (and InvocationsSentToDLQ): expect Sum >= 1 for the failing rule and no datapoint for the succeeding one. The emulator reports 0 for both spellings.
