Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.xsvc-lambda-1-esm-metrics-config-kafka-only-metric-rejected
Issue-Key: cloudwatch__dynamodb__lambda.xsvc-lambda-1-esm-metrics-config-kafka-only-metric-rejected|run#9|AssertionError|assert got == want, (
Issue-Commit: 551d2b0384fb46e58b4eda1c68177a3367972419

# cloudwatch__dynamodb__lambda: UpdateEventSourceMapping refuses a duplicated MetricsConfig metric with the invalid-value message instead of AWS's duplicate-metrics message (1 cause)

> _Note: this issue was identified against the code base at commit `551d2b0384fb46e58b4eda1c68177a3367972419`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEventSourceMapping

## Description
This issue carries 1 root cause. The open issue 20261029T180026Z-cloudwatch-dynamodb-lambda-cloudwatch-lambda-ddb-stream-no-esm-metrics-cfn.md names this same check in its Cause 5, but that cause describes the opposite emulator behaviour (the Kafka-only MetricsConfig being ACCEPTED with HTTP 202) and carries that acceptance as its replay key; the emulator now refuses those values correctly and fails on a different branch, so that issue does not cover this one.

Cause 1 of 1: UpdateEventSourceMapping's MetricsConfig validation reports a DUPLICATED metric with the invalid-value message instead of AWS's duplicate-metrics message.
AWS: Smithy (services/service-com-amazonaws-lambda-awsgirapiservice) models UpdateEventSourceMapping.MetricsConfig as EventSourceMappingMetricsConfig with member Metrics -> EventSourceMappingMetricList, @length min 0 max 3, whose member targets the EventSourceMappingMetric enum with values exactly [EventCount, ErrorCount, KafkaMetrics]. The list is NOT @uniqueItems and the enum declares all three values for every event source, while the member documentation marks ErrorCount and KafkaMetrics as Amazon MSK / self-managed Apache Kafka only — so both the source-type restriction and the duplicate check exist only in the service: recorded behavior only. The recording shows two independent refusal classes, both HTTP 400 InvalidParameterValueException: (a) a metric not permitted for the mapping's resolved event source is refused with the fixed text 'Invalid EventSourceMapping metrics. Expected values: [' + the comma-separated permitted list for THAT source type + ']' (for a DynamoDB-stream source the permitted list is EventCount); (b) a Metrics list that repeats a value is refused with the fixed text 'MetricsConfig contains duplicate metrics.', which carries no variable part. A list that repeats an otherwise-permitted value falls in class (b), not class (a). In both classes the mapping's stored MetricsConfig is left unset and the mapping stays usable, and the positive control MetricsConfig={'Metrics': ['EventCount']} is accepted, echoed back and read back by GetEventSourceMapping.
Emulator: for MetricsConfig={'Metrics': ['EventCount', 'EventCount']} on a DynamoDB-stream mapping it answers InvalidParameterValueException 'Invalid EventSourceMapping metrics. Expected values: [EventCount]' — i.e. class (a)'s message for a list whose only member IS the permitted value. The single-value negatives ['ErrorCount'] and ['KafkaMetrics'] now match AWS, so only the duplicate branch diverges.
Why: a client that keys on or surfaces the refusal text is told the metric name is unsupported for this event source when the metric is in fact the only supported one and the real fault is the repetition, so the caller corrects the wrong field; tests and tooling that assert AWS's wording break.
Fix: UpdateEventSourceMapping's (and CreateEventSourceMapping's) MetricsConfig input validation in the lambda service — detect a repeated entry in Metrics and refuse it with its own message, independently of the event-source-type vocabulary check.
Traces: cloudwatch__dynamodb__lambda.xsvc-lambda-1-esm-metrics-config-kafka-only-metric-rejected

## Evidence
cloudwatch__dynamodb__lambda.xsvc-lambda-1-esm-metrics-config-kafka-only-metric-rejected:
AssertionError: MetricsConfig ['EventCount', 'EventCount'] on a DynamoDB stream mapping raised ('InvalidParameterValueException', 'Invalid EventSourceMapping metrics. Expected values: [EventCount]'), expected ('InvalidParameterValueException', 'MetricsConfig contains duplicate metrics.')

Expected: HTTP 400, code InvalidParameterValueException, message 'MetricsConfig contains duplicate metrics.' — the whole message is fixed text; nothing in it comes from the request or from state.
Actual: HTTP 400, code InvalidParameterValueException, message 'Invalid EventSourceMapping metrics. Expected values: [EventCount]' — fixed text 'Invalid EventSourceMapping metrics. Expected values: [' ... ']' with the permitted-metric list for the mapping's event-source type as its variable part.
The earlier two sub-cases in the same check (['ErrorCount'] and ['KafkaMetrics']) matched AWS, so the divergence is isolated to the duplicate-entry branch.

## Steps to Reproduce
Boot the emulator and replay cloudwatch__dynamodb__lambda.xsvc-lambda-1-esm-metrics-config-kafka-only-metric-rejected.

1. Create an IAM role trusted by lambda.amazonaws.com with dynamodb:DescribeStream/GetRecords/GetShardIterator/ListStreams.
2. Create a DynamoDB table with StreamSpecification={'StreamEnabled': True, 'StreamViewType': 'NEW_IMAGE'} and wait for it.
3. Create a python function with that role and wait for Active.
4. CreateEventSourceMapping(FunctionName=fn, EventSourceArn=<LatestStreamArn>, StartingPosition='TRIM_HORIZON', BatchSize=10) and wait for State=Enabled.
5. UpdateEventSourceMapping(UUID=uuid, MetricsConfig={'Metrics': ['ErrorCount']}) -> 400 InvalidParameterValueException 'Invalid EventSourceMapping metrics. Expected values: [EventCount]'.
6. Same with ['KafkaMetrics'] -> the same refusal.
7. Same with ['EventCount', 'EventCount'] -> must be 400 InvalidParameterValueException 'MetricsConfig contains duplicate metrics.' (emulator returns the message from step 5 instead).
8. After each refusal GetEventSourceMapping(UUID=uuid) must show no MetricsConfig.Metrics; finally MetricsConfig={'Metrics': ['EventCount']} must be accepted, echoed back and read back.
