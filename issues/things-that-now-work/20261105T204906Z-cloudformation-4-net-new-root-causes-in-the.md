Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cdk-s3-auto-delete-objects-empties-past-destroy-manual-bucket-blocks-on-bucketnotempty cloudformation.stack-continue-update-rollback-client-request-token-marks-its-events cloudformation.stack-continue-update-rollback-force-rollback-skips-failed-resource cloudformation.continue-update-rollback-on-stable-stack-fails cloudformation.stack-continue-update-rollback-lifecycle cloudformation.stack-update-cancel-then-continue-rollback cloudformation.deep-cloudformation-1-cfn-enforce-fail-hook-blocks-unencrypted-queue-create cloudformation.deep-cloudformation-1-cfn-enforce-warn-hook-flags-but-allows-unencrypted-queue-create cloudformation.deploy-cdk-update-rejected-by-aws-rolls-back-keeping-prior-config
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.cdk-s3-auto-delete-objects-empties-past-destroy-manual-bucket-blocks-on-bucketnotempty|_cdk#12|AssertionError|assert result.returncode == expect_rc, (
Issue-Key: cloudformation.stack-continue-update-rollback-client-request-token-marks-its-events|run#5|AssertionError|assert _settle(cfn, stack_id, timeout_s=_cap(ctx, _DEADLINE_S)) == "CREATE_COMPLETE", f"create ended {_stack(cfn, stack_id)!r}"
Issue-Key: cloudformation.stack-continue-update-rollback-force-rollback-skips-failed-resource|run#4|AssertionError|assert _settle(cfn, stack_id, timeout_s=_cap(ctx, _DEADLINE_S)) == "CREATE_COMPLETE", f"create ended {_stack(cfn, stack_id)!r}"
Issue-Key: cloudformation.continue-update-rollback-on-stable-stack-fails|_next_request#32|Timeout|raise AssertionError(f"no custom-resource request arrived within {timeout_s}s")
Issue-Key: cloudformation.stack-continue-update-rollback-lifecycle|_next_request#25|Timeout|raise AssertionError(f"no custom-resource request arrived within {timeout_s}s")
Issue-Key: cloudformation.stack-update-cancel-then-continue-rollback|_next_request#30|Timeout|raise AssertionError(f"no custom-resource request arrived within {timeout_s}s")
Issue-Key: cloudformation.deep-cloudformation-1-cfn-enforce-fail-hook-blocks-unencrypted-queue-create|_deploy_guard_hook#14|KeyError|outs = {o["OutputKey"]: o["OutputValue"] for o in st.get("Outputs", [])}
Issue-Key: cloudformation.deep-cloudformation-1-cfn-enforce-warn-hook-flags-but-allows-unencrypted-queue-create|_deploy_guard_hook#13|KeyError|outs = {o["OutputKey"]: o["OutputValue"] for o in st.get("Outputs", [])}
Issue-Key: cloudformation.deploy-cdk-update-rejected-by-aws-rolls-back-keeping-prior-config|run#18|AssertionError|assert rc != 0, (
Issue-Commit: a6bd8162e254cd53a71309df843cd006b7af8737

# cloudformation: 4 net-new root causes in the custom-resource and rollback path — a Lambda-backed Custom:: resource is refused with ResourceConflict because its provider function is left Pending, an SNS-backed custom resource's request is never published so the stack never settles, DescribeStacks Outputs omit OutputValue, and an update AWS refuses is accepted instead of rolled back

