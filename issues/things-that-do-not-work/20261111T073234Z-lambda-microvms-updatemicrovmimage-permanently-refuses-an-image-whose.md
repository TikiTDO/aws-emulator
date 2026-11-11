Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-microvms.update-image-while-creating-rejected
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms|UpdateMicrovmImage|ValidationException
Issue-Commit: f4372f081be041f1c06808452ec2531466fafcb7

# lambda-microvms UpdateMicrovmImage permanently refuses an image whose first build failed: CREATE_FAILED is excluded from the states that admit an update

> _Note: this issue was identified against the code base at commit `f4372f081be041f1c06808452ec2531466fafcb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateMicrovmImage

## Description
REOPENS 20261025T210105Z-lambda-microvms-updatemicrovmimage-stub.md: its fix left the call refused. That issue carried this very check id (`lambda-microvms.update-image-while-creating-rejected`) and asked for a real `UpdateMicrovmImage` handler; the handler now exists and the in-flight (CREATING) refusal it added is correct, but the same check still fails on an `UpdateMicrovmImage` refusal — now a modelled ValidationException instead of the stub's 501 — on the call that AWS accepts.

This issue carries 1 net-new root cause. A fixer must close Cause 1 of 1 before submitting.

=== Cause 1 of 1: an image that has SETTLED in CREATE_FAILED is refused an update, so a failed image can never be re-configured ===
Failing check: lambda-microvms.update-image-while-creating-rejected.

AWS: Smithy `UpdateMicrovmImage` is `PUT /2025-09-09/microvm-images/{imageIdentifier}`, success code 200, documented "Updates the configuration of a MicroVM image and triggers a new version build. This operation uses PUT semantics - all required fields (codeArtifact, baseImageArn, buildRoleArn) must be provided with every request." Its output declares `state` (@required, MicrovmImageState) and `imageVersion` (@required). Its modelled errors are AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400); the model states NO rule restricting the update to images whose creation build succeeded. Recorded behavior only, as the check records it: while the image is still CREATING the call is refused with ValidationException and message `Cannot update MicroVM Image in its current state: <imageArn>` and adds no version; once the image has LEFT CREATING - and the check's settle set is explicitly {CREATED, CREATE_FAILED} - the identical call is ACCEPTED with HTTP 200, `state` = `UPDATING` and a NEW `imageVersion` distinct from the existing one, after which `ListMicrovmImageVersions` reports 2 versions. Re-PUTting the configuration is therefore how a client repairs an image whose first build failed (for example a `codeArtifact.uri` pointing at a bucket that does not exist): the failed image is not a dead record, it accepts a further version.

Emulator: once the image settles in `CREATE_FAILED`, `UpdateMicrovmImage` is refused with HTTP 400 `ValidationException`, message `Cannot update MicroVM Image in its current state: arn:aws:lambda:<region>:<account>:microvm-image:<name>`. In that message the fixed text is `Cannot update MicroVM Image in its current state: ` and the only variable part is the image ARN the service minted for the record; the wording itself is correct for the states that genuinely admit no update (the check's CREATING assertions on code and message both pass) - the defect is WHICH states are in that set. The guard admits only CREATED, UPDATED and UPDATE_FAILED, so an image whose creation build failed is refused forever and the emulator offers no path from CREATE_FAILED other than deletion.

Why: a client that reacts to a failed build by re-PUTting a corrected configuration - the normal repair loop for a bad code artifact, and the loop this check exercises as its positive control - gets a hard 400 against the emulator instead of the 200/UPDATING it gets from AWS. The error class is a client-fault validation error, so an SDK will not retry it; the client concludes its request is malformed rather than that the image is repairable, and the version history the update should have grown (2 versions) never appears. Any build-repair, re-base or CI retry path is dead code against the emulator.

Fix: the state-admission predicate behind the handler's pre-write guard - `image_state_admits_update` in the service's state module, consulted by the `UpdateMicrovmImage` handler before anything is written - must treat a settled-but-failed creation as updatable, alongside the settled CREATE/UPDATE states it already admits, while keeping the in-flight states (CREATING/UPDATING/DELETING) and the deletion states refused with the existing message. The accepted call must then mint the next `major.minor` version, report `state` = `UPDATING` and return the new `imageVersion`, so the subsequent `ListMicrovmImageVersions` shows both versions.

Related open issue, which does NOT cover this cause: 20261102T072953Z-lambda-microvms-image-version-records-are-written-from.md is about members of the update path's 200 response (`latestFailedImageVersion`) and about versions never settling FAILED; it carries no key for an UpdateMicrovmImage refusal and its fix would not widen the state-admission set.

Traces: lambda-microvms.update-image-while-creating-rejected (the check's POSITIVE CONTROL step, after `_await_image_state(..., {CREATED, CREATE_FAILED})` returns).

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service heading `### lambda-microvms`, check `update-image-while-creating-rejected` - verdict [error]:

  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the UpdateMicrovmImage operation: Cannot update MicroVM Image in its current state: arn:aws:lambda:us-west-2:000000000000:microvm-image:sim2real-behav-mvm-<id>

