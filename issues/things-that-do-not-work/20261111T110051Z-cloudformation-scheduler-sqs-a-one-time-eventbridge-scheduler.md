Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__scheduler__sqs.cfn-scheduler-schedule-in-place-at-update-delivers-retained-replacement-then-not-found
Issue-Key: cloudformation__scheduler__sqs.cfn-scheduler-schedule-in-place-at-update-delivers-retained-replacement-then-not-found|run#34|AssertionError|assert sent_ms >= (int(fire_at) - _SKEW_S) * 1000, (
Issue-Commit: 56af4a38801370870cefc18c3ef22e7084182017

# cloudformation__scheduler__sqs: a one-time EventBridge Scheduler at() schedule delivers to its target immediately instead of at the nominal instant

> _Note: this issue was identified against the code base at commit `56af4a38801370870cefc18c3ef22e7084182017`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateSchedule

## Description
One net-new root cause, grounded in a failing behavioral check the replay report left unmarked (no ALREADY-FILED marker, no OWNED marker).

Cause 1 of 1 — a schedule whose ScheduleExpression is `at(<yyyy-MM-ddTHH:mm:ss>)` is invoked as soon as it is created/updated rather than at the instant it names.

AWS: EventBridge Scheduler's one-time schedule expression `at(yyyy-MM-ddTHH:mm:ss)` names a single nominal invocation instant in the schedule's ScheduleExpressionTimezone (UTC by default). Scheduler invokes the target once, AT that instant (within its documented delivery window, and FlexibleTimeWindow Mode OFF means no deliberate jitter); it does not invoke before it. A target invocation is therefore observable no earlier than the nominal instant, so the SQS message the target produces carries a SentTimestamp >= that instant. The scheduler Smithy model constrains the expression string only (ScheduleExpression is a string member of CreateSchedule/UpdateSchedule); the firing-time rule is documented behaviour, and the recording encodes it as "SentTimestamp must not precede the at() instant".

Emulator: immediately after the stack update that sets the new `at(...)` expression, the target queue already holds the schedule's message. The delivered body is correct (the updated Input), but SentTimestamp is ahead of the nominal instant by the whole lead the check reserved — i.e. the schedule fired at update time, not at its at() time. In the replayed run the expression was `at(2026-11-11T10:24:30)` and the message's SentTimestamp was 1794392552594 ms, well before it (those two values are this run's clock; the requirement is the ordering, not the numbers).

Why: a one-time schedule is the mechanism clients use to defer work — a delayed retry, a trial expiry, a cutover, a scheduled shutdown. Firing it at creation time runs the deferred action NOW: the payload is delivered before the state it depends on exists, deduplication/ordering assumptions break, and a test or workflow that creates a schedule a few minutes out observes the effect instantly. It also makes the "has it fired yet?" observable useless, so nothing built on at() can be emulated at all.

Fix: the scheduler service's schedule evaluation/dispatch loop — parse `at(...)` into its nominal instant in ScheduleExpressionTimezone and enqueue the single invocation for that instant, rather than dispatching on create/update; a schedule whose instant is in the future must have no invocation until then (and one whose instant is in the past must follow AWS's past-instant handling, not be treated as "due now" for a future expression).

Traces: cloudformation__scheduler__sqs.cfn-scheduler-schedule-in-place-at-update-delivers-retained-replacement-then-not-found. The later legs of that check — the UpdateReplacePolicy: Retain replacement of the schedule and the post-delete not-found — were never reached and are not claimed here. Related but NOT covering it: 20261109T141116Z-scheduler-stepfunctions-scheduler-starts-a-step-functions-execution-before-the.md reports the same early-firing symptom for a Step Functions target and does not carry this check's replay key.

## Evidence
cloudformation__scheduler__sqs.cfn-scheduler-schedule-in-place-at-update-delivers-retained-replacement-then-not-found — verdict mismatch.

The create leg and the in-place update of the schedule both passed: the stack settled UPDATE_COMPLETE, DescribeStackResources still reported the original schedule as the physical id of logical id `Schedule`, the resource's events were exactly [UPDATE_IN_PROGRESS, UPDATE_COMPLETE], and scheduler:GetSchedule read back the new at() expression and the new Input. The delivered message body was also the updated Input, so delivery itself works.

Expected: the message the schedule puts on the target queue has SentTimestamp >= the at() instant (the check allows a small clock skew below it).
Actual (AssertionError from the check):
  a one-time schedule at at(2026-11-11T10:24:30) must not deliver before then; SentTimestamp 1794392552594

The fixed part of that requirement is the ordering "SentTimestamp >= the at() instant"; the timestamp and the instant above are this run's clock values and are not themselves the requirement.

Model note: recorded behaviour plus AWS documentation — the scheduler Smithy model (services/service-com-amazonaws-scheduler-awschronosservice) types ScheduleExpression as a string and does not express the firing-time semantics, so the citation is the one-time-schedule `at()` contract and this recording.

## Steps to Reproduce
1. Create a target AWS::SQS::Queue and a role EventBridge Scheduler can assume with sqs:SendMessage on it.
2. cloudformation:CreateStack with an `AWS::Scheduler::Schedule` whose ScheduleExpression is `at(<now + ~2 minutes, yyyy-MM-ddTHH:mm:ss UTC>)`, FlexibleTimeWindow {Mode: OFF}, and Target {Arn: <queue arn>, RoleArn: <role>, Input: "<payload-1>"}. Wait for CREATE_COMPLETE.
3. cloudformation:UpdateStack with the same schedule but a fresh `at(<now + ~2 minutes>)` and Input "<payload-2>". Wait for UPDATE_COMPLETE and confirm GetSchedule reads back the new expression and Input.
4. Immediately after the update returns, call sqs:ReceiveMessage on the target queue with AttributeNames=[SentTimestamp]. Expected: nothing until the at() instant, then one message whose SentTimestamp is at or after it. Observed: the message is already present with a SentTimestamp before the at() instant.
5. Equivalently without CloudFormation: scheduler:CreateSchedule with `at(<now + 2 minutes>)`, FlexibleTimeWindow Mode OFF and an SQS target, then poll the queue — no message may appear before the named instant.
