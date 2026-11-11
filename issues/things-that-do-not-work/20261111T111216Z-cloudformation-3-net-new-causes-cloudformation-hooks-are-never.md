Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.deep-cloudformation-1-cfn-enforce-fail-hook-blocks-unencrypted-queue-create cloudformation.deep-cloudformation-1-cfn-enforce-warn-hook-flags-but-allows-unencrypted-queue-create cloudformation.stack-continue-update-rollback-client-request-token-marks-its-events cloudformation.stack-continue-update-rollback-force-rollback-skips-failed-resource cloudformation.stack-continue-update-rollback-lifecycle cloudformation.stack-update-cancel-then-continue-rollback cloudformation.continue-update-rollback-on-stable-stack-fails cloudformation.cdk-s3-auto-delete-objects-empties-past-destroy-manual-bucket-blocks-on-bucketnotempty
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.deep-cloudformation-1-cfn-enforce-fail-hook-blocks-unencrypted-queue-create|run#21|AssertionError|assert bad["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation.deep-cloudformation-1-cfn-enforce-warn-hook-flags-but-allows-unencrypted-queue-create|run#20|AssertionError|assert len(hook_events) == 1, (
Issue-Key: cloudformation|ContinueUpdateRollback|ValidationError
Issue-Key: cloudformation.stack-continue-update-rollback-force-rollback-skips-failed-resource|run#7|AssertionError|assert _status_of(cfn, stack_id, "Custom") == "UPDATE_FAILED", f"Custom after the failed rollback: {_resources(cfn, stack_id)!r}"
Issue-Key: cloudformation.stack-continue-update-rollback-lifecycle|_next_request#33|Timeout|return json.loads(envelope["Message"])
Issue-Key: cloudformation.stack-update-cancel-then-continue-rollback|_next_request#31|Timeout|return json.loads(envelope["Message"])
Issue-Key: cloudformation.continue-update-rollback-on-stable-stack-fails|_next_request#38|Timeout|raise AssertionError(f"no custom-resource request arrived within {timeout_s}s")
Issue-Key: cloudformation.cdk-s3-auto-delete-objects-empties-past-destroy-manual-bucket-blocks-on-bucketnotempty|_cdk#12|AssertionError|assert result.returncode == expect_rc, (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# 3 net-new causes: CloudFormation hooks are never enforced, UPDATE_ROLLBACK_FAILED is never reached, and a Lambda-backed custom resource never completes

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Three independent root causes in the CloudFormation service, each with its own failing checks.

Cause 1 of 3: stack-level hooks are registered but never evaluated.
AWS: a Guard/Lambda hook activated for CREATE with FailureMode FAIL must block a non-compliant resource — the stack ends ROLLBACK_COMPLETE and the stack events carry a HOOK_COMPLETE_FAILED event for the invoked hook. With FailureMode WARN the same rule still fails, so exactly one HOOK_COMPLETE_FAILED stack event is emitted while the stack is allowed to reach CREATE_COMPLETE.
Emulator: neither happens. Under a FAIL hook a non-compliant stack (an unencrypted AWS::SQS::Queue) ends 'CREATE_COMPLETE' with StatusReason None instead of ROLLBACK_COMPLETE; under a WARN hook no HOOK_* stack event is emitted at all (the events seen are only the plain stack/resource events, all with no hook status).
Why: a client relying on hooks for policy enforcement gets no enforcement and no audit trail — non-compliant infrastructure deploys silently, and a WARN-mode caller never sees the warning it deployed hooks to get.
Fix: the CloudFormation hook-invocation stage of the provisioning pipeline (pre-create/pre-update hook evaluation) and the stack-event writer that must record HOOK_IN_PROGRESS/HOOK_COMPLETE_FAILED.
Failing checks: cloudformation.deep-cloudformation-1-cfn-enforce-fail-hook-blocks-unencrypted-queue-create, cloudformation.deep-cloudformation-1-cfn-enforce-warn-hook-flags-but-allows-unencrypted-queue-create.

