Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler.flexible-time-window-invocation-jitter-within-bounds scheduler.schedule-flexible-time-window-invocation-within-bounds scheduler.server-side-flexible-time-window-jitter-within-bounds scheduler.scheduler-cron-expression-fires-once-on-the-derived-minute-boundary scheduler.schedule-start-date-defers-first-invocation scheduler.scheduler-schedule-expression-timezone-interpreted-in-a-real-iana-zone scheduler.server-side-rate-expression-cadence-enforced scheduler.events-scheduler-semantics-one-time-at-schedule-fires-one-occurrence-and-persists-under-action-none scheduler.schedule-self-delete-after-completion scheduler.scheduler-action-after-completion-delete-removes-a-recurring-schedule-at-its-end-date scheduler.scheduler-group-deletion-halts-recurring-invocations scheduler.server-side-cascade-delete-halts-future-invocations scheduler.events-scheduler-semantics-same-schedule-name-in-two-groups-is-isolated-and-survives-the-sibling-group-deletion
Issue-Shape-Id: com.amazonaws.scheduler#AWSChronosService
Issue-Key: scheduler.flexible-time-window-invocation-jitter-within-bounds|run#17|AssertionError|assert delta >= -5, f"invocation observed {-delta:.1f}s before its scheduledTime"
Issue-Key: scheduler.schedule-flexible-time-window-invocation-within-bounds|run#25|AssertionError|assert invoked_at >= nominal - clock_skew_tolerance, (
Issue-Key: scheduler.server-side-flexible-time-window-jitter-within-bounds|run#16|AssertionError|assert sent_ms >= nominal_epoch_ms - slack_ms, (
Issue-Key: scheduler.scheduler-cron-expression-fires-once-on-the-derived-minute-boundary|run#17|AssertionError|assert early <= _EARLY_SLACK_S, (
Issue-Key: scheduler.schedule-start-date-defers-first-invocation|run#12|AssertionError|assert marker not in early_markers, "schedule invoked its target before its StartDate"
Issue-Key: scheduler.scheduler-schedule-expression-timezone-interpreted-in-a-real-iana-zone|run#26|AssertionError|assert early <= _EARLY_SLACK_S, (
Issue-Key: scheduler.server-side-rate-expression-cadence-enforced|run#10|AssertionError|assert 40.0 <= gap_s <= 110.0, f"unexpected inter-invocation gap: {gap_s}s"
Issue-Key: scheduler.events-scheduler-semantics-one-time-at-schedule-fires-one-occurrence-and-persists-under-action-none|run#18|AssertionError|raise AssertionError(
Issue-Key: scheduler.schedule-self-delete-after-completion|_poll_until#20|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: scheduler.scheduler-action-after-completion-delete-removes-a-recurring-schedule-at-its-end-date|_poll_until#15|AssertionError|raise AssertionError(f"waited {timeout_s}s for {description}; last observation was {last!r}")
Issue-Key: scheduler.scheduler-group-deletion-halts-recurring-invocations|run#15|AssertionError|assert _error(lambda: sch.get_schedule(Name=name, GroupName=group)) == (
Issue-Key: scheduler.server-side-cascade-delete-halts-future-invocations|_drain_collecting_execs#17|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: scheduler.events-scheduler-semantics-same-schedule-name-in-two-groups-is-isolated-and-survives-the-sibling-group-deletion|run#30|AssertionError|assert member_message == gone_message, (
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# scheduler occurrence timing and lifecycle: 7 causes — schedules fire before their nominal instant, StartDate and ScheduleExpressionTimezone are ignored, rate() is not server-paced, ActionAfterCompletion=DELETE never removes a schedule, group deletion leaves members armed, and GetSchedule names the schedule when the group is gone

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
This issue carries 7 root causes, all in the `scheduler` service. Each must be closed before this issue is submitted; each cause lists the failing check ids it is grounded in. None of these checks is marked ALREADY FILED or OWNED in the replay report. Related but NOT covering: open issue 20261025T072339Z-scheduler-3-unowned-behavioral-causes-flexible-time-window.md describes the FLEXIBLE window, rate() cadence and group-scoped firing from a time when NOTHING was ever delivered (its Issue-Keys are all Timeout keys); the emulator now delivers, and the divergences below are different observations (early firing, 1-second cadence, surviving members), so that issue's keys do not carry them. Open issue 20261108T032912Z-scheduler-5-net-new-behavioral-causes-three-refusal.md Cause 4 is the same not-found branch as Cause 7 here but does not carry this check id. The companion issue filed in this sweep covers the delivery-failure/authorization/universal-target causes.

