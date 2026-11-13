Issue-Severity: LOW
Issue-Gap-Class: fidelity
Issue-Commit: 2d01dae495528395daed937b86e32de0c5183a35

# The documented defaults of a scheduled report's schedulePeriod are never materialised

_Filed: 2026-10-31T09:30:00Z_
_Operation: CreateScheduledReport, UpdateScheduledReport | Class: fidelity_

> _Note: this issue was identified against the code base at commit `2d01dae495528395daed937b86e32de0c5183a35`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both members of `SchedulePeriod` document a default the service fills in when a write
leaves the member out, and neither write path of this service fills in either one: a
report created or updated with a period that names no `endTime` — or with no
`schedulePeriod` at all — reads back through `GetScheduledReport` with that member
absent, where the service reports the instant its documented default resolves to.

This is only about materialising those defaults. The CEILING the same sentence states on
`endTime` ("the maximum allowed value is 3 years from the current time") is enforced on
both write paths by `crate::state::schedule_period_refusal`, so a submitted end time past
it is already refused with `ValidationException`.

## Evidence

Smithy model, `smithy_query(query="shape",shape="SchedulePeriod")`:

> `startTime` — "The start time of the schedule period. If not specified, defaults to the
> time of the create or update request. The start time cannot be more than 5 minutes
> before the time of the request."
>
> `endTime` — "The end time of the schedule period. If not specified, defaults to 3 years
> from the time of the create or update request. The maximum allowed value is 3 years from
> the current time. Setting an end time beyond this limit returns a `ValidationException`."

`crate::state::resolve_schedule_config` (the create path) and
`crate::state::updated_schedule_config` (the update path) resolve `state` and nothing
else, so the submitted period is stored exactly as it arrived — including the members it
did not carry.

Two committed tests pin the CURRENT behaviour, so a fixer has to decide what the rule is
for an absent period as well as for an absent bound, and will have to carry those tests
with the decision:

* `tests/all/scheduled_report_lifecycle.rs:321` — "a schedule with no period must not grow
  one";
* `tests/all/schedule_period_ceiling.rs` — the two acceptance rows whose period names no
  end time assert only that the write was accepted, deliberately leaving what a read
  reports for that member to this issue.

The second documented rule on `startTime` — a start time may not be more than 5 minutes
before the request — is unenforced on both write paths as well, and
`tests/all/update_schedule_state_preservation.rs:34` submits a start time months in the
past and asserts a 200, so that refusal belongs with this one.

## Steps to Reproduce

1. Create a dashboard, then create a scheduled report whose `scheduleConfig` carries
   `{"schedulePeriod": {"startTime": <now>}}` and keep the report `arn`.
2. Call `GetScheduledReport` and observe `scheduleConfig.schedulePeriod` carrying the
   submitted `startTime` and no `endTime`, where the model documents an unspecified end
   time as defaulting to 3 years from the time of the create request.
3. Call `UpdateScheduledReport` with a `scheduleConfig` naming only a
   `scheduleExpression` and a `schedulePeriod` of `{}`, then read the report back: the
   stored period still carries neither bound.
4. Call `UpdateScheduledReport` with `{"scheduleConfig": {"schedulePeriod": {"startTime":
   <a week ago>}}}` and observe HTTP 200, where the model states a start time cannot be
   more than 5 minutes before the request.
