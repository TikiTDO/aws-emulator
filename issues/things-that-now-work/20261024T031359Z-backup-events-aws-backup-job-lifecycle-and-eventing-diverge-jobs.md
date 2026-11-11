Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__events.backup-job-failed-state-change-event-detail backup__events.backup-job-state-change-multiple-targets-fanout backup__events.eventbridge-rule-disabled-does-not-deliver-backup-event
Issue-Key: backup__events.backup-job-failed-state-change-event-detail|run#16|AssertionError|assert final_state == "FAILED", f"backup job ended in {final_state}, expected FAILED"
Issue-Key: backup__events.backup-job-state-change-multiple-targets-fanout|_receive_matching#28|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: backup__events|DisableRule|NotImplemented
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# AWS Backup job lifecycle and eventing diverge: jobs always settle COMPLETED with no StatusMessage, no 'Backup Job State Change' event ever reaches EventBridge, and DisableRule is a 501 stub (3 causes)

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBackupJob

## Description
Three distinct root causes behind the three net-new failing checks in the backup__events group. All three are reached through the backup/EventBridge pair exercised by those checks, so they are bundled as one work item for this service group; each must be closed separately.

=== Cause 1 of 3: an on-demand backup job can only ever end COMPLETED — there is no FAILED terminal state and StatusMessage is never populated ===
AWS: DescribeBackupJob returns `State` (enum BackupJobState) and `StatusMessage` (string) — Smithy: DescribeBackupJob output members `State` -> BackupJobState (enum), `StatusMessage` -> string; StartBackupJob input `IamRoleArn` (IAMRoleArn, @required) documented as "Specifies the IAM role ARN used to create the target recovery point". On real AWS, StartBackupJob is ACCEPTED synchronously for a role with the correct trust policy but no permissions, and the job then fails ASYNCHRONOUSLY: the job reaches the terminal state FAILED and DescribeBackupJob reports a non-empty StatusMessage describing the failure (an AccessDenied-class reason). The model enumerates FAILED as a BackupJobState value; the emulator can never produce it.
Emulator: every accepted job settles to COMPLETED. services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/start_backup_job.rs stores the job with `state: BACKUP_JOB_STATE_CREATED`, `status_message: None` and a single `settles_at` instant, and the only settle transition available moves it to COMPLETED — the IamRoleArn is validated for ARN syntax only and nothing else in the job record can ever select a failure outcome. Replay: the check polled DescribeBackupJob to a terminal state and got `backup job ended in COMPLETED, expected FAILED`; StatusMessage stayed absent.
Why: a client that drives Backup and branches on the job outcome (alarm on FAILED, surface StatusMessage to an operator, retry with a corrected role) can never exercise or test its failure path against the emulator — every backup appears to succeed, including ones AWS would refuse to perform, and recovery points are reported as created for jobs that would not have produced one.
Fix: the backup job state machine and its settle path — the job record and its terminal-state selection in start_backup_job.rs plus crate::state (the job settle / transition logic that DescribeBackupJob reads), which needs a FAILED outcome carrying a StatusMessage.
Traces: backup__events.backup-job-failed-state-change-event-detail.