=== Cause 1 of 7: an occurrence is delivered BEFORE its nominal instant ===
Failing checks: scheduler.flexible-time-window-invocation-jitter-within-bounds, scheduler.schedule-flexible-time-window-invocation-within-bounds, scheduler.server-side-flexible-time-window-jitter-within-bounds, scheduler.scheduler-cron-expression-fires-once-on-the-derived-minute-boundary
AWS: Smithy (com.amazonaws.scheduler) — FlexibleTimeWindow is required on CreateSchedule; Mode -> FlexibleTimeWindowMode (required) "Determines whether the schedule is invoked within a flexible time window"; MaximumWindowInMinutes -> integer @range 1..1440 "The maximum time window during which a schedule can be invoked." ScheduleExpression is required, @length 1..256, and accepts at(), rate() and cron() forms. The nominal instant is derived from ScheduleExpression (+ ScheduleExpressionTimezone). With Mode=OFF the invocation happens AT the nominal instant; with Mode=FLEXIBLE the service picks an instant inside [nominal, nominal + MaximumWindowInMinutes]. In neither mode is an occurrence ever delivered before the nominal instant — the window extends forward only.
Emulator: occurrences arrive well before the nominal instant: 58.1 s early, 88 s early, 58 s early for flexible-window schedules, and 148.2 s before the pinned cron minute for a Mode=OFF cron() schedule. The nominal instant is not used as the lower bound of the window at all.
Why: firing early is the one error a time-based service must not make. A client that schedules work for a specific minute (a cron boundary) or spreads load with a flexible window gets its target invoked against state that does not exist yet — a report run before its data lands, a window opened before it is valid — and the schedule reads back exactly as configured, so there is nothing to observe. Downstream checks that assert a Mode=OFF schedule cannot fire early pass against AWS and fail here.
Fix: the occurrence computation in the dispatch component (services/service-com-amazonaws-scheduler-awschronosservice/src/) must compute the nominal instant from ScheduleExpression and clamp the chosen invocation instant to [nominal, nominal + MaximumWindowInMinutes] — never earlier than nominal, for either Mode.

=== Cause 2 of 7: StartDate does not defer the first invocation ===
Failing check: scheduler.schedule-start-date-defers-first-invocation
AWS: StartDate is "The date, in UTC, after which the schedule can begin invoking its target" (a plain modelled timestamp on CreateSchedule/UpdateSchedule). A recurring schedule whose StartDate is in the future produces NO occurrence before that instant; its first invocation is the first occurrence of the expression at or after StartDate.
Emulator: the schedule invokes its target before its StartDate — the member is stored and echoed but never consulted when arming the occurrence.
Why: StartDate is how a client arms a schedule ahead of time (a maintenance window, a launch date) without it running meanwhile. On the emulator the target starts receiving invocations immediately, so work runs before it is allowed to, and no part of the API reports that it did.
Fix: the dispatch component must use the record's StartDate as a hard floor on the first (and every) occurrence instant.

