Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__rds__scheduler.xsvc-rds-scheduler-stop-db-instance-denied-without-permission-reaches-rds-with-it
Issue-Key: events__rds__scheduler.xsvc-rds-scheduler-stop-db-instance-denied-without-permission-reaches-rds-with-it|_await_message#21|AssertionError|raise AssertionError(f"{description}: nothing arrived within {timeout_s:.0f}s")
Issue-Commit: f6476a64244d41dc0fd9adea93cb75255e1386be

# events__rds__scheduler: an at() schedule whose aws-sdk:rds:stopDBInstance target fails writes nothing to its DeadLetterConfig queue (neither the IAM denial nor RDS's not-found)

> _Note: this issue was identified against the code base at commit `f6476a64244d41dc0fd9adea93cb75255e1386be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
One net-new root cause under the events__rds__scheduler heading.

Cause 1 of 1: a one-time at() schedule whose target is the universal (aws-sdk) target arn:aws:scheduler:::aws-sdk:rds:stopDBInstance produces no dead-letter message on Target.DeadLetterConfig.Arn, for either outcome of the invocation — the role-denied leg or the permitted-but-instance-absent leg.

AWS: Smithy (scheduler) Target.DeadLetterConfig.Arn (ResourceArn, length 1..1600, pattern ^arn:aws(-[a-z]+)?:sqs:[a-z0-9\-]+:\d{12}:[a-zA-Z0-9\-_]+$) — 'An object that contains information about an Amazon SQS queue that EventBridge Scheduler uses as a dead-letter queue for your schedule. If specified, EventBridge Scheduler delivers failed events that could not be successfully delivered to a target to the queue'; Target.RetryPolicy{MaximumRetryAttempts 0..185, MaximumEventAgeInSeconds 60..86400}. The dead-letter message's attributes are recorded behavior: at or after the at() instant the invocation is attempted with the schedule's RoleArn, and when it fails the DLQ receives exactly one message whose Body is the schedule's Target.Input verbatim, with message attributes SCHEDULE_ARN (the schedule's ARN), TARGET_ARN (the universal-target ARN as written, arn:aws:scheduler:::aws-sdk:rds:stopDBInstance), RETRY_ATTEMPTS='0' for a non-retryable denial, ERROR_CODE and ERROR_MESSAGE. For a role lacking rds:StopDBInstance the codes are IAM's as RDS's query protocol spells them: ERROR_CODE='AccessDenied' (not 'AccessDeniedException') and ERROR_MESSAGE='User: arn:aws:sts::<account>:assumed-role/<role-name>/<session> is not authorized to perform: rds:StopDBInstance on resource: <db instance ARN> because no identity-based policy allows the rds:StopDBInstance action' — fixed text apart from the account, role name, the per-invocation 32-hex session name Scheduler mints, and the instance ARN. For a role that IS allowed the call reaches RDS, which answers for the absent instance: ERROR_CODE='DBInstanceNotFound' and ERROR_MESSAGE='DBInstance <identifier> not found.' (fixed text apart from the identifier taken from the request). Exactly one letter per failed occurrence.

Emulator: nothing arrives on either dead-letter queue. The denied leg's DLQ is polled for 210 s (the 30 s lead to the at() instant plus a 180 s delivery budget) and stays empty, so neither the IAM denial nor the control leg's RDS not-found is observable; GetSchedule's echo of ScheduleExpression, Target.RoleArn, Target.Input and RetryPolicy had already passed, so the schedule exists as written and only the firing/failure path is missing.

Why: a client that relies on the schedule DLQ as the only record of a failed occurrence sees total silence — a schedule that never fired and one whose target was denied are indistinguishable, the two failure classes (identity denial vs the target service's own refusal) cannot be told apart, and no alarm or retry tooling keyed on ERROR_CODE/RETRY_ATTEMPTS can work.

Fix: the Scheduler occurrence dispatcher for universal (aws-sdk) targets and its failure path — the invocation must be attempted with Target.RoleArn, authorized, and its failure routed to Target.DeadLetterConfig.Arn with the attributes above.

Related open issue under the same heading that does NOT carry this replay key: 20261109T135215Z-events-rds-scheduler-a-schedule-whose-target-invocation.md; the broader family is also described by 20261109T141116Z-scheduler-delivery-failure-handling-5-causes-deadletterconfig-never.md.

Traces: events__rds__scheduler.xsvc-rds-scheduler-stop-db-instance-denied-without-permission-reaches-rds-with-it.

## Evidence
events__rds__scheduler.xsvc-rds-scheduler-stop-db-instance-denied-without-permission-reaches-rds-with-it (line 91, _await_message at line 288):
  AssertionError: the denied stopDBInstance reaching the schedule DLQ: nothing arrived within 210s

Expected (one message on the denied schedule's DLQ, at or after its at() instant):
  Body            = the schedule's Target.Input, i.e. {"DbInstanceIdentifier": "<id>"}
  ERROR_CODE      = "AccessDenied"
  ERROR_MESSAGE   = "User: arn:aws:sts::<account>:assumed-role/<role>/<32-hex session> is not authorized to perform: rds:StopDBInstance on resource: arn:aws:rds:<region>:<account>:db:<id> because no identity-based policy allows the rds:StopDBInstance action"
  RETRY_ATTEMPTS  = "0"
  SCHEDULE_ARN    = the denied schedule's ARN
  TARGET_ARN      = "arn:aws:scheduler:::aws-sdk:rds:stopDBInstance"
and on the control schedule's DLQ: ERROR_CODE="DBInstanceNotFound", ERROR_MESSAGE="DBInstance <id> not found.", SCHEDULE_ARN = the permitted schedule's ARN.
Actual: both queues stay empty for the whole window; the session name is the only per-run value in the expected message and is not part of the requirement.

## Steps to Reproduce
1. CreateQueue twice (one DLQ per schedule).
2. CreateRole trusting scheduler.amazonaws.com with ONLY sqs:SendMessage on its DLQ (denied leg); CreateRole with rds:StopDBInstance on arn:aws:rds:<region>:<account>:db:<id> plus sqs:SendMessage on its DLQ (control leg). The DB instance <id> is deliberately absent.
3. CreateSchedule twice with ScheduleExpression at(<now + 30s>), Target {Arn: 'arn:aws:scheduler:::aws-sdk:rds:stopDBInstance', RoleArn: <the leg's role>, Input: '{"DbInstanceIdentifier": "<id>"}', RetryPolicy {MaximumRetryAttempts: 1, MaximumEventAgeInSeconds: 60}, DeadLetterConfig {Arn: <the leg's DLQ>}}.
4. GetSchedule on both (echo of ScheduleExpression/Target/RetryPolicy must match) and wait past the at() instant.
5. ReceiveMessage with MessageAttributeNames=['All'] on each DLQ.
   Expected: exactly one message per leg with the attributes above, arriving no earlier than the at() instant.
   Emulator: no message arrives on either queue within 210 s.
