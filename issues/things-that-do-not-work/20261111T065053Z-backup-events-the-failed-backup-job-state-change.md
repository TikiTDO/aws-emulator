Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__events.backup-job-failed-event-matches-pattern
Issue-Key: backup__events.backup-job-failed-event-matches-pattern|_collect_messages#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=remaining)
Issue-Commit: d0a04fabcd83cf2b721ac740047fd92b0e777ff8

# backup__events: the FAILED 'Backup Job State Change' event for a permission-less StartBackupJob never reaches the rule's SQS target, so the detail.state=FAILED pattern check exhausts its budget

> _Note: this issue was identified against the code base at commit `d0a04fabcd83cf2b721ac740047fd92b0e777ff8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBackupJob

## Description
Cause 1 of 1: a backup job started with a role that trusts backup.amazonaws.com but carries NO permissions policy never produces an observable FAILED "Backup Job State Change" delivery on the EventBridge default bus, so a rule whose pattern is scoped to detail.state=FAILED receives nothing and the check runs out its budget instead of reaching any of its assertions. Failing check: backup__events.backup-job-failed-event-matches-pattern.

AWS: recorded behavior only (the EventBridge envelope is not in either service's Smithy model; Smithy for backup StartBackupJob models only InvalidParameterValueException / InvalidRequestException / LimitExceededException / MissingParameterValueException / ResourceNotFoundException (400) and ServiceUnavailableException (500), and BackupJobState includes FAILED). AWS accepts the StartBackupJob call, the job then fails asynchronously because the named role cannot perform dynamodb:CreateBackup, DescribeBackupJob reaches State=FAILED with a non-empty StatusMessage, and that transition is published on the default bus as source="aws.backup", detail-type="Backup Job State Change", detail.state="FAILED", detail.backupJobId=<the started job id>, alongside the vault/resource members; an ENABLED rule matching {"source":["aws.backup"],"detail-type":["Backup Job State Change"],"detail":{"state":["FAILED"]}} delivers that envelope unchanged to its SQS target, and delivers none of the same job's non-FAILED transitions.

Emulator: the check is reported as `Error: Check timed out after 60.0s`; no envelope for the job ever arrives on the target queue, so the assertions that the FAILED-state rule delivered the job's event, and that every delivered message carries detail.state=FAILED / detail-type="Backup Job State Change" / source="aws.backup", are never reached. The backup crate does now decide a withheld-denial outcome and call crate::state::publish_job_state_changes / drive_job_clock in services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/start_backup_job.rs, so the divergence is in the FAILED transition actually becoming a delivered aws.backup envelope within the job's settle time — Hypothesis: either the job does not reach the FAILED terminal state in time or the state-change publication/EventBridge dispatch does not carry it to the rule's SQS target. The determinable fact from the replay is that the detail.state=FAILED delivery never happens.

Why: a client that reacts to failed backups — alerting, remediation, a queue consumer branching on detail.state — observes nothing at all and cannot distinguish "no failure" from "not delivered". The FAILED half of the Backup eventing contract is unusable, and a negative test on it passes vacuously because nothing is ever delivered.

Fix: the backup job lifecycle's state-change emission path (the FAILED terminal transition for a job whose role cannot perform the backup must publish an aws.backup "Backup Job State Change" envelope with detail.state=FAILED on the EventBridge default bus) plus the EventBridge rule-matching/target-dispatch leg that must deliver that matched envelope to the rule's SQS target.

Related open issues, named for the fixer: 20261029T175130Z-backup-events-no-backup-copy-restore-recovery-point.md lists this check id and a replay key for it, but that key is the earlier `run#11 AssertionError: assert final["State"] == "FAILED"` shape (the job settling COMPLETED), which is NOT how the check fails now — the suite no longer attributes this failure to that issue, so it does not gate it. 20261107T205257Z-backup-events-4-net-new-causes-backup-emits.md and 20261110T013454Z-backup-aws-backup-2-net-new-causes-a-backup.md describe neighbouring emission/terminal-state gaps and likewise do not carry this check's current key.

Traces: backup__events.backup-job-failed-event-matches-pattern.

## Evidence
Expected (AWS): StartBackupJob with a trust-only, permission-less role is accepted; DescribeBackupJob reaches State=FAILED with a StatusMessage; the rule scoped to {"source":["aws.backup"],"detail-type":["Backup Job State Change"],"detail":{"state":["FAILED"]}} delivers at least one message whose body has source=="aws.backup", detail-type=="Backup Job State Change", detail.backupJobId==<started job id> and detail.state=="FAILED", and delivers no non-FAILED transition of that job.

Actual ([mismatch] backup__events.backup-job-failed-event-matches-pattern):
  Error: Check timed out after 60.0s
The check's own flow is: dynamodb.CreateTable -> backup.CreateBackupVault -> iam.CreateRole (trust backup.amazonaws.com, no policy) -> events.PutRule(detail.state=FAILED) + SQS target with an events.amazonaws.com SendMessage policy scoped by aws:SourceArn -> backup.StartBackupJob -> poll DescribeBackupJob to a terminal state -> collect queue messages. It never produces the FAILED-state delivery, so it reports the timeout rather than reaching the assertion "the FAILED-state-only rule never delivered the failed job's event".

The job id, table/vault/queue names and the harness budget are per-run values; the requirement is the FAILED envelope and its delivery.

## Steps to Reproduce
1. dynamodb.CreateTable (PAY_PER_REQUEST) and wait for ACTIVE; backup.CreateBackupVault.
2. iam.CreateRole with a trust policy allowing backup.amazonaws.com sts:AssumeRole and NO permissions policy attached.
3. events.PutRule(State=ENABLED, EventPattern={"source":["aws.backup"],"detail-type":["Backup Job State Change"],"detail":{"state":["FAILED"]}}); create an SQS queue, set a queue policy allowing events.amazonaws.com sqs:SendMessage with Condition ArnEquals aws:SourceArn=<rule arn>; events.PutTargets with the queue ARN.
4. backup.StartBackupJob(BackupVaultName=<vault>, ResourceArn=<table arn>, IamRoleArn=<powerless role arn>, IdempotencyToken=<uuid>).
5. Poll backup.DescribeBackupJob until a terminal state -> expect FAILED with a StatusMessage.
6. sqs.ReceiveMessage in a loop -> AWS yields the aws.backup "Backup Job State Change" envelope with detail.state="FAILED" for that backupJobId; the emulator yields nothing and the check times out.
7. Replay backup__events.backup-job-failed-event-matches-pattern.
