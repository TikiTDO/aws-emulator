Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__sqs.xsvc-cloudwatch-1-alarm-state-change-chain-delivered-to-sqs
Issue-Key: cloudwatch__events__sqs.xsvc-cloudwatch-1-alarm-state-change-chain-delivered-to-sqs|run#43|AssertionError|assert ok_e["detail"]["previousState"] == alarm_e["detail"]["state"], (
Issue-Commit: f4d39a015d946adb275431796e6cdc3cfcfe16a7

# cloudwatch__events__sqs: a CloudWatch Alarm State Change event's detail.previousState is re-evaluated at emit time instead of being the prior state verbatim, so the transition chain never links

> _Note: this issue was identified against the code base at commit `f4d39a015d946adb275431796e6cdc3cfcfe16a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
One net-new root cause.

Cause 1 of 1: the `detail.previousState` block of a `CloudWatch Alarm State Change` event is regenerated from a fresh metric evaluation at the moment the NEW transition is emitted, rather than being the snapshot of the state the alarm actually left. Failing check ids: cloudwatch__events__sqs.xsvc-cloudwatch-1-alarm-state-change-chain-delivered-to-sqs.

AWS: a metric alarm stores exactly ONE explanation per state. Smithy (com.amazonaws.cloudwatch, MetricAlarm): `StateValue` "The state value for the alarm", `StateReason` "An explanation for the alarm state, in text format" (@length max 1023), `StateReasonData` "An explanation for the alarm state, in JSON format" (@length max 4000), `StateUpdatedTimestamp` and `StateTransitionedTimestamp` "The date and time that the alarm's StateValue most recently changed". Each evaluator-driven transition publishes that stored snapshot as the event's `detail.state` = {value, reason, reasonData, timestamp}, and publishes the snapshot of the state being LEFT as `detail.previousState`. Because both come from the same single stored tuple, consecutive events chain exactly: for transitions n-1 and n of the same alarm, `event[n].detail.previousState` equals `event[n-1].detail.state` member-for-member, including the `reason` text, the `reasonData` JSON (version/queryDate/startDate/statistic/period/threshold/recentDatapoints/evaluatedDatapoints) and the `timestamp`. The one exception is the alarm's creation state, whose previousState carries a `reason` but no `reasonData`, because nothing was evaluated at creation. Recorded behavior only for the chaining itself (it is an EventBridge event payload, not a modelled shape); the recording shows the chain linking.

Fixed vs variable parts of the payload: the fixed text of a breaching `reason` is `Threshold Crossed: <n> out of the last <m> datapoints [<values with bucket times>] was greater than the threshold (<threshold>) (minimum <k> datapoint for <from> -> <to> transition).`; the counts, datapoint values, bucket timestamps, threshold and the from/to state names are values taken from the request (PutMetricAlarm's Threshold/EvaluationPeriods/DatapointsToAlarm/ComparisonOperator) and from the published metric data. In `reasonData`, `statistic`, `period` and `threshold` come from the alarm configuration; `startDate`, `recentDatapoints` and `evaluatedDatapoints` come from the bucket that was evaluated for THAT state; `queryDate` is the time of THAT evaluation. None of these are per-run constants — the requirement is that whatever tuple was published as `state` for a transition is republished byte-identically as `previousState` on the next one.

Emulator: `previousState.value` and `previousState.timestamp` are carried forward correctly, but `previousState.reason` and `previousState.reasonData` are recomputed when the later transition fires, so they describe a window that only exists AFTER the state they claim to explain had already ended. In the replay the ALARM event's own `state` reported a reason naming the datapoint bucket it evaluated, with `reasonData.startDate` = that bucket, `reasonData.queryDate` = the ALARM transition time and `evaluatedDatapoints[0].sampleCount` = 5.0; the following OK event's `previousState` for that same ALARM state reported the NEXT bucket (one `period` later) in both the reason text and `startDate`, a `queryDate` ~19 s later (the OK evaluation time), and `sampleCount` = 1.0. The `timestamp` of the two agreed. The equality assertion therefore fails: `AssertionError: the OK event's previousState {...} is not the ALARM event's state {...}`.

Why: a client reconstructing an alarm's transition history joins `event[n].detail.previousState` to `event[n-1].detail.state` — that is how a consumer detects a lost or duplicated transition and how incident tooling renders a state timeline with the evidence that caused each state. Against the emulator every link in that chain is broken, so such a consumer concludes transitions were dropped or reordered. Worse, the `reasonData` it reads for the prior state names datapoints and an evaluation window from AFTER that state ended, so any dashboard, runbook or post-incident report that quotes "the data that put this alarm into ALARM" quotes the wrong datapoints, the wrong sample count and the wrong query window.

Fix: belongs in the CloudWatch alarm evaluator's state store and the `CloudWatch Alarm State Change` event emitter. The evaluator must persist the (value, reason, reasonData, timestamp) tuple at the instant it sets a state and, on the next transition, emit that persisted tuple verbatim as `detail.previousState` (and, for the first transition after creation, emit the creation state's reason with `reasonData` absent). It must not call the evaluator again to synthesise the previous state at emit time. The same stored tuple must remain what DescribeAlarms serves as StateReason/StateReasonData for the current state.

Traces: cloudwatch__events__sqs.xsvc-cloudwatch-1-alarm-state-change-chain-delivered-to-sqs (assertion at check line 362).

## Evidence
Check cloudwatch__events__sqs.xsvc-cloudwatch-1-alarm-state-change-chain-delivered-to-sqs drives a high-resolution metric alarm INSUFFICIENT_DATA -> ALARM -> OK -> INSUFFICIENT_DATA with real PutMetricData values and collects the three delivered events from SQS.

Expected (AWS): ok_event['detail']['previousState'] == alarm_event['detail']['state'] — the same value, reason, reasonData and timestamp.

Actual (emulator):
  AssertionError: the OK event's previousState {'value': 'ALARM', 'reason': 'Threshold Crossed: 1 out of the last 1 datapoints [90.0 (13/11/26 06:02:30)] was greater than the threshold (50.0) (minimum 1 datapoint for OK -> ALARM transition).', 'reasonData': '{"version":"1.0","queryDate":"2026-11-13T06:02:49.686+0000","startDate":"2026-11-13T06:02:30.000+0000","statistic":"Maximum","period":10.0,"threshold":50.0,"recentDatapoints":[90.0],"evaluatedDatapoints":[{"timestamp":"2026-11-13T06:02:30.000+0000","sampleCount":1.0,"value":90.0}]}', 'timestamp': '2026-11-13T06:02:30.144+0000'} is not the ALARM event's state {'value': 'ALARM', 'reason': 'Threshold Crossed: 1 out of the last 1 datapoints [90.0 (13/11/26 06:02:20)] was greater than the threshold (50.0) (minimum 1 datapoint for OK -> ALARM transition).', 'reasonData': '{"version":"1.0","queryDate":"2026-11-13T06:02:30.144+0000","startDate":"2026-11-13T06:02:20.000+0000","statistic":"Maximum","period":10.0,"threshold":50.0,"recentDatapoints":[90.0],"evaluatedDatapoints":[{"timestamp":"2026-11-13T06:02:20.000+0000","sampleCount":5.0,"value":90.0}]}', 'timestamp': '2026-11-13T06:02:30.144+0000'}

Diverging members (generalised, the concrete times/counts above are this run's values and are not the requirement):
  - previousState.reason: names the bucket one `period` AFTER the bucket the ALARM state's own reason named.
  - previousState.reasonData.startDate: the later bucket, not the bucket that was evaluated for the ALARM state.
  - previousState.reasonData.queryDate: the time of the LATER (OK) evaluation, not the time of the ALARM evaluation.
  - previousState.reasonData.evaluatedDatapoints[0].timestamp / .sampleCount: the later bucket and a different sample count.
  - previousState.value and previousState.timestamp DO match, which is what shows the block is partly carried forward and partly recomputed.

The matching fixed text is present and correct in both; only the variable parts (which bucket, which query time, which sample count) are taken from the wrong evaluation.

## Steps to Reproduce
1. Create a standard SQS queue and give events.amazonaws.com sqs:SendMessage on it, scoped to the rule ARN below.
2. PutRule on the default bus with EventPattern {"source":["aws.cloudwatch"],"detail-type":["CloudWatch Alarm State Change"]}, State ENABLED, and PutTargets the queue.
3. PutMetricAlarm: Namespace=<unique>, MetricName=Load, Statistic=Maximum, Period=10, EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=50, ComparisonOperator=GreaterThanThreshold, TreatMissingData=missing, ActionsEnabled=false.
4. PutMetricData value 90 (StorageResolution=1) into successive 10 s buckets until DescribeAlarms reports StateValue=ALARM.
5. Stop publishing 90, then PutMetricData value 10 into later buckets until DescribeAlarms reports StateValue=OK.
6. Stop publishing entirely and wait until DescribeAlarms reports StateValue=INSUFFICIENT_DATA.
7. Drain the queue and de-duplicate the three events for the alarm ARN by event id, ordered by detail.state.timestamp.
8. Compare the OK event's detail.previousState with the ALARM event's detail.state (and the INSUFFICIENT_DATA event's previousState with the OK event's state). AWS: equal member-for-member. Emulator: value and timestamp match but reason / reasonData.startDate / reasonData.queryDate / reasonData.evaluatedDatapoints describe a later evaluation window, so the comparison fails.
