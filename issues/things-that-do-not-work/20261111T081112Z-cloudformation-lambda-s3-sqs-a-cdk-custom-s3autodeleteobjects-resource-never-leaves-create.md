Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__s3__sqs.deploy-cdk-bucket-sqs-notification-prefix-filter-and-auto-delete-objects-on-destroy
Issue-Key: cloudformation__lambda__s3__sqs.deploy-cdk-bucket-sqs-notification-prefix-filter-and-auto-delete-objects-on-destroy|_cdk#17|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Commit: f4372f081be041f1c06808452ec2531466fafcb7

# A CDK Custom::S3AutoDeleteObjects resource never leaves CREATE_IN_PROGRESS, failing the deploy and then the rollback

> _Note: this issue was identified against the code base at commit `f4372f081be041f1c06808452ec2531466fafcb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a Lambda-backed `Custom::S3AutoDeleteObjects` resource in a CDK-synthesized stack never reaches CREATE_COMPLETE, so the stack goes CREATE_FAILED and the subsequent rollback cannot delete the bucket-notifications custom resource either.
AWS: recorded behavior only - CloudFormation invokes the custom resource's provider function with RequestType Create, the provider PUTs SUCCESS to the ResponseURL, and the resource reaches CREATE_COMPLETE; `cdk deploy` then exits 0. On a failed create the rollback deletes each custom resource by invoking the provider with RequestType Delete, and reaches ROLLBACK_COMPLETE rather than ROLLBACK_FAILED.
Emulator: the resource is reported `CREATE_FAILED` with reason `Custom::S3AutoDeleteObjects <physical id> was still CREATE_IN_PROGRESS when the wait for it to be created ran out`, the stack then reports `ROLLBACK_FAILED` with `The following resource(s) failed to delete: [BucketNotifications8F2E257D]`, and `cdk deploy` exits rc=1. The fixed text of the failure reason is `Custom::<Type> <physical resource id> was still CREATE_IN_PROGRESS when the wait for it to be created ran out`; the type, physical id and resource logical id come from the stack.
Why: the standard CDK bucket idioms (`autoDeleteObjects: true`, `bucket.addEventNotification(...)`) are unusable - the deploy fails and the stack is then wedged in ROLLBACK_FAILED, which a client cannot clear.
Fix: the custom-resource provider of the CloudFormation create/rollback path - the Create and Delete dispatch to the provider Lambda and the ResponseURL callback that must retire the in-progress wait.

Traces: cloudformation__lambda__s3__sqs.deploy-cdk-bucket-sqs-notification-prefix-filter-and-auto-delete-objects-on-destroy.

## Evidence
Expected: `cdk deploy` rc=0 with the bucket, its prefix-filtered SQS notification and the auto-delete custom resource created.
Actual: AssertionError: `cdk deploy` rc=1, expected 0, with stack events 'CREATE_FAILED | Custom::S3AutoDeleteObjects ... was still CREATE_IN_PROGRESS when the wait for it to be created ran out' and 'ROLLBACK_FAILED | AWS::CloudFormation::Stack ... The following resource(s) failed to delete: [BucketNotifications8F2E257D].'

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Synthesize a CDK stack with an s3.Bucket using autoDeleteObjects: true and a prefix-filtered SQS event notification.
3. Run `cdk deploy --require-approval never` against the emulator and read the stack events.
4. Re-replay cloudformation__lambda__s3__sqs.deploy-cdk-bucket-sqs-notification-prefix-filter-and-auto-delete-objects-on-destroy.
