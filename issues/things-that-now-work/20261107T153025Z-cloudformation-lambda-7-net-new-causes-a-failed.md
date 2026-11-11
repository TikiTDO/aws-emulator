Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda.cfnuf-alias-update-to-missing-versions-rejected-with-rollback-disabled cloudformation__lambda.e2e-cfnuf-fix-cloudformation-update-failed-stack-lifecycle cloudformation__lambda.cfnuf-update-failed-events-name-the-version-qualified-missing-function cloudformation__lambda.cfnuf-fix-forward-from-update-failed-completes-without-touching-the-aliases cloudformation__lambda.cfnuf-default-update-to-missing-alias-version-rolls-back-automatically cloudformation__lambda.cfnuf-update-failed-reason-wraps-lambdas-updatealias-rejection-verbatim cloudformation__lambda.cfnuf-rollback-stack-from-update-failed-restores-the-last-stable-template cloudformation__lambda.cfnuf-create-with-missing-alias-version-and-rollback-disabled-keeps-its-resources cloudformation__lambda.sam-function-globals-autopublishalias-publishes-new-version
Issue-Key: cloudformation__lambda.cfnuf-alias-update-to-missing-versions-rejected-with-rollback-disabled|run#11|AssertionError|assert failed == ["Fn0Alias", "Fn1Alias"], (
Issue-Key: cloudformation__lambda.e2e-cfnuf-fix-cloudformation-update-failed-stack-lifecycle|phase#10|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: cloudformation__lambda.cfnuf-update-failed-events-name-the-version-qualified-missing-function|run#8|AssertionError|assert seq == ["UPDATE_IN_PROGRESS", "UPDATE_FAILED", "UPDATE_ROLLBACK_IN_PROGRESS",
Issue-Key: cloudformation__lambda.cfnuf-fix-forward-from-update-failed-completes-without-touching-the-aliases|run#12|AssertionError|assert aliases == {"Fn0Alias": "UPDATE_ROLLBACK_COMPLETE", "Fn1Alias": "UPDATE_ROLLBACK_COMPLETE"}, (
Issue-Key: cloudformation__lambda.cfnuf-default-update-to-missing-alias-version-rolls-back-automatically|run#12|AssertionError|assert "UPDATE_FAILED" in alias_seq and alias_seq[-1] == "UPDATE_COMPLETE", (
Issue-Key: cloudformation__lambda.cfnuf-update-failed-reason-wraps-lambdas-updatealias-rejection-verbatim|run#7|AssertionError|assert cfn_reason.startswith(_HANDLER_MSG_PREFIX), (
Issue-Key: cloudformation__lambda.cfnuf-rollback-stack-from-update-failed-restores-the-last-stable-template|run#10|AssertionError|assert seq == ["UPDATE_ROLLBACK_IN_PROGRESS", "UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS",
Issue-Key: cloudformation__lambda.cfnuf-create-with-missing-alias-version-and-rollback-disabled-keeps-its-resources|run#6|AssertionError|assert stack.resources() == {"Role": "CREATE_COMPLETE", "Fn0": "CREATE_COMPLETE",
Issue-Key: cloudformation__lambda.sam-function-globals-autopublishalias-publishes-new-version|run#36|AssertionError|assert versions == ["$LATEST", "1", "2"], (
Issue-Commit: afd9c28cd10d0281c7cc3c2a5c51ef0c1f3c612b

# cloudformation__lambda: 7 net-new causes — a failed update stops at the first failed resource, failed resources are never rolled back individually, UPDATE_FAILED events carry no reason, a rollback-disabled create retains nothing, and a code update publishes no new Lambda Version

