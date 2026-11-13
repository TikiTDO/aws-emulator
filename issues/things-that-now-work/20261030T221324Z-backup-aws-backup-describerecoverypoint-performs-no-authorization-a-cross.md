Issue-Source: trace-sweep-divergence
Issue-Trace: backup-recovery-point-describe-and-metadata-not-found
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-recovery-point-describe-and-metadata-not-found backup-recovery-point-error-paths-on-live-vault
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|DescribeRecoveryPoint|4xx->4xx|$.Message
Issue-Key: backup|DescribeRecoveryPoint|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|DescribeRecoveryPoint|4xx->4xx|status
Issue-Commit: 468023723f0688d7e48e7cd568bb3e02439c9d0a

# AWS Backup DescribeRecoveryPoint performs no authorization: a cross-account recovery point is refused 400 ResourceNotFoundException instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `468023723f0688d7e48e7cd568bb3e02439c9d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRecoveryPoint

## Description
Cause 1 of 1: DescribeRecoveryPoint on a recovery-point ARN owned by an account other than the caller's is misclassified as not-found instead of access-denied.

AWS: Smithy (com.amazonaws.backup#CryoControllerUserManager): DescribeRecoveryPoint is GET /backup-vaults/{BackupVaultName}/recovery-points/{RecoveryPointArn}, with BackupVaultName (httpLabel, required, @pattern ^[a-zA-Z0-9\-\_]{2,50}$), RecoveryPointArn (httpLabel, required, ARN) and the optional BackupVaultAccountId (httpQuery backupVaultAccountId, @pattern ^[0-9]{12}$); its modeled errors are InvalidParameterValueException, MissingParameterValueException and ResourceNotFoundException (all client, HTTP 400) plus ServiceUnavailableException (500). AccessDeniedException is NOT in that modeled set: it is recorded behavior only, produced by the authorization layer that runs AHEAD of any resource lookup. The recording shows HTTP 403 with header x-amzn-errortype: AccessDeniedException and the standard AWS authorization sentence, whose template is: "User: <callerPrincipalArn> is not authorized to perform: backup:DescribeRecoveryPoint on resource: <recoveryPointArn> because no resource-based policy allows the backup:DescribeRecoveryPoint action". The FIXED text of that message is "User: ", " is not authorized to perform: ", " on resource: ", " because no resource-based policy allows the " and " action"; the VARIABLE parts are the caller's principal ARN (from the signing credentials), the action name (service prefix plus the operation, i.e. backup:DescribeRecoveryPoint) and the RecoveryPointArn taken verbatim from the request path. Because AWS authorizes before it resolves the resource, a recovery point in an account the caller cannot reach never leaks its existence (or absence) as a not-found. Note the recorded principal ARN and the foreign account id / recovery-point uuid in the message are only the template's variable parts filled in by that run; they are not requirements.

Emulator: the request goes straight to a recovery-point state lookup, which misses, so the emulator answers HTTP 400 with x-amzn-errortype: ResourceNotFoundException and $.Message 'Cannot find recovery point'. Any ARN the local state does not hold - including one whose account differs from the caller's - collapses onto that single answer; both the live-vault error path (backup-recovery-point-error-paths-on-live-vault#task#4) and the describe/metadata not-found path (backup-recovery-point-describe-and-metadata-not-found#task#3) produce it.

Why: the status CLASS is wrong (400 client-not-found vs 403 access-denied) and the error code is wrong, so a client that branches on AccessDenied - to re-assume a role, to surface a permissions problem to the operator, or to stop retrying - instead concludes the recovery point does not exist and continues as though it were already expired or deleted. A cross-account verification step that AWS refuses reads as a confirmed absence. Clients and auditors that assert on AWS's authorization sentence (principal, action name, resource) get none of it, and an SDK sees a modeled 400 where AWS returned an unmodeled-but-standard 403.

Fix: DescribeRecoveryPoint's handler in the backup crate (services/service-com-amazonaws-backup-cryocontrollerusermanager) - insert the authorization/ownership check ahead of the recovery-point state lookup: when the RecoveryPointArn's (or BackupVaultAccountId's) account is not the caller's account, render AccessDeniedException with HTTP 403, the x-amzn-errortype response header and the message template above, built from the caller principal, the operation's action name and the request's RecoveryPointArn; only reach the state lookup (and its existing ResourceNotFoundException 'Cannot find recovery point') for ARNs in the caller's own account. The sibling divergence on DeleteRecoveryPoint is already filed as 20261030T023527Z-backup-aws-backup-deleterecoverypoint-performs-no-authorization-a-cross.md and needs the same check; the fix belongs in the shared authorization path both handlers call. Open issue 20261030T020405Z-backup-aws-backup-framework-and-recovery-point-reads-are.md records only that DescribeRecoveryPoint was an unimplemented 501 stub, and 20261029T175130Z-backup-aws-backup-recovery-point-restore-job-mpa-and.md likewise covers 501 scaffolds; this operation now DOES answer, and answers with the wrong status and error class, so neither covers this divergence.

Traces: backup-recovery-point-describe-and-metadata-not-found#task#3, backup-recovery-point-error-paths-on-live-vault#task#4 (same cluster).

## Evidence
Divergence cluster service=backup|DescribeRecoveryPoint|status=403->400|keys=$.Message,$headers.x-amzn-errortype,status (verdict: mismatch, affected_count 2).

Expected (recording): HTTP 403; $headers.x-amzn-errortype = 'AccessDeniedException'; $.Message = 'User: <callerPrincipalArn> is not authorized to perform: backup:DescribeRecoveryPoint on resource: <recoveryPointArn> because no resource-based policy allows the backup:DescribeRecoveryPoint action'.
Actual (emulator): HTTP 400; $headers.x-amzn-errortype = 'ResourceNotFoundException'; $.Message = 'Cannot find recovery point'.

Replay detail, verbatim: "status: expected 403, got 400 / $.Message: value_mismatch (expected 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: backup:DescribeRecoveryPoint on resource: arn:aws:backup:us-west-2:123456789012:recovery-point:fb698c18-a985-4704-b4fb-0c14fa8cfaa3 because no resource-based policy allows the backup:DescribeRecoveryPoint action', got 'Cannot find recovery point') / $headers.x-amzn-errortype: value_mismatch (expected 'AccessDeniedException', got 'ResourceNotFoundException')".

In that recorded message the fixed text is "User: ... is not authorized to perform: ... on resource: ... because no resource-based policy allows the ... action"; the principal ARN (arn:aws:iam::000000000000:user/emulator), the foreign account id (123456789012) and the recovery-point uuid are values taken from the request and the caller's credentials, not literal requirements.

Affected steps: backup-recovery-point-describe-and-metadata-not-found#task#3, backup-recovery-point-error-paths-on-live-vault#task#4. This cluster carries no stub_path and no owned_by, so it is net-new.

## Steps to Reproduce
1. Replay fixture backup-recovery-point-describe-and-metadata-not-found, step task#3: call backup.DescribeRecoveryPoint(BackupVaultName=<an existing vault name>, RecoveryPointArn=arn:aws:backup:<region>:<an account id that is NOT the caller's>:recovery-point:<uuid>).
2. Observe HTTP 400 with x-amzn-errortype: ResourceNotFoundException and body $.Message = 'Cannot find recovery point'.
3. The recording answers HTTP 403 with x-amzn-errortype: AccessDeniedException and $.Message = 'User: <callerPrincipalArn> is not authorized to perform: backup:DescribeRecoveryPoint on resource: <the RecoveryPointArn from the request> because no resource-based policy allows the backup:DescribeRecoveryPoint action'.
4. The same divergence reproduces via fixture backup-recovery-point-error-paths-on-live-vault, step task#4 (cross-account ARN against a live vault).
5. Control: DescribeRecoveryPoint for an ARN in the caller's OWN account that does not exist must still answer 400 ResourceNotFoundException 'Cannot find recovery point'.
