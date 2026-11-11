Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3.cdk-bootstrap-legacy-public-access-block-false-omits-the-block-and-tags-the-bucket cloudformation__s3.cdk-bootstrap-two-legacy-toolkit-stacks-in-one-environment-keep-separate-buckets-and-tags cloudformation__s3.cfn-s3-bucket-create-update-in-place-replace-retained-then-delete
Issue-Key: cloudformation__s3.cdk-bootstrap-legacy-public-access-block-false-omits-the-block-and-tags-the-bucket|_assert_legacy_toolkit#7|AssertionError|assert defaults == want_rules, (
Issue-Key: cloudformation__s3.cdk-bootstrap-two-legacy-toolkit-stacks-in-one-environment-keep-separate-buckets-and-tags|_assert_legacy_toolkit#11|AssertionError|assert defaults == want_rules, (
Issue-Key: cloudformation__s3.cfn-s3-bucket-create-update-in-place-replace-retained-then-delete|_update#30|AssertionError|assert stack["StackStatus"] == "UPDATE_COMPLETE", (
Issue-Commit: b26d594b442f382e51363fa18dcb67bd5cc86e1e

# cloudformation__s3: 2 net-new causes — the legacy CDK toolkit staging bucket reports AES256 plus an unsolicited BlockedEncryptionTypes instead of the template's aws:kms rule, and a retained bucket is deleted during a replacement so the update rolls back

