Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__kms__s3.cdk-bootstrap-customer-key-encrypts-the-staging-bucket-and-published-assets cloudformation__kms__s3.cdk-bootstrap-customer-key-disabled-blocks-asset-publish-and-the-deploy
Issue-Key: cloudformation__kms__s3.cdk-bootstrap-customer-key-encrypts-the-staging-bucket-and-published-assets|_cdk#28|AssertionError|assert result.returncode == expect_rc, (
Issue-Key: cloudformation__kms__s3.cdk-bootstrap-customer-key-disabled-blocks-asset-publish-and-the-deploy|_cdk#20|AssertionError|assert result.returncode == expect_rc, (
Issue-Commit: 6a777fbd6800e714176d50f5d7d416617b079449

# cloudformation__kms__s3: reading an SSE-KMS object under a CDK bootstrap customer CMK is refused AccessDenied/kms:Decrypt to the account's own caller, so every `cdk deploy` on that bootstrap fails

> _Note: this issue was identified against the code base at commit `6a777fbd6800e714176d50f5d7d416617b079449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1: an S3 read of an object encrypted with the customer-managed CMK that `cdk bootstrap --bootstrap-customer-key` created is denied kms:Decrypt for the very caller that just wrote it, even while the key is Enabled. The deploy dies after asset publication, at changeset creation.

AWS: the bootstrap stack creates the staging-bucket CMK with a key policy that grants the account (Principal "AWS": "arn:aws:iam::<account>:root", so identity policies in the account can grant kms:Decrypt) plus the bootstrap file-publishing/deployment roles. Publishing an asset writes it SSE-KMS under that key and the subsequent read of the same object by the same account caller succeeds (Smithy: s3 GetObject/HeadObject echo SSEKMSKeyId via x-amz-server-side-encryption-aws-kms-key-id; kms Decrypt declares no error for an Enabled key that the key policy allows). Recorded behavior: `cdk deploy <stack> --require-approval never` exits 0 — the template and Fn/Code assets publish and CloudFormation creates the changeset and the stack. kms:Decrypt is refused only when policy actually denies it, as AccessDeniedException whose message is AWS's fixed template "User: <caller ARN> is not authorized to perform: kms:<Action> on resource: <key ARN> because no resource-based policy allows the kms:<Action> action" (the caller ARN, action and key ARN are values from the request/state, the rest is fixed text).

Emulator: the read is refused for the account's own caller with exactly that denial, so the CLI aborts: `S3 error: User: arn:aws:iam::<account>:user/<caller> is not authorized to perform: kms:Decrypt on resource: arn:aws:kms:<region>:<account>:key/<key id> because no resource-based policy allows the kms:Decrypt action`, and `cdk deploy` exits 1 instead of 0. The write path (PutObject under the same key) succeeded in the same run, so the divergence is on the decrypt authorization of the read, not on the encryption.

Why: no CDK application can be deployed against a bootstrap that uses a customer key — the happy path never completes, and the negative path (disable the key and expect the publish to be blocked) cannot be distinguished from the broken happy path, so a client cannot tell a real key-state refusal from this spurious denial.

Fix: KMS key-policy/authorization evaluation for the Decrypt the S3 read path performs (does the account-root grant and the bootstrap role grants in the key policy authorize the caller?), and the S3 SSE-KMS read path that surfaces that verdict. Related open issue 20261106T070552Z-cloudformation-ecs-the-cdk-l2-ecs-asg-capacity-d82e65.md concerns a denied s3:ListBucket for the publishing role and does not carry these checks.

Traces: cloudformation__kms__s3.cdk-bootstrap-customer-key-encrypts-the-staging-bucket-and-published-assets (happy path), cloudformation__kms__s3.cdk-bootstrap-customer-key-disabled-blocks-asset-publish-and-the-deploy (its enabled-key control deploy).

## Evidence
Both checks fail at the same point, in `_cdk(... ['deploy', <stack>, '--require-approval', 'never'] ...)` expecting returncode 0:

Expected (AWS): exit 0 — assets publish, then "creating CloudFormation changeset..." proceeds and the stack is created.
Actual (emulator): exit 1 with output tail
  ...: success: Published <stack> Template (<account>-<region>-<hash>)
  ...: success: Published Fn/Code (<account>-<region>-<hash>)
  ...: creating CloudFormation changeset...
  S3 error: User: arn:aws:iam::<account>:user/<caller> is not authorized to perform: kms:Decrypt on resource: arn:aws:kms:<region>:<account>:key/<key-id> because no resource-based policy allows the kms:Decrypt action
  For more information check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html

Fixed text of the refusal: "User: " / " is not authorized to perform: kms:" / " on resource: " / " because no resource-based policy allows the kms:" / " action"; the variable parts are the caller ARN, the action (Decrypt) and the CMK ARN taken from the request and the emulator's state. In the disabled-key check this denial is raised on the CONTROL deploy, made while the bootstrap CMK is still Enabled.

## Steps to Reproduce
1. Run `cdk bootstrap --qualifier <q> --bootstrap-customer-key --toolkit-stack-name <toolkit-stack>` against the emulator.
2. Confirm the staging bucket cdk-<q>-assets-<account>-<region> has an SSE-KMS default (GetBucketEncryption) naming a customer-managed CMK, and that DescribeKey reports KeyManager=CUSTOMER, KeyState=Enabled.
3. `cdk deploy <stack> --require-approval never` for an app with a Lambda code asset, using context aws:cdk:bootstrap-qualifier=<q>.
   -> AWS: exit 0. Emulator: exit 1 after "creating CloudFormation changeset..." with the kms:Decrypt AccessDenied above.
4. (Second check) After that deploy succeeds, DisableKey the bootstrap CMK and deploy a second stack: AWS blocks asset publication and never reaches CloudFormation; the emulator fails the earlier enabled-key control instead.
