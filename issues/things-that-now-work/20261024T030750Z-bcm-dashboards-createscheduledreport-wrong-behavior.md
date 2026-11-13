Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# Make clientToken an idempotency key (replay the original ARN) and resolve the documented schedulePeriod defaults

_Filed: 2026-10-24T03:07:50Z_
_Operation: CreateScheduledReport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two things the create path computes wrongly:

1. `clientToken` is modelled `@idempotencyToken`, which means a repeated request carrying the same token must NOT create a second resource and must answer as the first one did — the caller gets the SAME `arn` back with HTTP 200. The handler instead refuses every later call presenting a known token with `ConflictException` (HTTP 409), forever. That breaks the exact case the token exists for: a client (or SDK retry) whose first response was lost retries with the same token and is told the request conflicts rather than receiving the ARN that was in fact created. The state already records the mapping needed for the replay (`scheduled_report_client_tokens: BTreeMap<token, arn>`, src/state.rs:250) but the ARN it stores is never read.

2. `SchedulePeriod.startTime`/`endTime` carry documented service-side defaults — "If not specified, defaults to the time of the create or update request" and "If not specified, defaults to 3 years from the time of the create or update request". The handler stores the submitted `scheduleConfig` with only `state` resolved, so a report created without a `schedulePeriod` (or with a half-filled one) reads back from GetScheduledReport/ListScheduledReports with those members still absent instead of with the resolved period the real service returns.

## Evidence

Spec: `CreateScheduledReportRequest.clientToken` carries `@idempotencyToken` (OPERATIONS.md / `smithy_query(query="operation",operation="CreateScheduledReport")`), whose established AWS semantics are at-most-once creation with an idempotent replay of the original answer; `smithy_query(query="shape",shape="SchedulePeriod")` states both defaults verbatim. Handler: `src/ops/create_scheduled_report.rs` lines ~206-218 return ConflictException whenever the token is already in `state.scheduled_report_client_tokens`, and line ~246 stores `resolve_schedule_config(requested.schedule_config)` which (src/state.rs:397-402) only fills `state`. Tests: `tests/all/scheduled_report_lifecycle.rs::a_client_token_creates_at_most_one_scheduled_report` currently pins the 409 answer, so the existing suite locks in the divergence rather than covering it.

## Steps to Reproduce

1. CreateDashboard to obtain a dashboard ARN.
2. CreateScheduledReport with header X-Amzn-Client-Token: tok-1 — note the returned arn A.
3. Repeat the identical CreateScheduledReport with X-Amzn-Client-Token: tok-1 — expect HTTP 200 with arn == A and still exactly one report in ListScheduledReports; actual HTTP 409 ConflictException.
4. CreateScheduledReport with a scheduleConfig that omits schedulePeriod, then GetScheduledReport on the new ARN — expect scheduleConfig.schedulePeriod.startTime ≈ the create time and endTime ≈ create time + 3 years; actual schedulePeriod is absent entirely.