The exception propagates out of the check (it is NOT caught by the check's `_error_or_none` wrapper, which only wraps the CREATING-state call), i.e. it was raised by the unwrapped positive-control call:

    _await_image_state(mvm, image_arn, {"CREATED", "CREATE_FAILED"}, ...)
    updated = mvm.update_microvm_image(imageIdentifier=image_arn, baseImageArn=base,
                                       buildRoleArn=role, codeArtifact=artifact,
                                       description="second")
    assert updated["state"] == "UPDATING"
    assert updated["imageVersion"] != versions[0]["imageVersion"]

So the earlier assertions all passed: the CREATING refusal returned Code `ValidationException` with exactly `Cannot update MicroVM Image in its current state: <imageArn>`, and the refused update left exactly 1 version with description 'first'. The image then reached a state in {CREATED, CREATE_FAILED} (the wait returned rather than raising), and the identical call was still refused.

Expected (AWS): HTTP 200, `state` = `UPDATING`, `imageVersion` != the existing version, and `ListMicrovmImageVersions(imageIdentifier=<imageArn>)` then returning 2 items including the new version.
Actual (emulator): HTTP 400 `ValidationException`, message `Cannot update MicroVM Image in its current state: <imageArn>`; no version added.

Code grounding in the checkout:
- services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/update_microvm_image.rs lines 324-333: the pre-write guard `if !crate::state::image_state_admits_update(&image.state) { return Err(validation(crate::state::image_not_updatable_message(&image.image_arn))); }`.
- services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state.rs lines 270-275: `pub fn image_state_admits_update(state: &str) -> bool { matches!(state, IMAGE_STATE_CREATED | IMAGE_STATE_UPDATED | IMAGE_STATE_UPDATE_FAILED) }` - `IMAGE_STATE_CREATE_FAILED` is deliberately excluded (see the doc comment at lines 256-269: "one whose creation never produced a version (IMAGE_STATE_CREATE_FAILED)").
- services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state.rs lines 284-286: `image_not_updatable_message` = `format!("Cannot update MicroVM Image in its current state: {image_arn}")`.

Model grounding (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json): UpdateMicrovmImage http PUT /2025-09-09/microvm-images/{imageIdentifier} code 200, @idempotent; required input baseImageArn, buildRoleArn, codeArtifact, imageIdentifier(@httpLabel); required output imageArn, name, state(MicrovmImageState), createdAt, updatedAt, baseImageArn, buildRoleArn, codeArtifact, imageVersion; errors ValidationException(400), ConflictException(409), ResourceNotFoundException(404), ServiceQuotaExceededException(402), AccessDeniedException(403), ThrottlingException(429), InternalServerException(500). No modelled constraint confines the operation to images whose create build succeeded.

## Steps to Reproduce
1. mvm = client('lambda-microvms'); base = ListManagedMicrovmImages()['items'][0]['imageArn']; role = an IAM role ARN trusting lambda.amazonaws.com.
2. CreateMicrovmImage(name=<unique>, baseImageArn=base, buildRoleArn=role, codeArtifact={'uri': 's3://<unique>-absent/code.zip'}, description='first') and confirm the response's state is 'CREATING'.
3. While the image is CREATING, call UpdateMicrovmImage(imageIdentifier=<imageArn>, baseImageArn=base, buildRoleArn=role, codeArtifact=<same>, description='second').
   Both AWS and the emulator refuse with ValidationException / 'Cannot update MicroVM Image in its current state: <imageArn>' and add no version. This part is CORRECT.
4. Poll GetMicrovmImage(imageIdentifier=<imageArn>) until state is in {'CREATED','CREATE_FAILED'} (with an absent artifact bucket it settles in the failed branch).
5. Repeat the identical UpdateMicrovmImage call from step 3.
   AWS: HTTP 200 with state='UPDATING' and a new imageVersion; ListMicrovmImageVersions(imageIdentifier=<imageArn>) then returns 2 items including that new version.
   Emulator: HTTP 400 ValidationException 'Cannot update MicroVM Image in its current state: <imageArn>'; still 1 version. The image can never be updated again.
6. Re-replay check lambda-microvms.update-image-while-creating-rejected to confirm.