=== Cause 3 of 7: ScheduleExpressionTimezone is not used to interpret the expression ===
Failing check: scheduler.scheduler-schedule-expression-timezone-interpreted-in-a-real-iana-zone
AWS: ScheduleExpressionTimezone (@length 1..50) is "The timezone in which the scheduling expression is evaluated". A cron()/at() expression with ScheduleExpressionTimezone='Asia/Kolkata' denotes a LOCAL wall-clock instant in that zone; the occurrence happens at the UTC instant that local literal maps to — never before it.
Emulator: the Asia/Kolkata schedule is delivered 18.4 s BEFORE the instant its local literal denotes, so the zone is not applied to the expression (or the resulting instant is not used as the lower bound).
Why: a zone-aware schedule that fires at the wrong instant breaks every business-hours or end-of-day workload, and the error is silent: GetSchedule echoes the zone back faithfully. Because the divergence here is a fire-early, it is also not detectable as a late-delivery delay.
Fix: the occurrence computation must evaluate ScheduleExpression in ScheduleExpressionTimezone (defaulting to UTC) and arm the resulting UTC instant, with the clamp of Cause 1 applied to it.

=== Cause 4 of 7: rate() cadence is not paced by the server — occurrences arrive ~1 s apart ===
Failing check: scheduler.server-side-rate-expression-cadence-enforced
AWS: ScheduleExpression (required, @length 1..256) accepts rate(value unit). A rate(2 minutes) schedule in State=ENABLED with FlexibleTimeWindow Mode=OFF fires repeatedly, and consecutive occurrences are spaced BY THE SERVER at the configured cadence — about 120 s apart — independently of how often the client polls the target. The check asserts the gap between the SQS SentTimestamp of two consecutive deliveries lies in [80 s, 220 s].
Emulator: the observed inter-invocation gap is 1.012 s — the rate unit is not honoured, so a rate(2 minutes) schedule fires roughly every second.
Why: recurring schedules are the primary use of this service. A client that drives periodic work off rate() gets it executed ~120x more often than configured: duplicated writes, duplicated notifications, exhausted downstream quotas, and a cost profile nothing in the API hints at. The control plane keeps reporting the configured expression.
Fix: the dispatch component must compute each occurrence's due instant from the rate value AND unit against the server clock and emit each occurrence at its own due instant — not on a fixed short tick and not on demand.

=== Cause 5 of 7: ActionAfterCompletion=DELETE never removes a completed schedule ===
Failing checks: scheduler.events-scheduler-semantics-one-time-at-schedule-fires-one-occurrence-and-persists-under-action-none, scheduler.schedule-self-delete-after-completion, scheduler.scheduler-action-after-completion-delete-removes-a-recurring-schedule-at-its-end-date
AWS: Smithy (com.amazonaws.scheduler) — ActionAfterCompletion is an enum NONE | DELETE and CreateScheduleInput.ActionAfterCompletion is "Specifies the action that EventBridge Scheduler applies to the schedule after the schedule completes invoking the target." A schedule completes when a one-time at() expression has fired, or when a recurring schedule has passed its EndDate (its last occurrence being the one closest to that instant). With DELETE the service then removes the schedule, after which GetSchedule answers 404 ResourceNotFoundException 'Schedule <Name> does not exist.'; with NONE the schedule remains resolvable.
Emulator: the schedule still resolves after it has completed, in all three checks — the DELETE twin of a one-time at() schedule is still present 180 s after it delivered, a self-deleting schedule is still present after 120 s, and a recurring schedule with EndDate past is still 'present' after 240 s. ActionAfterCompletion is stored and echoed but never acted on.
Why: DELETE is how clients keep one-shot and bounded schedules from accumulating. The store grows without bound, reconcilers keep treating completed schedules as live, and tests that wait for the schedule to disappear hang. Because the NONE twin in the same check behaves correctly, the divergence is confined to the DELETE action.
Fix: the dispatch component must apply the record's ActionAfterCompletion at the moment a schedule completes (last at() occurrence fired, or EndDate passed), deleting the record from the store so GetSchedule 404s.

