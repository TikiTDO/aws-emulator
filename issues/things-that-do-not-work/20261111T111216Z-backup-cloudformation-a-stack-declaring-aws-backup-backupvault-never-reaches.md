Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__cloudformation.cfn-backup-vault-create-update-in-place-replace-retained-then-delete
Issue-Key: backup__cloudformation.cfn-backup-vault-create-update-in-place-replace-retained-then-delete|run#10|AssertionError|assert created["StackStatus"] == "CREATE_COMPLETE", (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# A stack declaring AWS::Backup::BackupVault never reaches CREATE_COMPLETE — the vault resource fails to create

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: creating a backup vault through CloudFormation fails the resource and rolls the stack back.

AWS: a template with a single AWS::Backup::BackupVault (BackupVaultName plus optional tags/policy) provisions through Backup CreateBackupVault and the stack reaches CREATE_COMPLETE; DescribeBackupVault then answers for the vault name.

Emulator: the create fails. DescribeStacks reports StackStatus 'ROLLBACK_COMPLETE' with StatusReason 'The following resource(s) failed to create: [Vault]. ' — the fixed text is 'The following resource(s) failed to create: [' ... ']. ' and the variable part is the list of logical ids from the template. No vault exists afterwards.

Why: no CloudFormation-managed backup estate can be stood up at all, so nothing that depends on a vault (recovery points, backup plans, vault policies) is reachable; a caller sees ROLLBACK_COMPLETE where AWS returns CREATE_COMPLETE.

Fix: the AWS::Backup::BackupVault resource handler in the CloudFormation provisioner (and whatever Backup CreateBackupVault call it makes) — the create path must succeed and register the vault.

Traces: the failing check in trace_ids.

## Evidence
Expected: create ends 'CREATE_COMPLETE'.
Actual: AssertionError: create ended 'ROLLBACK_COMPLETE': 'The following resource(s) failed to create: [Vault]. '

## Steps to Reproduce
1. CreateStack a template whose only resource is an AWS::Backup::BackupVault with a BackupVaultName.
2. Poll DescribeStacks: the stack ends ROLLBACK_COMPLETE with reason 'The following resource(s) failed to create: [Vault]. '.
