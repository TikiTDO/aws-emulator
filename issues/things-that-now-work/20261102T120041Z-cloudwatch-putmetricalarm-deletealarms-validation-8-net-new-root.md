Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-evaluate-low-sample-count-percentile-unknown-value-rejected cloudwatch.alarm-period-not-a-multiple-of-sixty-rejected cloudwatch.put-metric-alarm-with-a-45-second-period-rejected cloudwatch.alarm-with-unknown-comparison-operator-rejected cloudwatch.albos-put-metric-alarm-with-an-unknown-missing-data-policy-rejected cloudwatch.cw-delete-alarms-two-composites-rejected-and-nothing-deleted cloudwatch.metric-math-alarm-with-datapoints-to-alarm-above-evaluation-periods-rejected cloudwatch.put-metric-alarm-with-statistic-and-extended-statistic-rejected cloudwatch.put-metric-alarm-with-unsupported-percentile-rejected
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-evaluate-low-sample-count-percentile-unknown-value-rejected|_error#9|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudwatch.alarm-period-not-a-multiple-of-sixty-rejected|run#8|AssertionError|assert message == "Period must be 10, 20, 30 or a multiple of 60", message
Issue-Key: cloudwatch.put-metric-alarm-with-a-45-second-period-rejected|run#8|AssertionError|assert message == "Period must be 10, 20, 30 or a multiple of 60", (
Issue-Key: cloudwatch.alarm-with-unknown-comparison-operator-rejected|run#7|AssertionError|assert got == ("ValidationError", (
Issue-Key: cloudwatch.albos-put-metric-alarm-with-an-unknown-missing-data-policy-rejected|_error#6|AssertionError|raise AssertionError(f"expected a rejection, but it succeeded: {result!r}")
Issue-Key: cloudwatch.cw-delete-alarms-two-composites-rejected-and-nothing-deleted|run#19|AssertionError|assert message == (
Issue-Key: cloudwatch.metric-math-alarm-with-datapoints-to-alarm-above-evaluation-periods-rejected|_error#7|AssertionError|raise AssertionError(f"expected a rejection, but it succeeded: {result!r}")
Issue-Key: cloudwatch.put-metric-alarm-with-statistic-and-extended-statistic-rejected|_error#9|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it: {result!r}")
Issue-Key: cloudwatch.put-metric-alarm-with-unsupported-percentile-rejected|_error#7|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: b8358f56f4f44e240ca755ea7919bfa62f43cb3d

# cloudwatch: PutMetricAlarm / DeleteAlarms validation — 8 net-new root causes (unknown enum values accepted, M-of-N unenforced on metric-math alarms, refusal texts emulator-authored)