=== Cause 6 of 7: DeleteScheduleGroup does not remove or disarm the group's member schedules ===
Failing checks: scheduler.scheduler-group-deletion-halts-recurring-invocations, scheduler.server-side-cascade-delete-halts-future-invocations
AWS: a schedule's identity is the (group, name) pair — GetScheduleOutput.Arn is @pattern ^arn:aws(-[a-z]+)?:scheduler:[a-z0-9\-]+:\d{12}:schedule\/[0-9a-zA-Z-_.]+\/[0-9a-zA-Z-_.]+$, group segment then name segment — and DeleteScheduleGroup cascades to every schedule in THAT group: once the group is gone its members no longer resolve and, decisively, no further occurrence of any member is delivered.
Emulator: the member schedule SURVIVES the deletion of its group (the check reports it still resolving afterwards), and the cascade variant runs to the 900 s harness limit without the invocations ever halting. The group deletion neither removes the member records nor disarms their occurrences.
Why: deleting a schedule group is how a client tears down a tenant, an environment or a feature. On the emulator the teardown reports success while the schedules keep firing forever, with no API surface that can be used to stop them (the group they belong to no longer exists), so work keeps being executed against decommissioned systems indefinitely.
Fix: src/ops/delete_schedule_group.rs, together with state.rs's settle (which materialises the DELETING window), must remove every schedules[(group, *)] entry and disarm exactly the occurrences belonging to that group — keyed by (group, name), the key state.rs already uses — leaving other groups' identically-named schedules armed.

=== Cause 7 of 7: GetSchedule against a schedule group that no longer exists names the SCHEDULE instead of the GROUP ===
Failing check: scheduler.events-scheduler-semantics-same-schedule-name-in-two-groups-is-isolated-and-survives-the-sibling-group-deletion
AWS: GetSchedule resolves the group named by its GroupName query parameter (GetSchedule.GroupName -> ScheduleGroupName, httpQuery 'groupName') BEFORE the schedule, so when that group does not exist — including after DeleteScheduleGroup has cascaded it away — the ResourceNotFoundException (httpError 404) names the GROUP: fixed text 'Schedule group ' + the GroupName from the request + ' does not exist.' The schedule name never appears. When the group DOES exist and only the schedule is absent, the message is 'Schedule ' + the Name + ' does not exist.'
Emulator: status and code are right (404 ResourceNotFoundException) but the message is always the schedule form, 'Schedule <Name> does not exist.', because the GetSchedule handler goes straight to the (group, name) lookup without a group-existence branch. UpdateSchedule on the same gone group answers the group form, so the two reads disagree.
Why: the client cannot distinguish a deleted group from a deleted schedule, so reconciliation re-creates the wrong resource — a schedule into a group that is gone, failing for a third reason — and cascade-delete verification that asserts the group-form message fails here while passing against AWS. Open issue 20261108T032912Z-scheduler-5-net-new-behavioral-causes-three-refusal.md Cause 4 describes the same handler branch from four other checks and does not carry this check id.
Fix: order the lookups in src/ops/get_schedule.rs so the group-existence branch runs first and uses the shared group-not-found helper crate::state::group_does_not_exist, exactly as CreateSchedule and UpdateSchedule already do.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator: 33/171 checks passed overall; scheduler 14/120 (29 failed, 77 unloadable). 1 of the 29 is OWNED and excluded; these 13 are the 7 causes here, the other 15 are the companion issue. All concrete names, distribution ids and timestamps are one run's values, not requirements.