> _Note: this issue was identified against the code base at commit `a6bd8162e254cd53a71309df843cd006b7af8737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _Resolution note. Cause 3 is served: the two Hook resource types publish the return values
> they document (`Ref` and `Fn::GetAtt HookArn`), so a stack declaring one reports every
> declared output with its value — both Guard-hook checks now get past `outs[...]` and fail on
> hook INVOCATION, which is the still-open half tracked by
> `20261025T015306Z-cloudformation-cannot-deploy-large-parts-of-the-resource.md` (no hook store,
> no Guard evaluation: a FAIL-mode hook blocks nothing and a WARN-mode one records no
> `HOOK_COMPLETE_FAILED` event). Cause 4 was already served before this change. Cause 2 is
> served for the addressing and the driver: the `ResponseURL` is now a pre-signed URL on this
> emulator's own endpoint (`/_aws/cloudformation/<stack>|<logical>|<request>`) that a subscriber
> can really answer, and a landed answer starts the work it owes itself
> (`state/provider_driver.rs`) instead of waiting for the client's next CloudFormation call.
> Cause 1 is served for an inline-code provider: the deployment package CloudFormation builds
> for inline `ZipFile` code now carries the `cfnresponse` module, so a Python-runtime provider
> answers its request and the stack settles on that answer.
>
> What is still open, with the evidence: (1) a provider whose package is a NODE bundle — the CDK
> provider framework, `cdk-s3-auto-delete-objects…` — cannot answer at all, because the Lambda
> runtime's Node core modules carry no `http`/`https`
> (`services/service-com-amazonaws-lambda-awsgirapiservice/src/state/execution/node/core_modules.rs`
> `CORE_MODULES`), so its `https.request` to the `ResponseURL` cannot be made; that module, wired
> to the existing `guest_http` seam, is what those providers need. (2) After a rollback whose
> provider answered the rollback's own request, `DescribeStackResources` reports the custom
> resource `UPDATE_COMPLETE` where AWS leaves it `UPDATE_FAILED`, which is what
> `ContinueUpdateRollback`'s `ResourcesToSkip` guard then refuses
> (`stack-continue-update-rollback-force-rollback-skips-failed-resource` run#7 and
> `…-client-request-token-marks-its-events`): the resource's planned terminal event is not
> rewritten to the failure on that path (`state/stack.rs::fail_stabilisation` ->
> `fail_planned_resource_event`)._

## Operation
ContinueUpdateRollback

## Description
Four net-new behavioral root causes of service `cloudformation`, grounded in the failing behavioral checks of this replay. A fixer must close all four before submitting.

Cause 1 of 4: a Custom:: resource whose ServiceToken is a Lambda function is never invoked — the provisioner calls the function while it is still in state `Pending` and the call is refused ResourceConflict, so the resource ends FAILED and every stack carrying one rolls back.
AWS: recorded behavior only. A stack that declares a Lambda function plus a Custom:: / AWS::CloudFormation::CustomResource resource pointing at it reaches CREATE_COMPLETE: CloudFormation waits for the function to leave Pending (Lambda's State goes Pending -> Active) before invoking the provider, then settles the resource on the provider's SUCCESS response. The refusal AWS does emit when a function really is unavailable is relayed by the resource handler as `Resource handler returned message: "The operation cannot be performed at this time. The function is currently in the following state: Pending" (RequestToken: <uuid>, HandlerErrorCode: ResourceConflict)` — fixed text with the request token as its only variable part — but it is a transient state AWS retries past, not the terminal outcome of a create.
Emulator: the provider invocation is issued against a function the emulator leaves in `Pending` forever, so the resource's terminal status is CREATE_FAILED with exactly that ResourceConflict message, and the stack ends ROLLBACK_COMPLETE with StackStatusReason `The following resource(s) failed to create: [<LogicalId>]. `.
Why: every CDK construct that ships a provider framework (BucketDeployment, auto-delete-objects, custom_resources.Provider) and every template with a Lambda-backed custom resource is undeployable; the side effect the provider exists to perform never happens, and the failure is reported as a client-side conflict rather than as the emulator's own gap.
Fix: the readiness wait (and retry of a ResourceConflict Pending refusal) in the cloudformation custom-resource provisioner, together with the lambda function-state transition it waits on.
Traces: cloudformation.cdk-s3-auto-delete-objects-empties-past-destroy-manual-bucket-blocks-on-bucketnotempty, cloudformation.stack-continue-update-rollback-client-request-token-marks-its-events, cloudformation.stack-continue-update-rollback-force-rollback-skips-failed-resource.

Cause 2 of 4: a Custom:: resource whose ServiceToken is an SNS topic never has its lifecycle request published, so the stack never leaves its in-progress status and the whole ContinueUpdateRollback / CancelUpdateStack state machine is unreachable.
AWS: recorded behavior only. For a Custom:: resource backed by an SNS ServiceToken, CloudFormation publishes the request (RequestType Create/Update/Delete, RequestId, StackId, LogicalResourceId, ResponseURL) to the topic; the subscriber answers by PUTting SUCCESS/FAILED to the ResponseURL, and only then does the stack advance. That handshake is the premise of the UPDATE_ROLLBACK_FAILED -> ContinueUpdateRollback -> UPDATE_ROLLBACK_COMPLETE path and of CancelUpdateStack.
Emulator: no request ever arrives on the subscribed queue, so the stack sits in CREATE_IN_PROGRESS / UPDATE_IN_PROGRESS and the checks exhaust their entire budget (replay reports `Check timed out after 60.0s`) before reaching any assertion. The refusal ContinueUpdateRollback must give on a stable stack is likewise never observable.
Why: a client cannot drive any SNS-backed custom resource, and the operations that only exist for failed rollbacks (ContinueUpdateRollback with ResourcesToSkip/ForceRollback, CancelUpdateStack) cannot be exercised at all.
Fix: the SNS publication step of the cloudformation custom-resource provisioner and the ResponseURL callback that settles the resource; and the state guard that must refuse ContinueUpdateRollback on a stack that is not in UPDATE_ROLLBACK_FAILED. The open issue 20261105T141713Z-cloudformation-sns-sqs-4-net-new-root-causes.md works on the SNS-backed provider protocol under the cloudformation__sns__sqs heading; it does not carry these checks' replay keys.
Traces: cloudformation.continue-update-rollback-on-stable-stack-fails, cloudformation.stack-continue-update-rollback-lifecycle, cloudformation.stack-update-cancel-then-continue-rollback.

Cause 3 of 4: DescribeStacks returns Outputs entries that carry OutputKey but no OutputValue.
AWS: Smithy DescribeStacks -> Stacks[].Outputs is a list of Output structures whose members are OutputKey, OutputValue, Description and ExportName; for a CREATE_COMPLETE stack every declared Output is present with its resolved OutputValue.
Emulator: each Output entry is returned with OutputKey only, so any client building a key->value map faults (`KeyError: 'OutputValue'`), as both Guard-hook checks do when reading the hook stack's outputs.
Why: stack Outputs are how one stack hands identifiers (ARNs, names, URLs) to its caller or to a dependent stack; with no value, every downstream step is blocked and the stack is effectively write-only. Here it blocks the entire STACK Guard-hook surface (FAIL mode must roll a non-compliant CreateStack back and WARN mode must record HOOK_COMPLETE_FAILED yet let the stack complete), none of which can even be set up.
Fix: the Output resolution/serialization in the cloudformation DescribeStacks handler (and the stack state it reads from). Several open issues report this same omission for other service headings — e.g. 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md and 20261105T022225Z-cloudformation-lambda-2-net-new-causes-describestacks-outputs.md — and none carries these two checks' replay keys.
Traces: cloudformation.deep-cloudformation-1-cfn-enforce-fail-hook-blocks-unencrypted-queue-create, cloudformation.deep-cloudformation-1-cfn-enforce-warn-hook-flags-but-allows-unencrypted-queue-create.

Cause 4 of 4: a stack update that AWS refuses at the resource level is accepted and reported as a successful update, so no rollback happens and the prior configuration is silently replaced.
AWS: recorded behavior only, from Lambda's documented reserved environment keys. An UpdateStack that adds the Lambda-reserved key `AWS_REGION` to AWS::Lambda::Function Environment.Variables fails the resource: the handler relays Lambda's HTTP 400 as `Resource handler returned message: "Lambda was unable to configure your environment variables because the environment variables you have provided contains reserved keys that are currently not supported for modification. Reserved keys used in this request: AWS_REGION (Service: Lambda, Status Code: 400, Request ID: <uuid>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)` — the reserved key name is taken from the request, the uuids and attempt count are per-run — and the stack ends UPDATE_ROLLBACK_COMPLETE with the function keeping its pre-update configuration, so `cdk deploy` exits non-zero.
Emulator: the update is applied and the stack reports success (`cdk deploy` exits 0, stack UPDATE_COMPLETE, outputs printed); the reserved key is accepted and the only signal is a client-side CDK template-validation warning, which is not AWS's refusal.
Why: a client is told a configuration AWS rejects is live. The whole-update atomicity guarantee is broken: the companion property change in the same update is committed with no way to learn the stack is in a state AWS would never have produced.
Fix: the AWS::Lambda::Function update path of the cloudformation provisioner must surface the resource handler's failure (relayed verbatim) and drive UPDATE_ROLLBACK_COMPLETE; the reserved-environment-key validation belongs in lambda's UpdateFunctionConfiguration validation.
Traces: cloudformation.deploy-cdk-update-rejected-by-aws-rolls-back-keeping-prior-config.