> _Note: this issue was identified against the code base at commit `b8358f56f4f44e240ca755ea7919bfa62f43cb3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Eight independent defects in the CloudWatch alarm write path. Each is a separate root cause and must be closed separately. Related open issues that do NOT carry these replay keys (so they do not cover these causes) and should be merged with by the fixer where they overlap: 20261029T182452Z-cloudwatch-request-validation-and-error-envelope-divergences-10.md, 20261029T203422Z-monitoring-cloudwatch-alarm-writes-and-pagination-skip-validation-or.md, 20261102T112036Z-monitoring-cloudwatch-validationexception-messages-drop-the-offending-value-and.md, 20261028T043126Z-cloudwatch-enum-constraint-validationerror-messages-are-rendered-from.md.

Cause 1 of 8: PutMetricAlarm accepts an unknown EvaluateLowSampleCountPercentile.
AWS: Smithy: PutMetricAlarm.EvaluateLowSampleCountPercentile targets EvaluateLowSampleCountPercentile (string, @length 1..255) — the model cannot express the value set, so recorded behavior only: AWS refuses any value other than `evaluate` or `ignore` with HTTP 400, error code `ValidationError`, message `Option <value> is not supported. Supported options for parameter EvaluateLowSampleCountPercentile are evaluate and ignore.` The fixed text is everything but `<value>`, which is the rejected request value. The refused update must leave the stored alarm (including AlarmConfigurationUpdatedTimestamp) untouched.
Emulator: the call succeeds (HTTP 200, empty body) and the policy is written, so there is no error code at all.
Why: a client that relies on the enum being enforced silently stores an alarm whose low-sample-count policy CloudWatch would never have accepted; a misspelled policy becomes a live alarm instead of a refusal.
Fix: PutMetricAlarm's validation (the pre-write parameter checks in the operation's handler).
Traces: cloudwatch.alarm-evaluate-low-sample-count-percentile-unknown-value-rejected.

Cause 2 of 8: the Period refusal message carries an emulator-authored suffix.
AWS: Smithy: PutMetricAlarm.Period targets Period (integer, @range min 1); the 10/20/30/multiple-of-60 grammar is a service rule the model cannot express. Recorded behavior: a Period outside that grammar is refused HTTP 400 `ValidationError` with the message exactly `Period must be 10, 20, 30 or a multiple of 60`. The whole message is fixed text — it names the grammar and carries no request value.
Emulator: the code is right but the message is `Period must be 10, 20, 30 or a multiple of 60; got 45`, i.e. AWS's text plus a `; got <period>` suffix built from the request value.
Why: callers that match the refusal text (CDK/CloudFormation error surfacing, retry classifiers, user-facing diagnostics) see a message AWS never emits; the appended request value makes the text per-request instead of constant.
Fix: the Period rejection message in PutMetricAlarm's validation.
Traces: cloudwatch.alarm-period-not-a-multiple-of-sixty-rejected, cloudwatch.put-metric-alarm-with-a-45-second-period-rejected.

Cause 3 of 8: the ComparisonOperator enum ValidationError drops the offending value and re-sorts the enum set.
AWS: Smithy: PutMetricAlarm.ComparisonOperator targets the ComparisonOperator enum. Recorded behavior: an off-enum value is refused HTTP 400 `ValidationError` with `1 validation error detected: Value '<value>' at 'comparisonOperator' failed to satisfy constraint: Member must satisfy enum value set: [GreaterThanUpperThreshold, LessThanLowerThreshold, GreaterThanThreshold, LessThanThreshold, LessThanOrEqualToThreshold, LessThanLowerOrGreaterThanUpperThreshold, GreaterThanOrEqualToThreshold]`. Fixed text: the `1 validation error detected: Value '…' at 'comparisonOperator' failed to satisfy constraint: Member must satisfy enum value set: […]` frame AND the member order inside the brackets; the only variable part is `<value>`, taken from the request.
Emulator: `1 validation error detected: Value at 'comparisonOperator' failed to satisfy constraint: Member must satisfy enum value set: [GreaterThanOrEqualToThreshold, GreaterThanThreshold, GreaterThanUpperThreshold, LessThanLowerOrGreaterThanUpperThreshold, LessThanLowerThreshold, LessThanOrEqualToThreshold, LessThanThreshold]` — the `'<value>' ` segment is omitted entirely and the set is alphabetised rather than emitted in AWS's order.
Why: a client cannot tell from the message which value it sent was rejected, and any test or parser pinned to AWS's wording/ordering fails against the emulator.
Fix: the shared enum-constraint ValidationError renderer used by PutMetricAlarm's validation.
Traces: cloudwatch.alarm-with-unknown-comparison-operator-rejected.

Cause 4 of 8: PutMetricAlarm accepts an unknown TreatMissingData policy.
AWS: Smithy: PutMetricAlarm.TreatMissingData targets TreatMissingData (string, @length 1..255) — the value set is a service rule. Recorded behavior: an unsupported value is refused HTTP 400 `ValidationError` with `The value <value> is not supported for TreatMissingData parameter. Supported values are [breaching, notBreaching, ignore, missing].` (fixed text apart from `<value>`), nothing is created and no alarm-history item is written.
Emulator: `TreatMissingData="breach"` returns HTTP 200 with an empty body and the alarm is created.
Why: the near-miss of a documented value (`breach` vs `breaching`) silently produces an alarm with a missing-data policy the caller never asked for, changing when it fires.
Fix: PutMetricAlarm's validation.
Traces: cloudwatch.albos-put-metric-alarm-with-an-unknown-missing-data-policy-rejected.

Cause 5 of 8: the DeleteAlarms two-composite refusal is emulator-authored.
AWS: Smithy: DeleteAlarms.AlarmNames (@length 0..100, required); the one-composite-per-call rule is a service rule. Recorded behavior: a DeleteAlarms batch naming more than one composite alarm is refused HTTP 400 `ValidationError` with `Cannot delete [<n>] Composite Alarms as only one composite alarm delete is allowed in single request` — fixed text apart from `<n>`, the count of composite alarms in the request — and nothing in the batch is deleted.
Emulator: the code is `ValidationError` but the message is `only one composite alarm delete is allowed per API call`, which names neither the count nor the constraint in AWS's words.
Why: the refusal text is part of the contract; a caller that surfaces or matches it gets prose AWS never emits, and the count AWS reports (which tells the caller how far over the limit it is) is absent.
Fix: the composite-count rejection message in DeleteAlarms' handler.
Traces: cloudwatch.cw-delete-alarms-two-composites-rejected-and-nothing-deleted.

Cause 6 of 8: the M-of-N relation is not enforced on a metric-math (Metrics=) alarm.
AWS: Smithy: PutMetricAlarm.DatapointsToAlarm and .EvaluationPeriods are both integers with @range min 1; the relation DatapointsToAlarm <= EvaluationPeriods is a service rule. Recorded behavior: violating it is refused HTTP 400 `ValidationError` with the exact message `DatapointsToAlarm must be less than or equal to EvaluationPeriods` (wholly fixed text) and nothing is created — identically whether the alarm is expressed with Namespace/MetricName/Statistic or with a `Metrics` query array.
Emulator: with `Metrics=[...]` (a MetricStat plus a returning Expression) and DatapointsToAlarm=3, EvaluationPeriods=2, the call returns HTTP 200 and the alarm is created. The relation is evidently checked only on the single-metric form.
Why: a metric-math alarm can be created in a state CloudWatch rejects, so it evaluates on a window the caller never configured and the client's validation assumptions silently differ between the two equivalent alarm forms.
Fix: PutMetricAlarm's validation — apply the M-of-N check on the `Metrics` path as well as the single-metric path.
Traces: cloudwatch.metric-math-alarm-with-datapoints-to-alarm-above-evaluation-periods-rejected.

Cause 7 of 8: PutMetricAlarm accepts both Statistic and ExtendedStatistic in one request.
AWS: Smithy: PutMetricAlarm.Statistic (Statistic enum) and .ExtendedStatistic (string) are mutually exclusive — a cross-member rule the model cannot express. Recorded behavior: naming both is refused HTTP 400 `ValidationError` with `PutMetricAlarm request can not be made with both Statistic and ExtendedStatistic parameters. Only one of them should be present` (wholly fixed text), and when the request is an update the stored alarm keeps its prior Statistic and gains no ExtendedStatistic.
Emulator: the request returns HTTP 200 with an empty body, so no refusal exists and the update is applied.
Why: an alarm ends up carrying a statistic pair CloudWatch never stores, and clients relying on the refusal to catch a bad update silently mutate a live alarm.
Fix: PutMetricAlarm's validation (mutual-exclusion check, before the write).
Traces: cloudwatch.put-metric-alarm-with-statistic-and-extended-statistic-rejected.

Cause 8 of 8: PutMetricAlarm accepts an out-of-range ExtendedStatistic percentile.
AWS: Smithy: PutMetricAlarm.ExtendedStatistic targets ExtendedStatistic (string) — the percentile grammar is a service rule. Recorded behavior: a percentile above 100 is refused HTTP 400 `InvalidParameterValue` with `The value <value> for parameter ExtendedStatistic is not supported.` (fixed text apart from `<value>`, the rejected statistic) and nothing is created, while an in-range fractional percentile such as `p99.9` is accepted and echoed verbatim.
Emulator: `ExtendedStatistic="p101"` returns HTTP 200 and the alarm is created, so the grammar is never parsed.
Why: a typo'd percentile yields a live alarm whose statistic CloudWatch cannot compute, instead of the InvalidParameterValue the caller is written against.
Fix: PutMetricAlarm's validation (percentile grammar parse).
Traces: cloudwatch.put-metric-alarm-with-unsupported-percentile-rejected.

## Evidence
Replayed against the emulator (all under suite service heading `### cloudwatch`):