Cause 1 — early fire:
  scheduler.flexible-time-window-invocation-jitter-within-bounds — AssertionError: invocation observed 58.1s before its scheduledTime
  scheduler.schedule-flexible-time-window-invocation-within-bounds — AssertionError: invoked at 2026-11-09T13:41:33.961000+00:00 before nominal 2026-11-09T13:43:02.172385+00:00 (≈88 s early)
  scheduler.server-side-flexible-time-window-jitter-within-bounds — AssertionError: invocation fired before its nominal time: sent=1794229613224 nominal=1794229671000 (≈58 s early)
  scheduler.scheduler-cron-expression-fires-once-on-the-derived-minute-boundary — AssertionError: the message was stamped by SQS at 2026-11-09T13:16:31.832000+00:00, 148.2s BEFORE the pinned cron minute 2026-11-09T13:19:00+00:00; a Mode=OFF schedule cannot fire early
  Expected: invocation instant in [nominal, nominal + MaximumWindowInMinutes]; for Mode=OFF, at nominal.

Cause 2 — StartDate:
  scheduler.schedule-start-date-defers-first-invocation — AssertionError: schedule invoked its target before its StartDate
  Expected: no delivery before StartDate.

Cause 3 — timezone:
  scheduler.scheduler-schedule-expression-timezone-interpreted-in-a-real-iana-zone — AssertionError: the Asia/Kolkata schedule was sent at 2026-11-09T13:58:45.584000+00:00, 18.4s BEFORE the instant 2026-11-09T13:59:04+00:00 its local literal denotes; a zone-aware scheduler cannot fire early

Cause 4 — rate() cadence:
  scheduler.server-side-rate-expression-cadence-enforced — AssertionError: unexpected inter-invocation gap: 1.012s
  Expected: for rate(2 minutes), gap between consecutive SQS SentTimestamps in [80.0 s, 220.0 s]. Actual: ~1 s.

Cause 5 — ActionAfterCompletion=DELETE:
  scheduler.events-scheduler-semantics-one-time-at-schedule-fires-one-occurrence-and-persists-under-action-none — AssertionError: the DELETE twin <name> still resolved 180s after it delivered; ActionAfterCompletion=DELETE must remove it (the ActionAfterCompletion=NONE twin in the same check behaved correctly)
  scheduler.schedule-self-delete-after-completion — AssertionError: condition not met within 120s. Last result: True (the schedule is still present)
  scheduler.scheduler-action-after-completion-delete-removes-a-recurring-schedule-at-its-end-date — AssertionError: waited 240s for ActionAfterCompletion=DELETE to remove <name> after its EndDate 2026-11-09T13:17:40+00:00 passed (its last invocation is the occurrence closest to that instant); last observation was 'present'
  Expected after completion: GetSchedule -> 404 ResourceNotFoundException 'Schedule <Name> does not exist.'

Cause 6 — group cascade:
  scheduler.scheduler-group-deletion-halts-recurring-invocations — AssertionError: the member schedule <name> survived the deletion of its group <group>
  scheduler.server-side-cascade-delete-halts-future-invocations — Error: Check timed out after 900.0s (the invocations never halt after the group is deleted)
  Expected: after DeleteScheduleGroup, the members no longer resolve and no further occurrence of any member is delivered.

Cause 7 — not-found resource after cascade:
  scheduler.events-scheduler-semantics-same-schedule-name-in-two-groups-is-isolated-and-survives-the-sibling-group-deletion — AssertionError: group A's cascade-deleted schedule '<name>' reported message 'Schedule <name> does not exist.'; expected the group's own not-found message 'Schedule group <group A> does not exist.'
  Code (ResourceNotFoundException) and status (404) match; only the Message names the wrong resource. Everything earlier in this check — distinct ARNs per group, per-group ListSchedules, both same-named schedules delivering their own marker — passed, so only the group-missing message branch diverges.

## Steps to Reproduce
Common setup (as the checks do): sqs.create_queue for the target and read its QueueArn; iam.create_role trusting scheduler.amazonaws.com with an inline sqs:SendMessage policy on that ARN. All calls via boto3 client('scheduler') against the emulator; delivery instants are read from the SQS SentTimestamp message attribute.

