Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-data-automation.invoke-blueprint-optimization-async-invalid-profile-rejected
Issue-Shape-Id: com.amazonaws.bedrockdataautomation#AmazonBedrockKeystoneBuildTimeService
Issue-Key: bedrock-data-automation.invoke-blueprint-optimization-async-invalid-profile-rejected|_error_code#9|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 65165f71b3b3c14984c314d4becd7f5d11cc460b

# InvokeBlueprintOptimizationAsync accepts an unresolvable dataAutomationProfileArn with 200 instead of refusing 400 ValidationException

> _Note: this issue was identified against the code base at commit `65165f71b3b3c14984c314d4becd7f5d11cc460b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeBlueprintOptimizationAsync

## Description
One net-new root cause in the bedrock-data-automation (Keystone build-time) emulator, found by replaying the behavioral suite simparity-behavioral-aws-v1. Every other failing check in this service's replay was reported OWNED by an open issue or ALREADY FILED against a scaffold stub (CopyBlueprintStage, CreateDataAutomationLibrary, ListDataAutomationProjects) and is NOT re-filed here.

=== Cause 1 of 1: InvokeBlueprintOptimizationAsync performs no dataAutomationProfileArn resolution, so a submission naming a nonexistent Data Automation profile is ACCEPTED (200, job minted) where AWS refuses it 400 ValidationException ===

AWS: Smithy (services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/smithy-model/com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice.json): InvokeBlueprintOptimizationAsync is POST /invokeBlueprintOptimizationAsync, success status 200, idempotent; its input REQUIRES blueprint (BlueprintOptimizationObject), samples, outputConfiguration and dataAutomationProfileArn, whose target DataAutomationProfileArn is @length(1..128) @pattern ^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:(aws|[0-9]{12}):data-automation-profile/[a-zA-Z0-9-_.]+$. The operation models ValidationException (errorKind client, @httpError 400) and ResourceNotFoundException (errorKind client, @httpError 404) among its errors. Recorded behavior: a dataAutomationProfileArn that satisfies that pattern but names no existing Data Automation profile is refused with HTTP 400, error code ValidationException and the message "Request has invalid Data Automation Profile Arn" — the message is entirely FIXED text with NO variable part (AWS echoes neither the offending ARN, nor the member name, nor the account/region), and the refusal happens before anything is minted, so the Blueprint referenced in the rejected call is left untouched (same blueprintArn, same schema on a subsequent GetBlueprint).

Emulator: the identical submission SUCCEEDS. The replay records HTTP 200 with body {"invocationArn": "arn:aws:bedrock:<region>:<account>:blueprint-optimization-invocation/<id>"} and no error at all, so the modelled ValidationException path for an unresolvable profile ARN is unreachable. Source confirmation: in services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/src/ops/invoke_blueprint_optimization_async.rs, handle() (from line 175) reaches the store and its ONLY pre-mint refusals are store.names_unknown_blueprint(...) -> ResourceNotFoundException (lines 197-209) and the OPTIMIZATION_JOB_QUOTA check (lines 213-226); it then mints an invocationArn and inserts a BlueprintOptimizationJobRecord (lines 228-255), moving input.data_automation_profile_arn straight into the record at line 248 without ever resolving or validating it beyond the framework's modelled @pattern/@length walk.

Why: an overly-permissive accept on a rejection axis. A client that mis-configures its Data Automation profile (wrong account, deleted profile, copied ARN from another region) gets a 200 and a minted invocationArn, records an in-flight optimization job, and then polls GetBlueprintOptimizationStatus forever for a job that AWS would never have created; the real fault — the profile ARN — is never surfaced. Conversely a test asserting the documented rejection ("a bogus profile ARN must be refused ValidationException, and must not create a job") passes in production and fails only against reality, because the emulator never raises the modelled exception, so every typed `except ValidationException` branch in the caller is dead code against the emulator.

Fix: the InvokeBlueprintOptimizationAsync handler's validation — resolve input.data_automation_profile_arn against the service's known Data Automation profiles and refuse with ValidationException / HTTP 400 / the fixed message "Request has invalid Data Automation Profile Arn" BEFORE the invocationArn is minted and the job persisted, so the referenced Blueprint stays untouched. Related open issues that do NOT cover this cause: 20261029T211335Z-bedrock-data-automation-ten-net-new-causes-create (its Cause 9 names this operation, but describes the emulator as answering 501 NotImplemented from a scaffold stub; that description is stale — the operation is now implemented and ACCEPTS the submission, a different emulator behavior and a different replay key, which is why the suite did not report this check as owned), and 20261030T134832Z-bedrock-data-automation-invokeblueprintoptimizationasync-does-not-validate-request-s3-uris-refusing (asks only for S3-URI validation of samples/outputConfiguration, says nothing about resolving the profile ARN).

Traces: bedrock-data-automation.invoke-blueprint-optimization-async-invalid-profile-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, 5/45 checks passed (19 mismatched, 21 errored, 0 skipped, 0 unloadable). All failures belong to service bedrock-data-automation. 26 of the failing entries were reported OWNED by 20261029T211335Z-bedrock-data-automation-ten-net-new-causes-create or ALREADY FILED against the CopyBlueprintStage / CreateDataAutomationLibrary (14 checks) / ListDataAutomationProjects scaffold stubs and are excluded. The single surviving failure is the one filed here.

[mismatch] bedrock-data-automation.invoke-blueprint-optimization-async-invalid-profile-rejected
  check_bedrock_data_automation_invoke_blueprint_optimization_async_invalid_profile_rejected.py, line 43 -> _error_code(), line 79:
  AssertionError: expected rejection, but the call SUCCEEDED and returned
  {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'invocationArn': 'arn:aws:bedrock:us-west-2:000000000000:blueprint-optimization-invocation/1cb91442-0000000000000001'}

The check creates a real Blueprint, then calls InvokeBlueprintOptimizationAsync with that blueprintArn and a syntactically valid but nonexistent dataAutomationProfileArn (arn:aws:bedrock:<region>:<account>:data-automation-profile/<random hex>), asserting:
  error code  == "ValidationException"
  error message == "Request has invalid Data Automation Profile Arn"
and then that GetBlueprint still returns the same blueprintArn and the same schema.

Expected (AWS): HTTP 400, code ValidationException, message exactly "Request has invalid Data Automation Profile Arn" (all fixed text, no variable parts), no invocationArn minted.
Actual (emulator): HTTP 200 with a minted invocationArn; no error raised.

The region, account id, blueprint name and the invocation/blueprint ids in the quoted response are that run's generated values and are NOT requirements. The requirements are: HTTP status 400, error code ValidationException, the exact fixed message text "Request has invalid Data Automation Profile Arn", and that no optimization job is created by the refused submission.

Model: InvokeBlueprintOptimizationAsync errors include ValidationException (client, httpError 400) and ResourceNotFoundException (client, httpError 404); dataAutomationProfileArn is required, target DataAutomationProfileArn @length(1..128) @pattern ^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:(aws|[0-9]{12}):data-automation-profile/[a-zA-Z0-9-_.]+$ — so a pattern-valid-but-unresolvable ARN passes the framework validation walk and reaches the handler.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite (simparity-behavioral-aws-v1) against the bedrock-data-automation emulator.
2. Replay check bedrock-data-automation.invoke-blueprint-optimization-async-invalid-profile-rejected: CreateBlueprint(blueprintName=<unique>, type=DOCUMENT, schema=<valid schema>), keep the returned blueprintArn.
3. Call InvokeBlueprintOptimizationAsync with blueprint={"blueprintArn": <that arn>}, one sample whose assetS3Object/groundTruthS3Object s3Uri values match the modelled S3Uri pattern, an outputConfiguration.s3Object, and dataAutomationProfileArn=arn:aws:bedrock:<region>:<account>:data-automation-profile/<random hex> (pattern-valid, no such profile exists).
4. Assert the call is REFUSED with HTTP 400, error code ValidationException and body message exactly "Request has invalid Data Automation Profile Arn" — not a 200 with a minted invocationArn.
5. Assert nothing was minted: no invocationArn is returned and GetBlueprintOptimizationStatus resolves no job for the refused submission.
6. Assert the referenced Blueprint is untouched: GetBlueprint(blueprintArn) still returns the same blueprintArn and the same schema.
