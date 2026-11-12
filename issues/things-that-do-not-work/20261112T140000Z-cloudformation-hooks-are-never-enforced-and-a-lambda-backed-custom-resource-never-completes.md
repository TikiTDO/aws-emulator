Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.deep-cloudformation-1-cfn-enforce-fail-hook-blocks-unencrypted-queue-create cloudformation.deep-cloudformation-1-cfn-enforce-warn-hook-flags-but-allows-unencrypted-queue-create cloudformation.cdk-s3-auto-delete-objects-empties-past-destroy-manual-bucket-blocks-on-bucketnotempty
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.deep-cloudformation-1-cfn-enforce-fail-hook-blocks-unencrypted-queue-create|run#21|AssertionError|assert bad["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation.deep-cloudformation-1-cfn-enforce-warn-hook-flags-but-allows-unencrypted-queue-create|run#20|AssertionError|assert len(hook_events) == 1, (
Issue-Key: cloudformation.cdk-s3-auto-delete-objects-empties-past-destroy-manual-bucket-blocks-on-bucketnotempty|_cdk#12|AssertionError|assert result.returncode == expect_rc, (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# 2 remaining causes: CloudFormation hooks are never enforced, and a Lambda-backed custom resource never completes

> _Provenance._ These are Causes 1 and 3 of
> `20261111T111216Z-cloudformation-3-net-new-causes-cloudformation-hooks-are-never.md`, re-filed
> unchanged with their own replay keys. That issue's Cause 2 (the update-rollback state machine
> and `ContinueUpdateRollback`) was fixed and its report moved to `things-that-now-work`; these
> two causes were NOT fixed by that change and nothing of them is implemented, so they are
> filed here rather than left in a resolved report. Each needs a capability the emulator does
> not have yet, named under "What each needs" below.

## Operation
CreateStack, UpdateStack

## Description

Cause 1 of 2: stack-level hooks are registered but never evaluated.
AWS: a Guard/Lambda hook activated for CREATE with FailureMode FAIL must block a non-compliant resource — the stack ends ROLLBACK_COMPLETE and the stack events carry a HOOK_COMPLETE_FAILED event for the invoked hook. With FailureMode WARN the same rule still fails, so exactly one HOOK_COMPLETE_FAILED stack event is emitted while the stack is allowed to reach CREATE_COMPLETE.
Emulator: neither happens. Under a FAIL hook a non-compliant stack (an unencrypted AWS::SQS::Queue) ends 'CREATE_COMPLETE' with StatusReason None instead of ROLLBACK_COMPLETE; under a WARN hook no HOOK_* stack event is emitted at all (the events seen are only the plain stack/resource events, all with no hook status).
Why: a client relying on hooks for policy enforcement gets no enforcement and no audit trail — non-compliant infrastructure deploys silently, and a WARN-mode caller never sees the warning it deployed hooks to get.
Fix: the CloudFormation hook-invocation stage of the provisioning pipeline (pre-create/pre-update hook evaluation) and the stack-event writer that must record HOOK_IN_PROGRESS/HOOK_COMPLETE_FAILED.
Failing checks: cloudformation.deep-cloudformation-1-cfn-enforce-fail-hook-blocks-unencrypted-queue-create, cloudformation.deep-cloudformation-1-cfn-enforce-warn-hook-flags-but-allows-unencrypted-queue-create.

Cause 2 of 2: a Lambda-backed custom resource never leaves CREATE_IN_PROGRESS.
AWS: a Custom:: resource backed by a Lambda function completes when the function PUTs its SUCCESS response to the pre-signed ResponseURL; CloudFormation then records CREATE_COMPLETE for it and the stack completes. CDK's Custom::S3AutoDeleteObjects provider is exactly this shape.
Emulator: the custom resource stays CREATE_IN_PROGRESS until CloudFormation's own wait expires, and the deploy fails with 'Custom::S3AutoDeleteObjects <physical-id> was still CREATE_IN_PROGRESS when the wait for it to be created ran out' / 'DeploymentError: Resource updates failed'. `cdk deploy` exits 1 where AWS exits 0.
Why: every CDK/CloudFormation pattern built on a Lambda-backed custom resource (auto-delete-objects, bucket deployments, provider framework, custom lookups) is unusable: the stack never completes and the caller cannot even destroy it cleanly.
Fix: the custom-resource response path end to end. CloudFormation's own half is in place and is exercised by the SNS-backed checks that now pass — it invokes the function asynchronously and serves the pre-signed ResponseURL the answer arrives at (services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioning/custom_resource.rs `send`/`response_url`, and src/service/custom_resource_plane.rs). What is missing is the Lambda runtime actually running CDK's bundled Node provider so that it PUTs its SUCCESS.
Failing check: cloudformation.cdk-s3-auto-delete-objects-empties-past-destroy-manual-bucket-blocks-on-bucketnotempty. Open issues record this shape for other services (20261108T043811Z-cloudformation-s3-a-lambda-backed-cloudformation-custom-resource-never-completes.md, 20261111T081112Z-apigateway-cloudformation-lambda-a-cloudformation-custom-resource-delete-never-completes-so.md); neither carries this replay key.

## What each needs

Cause 1 needs a hook VERDICT, which nothing in the emulator can produce today. `src/state/hook_results.rs` states it ("a Hook's verdict is produced by running the Hook's own handler (a Guard rule set, a Lambda function, a registered Hook's handler package), and no such handler exists here"), and `src/state/change_set_hooks.rs` says the same for a planned invocation. Serving it faithfully therefore needs: an engine that evaluates the hook's own rule set, the activated-hook/type-configuration plumbing that decides which hooks a stack operation invokes and under which FailureMode, the HOOK_IN_PROGRESS/HOOK_COMPLETE_FAILED stack-event channel, and the invocation records `ListHookResults`/`GetHookResult` read. Recording a hook status without an engine that evaluated the rule would be an outcome nothing produced.

Cause 2 needs the Lambda runtime in services/service-com-amazonaws-lambda-awsgirapiservice to run the CDK-bundled Node provider (an asset zip whose handler uses the AWS SDK and an HTTPS PUT) so that it answers at the ResponseURL CloudFormation already publishes. Nothing in the cloudformation crate can make that answer arrive.

## Evidence
Cause 1 — Expected: non-compliant stack under the FAIL hook ends ROLLBACK_COMPLETE. Actual: AssertionError: non-compliant stack under the FAIL hook ended 'CREATE_COMPLETE' (None), expected ROLLBACK_COMPLETE. Expected under WARN: exactly one HOOK_COMPLETE_FAILED stack event. Actual: got [('sim2real-cfnenf-warn-...', None), ('Q', None), ('Q', None), ('Q', None), ('sim2real-cfnenf-warn-...', None)] — no hook status on any event.
Cause 2 — Expected: `cdk deploy <stack> --require-approval never` exits 0. Actual: exited 1; tail: 'Custom::S3AutoDeleteObjects ... was still CREATE_IN_PROGRESS when the wait for it to be created ran out' / '❌ ... failed to deploy / DeploymentError: Resource updates failed'.

## Steps to Reproduce
Cause 1: activate a Guard hook for CREATE with FailureMode FAIL (then WARN) that rejects an unencrypted SQS queue; CreateStack a template with such a queue; read DescribeStacks status and DescribeStackEvents HookStatus.
Cause 2: `cdk deploy` a stack with an s3.Bucket(autoDeleteObjects=True, removalPolicy=DESTROY); the Custom::S3AutoDeleteObjects resource never leaves CREATE_IN_PROGRESS.