> _Note: this issue was identified against the code base at commit `afd9c28cd10d0281c7cc3c2a5c51ef0c1f3c612b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Seven net-new root causes in the CloudFormation per-resource update/rollback accounting exposed by the `cfnuf` (CloudFormation UPDATE_FAILED) check family. All of them are driven by the same carved stack: one IAM role plus, per function, an AWS::Lambda::Function, an AWS::Lambda::Version and an AWS::Lambda::Alias named LATEST, where an update rewrites each alias's FunctionVersion to a literal version number that does not exist, so Lambda's UpdateAlias refuses it and the alias resource must fail. None of these seven is covered by an existing open issue; the nearest open items (20261105T022225Z-cloudformation-lambda-2-net-new-causes-describestacks-outputs.md — a stack-declared AWS::Lambda::Alias does not exist after CREATE; 20261105T081225Z-cloudformation-lambda-ssm-a-stack-update-that-changes.md — a Lambda function update ends UPDATE_FAILED with ResourceConflict; 20261105T094842Z-cloudformation-lambda-s3-sqs-2-net-new-root-7fb457.md) describe different services and different failures, and here the create reaches CREATE_COMPLETE and the update does reach UPDATE_FAILED.

Cause 1 of 7: an update whose template breaks SEVERAL independent resources stops at the first failure.
AWS: recorded behavior only (the Smithy model types UpdateStack/DescribeStackEvents but states no ordering rule). Real AWS attempts every resource in the update whose dependencies allow it, so BOTH aliases get their own UPDATE_IN_PROGRESS + UPDATE_FAILED resource event, and the stack-level StackStatusReason names the whole failed set: fixed text `The following resource(s) failed to update: [` + `, `-joined logical ids + `]. ` — the logical ids are values from the request's template, the wrapper is the requirement.
Emulator: StackStatusReason is `The following resource(s) failed to update: [Fn0Alias]. ` — only the FIRST alias; the second alias is never attempted and DescribeStackEvents holds 0 UPDATE_FAILED events for it (the e2e's `GetDetectorOutcomeLambdaAlias has 0 UPDATE_FAILED events`).
Why: a client that diagnoses a failed deploy from the failed-resource list or from the per-resource events (exactly what the e2e flow does: read events, derive each broken version, fix forward) sees only one of the broken resources and ships a half-fix that fails again.
Fix: the stack update engine's per-resource execution/failure collection for cloudformation UpdateStack.
Traces: cloudformation__lambda.cfnuf-alias-update-to-missing-versions-rejected-with-rollback-disabled, cloudformation__lambda.e2e-cfnuf-fix-cloudformation-update-failed-stack-lifecycle (phase 3/5 solve).

Cause 2 of 7: on an UPDATE_FAILED stack (update submitted with DisableRollback=true) the failed resources are never rolled back individually.
AWS: recorded behavior only. With rollback disabled AWS still rolls back ONLY the resources that failed: each failed resource gets UPDATE_ROLLBACK_IN_PROGRESS then UPDATE_ROLLBACK_COMPLETE events and DescribeStackResources reports it as UPDATE_ROLLBACK_COMPLETE, while every untouched resource keeps CREATE_COMPLETE. A later fix-forward update whose template equals that already-rolled-back live state writes NO resource events and leaves the aliases at UPDATE_ROLLBACK_COMPLETE.
Emulator: the alias's event sequence stops at ['UPDATE_IN_PROGRESS', 'UPDATE_FAILED'] (no rollback events at all), and after the fix-forward the aliases read {'Fn0Alias': 'CREATE_COMPLETE', 'Fn1Alias': 'CREATE_COMPLETE'} instead of UPDATE_ROLLBACK_COMPLETE.
Why: a client cannot tell from DescribeStackResources which resources were rolled back and which were left applied, and the fix-forward re-touches resources AWS leaves alone.
Fix: the per-resource rollback step of the update engine (status + event emission for failed resources under DisableRollback).
Traces: cloudformation__lambda.cfnuf-update-failed-events-name-the-version-qualified-missing-function, cloudformation__lambda.cfnuf-fix-forward-from-update-failed-completes-without-touching-the-aliases.

Cause 3 of 7: in a DEFAULT (rollback-enabled) update the automatic rollback does not re-apply the old property to the failed resource.
AWS: recorded behavior only. The stack auto-rolls to UPDATE_ROLLBACK_COMPLETE and the failed resource's event sequence contains UPDATE_FAILED and then ENDS in UPDATE_COMPLETE (the rollback re-applies the previous FunctionVersion), with DescribeStackResources reporting the resource UPDATE_COMPLETE.
Emulator: the stack does reach UPDATE_ROLLBACK_COMPLETE, but the alias's events are ['UPDATE_IN_PROGRESS', 'UPDATE_FAILED'] — the rollback never re-applies the old value nor records the resource's recovery.
Why: the resource's recorded state contradicts the live resource after a successful rollback, so drift/inventory logic built on resource statuses is wrong.
Fix: the automatic-rollback path of the update engine (re-apply previous properties and emit the resource's terminal status).
Traces: cloudformation__lambda.cfnuf-default-update-to-missing-alias-version-rolls-back-automatically.

Cause 4 of 7: UPDATE_FAILED resource events carry no ResourceStatusReason.
AWS: recorded behavior only, confirmed verbatim against real AWS by the check. CloudFormation wraps the provisioner's own rejection: fixed text `Resource handler returned message: "` + the downstream service message verbatim + `" (RequestToken: <token>, HandlerErrorCode: <code>)`. For this case the wrapped message is Lambda's own UpdateAlias ResourceNotFoundException text (which names the version-qualified function ARN `<function-arn>:<version>`) and the HandlerErrorCode is `NotFound`. The token and the ARN/version are values from the request and state; the wrapper, the verbatim-carry rule and the HandlerErrorCode classification are the requirement.
Emulator: the UPDATE_FAILED event's ResourceStatusReason is the empty string `''` — neither the wrapper, nor Lambda's message, nor a HandlerErrorCode.
Why: the whole diagnose-from-events workflow is impossible — a client cannot learn WHY a resource failed or which version was missing, and cannot classify the failure.
Fix: resource-failure reason propagation in the update engine's event writer (carry the provisioner's error message and its handler error code).
Traces: cloudformation__lambda.cfnuf-update-failed-reason-wraps-lambdas-updatealias-rejection-verbatim.

Cause 5 of 7: RollbackStack's stack-level event sequence omits UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS.
AWS: recorded behavior only (RollbackStack is modeled with no extra errors). RollbackStack on an UPDATE_FAILED stack writes the stack-level statuses UPDATE_ROLLBACK_IN_PROGRESS -> UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS -> UPDATE_ROLLBACK_COMPLETE.
Emulator: the stack-level events are ['UPDATE_ROLLBACK_IN_PROGRESS', 'UPDATE_ROLLBACK_COMPLETE'] — the cleanup phase is never represented.
Why: a client that waits on or audits the documented rollback phases (CLI waiters, event-stream consumers, change auditors) never sees the cleanup phase and mis-reads the rollback timeline.
Fix: the rollback state machine's stack-level status transitions/event emission.
Traces: cloudformation__lambda.cfnuf-rollback-stack-from-update-failed-restores-the-last-stable-template.

Cause 6 of 7: a CREATE that fails with DisableRollback=true retains nothing, and DescribeStackResources lists no resources at all.
AWS: recorded behavior only. A create submitted with DisableRollback=true that fails on one resource lands CREATE_FAILED and KEEPS everything already created: DescribeStackResources reports the role, the function and the Version as CREATE_COMPLETE and the failing resource as CREATE_FAILED, and Lambda still resolves the created function until RollbackStack is called.
Emulator: DescribeStackResources on the CREATE_FAILED stack returns an empty set `{}` — no resource rows at all, so the kept estate is unobservable.
Why: the entire point of DisableRollback on create — inspect the partially created estate, then fix or roll back — is unusable; a client cannot see what exists and what leaked.
Fix: the create-failure path's resource bookkeeping (retain and report per-resource rows when rollback is disabled).
Traces: cloudformation__lambda.cfnuf-create-with-missing-alias-version-and-rollback-disabled-keeps-its-resources.

Cause 7 of 7: a stack update that changes a function's code publishes no new AWS::Lambda::Version and does not move the alias.
AWS: recorded behavior only, plus the SAM AutoPublishAlias contract (https://docs.aws.amazon.com/serverless-application-model/latest/developerguide/sam-resource-function.html#sam-function-autopublishalias): the transform mints a content-hashed `FnVersion<10 hex>` Version with DeletionPolicy Retain and an alias following it; an update that changes the code replaces that Version resource, so ListVersionsByFunction reports ['$LATEST', '1', '2'], the retained version 1 stays invokable, and the alias moves to '2'.
Emulator: after the UPDATE_COMPLETE update ListVersionsByFunction still reports ['$LATEST', '1'] — no version 2 is published, so the alias cannot move and an invoke through the alias serves the old code.
Why: every blue/green or alias-pinned deployment silently serves the previous code while the stack reports UPDATE_COMPLETE.
Fix: the AWS::Lambda::Version provisioner's replacement-on-update handling (and the alias update that follows it).
Traces: cloudformation__lambda.sam-function-globals-autopublishalias-publishes-new-version.

All seven are grounded in the cloudformation__lambda suite section; the handler surface is services/service-com-amazonaws-cloudformation-cloudformation/src/ops/ (update_stack.rs, rollback_stack.rs, describe_stack_events.rs, describe_stack_resources.rs) and the Lambda Version/Alias provisioning path.

## Evidence
Cause 1: expected StackStatusReason to name both aliases; got 'The following resource(s) failed to update: [Fn0Alias]. ' — AssertionError: StackStatusReason lists ['Fn0Alias'] as failed, expected both aliases. The e2e fails with 'GetDetectorOutcomeLambdaAlias has 0 UPDATE_FAILED events'.
Cause 2: AssertionError: Fn0Alias's events in the failed update are ['UPDATE_IN_PROGRESS', 'UPDATE_FAILED'] (expected ['UPDATE_IN_PROGRESS','UPDATE_FAILED','UPDATE_ROLLBACK_IN_PROGRESS','UPDATE_ROLLBACK_COMPLETE']); AssertionError: alias resource statuses after the fix-forward: {'Fn0Alias': 'CREATE_COMPLETE', 'Fn1Alias': 'CREATE_COMPLETE'} (expected both UPDATE_ROLLBACK_COMPLETE).
Cause 3: AssertionError: Fn0Alias's events: ['UPDATE_IN_PROGRESS', 'UPDATE_FAILED'] — expected UPDATE_FAILED present and the sequence to END in UPDATE_COMPLETE after the automatic rollback.
Cause 4: AssertionError: Fn1Alias: UPDATE_FAILED reason does not open with CloudFormation's handler-message wrapper 'Resource handler returned message: "': '' (empty reason; AWS also appends '(RequestToken: ..., HandlerErrorCode: NotFound)' and carries Lambda's UpdateAlias ResourceNotFoundException message verbatim).
Cause 5: AssertionError: the rollback's stack-level events are ['UPDATE_ROLLBACK_IN_PROGRESS', 'UPDATE_ROLLBACK_COMPLETE'] — expected UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS in the middle.
Cause 6: AssertionError: resources kept by the failed create: {} — expected {'Role': 'CREATE_COMPLETE', 'Fn0': 'CREATE_COMPLETE', 'Fn0Version': 'CREATE_COMPLETE', 'Fn0Alias': 'CREATE_FAILED'}.
Cause 7: AssertionError: the update must publish version 2 and keep the retained version 1; got ['$LATEST', '1'].

## Steps to Reproduce
1. CreateStack with Capabilities=[CAPABILITY_IAM] on a template holding one IAM role plus, per function i, an AWS::Lambda::Function (Code.ZipFile, python3.11), an AWS::Lambda::Version {FunctionName: Ref Fni} and an AWS::Lambda::Alias {Name: LATEST, FunctionVersion: !GetAtt FniVersion.Version}; wait for CREATE_COMPLETE.
2. Cause 1/2/4: UpdateStack with the SAME template except each alias's FunctionVersion is a literal nonexistent version (e.g. '593' and '268'), DisableRollback=True. Wait for UPDATE_FAILED, then DescribeStacks (StackStatusReason), DescribeStackEvents (per-alias event sequence and ResourceStatusReason) and DescribeStackResources.
3. Cause 2 (second half): UpdateStack again with the corrected template, DisableRollback=True; expect UPDATE_COMPLETE with no resource events and the aliases still UPDATE_ROLLBACK_COMPLETE.
4. Cause 3: repeat step 2 WITHOUT DisableRollback; the stack auto-rolls back — inspect the alias's events (must end UPDATE_COMPLETE) and DescribeStackResources.
5. Cause 5: from UPDATE_FAILED call RollbackStack and read the stack-level event statuses.
6. Cause 6: CreateStack a fresh stack whose alias already hardcodes a missing version, with DisableRollback=True; wait for CREATE_FAILED and call DescribeStackResources (and lambda GetFunction on the created function).
7. Cause 7: deploy the SAM template using Globals + AutoPublishAlias: live, then UpdateStack changing the function's code/parameter; call lambda ListVersionsByFunction and GetAlias.