=== Cause 2 of 3: AWS Backup publishes no 'Backup Job State Change' event, so an EventBridge rule's targets never receive one ===
AWS: recorded behavior only (the event envelope is not in either service's Smithy model). AWS Backup natively emits an event on the default event bus for every on-demand job with `source` = "aws.backup", `detail-type` = "Backup Job State Change" and a `detail` carrying at least `backupJobId`, `backupVaultName` and `state`; an ENABLED EventBridge rule matching that pattern delivers it to each configured target, including fan-out to an SQS queue target and an SNS topic target simultaneously (the SNS hop with RawMessageDelivery=true preserves `detail-type` and `detail`). The checks confirm this on replay against real AWS; they also record that a customer PutEvents entry cannot forge a reserved `aws.*` source, so the service itself is the only producer.
Emulator: no event is produced at all. start_backup_job.rs writes only vault, job and recovery-point state; there is no emission to the EventBridge default bus anywhere in the job lifecycle. Consequently the fan-out check's receive loop (its own budget 220s) never observes a message on either the direct SQS target or the SNS-fronted queue and the check runs to the harness's 60s cap — reported as `Check timed out after 60.0s`. The same absence is visible in cause 1's check, whose event wait could never have been satisfied either.
Why: any client built on Backup eventing — notify-on-completion, a Lambda or queue consumer reacting to job state, fan-out to several subscribers from one rule — observes absolutely nothing and hangs or times out, with no error to distinguish "not delivered" from "not yet".
Fix: the backup job lifecycle's event-emission path (publish a state-change event onto the EventBridge default bus on every job state transition) plus the EventBridge rule-matching/target-dispatch path that must deliver one matched event to ALL of a rule's targets, SQS and SNS alike.
Traces: backup__events.backup-job-state-change-multiple-targets-fanout (primary), backup__events.backup-job-failed-state-change-event-detail (same missing emission, masked by cause 1).

=== Cause 3 of 3: EventBridge DisableRule is an unimplemented scaffold stub (HTTP 501 NotImplemented) ===
AWS: Smithy: DisableRule input `Name` (RuleName, @required, len 1..64, pattern ^[\.\-_A-Za-z0-9]+$) and `EventBusName` (EventBusNameOrArn, optional, defaults to the default bus); output Unit (HTTP 200, empty body); modeled errors ConcurrentModificationException (400), ManagedRuleException (400), ResourceNotFoundException (400), InternalException (500). Documented: "Disables the specified rule. A disabled rule won't match any events, and won't self-trigger if it has a schedule expression." So a successful call answers 200 with no body and DescribeRule subsequently reports State=DISABLED, and the rule stops delivering to its targets.
Emulator: the call is refused with HTTP 501 and error code `NotImplemented`, message "operation DisableRule is scaffolded but not implemented in this emulator" — services/service-com-amazonaws-eventbridge-awsevents/src/ops/disable_rule.rs still returns `Err(crate::scaffolded::Scaffolded::not_implemented("DisableRule"))` and carries the SCAFFOLDED marker. This is NOT one of the stubs the suite auto-filed (only backup CreateBackupPlan/GetBackupPlan were), and no open issue covers it — the nearest open issues (20261022T194351Z-events-firehose-s3-... for EventBridge TagResource, 20261023T140640Z-cloudfront-events-... for target delivery) name different operations.
Why: a client cannot pause a rule. There is no 501 in the modeled error set, so an SDK surfaces an unmodeled failure rather than a modeled one, and the whole disabled-rule semantics (a disabled rule must deliver nothing) is unreachable and untestable.
Fix: implement the handler in services/service-com-amazonaws-eventbridge-awsevents/src/ops/disable_rule.rs — flip the stored rule's State to DISABLED (ResourceNotFoundException for an unknown rule name/bus), return the empty 200 output, delete the SCAFFOLDED marker, and have the rule-matching path skip DISABLED rules.
Traces: backup__events.eventbridge-rule-disabled-does-not-deliver-backup-event.

## Evidence
Suite: backup__events 0/4 (0%).

Cause 1 — [mismatch] backup__events.backup-job-failed-state-change-event-detail:
  AssertionError: backup job ended in COMPLETED, expected FAILED
  (check polls DescribeBackupJob until State in (COMPLETED, FAILED, ABORTED) after StartBackupJob with a role that has the backup.amazonaws.com trust policy and zero permissions; it then requires detail.state == "FAILED" and a non-empty detail.statusMessage).
  Expected: terminal State == FAILED with a non-empty StatusMessage. Actual: State == COMPLETED, StatusMessage absent.

Cause 2 — [mismatch] backup__events.backup-job-state-change-multiple-targets-fanout:
  Error: Check timed out after 60.0s
  The check puts ONE rule (pattern {"source": ["aws.backup"], "detail-type": ["Backup Job State Change"]}, State=ENABLED) with two targets (an SQS queue and an SNS topic fronting a second SQS queue with RawMessageDelivery=true), starts a real backup job, then waits up to 220s for the same job's event on BOTH queues.
  Expected: both queues receive an envelope with source == "aws.backup", detail-type == "Backup Job State Change" and detail.backupJobId == the started job id. Actual: no message on either queue; the emulator publishes no backup state-change event (start_backup_job.rs has no emission path), so the receive loop never completes.

Cause 3 — [error] backup__events.eventbridge-rule-disabled-does-not-deliver-backup-event:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DisableRule operation: operation DisableRule is scaffolded but not implemented in this emulator
  Expected: HTTP 200, empty body, rule State becomes DISABLED. Actual: HTTP 501, error code NotImplemented (unmodeled).

## Steps to Reproduce
Cause 1:
1. iam.CreateRole with a trust policy allowing backup.amazonaws.com to sts:AssumeRole and NO permissions attached.
2. dynamodb.CreateTable (PAY_PER_REQUEST) and wait for ACTIVE; backup.CreateBackupVault.
3. backup.StartBackupJob(BackupVaultName=vault, ResourceArn=<table arn>, IamRoleArn=<permissionless role arn>).
4. Poll backup.DescribeBackupJob(BackupJobId=...) until State is terminal. Emulator: COMPLETED with no StatusMessage; AWS: FAILED with a non-empty StatusMessage.

Cause 2:
1. events.PutRule(Name=r, State=ENABLED, EventPattern={"source":["aws.backup"],"detail-type":["Backup Job State Change"]}).
2. Create an SQS queue (policy allowing events.amazonaws.com) and an SNS topic (policy allowing events.amazonaws.com sns:Publish) subscribed to a second SQS queue with RawMessageDelivery=true.
3. events.PutTargets(Rule=r, Targets=[{Id:"sqs-target",Arn:<queue arn>},{Id:"sns-target",Arn:<topic arn>}]).
4. backup.StartBackupJob against a DynamoDB table in a created vault, then ReceiveMessage on both queues. Emulator: nothing ever arrives on either queue.

Cause 3:
1. events.PutRule(Name=r, EventPattern=..., State=ENABLED).
2. events.DisableRule(Name=r) -> HTTP 501 NotImplemented instead of 200; events.DescribeRule never reports State=DISABLED.
