Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecr__iam__s3__sqs__ssm__sts.cdk-bootstrap-deploy-without-execution-role-runs-as-the-session-tagged-deploy-role
Issue-Key: cloudformation__ecr__iam__s3__sqs__ssm__sts.cdk-bootstrap-deploy-without-execution-role-runs-as-the-session-tagged-deploy-role|_assert_modern_toolkit#20|AssertionError|assert defaults == _KMS_DEFAULT_RULES, (
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# cdk bootstrap staging bucket reports AES256 default encryption and an unsolicited BlockedEncryptionTypes instead of the template's aws:kms rule

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketEncryption

## Description
One net-new root cause.

Cause 1 of 1: a bucket whose creating template declares BucketEncryption with SSEAlgorithm aws:kms reports SSE-S3 (AES256) on read, and the read adds a BlockedEncryptionTypes member that was never configured.

AWS: Smithy (s3) GetBucketEncryption -> ServerSideEncryptionConfiguration.Rules[*]. ServerSideEncryptionRule carries ApplyServerSideEncryptionByDefault (ServerSideEncryptionByDefault.SSEAlgorithm is @required, enum ServerSideEncryption: AES256 | aws:kms | ...), BucketKeyEnabled and BlockedEncryptionTypes; the model documents BlockedEncryptionTypes as an opt-in bucket-level setting that "currently only supports blocking or unblocking server-side encryption with customer-provided keys (SSE-C)", so a bucket that never configured it reports no such member. The CDK toolkit template's staging bucket uses the SSE-KMS default with the AWS managed key and no S3 Bucket Key, so AWS answers exactly one rule: {ApplyServerSideEncryptionByDefault: {SSEAlgorithm: aws:kms}, BucketKeyEnabled: false}.

Emulator: the read answers {ApplyServerSideEncryptionByDefault: {SSEAlgorithm: AES256}, BucketKeyEnabled: false, BlockedEncryptionTypes: {EncryptionType: [SSE-C]}} - the template's aws:kms default was never applied, and a blocking setting nobody configured is reported.

Why: a client that reads back the encryption of a bucket it created from a template cannot distinguish SSE-KMS from SSE-S3, so compliance and drift checks over CloudFormation/CDK-created buckets read the wrong algorithm; a Get -> Put round trip of the returned Rules would additionally assert an SSE-C block the bucket never had.

Fix: the CloudFormation AWS::S3::Bucket resource handler's BucketEncryption property mapping onto S3's bucket-encryption state, and S3's GetBucketEncryption serializer (omit BlockedEncryptionTypes unless a blocking setting was configured).

Similar open issues under other service headings: 20261109T220008Z-cloudformation-dynamodb-ecr-iam-s3-ssm-the-cdk.md and 20261109T164957Z-cloudformation-kms-s3-a-bucket-created-by-a.md - neither carries this check's replay key.

Traces: the failing check below.

## Evidence
Check cloudformation__ecr__iam__s3__sqs__ssm__sts.cdk-bootstrap-deploy-without-execution-role-runs-as-the-session-tagged-deploy-role

AssertionError: staging bucket encryption rules are [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'AES256'}, 'BucketKeyEnabled': False, 'BlockedEncryptionTypes': {'EncryptionType': ['SSE-C']}}], expected [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'aws:kms'}, 'BucketKeyEnabled': False}]

## Steps to Reproduce
1. `cdk bootstrap` a toolkit stack against the emulator (or create any stack whose AWS::S3::Bucket declares BucketEncryption with SSEAlgorithm aws:kms).
2. Call s3:GetBucketEncryption on the staging bucket.
3. Observe SSEAlgorithm 'AES256' plus BlockedEncryptionTypes {'EncryptionType': ['SSE-C']} instead of the single aws:kms rule.
