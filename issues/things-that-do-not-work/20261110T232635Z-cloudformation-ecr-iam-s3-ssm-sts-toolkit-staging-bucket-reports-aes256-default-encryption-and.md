Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-add-tags-propagate-to-toolkit-resources-and-survive-a-tagless-re-bootstrap cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-customized-vendor-template-does-not-overwrite-the-default-toolkit cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-dumped-template-with-an-added-output-deploys-as-the-toolkit cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-re-bootstrap-after-stack-deletion-imports-the-retained-staging-bucket cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-re-bootstrap-with-a-new-tag-value-updates-every-toolkit-resource-in-place
Issue-Key: cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-add-tags-propagate-to-toolkit-resources-and-survive-a-tagless-re-bootstrap|_assert_modern_toolkit#10|AssertionError|assert defaults == _KMS_DEFAULT_RULES, (
Issue-Key: cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-customized-vendor-template-does-not-overwrite-the-default-toolkit|_assert_modern_toolkit#28|AssertionError|assert defaults == _KMS_DEFAULT_RULES, (
Issue-Key: cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-dumped-template-with-an-added-output-deploys-as-the-toolkit|_assert_modern_toolkit#19|AssertionError|assert defaults == _KMS_DEFAULT_RULES, (
Issue-Key: cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-re-bootstrap-after-stack-deletion-imports-the-retained-staging-bucket|_assert_modern_toolkit#15|AssertionError|assert defaults == _KMS_DEFAULT_RULES, (
Issue-Key: cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-re-bootstrap-with-a-new-tag-value-updates-every-toolkit-resource-in-place|_assert_modern_toolkit#12|AssertionError|assert defaults == _KMS_DEFAULT_RULES, (
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# Toolkit staging bucket reports AES256 default encryption and an unsolicited BlockedEncryptionTypes instead of the template's aws:kms rule

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketEncryption

## Description
One net-new root cause, hit by every toolkit read in this group.

Cause 1 of 1: a bucket whose creating template declares BucketEncryption with SSEAlgorithm aws:kms reports SSE-S3 (AES256) on read, and the read adds a BlockedEncryptionTypes member that was never configured.

AWS: Smithy (s3) GetBucketEncryption -> ServerSideEncryptionConfiguration.Rules[*]. ServerSideEncryptionRule carries ApplyServerSideEncryptionByDefault (ServerSideEncryptionByDefault.SSEAlgorithm is @required, enum ServerSideEncryption: AES256 | aws:kms | ...), BucketKeyEnabled, and BlockedEncryptionTypes, which the model documents as an opt-in bucket-level setting that "currently only supports blocking or unblocking server-side encryption with customer-provided keys (SSE-C)" - a bucket that never configured it reports no such member. The CDK toolkit template's staging bucket takes the SSE-KMS default with the AWS managed key and sets no S3 Bucket Key, so AWS answers exactly one rule: {ApplyServerSideEncryptionByDefault: {SSEAlgorithm: aws:kms}, BucketKeyEnabled: false} - and the same rule survives re-bootstrap, tag updates, a customized vendor template, a dumped template and re-import of a retained bucket.

Emulator: every one of those paths reads back {ApplyServerSideEncryptionByDefault: {SSEAlgorithm: AES256}, BucketKeyEnabled: false, BlockedEncryptionTypes: {EncryptionType: [SSE-C]}} - the template's aws:kms default is never applied and an SSE-C block nobody configured is reported.

Why: a client that reads back the encryption of a bucket it created from a template cannot distinguish SSE-KMS from SSE-S3, so compliance checks, key audits and drift detection over CloudFormation/CDK-created buckets all read the wrong algorithm; a client that round-trips the returned Rules (Get -> Put) additionally asserts an SSE-C block the bucket never had.

Fix: the CloudFormation AWS::S3::Bucket resource handler's BucketEncryption property mapping onto S3's bucket-encryption state, and S3's GetBucketEncryption serializer (omit BlockedEncryptionTypes unless a blocking setting was configured).

Similar open issues under other service headings: 20261109T220008Z-cloudformation-dynamodb-ecr-iam-s3-ssm-the-cdk.md and 20261109T164957Z-cloudformation-kms-s3-a-bucket-created-by-a.md - neither carries these checks' replay keys.

Traces: the five failing checks below.

## Evidence
All five checks fail on the same read:
AssertionError: staging bucket encryption rules are [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'AES256'}, 'BucketKeyEnabled': False, 'BlockedEncryptionTypes': {'EncryptionType': ['SSE-C']}}], expected [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'aws:kms'}, 'BucketKeyEnabled': False}]

Checks: cdk-bootstrap-add-tags-propagate-to-toolkit-resources-and-survive-a-tagless-re-bootstrap, cdk-bootstrap-customized-vendor-template-does-not-overwrite-the-default-toolkit, cdk-bootstrap-dumped-template-with-an-added-output-deploys-as-the-toolkit, cdk-bootstrap-re-bootstrap-after-stack-deletion-imports-the-retained-staging-bucket, cdk-bootstrap-re-bootstrap-with-a-new-tag-value-updates-every-toolkit-resource-in-place.

## Steps to Reproduce
1. `cdk bootstrap` a toolkit stack against the emulator (any of: with tags, re-bootstrapped, from a customized/dumped template, or re-bootstrapped after deleting the stack so the retained bucket is imported).
2. Call s3:GetBucketEncryption on the staging bucket.
3. Observe SSEAlgorithm 'AES256' plus BlockedEncryptionTypes {'EncryptionType': ['SSE-C']} instead of the single aws:kms rule the template declares.
