Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.blueprint-lifecycle-create-start-run-delete
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.blueprint-lifecycle-create-start-run-delete|run#25|AssertionError|assert message == expected_message, (
Issue-Commit: 8d2d24246537bab720d4405ea7ed505663212620

# Glue: GetBlueprint's EntityNotFoundException message is emulator-authored ('Blueprint <name> not found.') instead of Glue's fixed 'Blueprint with given name not found.'

> _Note: this issue was identified against the code base at commit `8d2d24246537bab720d4405ea7ed505663212620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBlueprint

## Description
1 cause; it must be closed before submitting.

Related open issue, named for the fixer: 20261029T185330Z-glue-10-net-new-root-causes-ten-refusal.md collects ten Glue refusal-message divergences and its Cause 8 covers CreateBlueprint's DUPLICATE-name message ('Blueprint with given name already exists'). It does NOT cover the blueprint NOT-FOUND message below, and it is grounded in glue.blueprint-duplicate-name-rejected, not in the check listed here. This is the not-found path of the same resource family.

Cause 1 of 1: a blueprint lookup miss is reported with a request-derived sentence instead of Glue's constant not-found text.
AWS: Smithy: GetBlueprint (input Name, NameString @length 1..255, required) models EntityNotFoundException (errorKind client, HTTP 400); StartBlueprintRun (input BlueprintName, OrchestrationNameString @length 1..128, required) models the same error. When the named blueprint does not exist in the account's catalog — never created, or deleted by DeleteBlueprint — real Glue answers EntityNotFoundException whose Message is the FIXED, invariant text `Blueprint with given name not found.` The whole sentence is fixed text: no blueprint name, no request- or state-derived substitution anywhere in it, and the same text whichever of the blueprint reads was called.
Emulator: the HTTP status (400) and the error code (EntityNotFoundException) are already right; only the Message diverges. The emulator answers `Blueprint <Name> not found.` — the request's Name interpolated into emulator-authored prose. In the replayed check the name happened to be a per-run generated value (`sim2real-blueprint-d57f7e24d9c0`); that value is not part of the requirement, the point is that AWS's message carries NO name at all while the emulator's varies with every request.
Why: the error code EntityNotFoundException is shared by every Glue entity (table, database, connection, session, blueprint, trigger, ...), so the only thing on the wire that identifies WHICH entity was missing is the fixed message text. A client, SDK helper or test that matches Glue's documented sentence never matches, and because the emulator's text varies per request it cannot be matched exactly at all. A caller distinguishing 'this blueprint is gone' from any other not-found therefore misclassifies the refusal.
Fix: the blueprint-lookup-miss message shared by the blueprint handlers — the helper `crate::state::messages::blueprint_not_found` called at services/service-com-amazonaws-glue-awsglue/src/ops/get_blueprint.rs:174 (and the same helper on the StartBlueprintRun / DeleteBlueprint miss paths) must return the bare fixed text instead of formatting the requested name into it.
Traces: glue.blueprint-lifecycle-create-start-run-delete (the assertion fires on GetBlueprint after DeleteBlueprint; the same check then asserts the identical fixed message for StartBlueprintRun on the deleted blueprint, which is unreached while GetBlueprint diverges).

## Evidence
Failing check glue.blueprint-lifecycle-create-start-run-delete (verdict: mismatch), at check_glue_blueprint_lifecycle_create_start_run_delete.py line 100:

  AssertionError: expected GetBlueprint's not-found message 'Blueprint with given name not found.', got 'Blueprint sim2real-blueprint-d57f7e24d9c0 not found.'

Expected (AWS): HTTP 400, Error.Code = EntityNotFoundException, Error.Message = 'Blueprint with given name not found.' — the entire sentence is fixed text.
Actual (emulator): HTTP 400, Error.Code = EntityNotFoundException (both correct), Error.Message = 'Blueprint <Name> not found.' where <Name> is the GetBlueprint request's Name member. 'sim2real-blueprint-d57f7e24d9c0' in the replay is a per-run generated name filling that slot, not a requirement.

The check reaches the assertion only after the preceding steps pass (CreateBlueprint settles ACTIVE, StartBlueprintRun reaches SUCCEEDED, DeleteBlueprint succeeds), so the divergence is isolated to the wording of the post-delete GetBlueprint refusal. The same check asserts the same fixed message for StartBlueprintRun against the deleted blueprint.

## Steps to Reproduce
1. glue.CreateBlueprint(Name=B, BlueprintLocation='s3://<bucket>/<key>.zip') and wait for GetBlueprint(Name=B).Blueprint.Status == 'ACTIVE'.
2. glue.StartBlueprintRun(BlueprintName=B, Parameters=<json>, RoleArn=<glue role>) and wait for the run to reach a terminal state.
3. glue.DeleteBlueprint(Name=B).
4. glue.GetBlueprint(Name=B) -> must be HTTP 400 EntityNotFoundException with Error.Message exactly 'Blueprint with given name not found.' (no name interpolated). The emulator returns 'Blueprint B not found.'
5. glue.StartBlueprintRun(BlueprintName=B, ...) on the same deleted blueprint -> must be HTTP 400 EntityNotFoundException with the same exact Message 'Blueprint with given name not found.'
A GetBlueprint(Name=<never created>) reproduces step 4 on its own.
