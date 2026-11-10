Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.create-pipeline-unknown-action-provider-rejected codepipeline.update-pipeline-malformed-declarations-rejected codepipeline.update-pipeline-invalid-action-declaration-rejected codepipeline.update-pipeline-invalid-stage-declaration-rejected codepipeline.disable-stage-transition-nonexistent-stage-rejected codepipeline.disable-stage-transition-without-permission-denied codepipeline.put-webhook-invalid-filter-pattern-rejected codepipeline.list-tags-for-resource-invalid-arn-rejected codepipeline.untag-resource-nonexistent-pipeline-rejected codepipeline.third-party-job-success-after-failure-not-permissive codepipeline.pipeline-stage-actions-execute-in-declared-order codepipeline.retry-stage-execution-not-failed-rejected codepipeline.retry-stage-execution-stale-token-rejected codepipeline.webhook-url-rejects-a-wrong-hmac-signature-and-starts-no-execution codepipeline.stop-completed-execution-rejected codepipeline.concurrent-pipeline-executions-limit-exceeded codepipeline.start-pipeline-execution-concurrency-limit-exceeded
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.create-pipeline-unknown-action-provider-rejected|run#13|AssertionError|assert code == "InvalidActionDeclarationException", (
Issue-Key: codepipeline.update-pipeline-malformed-declarations-rejected|_error#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: codepipeline.update-pipeline-invalid-action-declaration-rejected|_error#10|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: codepipeline.update-pipeline-invalid-stage-declaration-rejected|_error#11|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: codepipeline.disable-stage-transition-nonexistent-stage-rejected|_stage_inbound_enabled#6|KeyError|return s["inboundTransitionState"]["enabled"]
Issue-Key: codepipeline.disable-stage-transition-without-permission-denied|_error#10|AssertionError|raise AssertionError(f"expected AccessDeniedException, but the call SUCCEEDED and returned {result!r}")
Issue-Key: codepipeline|PutWebhook|PipelineNotFoundException
Issue-Key: codepipeline.untag-resource-nonexistent-pipeline-rejected|run#13|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: codepipeline.third-party-job-success-after-failure-not-permissive|run#13|AssertionError|assert fail_notfound_code == "JobNotFoundException", f"unexpected code: {fail_notfound_code}"
Issue-Key: codepipeline.pipeline-stage-actions-execute-in-declared-order|_wait_for_an_execution_id#14|AssertionError|raise AssertionError(f"pipeline {pipeline_name} never showed an auto-triggered execution within {timeout_s}s")
Issue-Key: codepipeline.retry-stage-execution-not-failed-rejected|_initial_execution_id#13|AssertionError|raise AssertionError(f"no CreatePipeline-triggered execution appeared for {pipeline_name} within {timeout_s}s")
Issue-Key: codepipeline.retry-stage-execution-stale-token-rejected|_discard_auto_triggered_execution#7|AssertionError|raise AssertionError("CreatePipeline's auto-triggered execution never became visible to list_pipeline_executions")
Issue-Key: codepipeline.webhook-url-rejects-a-wrong-hmac-signature-and-starts-no-execution|_poll#13|AssertionError|raise AssertionError(
Issue-Key: codepipeline.stop-completed-execution-rejected|_poll_until#6|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: codepipeline.concurrent-pipeline-executions-limit-exceeded|run#17|AssertionError|assert quota_error is not None, (
Issue-Key: codepipeline.start-pipeline-execution-concurrency-limit-exceeded|run#10|AssertionError|assert limit_code == "ConcurrentPipelineExecutionsLimitExceededException", (
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# CodePipeline control plane: declaration validation, stage-transition state/authorization, PutWebhook pipeline resolution, UntagResource/third-party-job stubs and CreatePipeline's initial execution (10 root causes)

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePipeline

## Description
Ten distinct root causes found in the codepipeline behavioral sweep. All ten must be closed before this issue is submitted. None of these failing checks is marked ALREADY FILED or OWNED by the replay report. Related open issue: 20261022T173520Z-codepipeline-executions-never-progress-structural-validation-absent-error.md — it covers CreatePipeline structural validation (its Cause 2) and the concurrency ceiling (its Cause 7), but its trace list does not carry any of the check ids below; causes 1-3 here are validation classes it does not describe at all (and it says nothing about UpdatePipeline), and cause 10 is the same accounting gap reached from two further checks.

