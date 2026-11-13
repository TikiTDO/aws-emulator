Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__rds.xsvc-rds-2-disabled-cmk-rejected-for-encrypted-create-and-snapshot-copy
Issue-Key: kms__rds.xsvc-rds-2-disabled-cmk-rejected-for-encrypted-create-and-snapshot-copy|run#29|AssertionError|assert code == _KMS_FAULT, (
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# RDS accepts a DISABLED KMS CMK for encrypted creates and snapshot copies instead of returning KMSKeyNotAccessibleFault

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
One cause. AWS: Smithy AmazonRDSv19 models KMSKeyNotAccessibleFault on CreateDBInstance ('An error occurred accessing an Amazon Web Services KMS key', errorKind client, httpError 400, awsQueryError code KMSKeyNotAccessibleFault) and on CopyDBSnapshot/CopyDBClusterSnapshot. A customer managed key whose KeyState is Disabled cannot be used for a cryptographic operation, so RDS refuses CreateDBInstance(StorageEncrypted=true, KmsKeyId=<disabled CMK>) with HTTP 400 and Error.Code KMSKeyNotAccessibleFault before any instance is created, and refuses an encrypted snapshot copy under the same key the same way.

Emulator: the call SUCCEEDS with HTTP 200 and returns a DBInstance in status 'creating' with StorageEncrypted=true and KmsKeyId set to the disabled key's ARN. No KMS key-state check is performed at all — the key id is recorded verbatim.

Why: a client gets a database it believes is encrypted under a usable key, while the key it names cannot perform cryptographic operations; the whole point of the guard is to fail the create rather than leave an unusable encrypted resource. Error handling keyed on KMSKeyNotAccessibleFault (disable-the-key break-glass drills, key-rotation tooling, compliance checks) never fires, and the emulator will also accept snapshot copies under keys AWS refuses.

Fix: the CreateDBInstance handler's KMS key resolution (and the same resolution in CopyDBSnapshot / CopyDBClusterSnapshot) must look the key up through the KMS service state, check its key state, and raise the modeled KMSKeyNotAccessibleFault at 400 when the key is not enabled.

Traces: kms__rds.xsvc-rds-2-disabled-cmk-rejected-for-encrypted-create-and-snapshot-copy.

## Evidence
AssertionError: CreateDBInstance(StorageEncrypted=True) under a DISABLED CMK must be refused with KMSKeyNotAccessibleFault; got code='<call succeeded>' status=200 message='' result={'DBInstance': {'DBInstanceIdentifier': 'sim2real-xsvc-enc-...', 'DBInstanceStatus': 'creating', ..., 'StorageEncrypted': True, 'KmsKeyId': 'arn:aws:kms:us-west-2:...:key/4fe7ca65-4629-4f32-b84c-d8e15d58da92', ...}}. Expected HTTP 400 with awsQuery Error.Code KMSKeyNotAccessibleFault.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. kms: CreateKey, then DisableKey on it.
3. rds: CreateDBSubnetGroup over two subnets, then Action=CreateDBInstance with StorageEncrypted=true and KmsKeyId=<the disabled key's ARN>.
4. The call returns 200 with a DBInstance instead of 400 / KMSKeyNotAccessibleFault; the same holds for Action=CopyDBSnapshot with that KmsKeyId.
