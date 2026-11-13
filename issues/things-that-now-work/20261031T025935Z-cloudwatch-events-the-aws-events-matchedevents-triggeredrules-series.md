Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule
Issue-Key: cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule|_poll_until_stable#25|Timeout|time.sleep(interval_s)
Issue-Commit: 26a6b9756b64727e017084c2f7bc8019f98b8844

# cloudwatch__events: the AWS/Events MatchedEvents/TriggeredRules series for a custom bus's rule is never readable, so the only routing-accounting check hangs to the 60 s cap

> _Note: this issue was identified against the code base at commit `26a6b9756b64727e017084c2f7bc8019f98b8844`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
1 net-new root cause in the EventBridge -> CloudWatch metric-accounting path.

Cause 1 of 1: reading the AWS/Events routing counters a custom bus's rule publishes.

AWS: EventBridge accounts every pattern match of a rule on the AWS/Events namespace under the combined {EventBusName, RuleName} dimension set: MatchedEvents and TriggeredRules, one observation of value 1 per match, Unit 'Count', at standard (60-second) resolution. A reader asks for them with CloudWatch GetMetricStatistics (Smithy: GetMetricStatistics input Namespace/MetricName/Dimensions/StartTime/EndTime/Period/Statistics all required or filtering, output Datapoints targets Datapoints; modeled errors InvalidParameterCombinationException, InvalidParameterValueException, MissingRequiredParameterException, InternalServiceFault) and discovers the series with ListMetrics (Smithy: ListMetrics input Dimensions targets DimensionFilters, output Metrics). Recorded behavior only for the dimension layout: the check records a measurement against real AWS (us-west-2) that on a CUSTOM bus these two counters exist ONLY under {EventBusName, RuleName} — {EventBusName} alone, {RuleName} alone and {EventSourceName} alone stay permanently absent — and that a rule whose pattern did NOT match the published event accounts for no datapoint at all on that series. Units on the returned datapoints are exactly {'Count'}, and ListMetrics filtered to the rule's name must report the ('EventBusName','RuleName') dimension set.

Emulator: the write side is present — PutEvents counts each match through crate::state::count_rule_matches, which appends a StoredDatum (sample_count/sum/min/max = 1.0, unit 'Count', standard resolution) to the shared CloudWatch store for both MATCHED_EVENTS_METRIC and TRIGGERED_RULES_METRIC under the rule's {EventBusName, RuleName} key. The READ side never surfaces it: the check's terminal leg reads AWS/Events MatchedEvents and TriggeredRules for {EventBusName=<the check's bus>, RuleName=<the matching rule>} with GetMetricStatistics and never obtains a complete pair of values, so the check makes no progress and is killed by the suite's per-check cap ('Error: Check timed out after 60.0s'). Hypothesis: this is the same unqueryable-metric-store read path already open as 20261029T203422Z-monitoring-cloudwatch-published-metric-data-is-unqueryable-and-metric-query-validation-diverges (Cause 1: GetMetricStatistics answers 200 with Datapoints: [] over a window into which data was just published) — that issue is grounded in the wire corpus on client-published custom-namespace data and names neither AWS/Events nor this check, so it does not cover this one; a fix there may well close this one too. Note also that this check's own measurement windows (120 s of stability on the positive series, a 180 s absence hold on the non-matching twin, a 600 s metric budget) exceed the suite's 60 s per-check cap, so the timeout alone does not localise WHERE it stalled; what is established from the code is that the series is written into the store and that the store's GetMetricStatistics read path is known not to return written data.

Why: a client cannot observe how many events EventBridge thought it routed. Every dashboard, capacity report and alarm built on AWS/Events MatchedEvents/TriggeredRules reads empty against the emulator while the same code reads real counts against AWS, so an over-counting router (one that fans every event out to every rule and credits TriggeredRules to rules that did not match) and a router with no accounting at all are indistinguishable — delivery-only assertions pass in both.

Fix: CloudWatch's metric read path — GetMetricStatistics' aggregation/lookup over the stored datapoints by (namespace, metric name, dimension set, period) and ListMetrics' enumeration of the dimension sets present for a metric — so that datapoints a service appended for its own AWS/ namespace are served on the same terms as PutMetricData-ingested ones, including the Unit they were stored with. The EventBridge write side (count_rule_matches / count_rule_metric) already produces the required series and dimension set; no change is implied there beyond keeping {EventBusName, RuleName} as the only published set for these two counters on a custom bus.

Traces: cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1), service group cloudwatch__events: 0/1 passed, 1 failed.

[mismatch] cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule
  Error: Check timed out after 60.0s