## Evidence
Cause 1 — cloudformation.cdk-s3-auto-delete-objects-empties-past-destroy-manual-bucket-blocks-on-bucketnotempty [mismatch]:
  AssertionError: `cdk deploy <stack> --require-approval never` exited 1, expected 0; output tail: '… DeploymentError: Resource updates failed:\n<stack>/Auto/AutoDeleteObjectsCustomResource/Default  (Custom::S3AutoDeleteObjects …)\n  Resource handler returned message: "The operation cannot be performed at this time. The function is currently in the following state: Pending" (RequestToken: <uuid>, HandlerErrorCode: ResourceConflict)'
Cause 1 — cloudformation.stack-continue-update-rollback-client-request-token-marks-its-events [mismatch]:
  AssertionError: create ended {… 'StackStatus': 'ROLLBACK_COMPLETE', 'StackStatusReason': 'The following resource(s) failed to create: [Custom]. ' …} — expected CREATE_COMPLETE for the v1 template (resources Custom/Fn/Role).
Cause 1 — cloudformation.stack-continue-update-rollback-force-rollback-skips-failed-resource [mismatch]:
  AssertionError: create ended {… 'StackStatus': 'ROLLBACK_COMPLETE', 'StackStatusReason': 'The following resource(s) failed to create: [Custom]. ' …}
  Fixed text of the observed handler message: 'Resource handler returned message: "The operation cannot be performed at this time. The function is currently in the following state: Pending" (RequestToken: …, HandlerErrorCode: ResourceConflict)'. Variable: the request token. On AWS this is a transient state, never a create's terminal outcome.

