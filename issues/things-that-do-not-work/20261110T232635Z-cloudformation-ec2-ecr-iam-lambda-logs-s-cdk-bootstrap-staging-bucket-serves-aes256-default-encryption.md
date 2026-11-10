Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-session-tags-unlock-lookup-deploy-and-asset-publishing
Issue-Key: cloudformation__ec2__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-session-tags-unlock-lookup-deploy-and-asset-publishing|_assert_modern_toolkit#26|AssertionError|assert defaults == _KMS_DEFAULT_RULES, (
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# cdk bootstrap staging bucket serves AES256 default encryption plus an unsolicited BlockedEncryptionTypes instead of the template's aws:kms rule

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketEncryption

## Description
One net-new root cause.

Cause 1 of 1: a bucket whose creating template declares BucketEncryption with SSEAlgorithm aws:kms reports SSE-S3 (AES256) on read, and the read adds a BlockedEncryptionTypes member that was never configured.

AWS: Smithy (s3) GetBucketEncryption -> ServerSideEncryptionConfiguration.Rules[*], where ServerSideEncryptionRule has ApplyServerSideEncryptionByDefault (ServerSideEncryptionByDefault.SSEAlgorithm is @required, enum ServerSideEncryption with AES256 | aws:kms | ...), BucketKeyEnabled, and BlockedEncryptionTypes. The model documents BlockedEncryptionTypes as "a bucket-level setting for Amazon S3 general purpose buckets used to prevent the upload of new objects encrypted with the specified server-side encryption type" and "currently, this parameter only supports blocking or unblocking server-side encryption with customer-provided keys (SSE-C)" - i.e. an opt-in bucket setting, so a bucket that never had it configured reports no such member. The CDK bootstrap toolkit template gives its staging bucket an SSE-KMS default with the AWS managed key and sets no S3 Bucket Key, so AWS answers exactly one rule: ApplyServerSideEncryptionByDefault {SSEAlgorithm: aws:kms}, BucketKeyEnabled false.

Emulator: GetBucketEncryption on the staging bucket answers one rule {ApplyServerSideEncryptionByDefault: {SSEAlgorithm: AES256}, BucketKeyEnabled: false, BlockedEncryptionTypes: {EncryptionType: [SSE-C]}} - the template's aws:kms default was never applied (the bucket reports the account-wide SSE-S3 default), and a blocking setting nobody asked for is reported.

Why: a client that reads back the encryption of a bucket it just created from a template cannot tell SSE-KMS from SSE-S3, so any compliance check, drift detector or key-policy audit over a CloudFormation/CDK-created bucket reads the wrong algorithm; and a client that round-trips the Rules it is given (Get -> Put) will now assert an SSE-C block the bucket never had.

Fix: the CloudFormation AWS::S3::Bucket resource handler's BucketEncryption property mapping onto S3's bucket-encryption state, and S3's GetBucketEncryption serializer, which must omit BlockedEncryptionTypes unless a blocking setting was configured.

A similar open issue exists for a different service heading - 20261109T220008Z-cloudformation-dynamodb-ecr-iam-s3-ssm-the-cdk.md ("the CDK bootstrap staging bucket is created with SSE-S3 (AES256) defaults instead of the aws:kms default") and 20261109T164957Z-cloudformation-kms-s3-a-bucket-created-by-a.md - neither carries this check's replay key.

Traces: the failing check below.

## Evidence
Check cloudformation__ec2__ecr__iam__lambda__logs__s3__ssm.cdk-bootstrap-session-tags-unlock-lookup-deploy-and-asset-publishing

AssertionError: staging bucket encryption rules are [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'AES256'}, 'BucketKeyEnabled': False, 'BlockedEncryptionTypes': {'EncryptionType': ['SSE-C']}}], expected [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'aws:kms'}, 'BucketKeyEnabled': False}]

Expected (AWS): exactly one rule, SSEAlgorithm 'aws:kms', BucketKeyEnabled false, no BlockedEncryptionTypes member.
Actual (emulator): SSEAlgorithm 'AES256' and BlockedEncryptionTypes {'EncryptionType': ['SSE-C']}.

## Steps to Reproduce
1. `cdk bootstrap` a toolkit stack (the modern template) against the emulator, or create a stack whose AWS::S3::Bucket declares BucketEncryption with ServerSideEncryptionByDefault.SSEAlgorithm = aws:kms.
2. Call s3:GetBucketEncryption on the resulting staging bucket.
3. Observe SSEAlgorithm 'AES256' and an extra BlockedEncryptionTypes {'EncryptionType': ['SSE-C']} member instead of the template's single aws:kms rule.
