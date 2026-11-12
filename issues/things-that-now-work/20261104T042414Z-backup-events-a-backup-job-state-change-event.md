Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__events.backup-job-state-change-invokes-lambda-target
Issue-Key: backup__events.backup-job-state-change-invokes-lambda-target|_poll_logs_for_marker#19|Timeout|time.sleep(4.0)
Issue-Commit: 637c0a54c7e9b591effc4af895837fd7874d2d9c

# backup__events: a Backup Job State Change event never invokes the rule's Lambda target, so the backup-job -> EventBridge -> Lambda path never converges

> _Note: this issue was identified against the code base at commit `637c0a54c7e9b591effc4af895837fd7874d2d9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBackupJob

## Description
One net-new root cause in the composite suite `backup__events` (AWS Backup + EventBridge + Lambda + CloudWatch Logs). Related open issues describe the same missing-event family but do NOT cover this check (the replay reports it as owned by no open issue): `20261029T175130Z-backup-events-no-backup-copy-restore-recovery-point.md`, `20261102T091831Z-backup-events-no-aws-backup-state-change-event.md` and `20261024T031359Z-backup-events-aws-backup-job-lifecycle-and-eventing-diverge-jobs.md`. Those cover SQS/SNS/Step-Functions/CFN deliveries and the job lifecycle; this check is the direct Lambda-target delivery, which has its own target-invocation path (lambda:InvokeFunction via the resource policy added by AddPermission, and the function's /aws/lambda/<name> log group).

Cause 1 of 1: a real on-demand backup job produces no `Backup Job State Change` event delivery to an EventBridge rule's Lambda target, so the function is never invoked and nothing is ever written to its log group.
AWS: recorded behavior only — the backup Smithy model describes no eventing. AWS Backup publishes a state-change notification for every backup job it runs, with `source: "aws.backup"` and `detail-type: "Backup Job State Change"`, whose `detail` carries the job's identity (including the backup job id) and its `state` (CREATED/PENDING/RUNNING/COMPLETED/...). An ENABLED rule whose pattern matches that source/detail-type/state, with a Lambda target and a resource policy allowing `lambda:InvokeFunction` to principal `events.amazonaws.com` for the rule ARN, is invoked with that event as its payload; the invocation's stdout lands in the function's `/aws/lambda/<function-name>` log group and is readable via FilterLogEvents. The check needs only ONE of the job's real early states to match.
Emulator: every API call in the setup is accepted (CreateRole/AttachRolePolicy, CreateFunction, PutRule, AddPermission, PutTargets, CreateTable, CreateBackupVault, StartBackupJob all return success), but no invocation ever happens: the log group never contains a line carrying the backup job id, and the check exhausts its budget and is reported as `Check timed out after 60.0s`. Hypothesis: the backup service's job state machine publishes no `aws.backup` state-change event onto the event bus at all (the same gap the related issues above report for SQS/SNS targets), so the rule never matches and the Lambda target is never invoked; a secondary possibility is that the bus matches but the Lambda target invocation path is not wired. Either way the emulator serves the whole pipeline and the observable end state — no invocation, no log line — is the emulator's.
Why: a client that reacts to backup jobs through EventBridge — the documented way to drive remediation, alerting or chained jobs from AWS Backup — sees nothing happen. The consumer never runs, so every downstream effect it owns is silently absent, with no error anywhere to point at; the application simply hangs or misses the event forever.
Fix: the backup service's job lifecycle must publish the `Backup Job State Change` event (source `aws.backup`, detail-type `Backup Job State Change`, detail including the job id, resource ARN, vault and state) onto the default event bus at each state transition, and the EventBridge rule-matching/target-dispatch path must deliver a matched event to a Lambda target (honoring the function's resource policy for principal events.amazonaws.com) and let the invocation's output reach the function's `/aws/lambda/<name>` log group.
Traces: behavioral check backup__events.backup-job-state-change-invokes-lambda-target.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 21/110 passed; service `backup__events` 8/50 (42 failed). Check backup__events.backup-job-state-change-invokes-lambda-target — verdict mismatch:

  Error: Check timed out after 60.0s

What the check does before timing out (all these calls are accepted by the emulator): IAM CreateRole + AttachRolePolicy for lambda.amazonaws.com; Lambda CreateFunction (python3.12, prints 'RECEIVED_EVENT::' + the event); EventBridge PutRule with pattern {"source":["aws.backup"],"detail-type":["Backup Job State Change"],"detail":{"state":["RUNNING","PENDING","CREATED","COMPLETED"]}} State=ENABLED; Lambda AddPermission (Action lambda:InvokeFunction, Principal events.amazonaws.com, SourceArn=<rule arn>); EventBridge PutTargets with the function ARN; DynamoDB CreateTable; backup CreateBackupVault; IAM role for backup.amazonaws.com; backup StartBackupJob(BackupVaultName, ResourceArn=<table arn>, IamRoleArn) returning a BackupJobId.

Expected: logs FilterLogEvents on /aws/lambda/<function> contains a line with the BackupJobId (the function echoes the whole event), within the check's polling budget.
Actual: no matching log event is ever produced — the Lambda target is never invoked — and the check is cut off at its 60s budget. Sibling checks in this suite fail the same way and their owning issues state that no 'Backup Job State Change' event is ever published, which is consistent with this observation.

No model evidence is cited for the event itself: eventing is not described by the backup Smithy model (recorded behavior only).

## Steps to Reproduce
1. Boot the emulator built from this checkout (backup, events, lambda, iam, logs).
2. Create an IAM role trusting lambda.amazonaws.com and a Lambda function (python3.12) whose handler prints the event JSON.
3. PutRule with EventPattern {"source":["aws.backup"],"detail-type":["Backup Job State Change"],"detail":{"state":["RUNNING","PENDING","CREATED","COMPLETED"]}}, State=ENABLED. AddPermission on the function for Principal events.amazonaws.com / Action lambda:InvokeFunction / SourceArn=<rule arn>. PutTargets with the function ARN.
4. Create a DynamoDB table, a backup vault, and an IAM role trusting backup.amazonaws.com; call StartBackupJob for the table ARN into that vault and note the returned BackupJobId.
5. Poll logs FilterLogEvents on /aws/lambda/<function-name> for the BackupJobId. Observe that no event is ever logged — the function is never invoked — so the wait runs to the check's budget and the check reports 'Check timed out after 60.0s'. On AWS the job's first matching state-change event invokes the target and the job id appears in the function's log group.
6. Re-replay behavioral check backup__events.backup-job-state-change-invokes-lambda-target to confirm the fix.