Cause 1: nominal = now + 90 s; create_schedule(Name=n, ScheduleExpression='at(<nominal>)', ScheduleExpressionTimezone='UTC', FlexibleTimeWindow={'Mode':'FLEXIBLE','MaximumWindowInMinutes':1}, State='ENABLED', Target={'Arn':queue_arn,'RoleArn':role_arn,'Input':<marker>}); read SentTimestamp of the delivered message. On AWS it is >= nominal; on the emulator it is ~58-88 s earlier. Repeat with MaximumWindowInMinutes=2, and with a cron() expression pinned to a future minute under FlexibleTimeWindow {'Mode':'OFF'} (observed 148 s early). Checks: the four ids listed for Cause 1.

Cause 2: create_schedule(..., ScheduleExpression='rate(1 minute)', FlexibleTimeWindow={'Mode':'OFF'}, StartDate=<now + several minutes>, Target={...}). Poll the queue before StartDate: on AWS it stays empty; on the emulator a message arrives. Check scheduler.schedule-start-date-defers-first-invocation.

Cause 3: create_schedule(..., ScheduleExpression='cron(...)' or 'at(<local literal>)', ScheduleExpressionTimezone='Asia/Kolkata', FlexibleTimeWindow={'Mode':'OFF'}, Target={...}); compare SentTimestamp against the UTC instant the local literal denotes. On AWS it is not earlier; on the emulator it is ~18 s earlier. Check scheduler.scheduler-schedule-expression-timezone-interpreted-in-a-real-iana-zone.

Cause 4: create_schedule(..., ScheduleExpression='rate(2 minutes)', FlexibleTimeWindow={'Mode':'OFF'}, State='ENABLED', Target={...}); collect two consecutive deliveries with their SentTimestamps. On AWS the gap is ~120 s (in [80,220]); on the emulator ~1.0 s. Check scheduler.server-side-rate-expression-cadence-enforced.

Cause 5: (a) create_schedule(..., ScheduleExpression='at(<now+~1min>)', FlexibleTimeWindow={'Mode':'OFF'}, ActionAfterCompletion='DELETE', Target={...}) beside an identical twin with ActionAfterCompletion='NONE'; after the delivery, poll get_schedule on both: on AWS the DELETE one 404s and the NONE one resolves; on the emulator both resolve. (b) create_schedule(..., ScheduleExpression='rate(1 minute)', ActionAfterCompletion='DELETE', EndDate=<now + a few minutes>); after EndDate passes, poll get_schedule: on AWS it 404s; on the emulator it stays 'present' (observed after 240 s). Checks scheduler.events-scheduler-semantics-one-time-at-schedule-fires-one-occurrence-and-persists-under-action-none, scheduler.schedule-self-delete-after-completion, scheduler.scheduler-action-after-completion-delete-removes-a-recurring-schedule-at-its-end-date.

Cause 6: create_schedule_group(Name=g); create_schedule(Name=n, GroupName=g, ScheduleExpression='rate(1 minute)', FlexibleTimeWindow={'Mode':'OFF'}, Target={...}); wait for a delivery; delete_schedule_group(Name=g); poll get_schedule_group(Name=g) until it 404s. On AWS get_schedule(Name=n, GroupName=g) no longer resolves and the queue receives nothing further; on the emulator the member still resolves and deliveries continue (observed until the 900 s harness limit). Checks scheduler.scheduler-group-deletion-halts-recurring-invocations, scheduler.server-side-cascade-delete-halts-future-invocations.

Cause 7: create_schedule_group(Name=A) and (Name=B); create the SAME schedule name n in both with distinct Target.Input markers; after both have delivered, delete_schedule_group(Name=A) and wait until get_schedule_group(Name=A) 404s; then get_schedule(Name=n, GroupName=A). On AWS: 404 ResourceNotFoundException 'Schedule group A does not exist.'; on the emulator: 404 'Schedule n does not exist.' Check scheduler.events-scheduler-semantics-same-schedule-name-in-two-groups-is-isolated-and-survives-the-sibling-group-deletion.