=== Cause 1 of 10: actionTypeId provider is never validated on CreatePipeline / UpdatePipeline ===
AWS: Smithy: CreatePipeline.errors and UpdatePipeline.errors both include InvalidActionDeclarationException (errorKind client, httpError 400). A declaration naming an actionTypeId whose owner is AWS and whose provider is not a real CodePipeline provider is structurally impossible and is refused before the pipeline is written.
Emulator: the declaration is accepted with HTTP 200 and the pipeline is created/updated. CreatePipeline with actionTypeId Deploy/AWS/NoSuchProvider/1 reports "error code was None, expected InvalidActionDeclarationException"; UpdatePipeline with Source/AWS/NoSuchProviderSim2Real/1 returns 200 with the bad declaration echoed back.
Why: CreatePipeline/UpdatePipeline refusal is the client's only lint for a mistyped provider. A pipeline that can never run is created successfully, so the failure moves from request time to (never-reached) execution time, and negative tests that assert a 400 against AWS pass there and fail here.
Fix: actionTypeId resolution in the shared pipeline-declaration validator used by the CreatePipeline and UpdatePipeline handlers.
Traces: codepipeline.create-pipeline-unknown-action-provider-rejected, codepipeline.update-pipeline-malformed-declarations-rejected.

=== Cause 2 of 10: UpdatePipeline accepts an Approval action that declares outputArtifacts ===
AWS: Smithy: UpdatePipeline.errors includes InvalidActionDeclarationException (client, 400). An Approval/AWS/Manual action produces no artifact, so an outputArtifacts entry on it is refused.
Emulator: HTTP 200, pipeline version bumped to 2, declaration echoed back with 'outputArtifacts': [{'name': 'ApprovalOutput'}] on the Manual approval action.
Why: the client believes it declared an artifact that will never exist; downstream actions wired to it silently reference nothing instead of being rejected at declaration time.
Fix: per-action-category artifact-arity validation in the UpdatePipeline handler's declaration validator.
Traces: codepipeline.update-pipeline-invalid-action-declaration-rejected.

=== Cause 3 of 10: UpdatePipeline accepts duplicate stage names ===
AWS: Smithy: UpdatePipeline.errors includes InvalidStageDeclarationException (client, 400). Stage names are the identity a pipeline's stages are addressed by (GetPipelineState.stageStates[].stageName, DisableStageTransition.stageName), so two stages with the same name are refused.
Emulator: HTTP 200 for a declaration carrying two stages both named 'Source' (plus an 'Approval' stage between them), echoed back verbatim at version 2.
Why: every stage-addressed operation becomes ambiguous — a client cannot tell which 'Source' a transition or a state entry refers to — and the error AWS raises at declaration time never appears.
Fix: stage-name uniqueness validation in the UpdatePipeline handler's declaration validator (same validator as causes 1 and 2).
Traces: codepipeline.update-pipeline-invalid-stage-declaration-rejected.

=== Cause 4 of 10: GetPipelineState omits inboundTransitionState from stage states ===
AWS: Smithy: GetPipelineStateOutput.stageStates -> StageStateList -> StageState, whose member inboundTransitionState targets TransitionState ("The state of the inbound transition, which is either enabled or disabled"), a structure of enabled (boolean, default false), lastChangedBy, lastChangedAt and disabledReason. A stage that has an inbound transition reports it, enabled=true until DisableStageTransition is called.
Emulator: the stageStates entries carry no inboundTransitionState member at all — the check dies with KeyError: 'inboundTransitionState' reading s["inboundTransitionState"]["enabled"] for the Source stage.
Why: a client cannot observe whether a stage transition is enabled or disabled, so DisableStageTransition/EnableStageTransition have no readable effect and any console-style stage view is unrenderable. It also makes every transition assertion in the suite unreachable.
Fix: stage-state projection/serialization in the GetPipelineState handler and the transition state it reads from pipeline state.
Traces: codepipeline.disable-stage-transition-nonexistent-stage-rejected.