What the check does and what it expects:
1. CreateEventBus on a fresh custom bus; two rules on that one bus differing ONLY in the source they pin (one matching, one not), both targeting one SQS queue whose policy admits events.amazonaws.com.
2. PutEvents with a single entry naming the matching source. Asserts FailedEntryCount == 0 (this leg is reported satisfied in the run — the failure is not a refused publication).
3. Delivery leg, read at the CONSUMER: exactly one message on the target queue, whose envelope 'source' is the matching source and whose 'id' is the EventId PutEvents returned.
4. Terminal leg, read at CloudWatch: GetMetricStatistics(Namespace='AWS/Events', MetricName='MatchedEvents' then 'TriggeredRules', Dimensions=[{EventBusName: <bus>}, {RuleName: <matching rule>}], Period=60, Statistics=['Sum','SampleCount']) must settle on Sum == 1.0 for BOTH metrics; every returned datapoint's Unit must be exactly 'Count'; ListMetrics(Namespace='AWS/Events', Dimensions=[{Name:'RuleName', Value:<matching rule>}]) must report the ('EventBusName','RuleName') dimension set.
5. Negative leg: TriggeredRules for {EventBusName: <bus>, RuleName: <non-matching twin>} must have NO datapoints at all while the positive series is being served.

Expected: the two counters settle at Sum 1.0 on the matching rule's {EventBusName, RuleName} series with Unit 'Count', ListMetrics reports that two-dimension set, and the twin's series stays absent.
Actual: the check never reaches a complete reading and is killed at the 60.0 s cap; no AWS/Events value for the rule's series is ever obtained.

Corroborating code in the checkout (citations, not requirements):
- services/service-com-amazonaws-eventbridge-awsevents/src/ops/put_events.rs:399-408 — PutEvents takes the shared CloudWatch store and calls crate::state::count_rule_matches(cloudwatch, &deliveries) for every match.
- services/service-com-amazonaws-eventbridge-awsevents/src/state.rs:3547-3555 — count_rule_match writes MATCHED_EVENTS_METRIC and TRIGGERED_RULES_METRIC.
- services/service-com-amazonaws-eventbridge-awsevents/src/state.rs:3660-3698 — count_rule_metric appends a StoredDatum (sum 1.0, unit 'Count', standard resolution) under MetricKey{namespace: AWS/Events, metric_name, dimensions from rule_dimension_sets(bus, rule)} via cloudwatch.append_datapoints, i.e. into the very store ListMetrics and GetMetricStatistics read.
- Open, related, NOT covering this check: 20261029T203422Z-monitoring-cloudwatch-published-metric-data-is-unqueryable-and-metric-query-validation-diverges — Cause 1 records GetMetricStatistics answering 200 with Datapoints: [] over a window into which data was just published, and Cause 2 the same for GetMetricData. Grounded in the wire corpus on client-published custom namespaces; it names neither AWS/Events nor this behavioral check.

Values that are per-run and NOT requirements: the generated bus name, the two generated rule names, the queue URL/ARN, the account id, the EventId, and the datapoint timestamps. The requirements are the namespace AWS/Events, the metric names MatchedEvents and TriggeredRules, the {EventBusName, RuleName} dimension set, Sum == 1.0 per single matched event, Unit 'Count', and the absence of any datapoint on the non-matching rule's series.

## Steps to Reproduce
1. Boot the emulator from this branch (CloudWatch and EventBridge both answer; the events group passes 139/191, so routing itself is live).
2. events.create_event_bus(Name='probe-bus').
3. Create an SQS queue and set its policy to allow sqs:SendMessage for Principal Service events.amazonaws.com with Condition aws:SourceAccount = the caller's account.
4. On 'probe-bus' create two enabled rules with the SAME detail-type and DIFFERENT sources — rule 'hit' pinning source 'probe.match', rule 'miss' pinning source 'probe.nomatch' — and PutTargets the queue ARN on each.
5. events.put_events with one entry: EventBusName='probe-bus', Source='probe.match', DetailType=<the shared detail-type>, Detail='{}'. Confirm FailedEntryCount == 0 and note the returned EventId.
6. ReceiveMessage on the queue until one message arrives; confirm its envelope 'source' and 'id'.
7. cloudwatch.get_metric_statistics(Namespace='AWS/Events', MetricName='MatchedEvents', Dimensions=[{'Name':'EventBusName','Value':'probe-bus'},{'Name':'RuleName','Value':'hit'}], StartTime=now-20min, EndTime=now+5min, Period=60, Statistics=['Sum','SampleCount']) and the same for MetricName='TriggeredRules'.
   Expected: one or more Datapoints whose Sum totals 1.0, each carrying Unit 'Count'. Observed: no value is ever returned for the series.
8. cloudwatch.list_metrics(Namespace='AWS/Events', Dimensions=[{'Name':'RuleName','Value':'hit'}]).
   Expected: a metric whose dimension names are exactly ('EventBusName','RuleName').
9. Repeat step 7 for RuleName='miss'.
   Expected: no Datapoints at all, held while the 'hit' series is being served.
10. Replay the single check directly to see the timeout: behavioral check id cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule.
