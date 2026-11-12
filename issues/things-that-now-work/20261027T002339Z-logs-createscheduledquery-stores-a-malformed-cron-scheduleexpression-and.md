Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.create-scheduled-query-invalid-cron-expression-rejected
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.create-scheduled-query-invalid-cron-expression-rejected|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 37938bae9928f6f0327a9972c93fc7656bc513a9

# logs: CreateScheduledQuery stores a malformed cron scheduleExpression and answers 200 instead of refusing it with ValidationException (1 cause)

> _Note: this issue was identified against the code base at commit `37938bae9928f6f0327a9972c93fc7656bc513a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateScheduledQuery

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: `scheduleExpression` is stored verbatim with no schedule-expression format validation ===

AWS: Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json) models `CreateScheduledQuery.scheduleExpression` as `@required` targeting `ScheduleExpression` (@length 0..256), documented as "A cron expression that defines when the scheduled query runs. The expression uses standard cron syntax and supports minute-level precision", and models `ValidationException` (HTTP 400, "One of the parameters for the request is not valid") on the operation. AWS parses the expression BEFORE creating anything: a `cron(...)` body that does not carry the six required fields (minutes hours day_of_month month day_of_week year) and is not a well-formed `rate(...)` is refused with `ValidationException` and the message "1 validation errors detected: Invalid cron expression format. Expected format: cron(minutes hours day_of_month month day_of_week year)". Message rule: that entire string is FIXED text for this error class — the only variable part is the leading violation count ("1 validation errors detected" is the number of validation violations detected in the request); no name, ARN or value from the request appears in it. Because the create is refused, no scheduled query comes into being, so a subsequent `GetScheduledQuery` on the same identifier answers `ResourceNotFoundException` with "Scheduled query with identifier <identifier> not found", whose fixed text is "Scheduled query with identifier " + " not found" and whose variable part is the `identifier` taken from the request. A well-formed expression (e.g. the six-field form) is accepted and stored byte-for-byte, so `GetScheduledQuery.scheduleExpression` reads back exactly what was sent.

Emulator: the malformed expression is ACCEPTED. The call returns HTTP 200 with a populated `scheduledQueryArn` (an ARN derived from the requested name) and `state: ENABLED`, and the record is inserted into the store, so the name is thereafter live and occupied. `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_scheduled_query.rs` validates only name uniqueness (ConflictException) and the per-account count ceiling (ServiceQuotaExceededException); `input.schedule_expression` is copied straight into `ScheduledQueryRecord::new(...)` with no parse of `cron(...)`/`rate(...)` at all, and `ValidationException` is never constructed for a schedule expression.

Why: a client that mistypes a schedule gets a success and an ARN, and believes a recurring query is installed. On AWS the mistake is reported synchronously at the API boundary with a message naming the expected format, so the client fixes it immediately; here the query exists but can never run on the schedule the client asked for, and the name is consumed so a corrected retry under the same name now collides with ConflictException. Negative-path code that asserts the 400/ValidationException contract (CloudFormation/CDK schedule validation, CLI wrappers, IaC drift checks) takes the success branch.

Fix: the input-validation stage of the CreateScheduledQuery handler — it must parse `scheduleExpression` as either `rate(<value> <unit>)` or a six-field `cron(...)` before any store mutation, and return the modeled `ValidationException` (HTTP 400) with AWS's fixed message for this class when it does not parse; the same validation belongs on UpdateScheduledQuery, which shares the member. No record may be inserted on the rejected path.

Traces: logs.create-scheduled-query-invalid-cron-expression-rejected.

## Evidence
Failing check `logs.create-scheduled-query-invalid-cron-expression-rejected` (verdict: mismatch).

Request: CreateScheduledQuery with queryLanguage=CWLI, queryString="fields @timestamp | limit 1", scheduleExpression="cron(0 12 * *)" (4 fields, not 6, and not a rate()), executionRoleArn=<an execution role the check created>, logGroupIdentifiers=[<a log group the check created>], startTimeOffset=3600, endTimeOffset=0.

Expected (AWS): HTTP 400, error code `ValidationException`, message exactly "1 validation errors detected: Invalid cron expression format. Expected format: cron(minutes hours day_of_month month day_of_week year)"; then GetScheduledQuery on that name -> `ResourceNotFoundException`, message "Scheduled query with identifier <name> not found".

Actual (emulator): HTTP 200. Replay output: `AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'scheduledQueryArn': 'arn:aws:logs:us-west-2:000000000000:scheduled-query:<requested-name>', 'state': 'ENABLED', 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', ...}}}` (the ARN/account/name in that line are this run's generated values, not part of the requirement). No ValidationException was produced and the scheduled query was stored under the requested name.

Handler evidence: services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_scheduled_query.rs lines 207-280 — only the name-collision (ConflictException, l.214) and MAX_SCHEDULED_QUERIES (ServiceQuotaExceededException, l.231) checks run before `store.scheduled_queries.insert(...)` at l.280; `input.schedule_expression` is passed through at l.253 unparsed.

## Steps to Reproduce
1. logs CreateLogGroup <group>.
2. Create an IAM role usable as the scheduled-query execution role.
3. logs CreateScheduledQuery name=<new-name> queryLanguage=CWLI queryString="fields @timestamp | limit 1" scheduleExpression="cron(0 12 * *)" executionRoleArn=<role> logGroupIdentifiers=[<group>] startTimeOffset=3600 endTimeOffset=0.
   Observed: HTTP 200 with scheduledQueryArn and state=ENABLED. Expected: HTTP 400 ValidationException, "1 validation errors detected: Invalid cron expression format. Expected format: cron(minutes hours day_of_month month day_of_week year)".
4. logs GetScheduledQuery identifier=<new-name>.
   Observed: the query exists. Expected: ResourceNotFoundException "Scheduled query with identifier <new-name> not found".
5. Positive control (must keep working): same call with scheduleExpression="cron(0 12 * * ? *)" -> 200, and GetScheduledQuery returns that expression stored verbatim.