Cause 2 — cloudformation.continue-update-rollback-on-stable-stack-fails [mismatch]: Error: Check timed out after 60.0s.
Cause 2 — cloudformation.stack-continue-update-rollback-lifecycle [mismatch]: Error: Check timed out after 60.0s (the check's first step is `req = _next_request(sqs, queue_url, timeout_s=90.0)` expecting the Create request CloudFormation publishes to the Custom:: resource's SNS ServiceToken; nothing arrives).
Cause 2 — cloudformation.stack-update-cancel-then-continue-rollback [mismatch]: Error: Check timed out after 60.0s (same SNS->SQS handshake, used as the check's own positive control before CancelUpdateStack).

Cause 3 — cloudformation.deep-cloudformation-1-cfn-enforce-fail-hook-blocks-unencrypted-queue-create [error]:
  KeyError: 'OutputValue' at `outs = {o["OutputKey"]: o["OutputValue"] for o in st.get("Outputs", [])}` on the Guard-hook stack's DescribeStacks response.
Cause 3 — cloudformation.deep-cloudformation-1-cfn-enforce-warn-hook-flags-but-allows-unencrypted-queue-create [error]:
  KeyError: 'OutputValue' at the same expression.
  Expected: every Outputs entry carries OutputKey AND the resolved OutputValue. Observed: OutputValue absent.

Cause 4 — cloudformation.deploy-cdk-update-rejected-by-aws-rolls-back-keeping-prior-config [mismatch]:
  AssertionError: `cdk deploy` exited 0 updating Environment.Variables to include the Lambda-reserved key AWS_REGION; AWS's UpdateFunctionConfiguration must refuse this. Output tail: '… ✅  <stack> … Deployment time: 1.13s … Outputs: <stack>.FunctionName = … [Warning] Template validation found issues in your templates (reported as warnings). … WARNING Environment.Variables: Environment variable 'AWS_REGION' is a Lambda reserved key (CloudFormation Validate)'
  Expected: non-zero exit, stack UPDATE_ROLLBACK_COMPLETE, resource failure reason opening 'Resource handler returned message: "Lambda was unable to configure your environment variables because the environment variables you have provided contains reserved keys that are currently not supported for modification. Reserved keys used in this request: AWS_REGION', with '(Service: Lambda, Status Code: 400' and HandlerErrorCode InvalidRequest, and the function keeping its pre-update Environment.

Smithy consulted: ContinueUpdateRollback (input StackName required, RoleARN, ResourcesToSkip, ClientRequestToken @length 1..128 @pattern ^[a-zA-Z0-9][-a-zA-Z0-9]*$, ForceRollback; error TokenAlreadyExistsException 400) and DescribeStacks output Stacks[].Outputs (Output: OutputKey, OutputValue, Description, ExportName). The handler-relayed message texts are recorded behavior.

## Steps to Reproduce
1. Cause 1: deploy a stack with an AWS::IAM::Role, an AWS::Lambda::Function and a Custom:: resource whose ServiceToken is that function (e.g. the CDK auto-delete-objects construct, or a template with resources Role/Fn/Custom) — expect CREATE_COMPLETE; observe the Custom resource CREATE_FAILED with 'Resource handler returned message: "The operation cannot be performed at this time. The function is currently in the following state: Pending" (… HandlerErrorCode: ResourceConflict)' and the stack ROLLBACK_COMPLETE.
2. Cause 2: create an SQS queue subscribed to an SNS topic, deploy a stack whose Custom:: resource uses that topic as ServiceToken, and receive from the queue — expect a request with RequestType 'Create', RequestId, StackId, LogicalResourceId and ResponseURL; observe nothing. Then, on a stable (CREATE_COMPLETE) stack, call ContinueUpdateRollback and confirm it is refused, and drive the UPDATE_ROLLBACK_FAILED -> ContinueUpdateRollback(ResourcesToSkip/ForceRollback) -> UPDATE_ROLLBACK_COMPLETE path and CancelUpdateStack.
3. Cause 3: create any stack that declares Outputs and call DescribeStacks — every Outputs entry must carry OutputValue alongside OutputKey.
4. Cause 4: deploy a stack with an AWS::Lambda::Function, then update it so Environment.Variables contains 'AWS_REGION' — expect the update to fail, the stack to reach UPDATE_ROLLBACK_COMPLETE with the Lambda reserved-key message relayed by the resource handler, and GetFunctionConfiguration to still show the pre-update Environment; observe UPDATE_COMPLETE with the reserved key applied.
5. Re-replay: cloudformation.cdk-s3-auto-delete-objects-empties-past-destroy-manual-bucket-blocks-on-bucketnotempty, cloudformation.stack-continue-update-rollback-client-request-token-marks-its-events, cloudformation.stack-continue-update-rollback-force-rollback-skips-failed-resource, cloudformation.continue-update-rollback-on-stable-stack-fails, cloudformation.stack-continue-update-rollback-lifecycle, cloudformation.stack-update-cancel-then-continue-rollback, cloudformation.deep-cloudformation-1-cfn-enforce-fail-hook-blocks-unencrypted-queue-create, cloudformation.deep-cloudformation-1-cfn-enforce-warn-hook-flags-but-allows-unencrypted-queue-create, cloudformation.deploy-cdk-update-rejected-by-aws-rolls-back-keeping-prior-config.
