Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns__sqs.actions-suppressor-phases-on-high-resolution-alarms-withhold-then-release-the-composite-notification
Issue-Key: cloudwatch__sns__sqs.actions-suppressor-phases-on-high-resolution-alarms-withhold-then-release-the-composite-notification|_poll_until#47|AssertionError|raise AssertionError(
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# cloudwatch__sns__sqs: a composite alarm with an ActionsSuppressor never reports ActionsSuppressedBy, so its actions are never withheld

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutCompositeAlarm

## Description
Cause 1 of 1: a composite alarm configured with an ActionsSuppressor accepts and reads back the suppressor configuration, but its evaluation never produces a suppression state: DescribeAlarms carries no ActionsSuppressedBy (nor ActionsSuppressedReason) field when the alarm goes into ALARM, so none of the suppression phases exist. Failing check: cloudwatch__sns__sqs.actions-suppressor-phases-on-high-resolution-alarms-withhold-then-release-the-composite-notification.

AWS: Smithy (cloudwatch 2010-08-01) PutCompositeAlarm declares ActionsSuppressor, ActionsSuppressorWaitPeriod and ActionsSuppressorExtensionPeriod, and CompositeAlarm declares ActionsSuppressedBy (enum ActionsSuppressedBy: WaitPeriod | ExtensionPeriod | Alarm) and ActionsSuppressedReason. When such a composite transitions into ALARM while its suppressor alarm is NOT in ALARM, CloudWatch withholds the actions and reports ActionsSuppressedBy = 'WaitPeriod' for up to ActionsSuppressorWaitPeriod seconds, waiting for the suppressor to confirm a maintenance window; if the suppressor then reaches ALARM the value becomes 'Alarm'; when the suppressor leaves ALARM it becomes 'ExtensionPeriod' for ActionsSuppressorExtensionPeriod seconds; once nothing suppresses it the field is absent/empty and the action finally fires. While suppressed, the alarm's DescribeAlarmHistory Action items are recorded with actionState 'Suppressed' and no notification is published, while an otherwise identical composite with no suppressor delivers.

Emulator: PutCompositeAlarm stores the suppressor correctly (ActionsSuppressor, ActionsSuppressorWaitPeriod and ActionsSuppressorExtensionPeriod all read back as written, and the unsuppressed twin reports none), but when the child alarm transitions and the rule evaluates TRUE the composite goes straight to StateValue 'ALARM' with StateReason "<child alarm arn> transitioned to ALARM at <ts> and the composite alarm's rule evaluated to TRUE, so the alarm is in ALARM state" and the DescribeAlarms entry contains NO ActionsSuppressedBy key at all — the suppressor is in OK, so AWS would be in the WaitPeriod phase. Since the field is never set, the 'Alarm' and 'ExtensionPeriod' phases and the release-after-extension behaviour are unreachable too.

Why: alarm suppression is the mechanism customers use to mute alerting during a known maintenance window or a parent outage. With ActionsSuppressedBy never populated, a client cannot tell a suppressed alarm from a firing one, every suppressed composite notifies anyway (its actions are not withheld), and tooling that reads ActionsSuppressedBy/ActionsSuppressedReason to explain 'why did I not get paged' gets nothing.

Fix: the CloudWatch composite-alarm evaluator and its action dispatcher — when a composite with an ActionsSuppressor enters ALARM, compute the suppression phase from the suppressor alarm's state and the wait/extension clocks, expose it as ActionsSuppressedBy (+ActionsSuppressedReason) on the DescribeAlarms CompositeAlarm, withhold the configured actions while it is set, and record the alarm-history Action items as actionState 'Suppressed' until the suppression ends.

Note: open issue 20261109T183935Z-cloudwatch-6-net-new-causes-in-alarm-evaluation.md reports that mute rules never suppress actions and 20261109T183935Z-cloudwatch-sns-sqs-3-net-new-causes-the.md reports a null EvaluationState; neither carries this check's replay key, and they are named here in case one fix in the evaluator covers them.

Traces: the failing check id above.

## Evidence
Expected (AWS): while the suppressor metric alarm is in OK and the child is in ALARM, DescribeAlarms(AlarmTypes=['CompositeAlarm']) for the suppressed composite reports StateValue 'ALARM' together with ActionsSuppressedBy 'WaitPeriod', its Action history items are actionState 'Suppressed', and no SNS notification for that alarm reaches the subscribed queue while the unsuppressed twin's notification does.
Actual (emulator): `AssertionError: the suppressed composite '<name>' entered ALARM reporting ActionsSuppressedBy == 'WaitPeriod'. ... did not happen within 90.0s; last observation {'ActionsEnabled': True, 'AlarmActions': ['arn:aws:sns:...'], 'AlarmArn': 'arn:aws:cloudwatch:...:alarm:<name>', 'AlarmRule': 'ALARM("<child>")', 'StateValue': 'ALARM', 'StateReason': "<child arn> transitioned to ALARM at <ts> and the composite alarm's rule evaluated to TRUE, so the alarm is in ALARM state", ...}` — the observation has no ActionsSuppressedBy key. Alarm names, ARNs and timestamps are per-run; the missing ActionsSuppressedBy is the divergence. The earlier assertions in the same check pass, so PutCompositeAlarm's storage of ActionsSuppressor/WaitPeriod/ExtensionPeriod is correct.

## Steps to Reproduce
1. PutMetricAlarm twice over high-resolution (10 s period, 3 evaluation periods) custom metrics: a 'suppressor' alarm and a 'child' alarm, and drive both to OK with real PutMetricData datapoints.
2. PutCompositeAlarm 'suppressed' with AlarmRule='ALARM("<child>")', AlarmActions=[an SNS topic subscribed by an SQS queue], ActionsSuppressor='<suppressor>', ActionsSuppressorWaitPeriod=120, ActionsSuppressorExtensionPeriod=60; PutCompositeAlarm an identical 'control' with no suppressor.
3. Publish breaching datapoints for the child until it reaches ALARM (not SetAlarmState).
4. DescribeAlarms(AlarmNames=['suppressed'], AlarmTypes=['CompositeAlarm']): the composite is in ALARM but reports no ActionsSuppressedBy, where AWS reports 'WaitPeriod' because the suppressor is still in OK.