Cause 2 of 3: a resource whose rollback fails never leaves the stack in UPDATE_ROLLBACK_FAILED, so ContinueUpdateRollback cannot be driven.
AWS: when an update fails and the automatic rollback of a resource also fails, that resource stays UPDATE_FAILED and the stack reaches UPDATE_ROLLBACK_FAILED. ContinueUpdateRollback then accepts that resource in ResourcesToSkip (it is in UPDATE_FAILED), resumes the rollback, and marks the resulting events with the supplied ClientRequestToken. ContinueUpdateRollback against a stack that is NOT in UPDATE_ROLLBACK_FAILED is refused with ValidationError.
Emulator: the resource is reported as 'UPDATE_COMPLETE' after the failed rollback (resource view {'Custom': 'UPDATE_COMPLETE', 'Fn': 'CREATE_COMPLETE', 'Role': 'CREATE_COMPLETE'}), so the stack never enters UPDATE_ROLLBACK_FAILED and ContinueUpdateRollback refuses the skip with HTTP 400 ValidationError and message 'ResourcesToSkip can only name resources in the UPDATE_FAILED state; [Custom] is not one of them' — the fixed text is 'ResourcesToSkip can only name resources in the UPDATE_FAILED state; [' ... '] is not one of them', the variable part being the logical ids the caller passed. Three further checks in this family never settle at all and end 'Error: Check timed out after 60.0s', i.e. the stack never reaches a terminal rollback state.
Why: the documented recovery path out of a stuck rollback (ContinueUpdateRollback, optionally with ResourcesToSkip, and CancelUpdateStack followed by it) is unusable; an operator cannot un-wedge a stack, and a caller that polls for UPDATE_ROLLBACK_FAILED waits forever.
Fix: the CloudFormation update-rollback state machine (per-resource rollback outcome recording and the UPDATE_ROLLBACK_FAILED transition) plus the ContinueUpdateRollback handler's ResourcesToSkip validation.
Failing checks: cloudformation.stack-continue-update-rollback-client-request-token-marks-its-events, cloudformation.stack-continue-update-rollback-force-rollback-skips-failed-resource, cloudformation.stack-continue-update-rollback-lifecycle, cloudformation.stack-update-cancel-then-continue-rollback, cloudformation.continue-update-rollback-on-stable-stack-fails.

Cause 3 of 3: a Lambda-backed custom resource never leaves CREATE_IN_PROGRESS.
AWS: a Custom:: resource backed by a Lambda function completes when the function PUTs its SUCCESS response to the pre-signed ResponseURL; CloudFormation then records CREATE_COMPLETE for it and the stack completes. CDK's Custom::S3AutoDeleteObjects provider is exactly this shape.
Emulator: the custom resource stays CREATE_IN_PROGRESS until CloudFormation's own wait expires, and the deploy fails with 'Custom::S3AutoDeleteObjects <physical-id> was still CREATE_IN_PROGRESS when the wait for it to be created ran out' / 'DeploymentError: Resource updates failed'. `cdk deploy` exits 1 where AWS exits 0.
Why: every CDK/CloudFormation pattern built on a Lambda-backed custom resource (auto-delete-objects, bucket deployments, provider framework, custom lookups) is unusable: the stack never completes and the caller cannot even destroy it cleanly.
Fix: the CloudFormation custom-resource response path — the ResponseURL the provisioner hands the function and the ingest of the SUCCESS/FAILED response that must flip the resource to CREATE_COMPLETE.
Failing check: cloudformation.cdk-s3-auto-delete-objects-empties-past-destroy-manual-bucket-blocks-on-bucketnotempty. Open issues record this shape for other services (20261108T043811Z-cloudformation-s3-a-lambda-backed-cloudformation-custom-resource-never-completes.md, 20261111T081112Z-apigateway-cloudformation-lambda-a-cloudformation-custom-resource-delete-never-completes-so.md); neither carries this replay key.

## Evidence
Cause 1 — Expected: non-compliant stack under the FAIL hook ends ROLLBACK_COMPLETE. Actual: AssertionError: non-compliant stack under the FAIL hook ended 'CREATE_COMPLETE' (None), expected ROLLBACK_COMPLETE. Expected under WARN: exactly one HOOK_COMPLETE_FAILED stack event. Actual: got [('sim2real-cfnenf-warn-...', None), ('Q', None), ('Q', None), ('Q', None), ('sim2real-cfnenf-warn-...', None)] — no hook status on any event.
Cause 2 — Expected: the failed resource is UPDATE_FAILED and ContinueUpdateRollback accepts it in ResourcesToSkip. Actual: AssertionError: Custom after the failed rollback: {'Custom': 'UPDATE_COMPLETE', 'Fn': 'CREATE_COMPLETE', 'Role': 'CREATE_COMPLETE'}; and botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the ContinueUpdateRollback operation: ResourcesToSkip can only name resources in the UPDATE_FAILED state; [Custom] is not one of them. Three sibling checks end 'Error: Check timed out after 60.0s'.
Cause 3 — Expected: `cdk deploy <stack> --require-approval never` exits 0. Actual: exited 1; tail: 'Custom::S3AutoDeleteObjects ... was still CREATE_IN_PROGRESS when the wait for it to be created ran out' / '❌ ... failed to deploy / DeploymentError: Resource updates failed'.

## Steps to Reproduce
Cause 1: activate a Guard hook for CREATE with FailureMode FAIL (then WARN) that rejects an unencrypted SQS queue; CreateStack a template with such a queue; read DescribeStacks status and DescribeStackEvents HookStatus.
Cause 2: deploy a stack with a custom resource, UpdateStack so the resource fails, let the automatic rollback fail, then DescribeStackResources (resource should be UPDATE_FAILED, stack UPDATE_ROLLBACK_FAILED) and call ContinueUpdateRollback with ResourcesToSkip=['Custom'].
Cause 3: `cdk deploy` a stack with an s3.Bucket(autoDeleteObjects=True, removalPolicy=DESTROY); the Custom::S3AutoDeleteObjects resource never leaves CREATE_IN_PROGRESS.