1. alarm-evaluate-low-sample-count-percentile-unknown-value-rejected — expected ('ValidationError', 'Option sometimes is not supported. Supported options for parameter EvaluateLowSampleCountPercentile are evaluate and ignore.'); actual: HTTP 200, no error ('the call succeeded; real AWS rejects it, so there is no error code').
2. alarm-period-not-a-multiple-of-sixty-rejected / put-metric-alarm-with-a-45-second-period-rejected — expected message 'Period must be 10, 20, 30 or a multiple of 60'; actual 'Period must be 10, 20, 30 or a multiple of 60; got 45'.
3. alarm-with-unknown-comparison-operator-rejected — expected "1 validation error detected: Value 'GreaterThan' at 'comparisonOperator' failed to satisfy constraint: Member must satisfy enum value set: [GreaterThanUpperThreshold, LessThanLowerThreshold, GreaterThanThreshold, LessThanThreshold, LessThanOrEqualToThreshold, LessThanLowerOrGreaterThanUpperThreshold, GreaterThanOrEqualToThreshold]"; actual "1 validation error detected: Value at 'comparisonOperator' failed to satisfy constraint: Member must satisfy enum value set: [GreaterThanOrEqualToThreshold, GreaterThanThreshold, GreaterThanUpperThreshold, LessThanLowerOrGreaterThanUpperThreshold, LessThanLowerThreshold, LessThanOrEqualToThreshold, LessThanThreshold]".
4. albos-put-metric-alarm-with-an-unknown-missing-data-policy-rejected — expected ('ValidationError', 'The value breach is not supported for TreatMissingData parameter. Supported values are [breaching, notBreaching, ignore, missing].'); actual {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', 'content-length': '2', ...}}}.
5. cw-delete-alarms-two-composites-rejected-and-nothing-deleted — expected message 'Cannot delete [2] Composite Alarms as only one composite alarm delete is allowed in single request'; actual 'only one composite alarm delete is allowed per API call'.
6. metric-math-alarm-with-datapoints-to-alarm-above-evaluation-periods-rejected — expected ('ValidationError', 'DatapointsToAlarm must be less than or equal to EvaluationPeriods'); actual HTTP 200, content-length 2 ('expected a rejection, but it succeeded').
7. put-metric-alarm-with-statistic-and-extended-statistic-rejected — expected ('ValidationError', 'PutMetricAlarm request can not be made with both Statistic and ExtendedStatistic parameters. Only one of them should be present'); actual HTTP 200, content-length 2.
8. put-metric-alarm-with-unsupported-percentile-rejected — expected ('InvalidParameterValue', 'The value p101 for parameter ExtendedStatistic is not supported.'); actual the call succeeded with no error.

## Steps to Reproduce
1. Cause 1: cw.put_metric_alarm(AlarmName=N, Namespace=NS, MetricName='QueryDuration', ExtendedStatistic='p90', Period=300, EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=5000.0, ComparisonOperator='GreaterThanThreshold', EvaluateLowSampleCountPercentile='sometimes') — expect 400 ValidationError, observe 200.
2. Cause 2: cw.put_metric_alarm(..., Period=45) — observe the '; got 45' suffix on the ValidationError message.
3. Cause 3: cw.put_metric_alarm(..., ComparisonOperator='GreaterThan') — compare the ValidationError message with AWS's.
4. Cause 4: cw.put_metric_alarm(..., TreatMissingData='breach') — expect 400 ValidationError, observe 200 and then describe_alarms showing the alarm.
5. Cause 5: create two composite alarms plus one metric alarm, then cw.delete_alarms(AlarmNames=[c1, c2, metric]) — compare the ValidationError message.
6. Cause 6: cw.put_metric_alarm(AlarmName=N, Metrics=[{'Id':'m','ReturnData':False,'MetricStat':{...}},{'Id':'e1','ReturnData':True,'Expression':'m'}], DatapointsToAlarm=3, EvaluationPeriods=2, Threshold=0.0, ComparisonOperator='GreaterThanThreshold') — expect 400 ValidationError, observe 200.
7. Cause 7: create an alarm with Statistic='Average', then re-put the same alarm with Statistic='Average' AND ExtendedStatistic='p90' — expect 400 ValidationError, observe 200.
8. Cause 8: cw.put_metric_alarm(..., ExtendedStatistic='p101') — expect 400 InvalidParameterValue, observe 200.
In every case also re-read with describe_alarms to confirm the refused request stored nothing.
