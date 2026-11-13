Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# CreateScheduledReport accepts a non-cron scheduleExpression and widgetIds that name no widget of the dashboard

_Filed: 2026-10-24T02:47:16Z_
_Operation: CreateScheduledReport | Class: validation_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler persists `scheduledReport` verbatim after checking only that `dashboardArn` resolves to a stored dashboard, so several spec-required input validations never run and invalid configurations are accepted with HTTP 200:
1. `scheduleConfig.scheduleExpression` is documented as "This value must be a cron expression consisting of six fields separated by white spaces: cron(minutes hours day_of_month month day_of_week year)". Its modelled shape is an unconstrained GenericString (pattern ^[\S\s]*$), so nothing at the boundary enforces the documented form and the handler adds no check: `rate(7 days)`, `not a schedule`, or `cron(0 8 1 * ?)` (five fields) are all stored and read back by GetScheduledReport/ListScheduledReports. Expected: ValidationException (HTTP 400) and no report created.
2. `widgetIds` is documented as "The list of widget identifiers to include in the scheduled report. If not specified, all widgets in the dashboard are included" — the ids must therefore identify widgets of the named dashboard. The handler copies `requested.widget_ids` into the record without comparing them against `state.dashboards[dashboard_arn].widgets`, so a syntactically valid WidgetId (32 hex chars) belonging to no widget of that dashboard is accepted and persisted. Expected: ValidationException.
3. `widgetDateRangeOverride` (DateTimeRange with required startTime/endTime) is stored without checking that the range is ordered (endTime not before startTime). Expected: ValidationException for an inverted range.

## Evidence

Smithy model docs via smithy_query(query="shape", shape="ScheduleConfig") — scheduleExpression: "must be a cron expression consisting of six fields separated by white spaces: cron(...)"; shape="ScheduledReportInput" — widgetIds: "The list of widget identifiers to include in the scheduled report. If not specified, all widgets in the dashboard are included"; shape="DateTimeRange" — startTime/endTime both @required, describing "a time period with explicit start and end times". CreateScheduledReport declares ValidationException (HTTP 400). Handler: src/ops/create_scheduled_report.rs lines 183-259 — the only pre-store check is `state.dashboards.contains_key(&requested.dashboard_arn)`; `schedule_config` goes straight through `crate::state::resolve_schedule_config` (which only defaults `state`), and `widget_ids`/`widget_date_range_override` are moved into the record unexamined. Tests ASSERT the permissive behavior: tests/all/scheduled_report_lifecycle.rs:129-144 stores `"scheduleExpression": "rate(7 days)"` as valid ("free-form GenericString... stored verbatim"), and no test submits a widget id foreign to the source dashboard.

## Steps to Reproduce

1. CreateDashboard with a valid name and one widget; note the dashboard `arn`.
2. Call CreateScheduledReport with scheduledReport = {name, dashboardArn, scheduledReportExecutionRoleArn: arn:aws:iam::000000000000:role/BcmReports, scheduleConfig: {scheduleExpression: "rate(7 days)"}} — expected ValidationException (400) because the expression is not a six-field cron(...); observed 200 with a minted scheduled-report ARN, and GetScheduledReport reads the bogus expression back.
3. Call CreateScheduledReport with scheduleConfig: {scheduleExpression: "cron(0 8 1 * ? *)"} and widgetIds: ["ffffffffffffffffffffffffffffffff"] (a well-formed WidgetId that no widget of the dashboard carries) — expected ValidationException; observed 200 and the id persisted.
4. Call CreateScheduledReport with widgetDateRangeOverride {startTime: 2025-06-01T00:00:00Z, endTime: 2025-01-01T00:00:00Z} (end before start) — expected ValidationException; observed 200 and the inverted range persisted.