> _Note: this issue was identified against the code base at commit `b26d594b442f382e51363fa18dcb67bd5cc86e1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketEncryption

## Description
Two net-new root causes in the cloudformation__s3 surface, grounded in three failing checks the replay report marks neither ALREADY FILED nor OWNED.

Cause 1 of 2: a legacy CDK toolkit stack's staging bucket serves a default-encryption rule the template never asked for.
AWS: Smithy (s3) GetBucketEncryption -> ServerSideEncryptionConfiguration.Rules[*]{ApplyServerSideEncryptionByDefault{SSEAlgorithm, KMSMasterKeyID}, BucketKeyEnabled} answers exactly the rule set the bucket was configured with - nothing is added. The legacy `cdk bootstrap` toolkit template configures the staging bucket with a single aws:kms rule, so the recording reads back [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'aws:kms'}, 'BucketKeyEnabled': False}].
Emulator: the same read answers [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'AES256'}, 'BucketKeyEnabled': False, 'BlockedEncryptionTypes': {'EncryptionType': ['SSE-C']}}] - the template's aws:kms algorithm is replaced by AES256 and an extra BlockedEncryptionTypes member is invented.
Why: a client that verifies its buckets are KMS-encrypted (compliance gates, `cdk doctor`-style checks, config rules) is told the bootstrap bucket is SSE-S3, and the unmodelled extra member breaks strict response comparisons and schema-validating clients.
Fix: the AWS::S3::Bucket BucketEncryption provisioner and the s3 GetBucketEncryption serializer - store and echo the template's rule set verbatim and emit no member the request never set.
Traces: cloudformation__s3.cdk-bootstrap-legacy-public-access-block-false-omits-the-block-and-tags-the-bucket, cloudformation__s3.cdk-bootstrap-two-legacy-toolkit-stacks-in-one-environment-keep-separate-buckets-and-tags. The same symptom is recorded for the MODERN bootstrap/toolkit buckets in the open issues 20261110T232635Z-cloudformation-ec2-ecr-iam-lambda-logs-s-cdk-bootstrap-staging-bucket-serves-aes256-default-encryption.md, 20261110T232635Z-cloudformation-ecr-iam-s3-sqs-ssm-sts-cdk-bootstrap-staging-bucket-reports-aes256-default-encryption.md and 20261110T232635Z-cloudformation-ecr-iam-s3-ssm-sts-toolkit-staging-bucket-reports-aes256-default-encryption-and.md; none of them carries these two legacy-toolkit check keys, so these recorded units are not re-replayed by them.

Cause 2 of 2: UpdateReplacePolicy Retain is ignored, so the replacement's cleanup tries to delete the retained bucket and the update rolls back.
AWS: a resource whose UpdateReplacePolicy is Retain is NOT deleted in the cleanup phase of a replacement: CloudFormation emits DELETE_SKIPPED (a modeled cloudformation#ResourceStatus value) carrying the OLD PhysicalResourceId, the old bucket and its objects survive, and the stack reaches UPDATE_COMPLETE.
Emulator: the cleanup phase calls DeleteBucket on the old, non-empty bucket; S3 answers 409 with 'The bucket you tried to delete is not empty. You must delete all versions in the bucket.' and CloudFormation turns a successful replacement into UPDATE_ROLLBACK_COMPLETE with stack reason 'The following resource(s) failed to delete: [<LogicalResourceId>]. ' and a resource reason wrapping that S3 error (HandlerErrorCode GeneralServiceException).
Why: the documented retention guarantee is inverted twice over - the emulator attempts the delete AWS must skip, and the attempt's failure fails the whole update, so a template change that AWS applies cleanly leaves the stack rolled back. A caller that relies on Retain to keep bucket contents across a replacement cannot update at all, and with an empty bucket the same path would silently destroy the data instead.
Fix: the CloudFormation update engine's cleanup phase - honour the resource's UpdateReplacePolicy (Retain/Snapshot/Delete) and emit DELETE_SKIPPED with the old physical id when it is Retain, instead of calling the owning service's delete.
Traces: cloudformation__s3.cfn-s3-bucket-create-update-in-place-replace-retained-then-delete. The same engine-level behaviour is recorded for AWS::SSM::Parameter in the open issue 20261110T232635Z-cloudformation-ssm-a-replacement-update-reports-create-delete-resource-events.md, whose replay set names only the ssm check.

## Evidence
Replay output (behavioral suite, section ### cloudformation__s3):

Cause 1 - [mismatch] cloudformation__s3.cdk-bootstrap-legacy-public-access-block-false-omits-the-block-and-tags-the-bucket and [mismatch] cloudformation__s3.cdk-bootstrap-two-legacy-toolkit-stacks-in-one-environment-keep-separate-buckets-and-tags, both:
  AssertionError: legacy staging bucket encryption rules are [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'AES256'}, 'BucketKeyEnabled': False, 'BlockedEncryptionTypes': {'EncryptionType': ['SSE-C']}}], expected [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'aws:kms'}, 'BucketKeyEnabled': False}]

Cause 2 - [mismatch] cloudformation__s3.cfn-s3-bucket-create-update-in-place-replace-retained-then-delete:
  AssertionError: update ended 'UPDATE_ROLLBACK_COMPLETE': 'The following resource(s) failed to delete: [Bucket]. '; [('Bucket', 'Resource handler returned message: "The bucket you tried to delete is not empty. You must delete all versions in the bucket. (Service: S3, Status Code: 409, Request ID: <id>, Extended Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <id>, HandlerErrorCode: GeneralServiceException)')]
Expected (AWS): StackStatus UPDATE_COMPLETE, a DELETE_SKIPPED event carrying the old bucket name and the old bucket still readable. The request ids, tokens and bucket names above are per-run values; the requirement is that no delete is attempted at all.

## Steps to Reproduce
Cause 1:
1. Build and boot the emulator from this checkout (cloudformation and s3 are bootstrapped and serving).
2. Deploy the LEGACY CDK toolkit stack (cdk bootstrap with the legacy template, publicAccessBlockConfiguration false).
3. Call s3 GetBucketEncryption on the staging bucket. Expect exactly [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'aws:kms'}, 'BucketKeyEnabled': False}]; observe SSEAlgorithm 'AES256' plus an unsolicited 'BlockedEncryptionTypes': {'EncryptionType': ['SSE-C']}.
4. Equivalently: re-replay cloudformation__s3.cdk-bootstrap-legacy-public-access-block-false-omits-the-block-and-tags-the-bucket and cloudformation__s3.cdk-bootstrap-two-legacy-toolkit-stacks-in-one-environment-keep-separate-buckets-and-tags.

Cause 2:
1. CreateStack with one AWS::S3::Bucket (pinned BucketName, versioning enabled) declaring "UpdateReplacePolicy": "Retain"; put an object into it.
2. UpdateStack with the same template but a different BucketName (create-only, so a replacement). Wait for a terminal status.
3. Expect UPDATE_COMPLETE, a DELETE_SKIPPED event for the old bucket name and the old bucket plus its object still present. Observe UPDATE_ROLLBACK_COMPLETE with 'The following resource(s) failed to delete: [Bucket]. ' and the S3 409 'The bucket you tried to delete is not empty...' wrapped as the resource reason.
4. Equivalently: re-replay cloudformation__s3.cfn-s3-bucket-create-update-in-place-replace-retained-then-delete.
