Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__sts.cfn-s3-bucket-policy-create-update-in-place-replace-retained-then-delete
Issue-Key: cloudformation__s3__sts.cfn-s3-bucket-policy-create-update-in-place-replace-retained-then-delete|run#30|AssertionError|assert policy_v2 != policy_v1, f"a replacement gets a new physical id, got {policy_v2!r}"
Issue-Commit: b26d594b442f382e51363fa18dcb67bd5cc86e1e

# cloudformation__s3__sts: a replaced AWS::S3::BucketPolicy keeps the old PhysicalResourceId, so the replacement is indistinguishable from an in-place update

> _Note: this issue was identified against the code base at commit `b26d594b442f382e51363fa18dcb67bd5cc86e1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
One net-new root cause (Cause 1 of 1), grounded in a failing behavioral check the replay report marks neither ALREADY FILED nor OWNED.

Cause 1 of 1: an update that replaces an AWS::S3::BucketPolicy reuses the resource's PhysicalResourceId instead of minting a new one.
AWS: the physical id of an AWS::S3::BucketPolicy is a CloudFormation-generated name of the shape <stack-name>-<logical-id>-<random suffix>, reported in DescribeStackResources/DescribeStackResource and in StackEvents[*].PhysicalResourceId (Smithy cloudformation#StackResource.PhysicalResourceId, cloudformation#StackEvent.PhysicalResourceId). When an update changes a create-only property of that resource (its Bucket), the resource is REPLACED: CloudFormation creates a new physical resource and reports a NEW physical id - the suffix is regenerated, so the post-update id never equals the pre-update id.
Emulator: after the replacing update the stack resource's PhysicalResourceId is byte-identical to the one reported before the update (the same <stack>-Policy-<suffix> string), so no new physical resource is recorded.
Why: PhysicalResourceId is the handle every client uses to correlate a stack resource with the thing it created and to detect that a replacement happened - drift detection, resource import, DELETE_SKIPPED correlation and change-set review all compare it across an update. An id that never changes makes a replacement look like a no-interruption update, so cleanup, retention and audit decisions keyed on it are all taken against the wrong resource.
Fix: the AWS::S3::BucketPolicy arm of the CloudFormation resource provisioner on the replacement path - mint and record a fresh generated physical id (and keep the old one only for the cleanup/DELETE_SKIPPED event).
Traces: cloudformation__s3__sts.cfn-s3-bucket-policy-create-update-in-place-replace-retained-then-delete.

## Evidence
Replay output (behavioral suite, section ### cloudformation__s3__sts):
  [mismatch] cloudformation__s3__sts.cfn-s3-bucket-policy-create-update-in-place-replace-retained-then-delete
  AssertionError: a replacement gets a new physical id, got '<stack-name>-Policy-<suffix>'

Expected (AWS): the PhysicalResourceId read after the replacing update differs from the one read before it (both of the form <stack-name>-<logical-id>-<random suffix>).
Actual (emulator): the two reads return the identical string. The concrete name/suffix in the message is a per-run generated value; the requirement is only that the replacement yields a different id.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (cloudformation, s3 and sts are all bootstrapped and serving).
2. CreateStack with two AWS::S3::Bucket resources and one AWS::S3::BucketPolicy whose Bucket refers to the first bucket. Wait for CREATE_COMPLETE and record the policy's PhysicalResourceId from DescribeStackResources.
3. UpdateStack with the same template except that the BucketPolicy's Bucket now refers to the second bucket (Bucket is create-only for AWS::S3::BucketPolicy, so this is a replacement). Wait for UPDATE_COMPLETE.
4. Read the policy's PhysicalResourceId again. Expect a different generated id; observe the same string as before the update.
5. Equivalently: re-replay cloudformation__s3__sts.cfn-s3-bucket-policy-create-update-in-place-replace-retained-then-delete.
