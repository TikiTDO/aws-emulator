Issue-Source: trace-sweep-divergence
Issue-Trace: backup-recovery-point-error-paths-on-live-vault
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-recovery-point-error-paths-on-live-vault backup-recovery-point-mutation-not-found
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|DeleteRecoveryPoint|4xx->4xx|$.Message
Issue-Commit: 914d2c73e73c203555b0bde73b4090e0da9a0d25

# AWS Backup DeleteRecoveryPoint access-denied body returns a generic message instead of the AWS authorization sentence

> _Note: this issue was identified against the code base at commit `914d2c73e73c203555b0bde73b4090e0da9a0d25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRecoveryPoint

## Description
Cause 1 of 1: DeleteRecoveryPoint on a recovery-point ARN owned by an account other than the caller's is now correctly refused with HTTP 403 / AccessDeniedException, but the body's $.Message is a generic emulator sentence rather than AWS's authorization message.

AWS: Smithy (com.amazonaws.backup#CryoControllerUserManager): DeleteRecoveryPoint is DELETE /backup-vaults/{BackupVaultName}/recovery-points/{RecoveryPointArn}, BackupVaultName (httpLabel, required, @pattern ^[a-zA-Z0-9\-\_]{2,50}$) and RecoveryPointArn (httpLabel, required, ARN); modeled errors are InvalidParameterValueException, InvalidRequestException, InvalidResourceStateException, MissingParameterValueException, ResourceNotFoundException (all client, HTTP 400) and ServiceUnavailableException (500). AccessDeniedException is NOT in that modeled set, so for its body this is recorded behavior only: the recording answers HTTP 403 with the standard AWS authorization sentence, whose template is "User: <callerPrincipalArn> is not authorized to perform: backup:DeleteRecoveryPoint on resource: <recoveryPointArn> because no resource-based policy allows the backup:DeleteRecoveryPoint action". The FIXED text of that message is "User: ", " is not authorized to perform: ", " on resource: ", " because no resource-based policy allows the " and " action". The VARIABLE parts are the caller's principal ARN (from the signing credentials), the action name (the service prefix plus the operation, i.e. backup:DeleteRecoveryPoint) and the RecoveryPointArn taken verbatim from the request path. The principal ARN, the foreign account id and the recovery-point uuid seen in any one recording are only that run's fill-ins for the template, not requirements.

Emulator: the authorization refusal itself is now right (HTTP 403, same status class as the recording), but the body carries the fixed literal $.Message 'Insufficient privileges to perform this action.' with no principal, no action name and no resource ARN. The same single sentence is served for every unauthorized DeleteRecoveryPoint, whatever the caller, action or target.

Why: a client or auditor that parses AWS's authorization sentence to learn WHICH principal lacked WHICH action on WHICH resource gets nothing it can act on: it cannot name the missing IAM permission (backup:DeleteRecoveryPoint), cannot identify the recovery point it was refused on, and cannot distinguish a resource-policy refusal from any other privilege failure. Log scrapers, permission-advisory tooling and tests that assert on the AWS wording fail against the emulator even though the status and error code now agree.

Fix: belongs in the shared authorization/refusal path the backup recovery-point handlers call (service crate services/service-com-amazonaws-backup-cryocontrollerusermanager, DeleteRecoveryPoint's handler and the AccessDeniedException rendering it delegates to): render the message from the template above, substituting the caller principal ARN from the request's credentials, the action name derived from the operation (backup:DeleteRecoveryPoint) and the RecoveryPointArn from the request path, instead of emitting a fixed generic sentence. Related issues 20261030T023527Z-backup-aws-backup-deleterecoverypoint-performs-no-authorization-a-cross.md and 20261030T221324Z-backup-aws-backup-describerecoverypoint-performs-no-authorization-a-cross.md are both about the STATUS/error-class (400 ResourceNotFoundException instead of 403 AccessDeniedException); the first is already resolved and the second is open and owns the DescribeRecoveryPoint message cluster, so neither covers this remaining DeleteRecoveryPoint message-body divergence — the replay reports it with no owner.

Traces: backup-recovery-point-error-paths-on-live-vault#task#9, backup-recovery-point-mutation-not-found#task#3 (same cluster).

## Evidence
Divergence cluster service=backup|DeleteRecoveryPoint|status=403->403|keys=$.Message (verdict: mismatch, affected_count 2, stub_path empty, owned_by empty — net-new).

Expected (recording): HTTP 403, $.Message = 'User: <callerPrincipalArn> is not authorized to perform: backup:DeleteRecoveryPoint on resource: <recoveryPointArn> because no resource-based policy allows the backup:DeleteRecoveryPoint action'.
Actual (emulator): HTTP 403, $.Message = 'Insufficient privileges to perform this action.'

Replay detail, verbatim: "$.Message: value_mismatch (expected 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: backup:DeleteRecoveryPoint on resource: arn:aws:backup:us-west-2:111111111111:recovery-point:00000000-0000-0000-0000-000000000000 because no resource-based policy allows the backup:DeleteRecoveryPoint action', got 'Insufficient privileges to perform this action.')".

In that recorded message the fixed text is "User: ... is not authorized to perform: ... on resource: ... because no resource-based policy allows the ... action"; the principal ARN (arn:aws:iam::000000000000:user/emulator), the foreign account id (111111111111) and the recovery-point uuid (00000000-0000-0000-0000-000000000000) are values taken from the caller's credentials and the request path, not literal requirements. The action name 'backup:DeleteRecoveryPoint' is derived from the operation and IS required.

Affected steps: backup-recovery-point-error-paths-on-live-vault#task#9, backup-recovery-point-mutation-not-found#task#3.

## Steps to Reproduce
1. Replay fixture backup-recovery-point-error-paths-on-live-vault, step task#9: call backup.DeleteRecoveryPoint(BackupVaultName=<an existing vault name>, RecoveryPointArn=arn:aws:backup:<region>:<an account id that is NOT the caller's>:recovery-point:<uuid>).
2. Observe HTTP 403 (correct) with body $.Message = 'Insufficient privileges to perform this action.'
3. The recording answers HTTP 403 with $.Message = 'User: <the caller's principal ARN> is not authorized to perform: backup:DeleteRecoveryPoint on resource: <the RecoveryPointArn from the request> because no resource-based policy allows the backup:DeleteRecoveryPoint action'.
4. The same divergence reproduces via fixture backup-recovery-point-mutation-not-found, step task#3.
5. Control: DeleteRecoveryPoint for an ARN in the caller's OWN account that does not exist must still answer 400 ResourceNotFoundException, not this 403.