=== Cause 5 of 10: DisableStageTransition performs no authorization — a principal without the permission succeeds ===
AWS: recorded behavior only (IAM authorization is not in DisableStageTransition's modeled error list; Smithy: DisableStageTransition.errors = PipelineNotFoundException, StageNotFoundException, ValidationException). The check drives the call with a deliberately restricted principal and AWS refuses it with error code AccessDeniedException at HTTP 400; the transition stays enabled.
Emulator: the call returns HTTP 200 with an empty body ({} , content-length 2) and the transition is actually disabled for the restricted caller.
Why: the emulator cannot be used to test least-privilege policies — a request that production IAM denies succeeds here, so a policy regression is invisible and an unauthorized caller can gate another team's pipeline.
Fix: the authorization check in the DisableStageTransition handler (and the request-principal/policy evaluation it must consult).
Traces: codepipeline.disable-stage-transition-without-permission-denied.

=== Cause 6 of 10: PutWebhook resolves targetPipeline and refuses an absent one, short-circuiting webhook validation ===
AWS: Smithy: PutWebhook.errors includes PipelineNotFoundException, InvalidWebhookFilterPatternException, InvalidWebhookAuthenticationParametersException and ValidationException (all client, 400). Recorded behavior: PutWebhook does NOT require its webhook.targetPipeline to exist — the corpus's own check_put_webhook_nonexistent_pipeline_accepted.py asserts AWS ACCEPTS a definition whose targetPipeline is absent and returns webhook.arn. What AWS does validate is the definition itself: a jsonPath that is not a parseable JSONPath -> InvalidWebhookFilterPatternException, and authentication GITHUB_HMAC with an empty authenticationConfiguration -> InvalidWebhookAuthenticationParametersException.
Emulator: PutWebhook raises PipelineNotFoundException (400, message e.g. 'pipeline sim2real-behav-absent-pipeline-0866cc502214 was not found') for a well-formed definition whose targetPipeline does not exist. Because the pipeline lookup runs first, the two webhook-definition rejection classes above are unreachable, and PutWebhook can no longer be used as the cheap ARN-bearing resource that a tagging check needs.
Why: a client that registers a webhook before (or independently of) its pipeline — the normal order for an absent/renamed target — is refused where AWS accepts, and no client can ever observe the filter-pattern or HMAC-configuration rejections that AWS raises.
Fix: ordering and pipeline resolution in the PutWebhook handler: validate the webhook definition (filters/jsonPath, authentication vs authenticationConfiguration) and stop resolving targetPipeline as a precondition.
Traces: codepipeline.put-webhook-invalid-filter-pattern-rejected, codepipeline.list-tags-for-resource-invalid-arn-rejected (both die inside PutWebhook during setup, before reaching their own assertions).

=== Cause 7 of 10: UntagResource is an unimplemented scaffold stub ===
AWS: Smithy: UntagResource takes resourceArn (required, pattern ^arn:aws(-[\w]+)*:codepipeline:.+:[0-9]{12}:.+$) and tagKeys (required) and models ResourceNotFoundException, InvalidArnException, InvalidTagsException, ConcurrentModificationException, ValidationException — all client, HTTP 400. A well-formed ARN naming a pipeline that does not exist is refused with error code ResourceNotFoundException.
Emulator: the operation answers with error code NotImplemented ("operation ... is scaffolded but not implemented in this emulator") — the check reports "expected ResourceNotFoundException, got NotImplemented".
Why: tag removal is unavailable, and the error a client catches to distinguish "no such resource" from a transport fault is replaced by a code that is not in the operation's modeled error set.
Fix: implement the UntagResource handler (services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops/untag_resource.rs) against the tag store.
Traces: codepipeline.untag-resource-nonexistent-pipeline-rejected.

=== Cause 8 of 10: PutThirdPartyJobFailureResult / PutThirdPartyJobSuccessResult are unimplemented scaffold stubs ===
AWS: Smithy: both operations model JobNotFoundException, InvalidJobStateException, InvalidClientTokenException and ValidationException (client, 400). A syntactically valid jobId that was never issued is refused with JobNotFoundException from BOTH terminal-result operations.
Emulator: error code NotImplemented from PutThirdPartyJobFailureResult — the check reports "unexpected code: NotImplemented" where JobNotFoundException was required; PutThirdPartyJobSuccessResult is the next call and is a stub as well. (Note: ListActionTypes(actionOwnerFilter="ThirdParty") and PollForThirdPartyJobs did answer, so this is specifically the two terminal-result handlers.)
Why: a partner integration cannot report either outcome of a job, and cannot distinguish an unknown job from an already-completed one, because neither modeled error is ever produced.
Fix: implement the PutThirdPartyJobFailureResult and PutThirdPartyJobSuccessResult handlers (src/ops/put_third_party_job_failure_result.rs, src/ops/put_third_party_job_success_result.rs) against the job store. A scaffold issue exists for PutThirdPartyJobFailureResult alone (reported there as blocking 1 check); this cause records the second check it also blocks and the sibling success-result stub.
Traces: codepipeline.third-party-job-success-after-failure-not-permissive.

=== Cause 9 of 10: CreatePipeline does not start the pipeline's initial execution ===
AWS: recorded behavior only (AWS CodePipeline User Guide: a pipeline starts an execution as soon as it is created). Five independent checks poll ListPipelineExecutions / GetPipelineState immediately after CreatePipeline and AWS shows exactly one automatically-triggered execution, each with its own 20-30 s internal budget; the suite's own helper is named _first_execution(... what="CreatePipeline to auto-start an execution of <name>").
Emulator: no execution ever appears. Verbatim: "pipeline sim2real-cp-pipe-3838773b8ced never showed an auto-triggered execution within 30.0s"; "no CreatePipeline-triggered execution appeared for sim2real-cp-pipe-f5f98eda8fff within 30.0s"; "CreatePipeline's auto-triggered execution never became visible to list_pipeline_executions"; "timed out after 20s waiting for CreatePipeline's own automatic execution of 'sim2real-behav-cpwh-c34503c9cf90' to appear; last observed value []"; "condition not met within 30.0s; last=None". These are the checks' own assertions, not the harness ceiling.
Why: every client and every test that creates a pipeline and then observes/stops/retries its first execution finds an empty execution history, so RetryStageExecution, StopPipelineExecution and rollback paths have nothing to act on. It is also distinct from the open issue's Cause 1 (an execution started explicitly by StartPipelineExecution exists but never progresses): here no execution is created at all.
Fix: initial-execution trigger in the CreatePipeline handler (and the execution record it must publish to ListPipelineExecutions / GetPipelineState).
Traces: codepipeline.pipeline-stage-actions-execute-in-declared-order, codepipeline.retry-stage-execution-not-failed-rejected, codepipeline.retry-stage-execution-stale-token-rejected, codepipeline.webhook-url-rejects-a-wrong-hmac-signature-and-starts-no-execution, codepipeline.stop-completed-execution-rejected.

=== Cause 10 of 10: the active-execution ceiling is still not enforced on these two checks ===
AWS: Smithy: StartPipelineExecution.errors includes ConcurrentPipelineExecutionsLimitExceededException (client, httpError 400). Once a pipeline's active-execution ceiling is reached (AWS General Reference, CodePipeline quotas: 50 active executions per pipeline, non-adjustable), the overflow start is refused with that code.
Emulator: every start succeeds. Verbatim: "never observed ConcurrentPipelineExecutionsLimitExceededException within 55 sequential attempts" (55 execution ids returned) and "never hit the concurrency limit after 55 executions; last error code=None".
Why: a client that handles the ceiling (back off, queue, surface it) never exercises that path against the emulator, so a load-shedding regression ships unnoticed.
Fix: concurrency accounting in the StartPipelineExecution handler and the pipeline execution state. Same accounting as Cause 7 of 20261022T173520Z-codepipeline-executions-never-progress-structural-validation-absent-error.md; these two check ids are not in that issue's trace list, so re-replay them too.
Traces: codepipeline.concurrent-pipeline-executions-limit-exceeded, codepipeline.start-pipeline-execution-concurrency-limit-exceeded.

## Evidence
Cause 1 — expected HTTP 400 InvalidActionDeclarationException. actual: "CreatePipeline accepted (or misclassified) actionTypeId Deploy/AWS/NoSuchProvider/1: error code was None, expected InvalidActionDeclarationException"; UpdatePipeline: "expected the call to be rejected, but it SUCCEEDED and returned {'pipeline': {... 'provider': 'NoSuchProviderSim2Real' ...}, 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}".
Cause 2 — expected HTTP 400 InvalidActionDeclarationException. actual: "real AWS rejects this call, but it SUCCEEDED and returned {'pipeline': {... 'actionTypeId': {'category': 'Approval', 'owner': 'AWS', 'provider': 'Manual', 'version': '1'}, 'outputArtifacts': [{'name': 'ApprovalOutput'}] ...}, 'HTTPStatusCode': 200}".
Cause 3 — expected HTTP 400 InvalidStageDeclarationException. actual: "expected rejection, but the call SUCCEEDED and returned {'pipeline': {... stages: [{'name': 'Source'...}, {'name': 'Approval'...}, {'name': 'Source'...}] ...}, 'HTTPStatusCode': 200}".
Cause 4 — expected StageState.inboundTransitionState.enabled == True. actual: KeyError: 'inboundTransitionState' at return s["inboundTransitionState"]["enabled"].
Cause 5 — expected error code AccessDeniedException, HTTP 400. actual: "expected AccessDeniedException, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}}}".
Cause 6 — expected PutWebhook to accept a definition with an absent targetPipeline (then reject the bad jsonPath with InvalidWebhookFilterPatternException). actual: "botocore.errorfactory.PipelineNotFoundException: An error occurred (PipelineNotFoundException) when calling the PutWebhook operation: pipeline sim2real-behav-absent-pipeline-0866cc502214 was not found" (same for ...-aebd82c95d2e in the tagging check).
Cause 7 — expected error code ResourceNotFoundException, HTTP 400. actual: "expected ResourceNotFoundException, got NotImplemented".
Cause 8 — expected error code JobNotFoundException with message "Job with id '<id>' does not exist.". actual: "unexpected code: NotImplemented".
Cause 9 — expected exactly one auto-triggered execution after CreatePipeline. actual: "never showed an auto-triggered execution within 30.0s" / "last observed value []" / "condition not met within 30.0s; last=None".
Cause 10 — expected HTTP 400 ConcurrentPipelineExecutionsLimitExceededException on the overflow start. actual: "never observed ConcurrentPipelineExecutionsLimitExceededException within 55 sequential attempts"; "never hit the concurrency limit after 55 executions; last error code=None".

## Steps to Reproduce
1. Boot the emulator and replay the codepipeline behavioral checks listed in trace_ids.
2. Cause 1: CreatePipeline (and then UpdatePipeline) with an action whose actionTypeId is {category: Source|Deploy, owner: AWS, provider: 'NoSuchProvider', version: '1'} — both return HTTP 200 instead of 400 InvalidActionDeclarationException.
3. Cause 2: UpdatePipeline with an Approval/AWS/Manual action carrying outputArtifacts: [{name: 'ApprovalOutput'}] — returns 200.
4. Cause 3: UpdatePipeline with two stages both named 'Source' — returns 200.
5. Cause 4: CreatePipeline, then GetPipelineState(name=...) and read stageStates[].inboundTransitionState — the member is absent.
6. Cause 5: create an IAM principal with no codepipeline:DisableStageTransition permission and call DisableStageTransition with it — returns 200 and the transition is disabled.
7. Cause 6: PutWebhook with a well-formed definition whose targetPipeline names a pipeline that does not exist — refused with PipelineNotFoundException; repeat with filters: [{jsonPath: '((', matchEquals: 'x'}] and the InvalidWebhookFilterPatternException is never reached.
8. Cause 7: UntagResource with a well-formed codepipeline ARN for an absent pipeline — error code NotImplemented.
9. Cause 8: PutThirdPartyJobFailureResult (then PutThirdPartyJobSuccessResult) with a random uuid jobId and any clientToken — error code NotImplemented.
10. Cause 9: CreatePipeline, then poll ListPipelineExecutions for 30 s — pipelineExecutionSummaries stays empty.
11. Cause 10: create a PARALLEL-execution-mode pipeline and submit 55 StartPipelineExecution calls — all 55 succeed.
