Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__opensearch__sns__sqs.obs-opensearch-2-searchable-documents-metric-crosses-threshold-and-alarm-notifies-sqs
Issue-Key: cloudwatch__opensearch__sns__sqs.obs-opensearch-2-searchable-documents-metric-crosses-threshold-and-alarm-notifies-sqs|_wait_alarm_state#14|TimeoutError|raise TimeoutError(f"alarm {name!r} reaching {want!r}: the run budget ended first; not graded")
Issue-Commit: 56af4a38801370870cefc18c3ef22e7084182017

# cloudwatch__opensearch__sns__sqs: a metric alarm on AWS/ES SearchableDocuments for a live domain never leaves its initial state — it never reaches OK, so the indexing-driven OK->ALARM transition and its SNS notification are unreachable

> _Note: this issue was identified against the code base at commit `56af4a38801370870cefc18c3ef22e7084182017`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
ONE net-new root cause, grounded in the one failing check of this suite's `cloudwatch__opensearch__sns__sqs` service that the replay report marks neither ALREADY FILED nor OWNED.

Related OPEN issues, named for context only — neither carries this check's replay key, so neither covers this failure: 20261025T052047Z-cloudwatch-opensearch-sns-sqs-both-aws-es-searchabledocuments.md (same surface, recorded when both checks hung at the 60 s harness cap) and 20261111T000041Z-cloudwatch-opensearch-no-aws-es-metric-series-is.md (the metric-catalogue side of the same gap).

Cause 1 of 1: an alarm whose metric is AWS/ES SearchableDocuments (Dimensions ClientId + DomainName) for an existing, searchable domain never evaluates into OK.

AWS: recorded behavior only (CloudWatch alarm evaluation is not expressed in the Smithy model). An OpenSearch Service domain emits AWS/ES SearchableDocuments at 1-minute resolution; a MetricAlarm created over it with Period=60, EvaluationPeriods=1, Statistic=Maximum, Threshold=10, GreaterThanOrEqualToThreshold and TreatMissingData=ignore leaves INSUFFICIENT_DATA and settles in OK within a few minutes while the domain holds fewer than 10 documents, and DescribeAlarms reports StateValue "OK". Indexing 10 documents then drives exactly one StateUpdate OK -> ALARM whose StateReason names the crossed "threshold (10.0)", and the alarm's AlarmActions topic publishes one CloudWatch notification that reaches the subscribed SQS queue.

Emulator: DescribeAlarms never reports OK for that alarm. The check polls DescribeAlarms (its only observable) for up to 600 s and the state never becomes OK, so the graded OK -> ALARM leg, the single ALARM StateUpdate in DescribeAlarmHistory, and the SNS->SQS notification are never reached. The earlier legs of the same check did work — the domain was created, became searchable, its REST data plane answered, PutMetricAlarm stored the alarm and DescribeAlarms reported it with AlarmActions = [the topic] and a non-ALARM initial state — so the emulator is serving and the divergence is confined to alarm evaluation over this metric. Hypothesis (consistent with the sibling open issue above): no AWS/ES metric series is published for a domain, so the alarm has no datapoints to evaluate and, with TreatMissingData=ignore, never transitions out of its initial state. Note for the fixer: this replay ended with the harness's run-budget TimeoutError rather than the check's own assertion (the check labels that outcome "not graded"), so re-confirm with a direct probe — create the alarm over a live domain and poll DescribeAlarms — rather than from the timed replay alone.

Why: alarms over AWS/ES metrics are the standard way an OpenSearch domain is monitored. A client that waits for OK before asserting a breach waits forever, dashboards show a permanently uninitialised alarm, and no alarm-driven SNS notification is ever produced, so the whole metric->alarm->notify pipeline is untestable against the emulator.

Fix: the CloudWatch alarm-evaluation component and the AWS/ES metric publication that feeds it — the OpenSearch domain must emit SearchableDocuments datapoints under namespace AWS/ES with the ClientId and DomainName dimensions, and the alarm evaluator must consume them to drive INSUFFICIENT_DATA -> OK -> ALARM with the StateReason naming the crossed threshold and the AlarmActions publish.

Traces: cloudwatch__opensearch__sns__sqs.obs-opensearch-2-searchable-documents-metric-crosses-threshold-and-alarm-notifies-sqs (error).

## Evidence
Failing check (suite service heading `### cloudwatch__opensearch__sns__sqs`): cloudwatch__opensearch__sns__sqs.obs-opensearch-2-searchable-documents-metric-crosses-threshold-and-alarm-notifies-sqs

TimeoutError raised from _wait_alarm_state at the OK wait: "alarm '<alarm name>' reaching 'OK': the run budget ended first; not graded" (line 319 of the check: ok = _wait_alarm_state(ctx, cw, alarm_name, "OK", timeout_s=600.0)); the alarm name is a per-run generated value.

Expected: DescribeAlarms reports StateValue "OK" for a PutMetricAlarm over Namespace AWS/ES, MetricName SearchableDocuments, Dimensions ClientId=<account>/DomainName=<domain>, Period 60, EvaluationPeriods 1, Statistic Maximum, Threshold 10.0, GreaterThanOrEqualToThreshold, TreatMissingData ignore, while the domain holds no documents; then, after 10 documents are indexed, exactly one StateUpdate OK -> ALARM with a StateReason containing "threshold (10.0)" and one CloudWatch notification delivered through the AlarmActions topic to the subscribed queue.
Actual: the alarm's StateValue never becomes OK within the polling window; everything downstream of that wait is unreachable. Preceding assertions in the same check passed: the alarm exists, its stored AlarmActions equal [the topic arn], and its initial state is not ALARM.

## Steps to Reproduce
1. sns:CreateTopic; sqs:CreateQueue with a policy allowing sns.amazonaws.com sqs:SendMessage from that topic; sns:Subscribe(Protocol=sqs).
2. opensearch:CreateDomain (single node t3.small.search, gp3 10 GiB, account-wide access policy); wait until it is searchable and its REST endpoint answers.
3. cloudwatch:PutMetricAlarm(Namespace="AWS/ES", MetricName="SearchableDocuments", Dimensions=[ClientId=<account>, DomainName=<domain>], Statistic="Maximum", Period=60, EvaluationPeriods=1, Threshold=10, ComparisonOperator="GreaterThanOrEqualToThreshold", TreatMissingData="ignore", AlarmActions=[<topic>]).
4. Poll cloudwatch:DescribeAlarms(AlarmNames=[<alarm>]) — EXPECT StateValue to settle at "OK"; OBSERVE it never reaching OK.
5. (Unreachable today) index 10 documents via the domain's _bulk, then expect one StateUpdate OK -> ALARM naming "threshold (10.0)" in DescribeAlarmHistory and exactly one CloudWatch notification in the subscribed queue.
Replay: cloudwatch__opensearch__sns__sqs.obs-opensearch-2-searchable-documents-metric-crosses-threshold-and-alarm-notifies-sqs.
