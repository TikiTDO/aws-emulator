Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.create-pipeline-dangling-input-artifact-rejected codepipeline.update-pipeline-invalid-stage-declaration-rejected codepipeline.create-pipeline-v1-rejects-v2-only-fields codepipeline.action-configuration-empty-vs-absent-preserved codepipeline.condition-not-overridable-rejected codepipeline.poll-for-jobs-unknown-action-type-rejected codepipeline.third-party-job-details-reflect-polled-job codepipeline.third-party-job-nonce-single-use-visibility codepipeline.override-stage-condition-conflict-detection codepipeline.deploy-artifact-bytes-are-identical-to-the-source-object codepipeline.rollback-stage-restores-the-previous-artifact-bytes codepipeline.source-revision-override-delivers-the-pinned-object-version codepipeline.list-action-executions-chronological-order codepipeline.list-rule-executions-order-matches-invocation-sequence
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.create-pipeline-dangling-input-artifact-rejected|run#11|AssertionError|assert code == "InvalidActionDeclarationException", (
Issue-Key: codepipeline.update-pipeline-invalid-stage-declaration-rejected|run#12|AssertionError|assert code == "InvalidStageDeclarationException", f"expected InvalidStageDeclarationException, got {code}"
Issue-Key: codepipeline.create-pipeline-v1-rejects-v2-only-fields|run#16|AssertionError|assert msg == _MSG_VARIABLES, (
Issue-Key: codepipeline.action-configuration-empty-vs-absent-preserved|run#17|AssertionError|assert message == (
Issue-Key: codepipeline.condition-not-overridable-rejected|run#12|AssertionError|assert code == "ConditionNotOverridableException", code
Issue-Key: codepipeline.poll-for-jobs-unknown-action-type-rejected|run#7|KeyError|assert positive["jobs"] == [], f"expected no jobs for a freshly-registered action type, got {positive['jobs']!r}"
Issue-Key: codepipeline.third-party-job-details-reflect-polled-job|run#4|KeyError|assert custom_poll["jobs"] == [], custom_poll
Issue-Key: codepipeline.third-party-job-nonce-single-use-visibility|run#4|KeyError|assert custom_poll["jobs"] == [], custom_poll
Issue-Key: codepipeline.override-stage-condition-conflict-detection|_upload_output_artifact#10|KeyError|creds = job["data"]["artifactCredentials"]
Issue-Key: codepipeline.deploy-artifact-bytes-are-identical-to-the-source-object|run#25|AssertionError|assert [r.get("revisionId") for r in revisions] == [version_id], (
Issue-Key: codepipeline.rollback-stage-restores-the-previous-artifact-bytes|run#35|AssertionError|assert [r.get("revisionId") for r in record.get("artifactRevisions", [])] \
Issue-Key: codepipeline.source-revision-override-delivers-the-pinned-object-version|run#28|AssertionError|assert [r.get("revisionId") for r in override_record.get("artifactRevisions", [])] \
Issue-Key: codepipeline.list-action-executions-chronological-order|run#12|AssertionError|assert stage_order == ["Build", "Source"], stage_order
Issue-Key: codepipeline.list-rule-executions-order-matches-invocation-sequence|run#18|AssertionError|assert returned == sorted((n, "Succeeded") for n in expected_declared), returned
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# CodePipeline declaration validation and response shapes: 10 net-new root causes (misclassified declaration errors, OverrideStageCondition NotImplemented, PollForJobs omits jobs/artifactCredentials, artifactRevisions lose revisionId, wrong list order)

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PollForJobs

## Description
TEN net-new root causes in codepipeline declaration validation and response shaping. A fixer must close ALL TEN before submitting; each cause names its own failing check ids. None of these checks is marked ALREADY FILED or OWNED in this replay. Related but NOT covering: the open issue 20261023T080113Z-codepipeline-control-plane-declaration-validation-stage-transition-state.md carries other declaration-validation ids (create-pipeline-unknown-action-provider-rejected, update-pipeline-malformed-declarations-rejected, update-pipeline-invalid-action-declaration-rejected), and 20261028T112857Z-codepipeline-job-worker-lifecycle-and-refusal-wording-8.md carries other job-worker ids; neither carries the ids below.

=== Cause 1 of 10: CreatePipeline misclassifies a dangling inputArtifacts reference as InvalidStructureException ===
AWS: Smithy models both InvalidActionDeclarationException and InvalidStructureException on CreatePipeline. An action whose inputArtifacts names an artifact no upstream action produces is a faulty ACTION declaration: the recorded AWS refusal is InvalidActionDeclarationException.
Emulator: refuses the same declaration with InvalidStructureException.
Why: a client branching on the refusal code cannot tell a per-action declaration fault from a whole-structure fault, and templates validated against AWS's code fail differently here.
Fix: the artifact-graph validation in the CreatePipeline handler's declaration validator (classification, not detection — the call IS refused).
Traces: codepipeline.create-pipeline-dangling-input-artifact-rejected.

=== Cause 2 of 10: UpdatePipeline misclassifies an invalid stage declaration as InvalidStructureException ===
AWS: Smithy models InvalidStageDeclarationException on UpdatePipeline. A stage-level declaration fault is refused with that code.
Emulator: answers InvalidStructureException.
Why: same client-visible consequence as Cause 1, on the update path.
Fix: stage-level validation classification in the UpdatePipeline handler's validator.
Traces: codepipeline.update-pipeline-invalid-stage-declaration-rejected.

=== Cause 3 of 10: the V1-pipeline refusal for a V2-only field does not name the field the way AWS does ===
AWS: recorded behavior — CreatePipeline with pipelineType V1 and a `variables` list is refused with a message naming the V2-only feature: `Pipeline variable can only be used with V2 pipelines`.
Emulator: refuses with its own wording `pipeline.variables is only supported on a V2 pipeline, but this declaration states pipelineType V1`.
Why: a blanket V1 rejection text is not the same contract — tooling that detects "upgrade this pipeline to V2 because of variables" by message does not match.
Fix: the V1/V2 field-gating messages in the CreatePipeline declaration validator.
Traces: codepipeline.create-pipeline-v1-rejects-v2-only-fields.

=== Cause 4 of 10: the ValidationException for an empty-string action configuration value reports the wrong length limit ===
AWS: recorded behavior — UpdatePipeline with an action configuration value of "" is refused ValidationException with `1 validation error detected: Value at 'pipeline.stages.<i>.member.actions.<j>.member.configuration' failed to satisfy constraint: Map value must satisfy constraint: [Member must have length less than or equal to 50000, Member must have length greater than or equal to 1]`. The member path comes from the request; the two bounds are fixed text. (Smithy ActionConfigurationValue carries @length min 1 max 1000, but the recorded service response states 50000 — the recording is the requirement.)
Emulator: same code and same path, but reports `less than or equal to 1000`.
Why: the message tells a client the maximum configuration-value size; 1000 vs 50000 misstates the service limit by 50x and breaks message-equality checks.
Fix: the constraint bounds used by the action-configuration validation message in the codepipeline declaration validator.
Traces: codepipeline.action-configuration-empty-vs-absent-preserved.

=== Cause 5 of 10: OverrideStageCondition answers the non-AWS error code `NotImplemented` ===
AWS: Smithy models OverrideStageCondition with errors ConditionNotOverridableException, ConcurrentPipelineExecutionsLimitExceededException, ConflictException, NotLatestPipelineExecutionException, PipelineNotFoundException, StageNotFoundException, ValidationException. Overriding a condition that resolved with a Skip result is refused ConditionNotOverridableException with `The condition type <conditionType> in stage <stageName> cannot be overridden now. Either it contains no failed or in-progress rule, the execution '<id>' is already superseded, or the pipeline definition has changed since the rules(s) failed.`
Emulator: returns error code `NotImplemented`, which is not in the operation's modeled error set.
Why: the operation is unusable and the refusal is unclassifiable — no SDK error type matches, so clients see an unmodeled error for every condition override.
Fix: the OverrideStageCondition handler plus the stage-condition state it must read.
Traces: codepipeline.condition-not-overridable-rejected.

=== Cause 6 of 10: PollForJobs / PollForThirdPartyJobs omit the `jobs` member when there is no job ===
AWS: Smithy PollForJobsOutput has member `jobs: JobList` (and PollForThirdPartyJobsOutput `jobs`); AWS always serialises the list, empty as `"jobs": []`.
Emulator: the response body omits `jobs` entirely when no job is available, so an SDK access to response['jobs'] raises KeyError instead of yielding [].
Why: every job-worker polling loop written against the model (`resp['jobs']`) crashes on an idle poll rather than looping.
Fix: the PollForJobs/PollForThirdPartyJobs serializers in the codepipeline crate (always emit the list member).
Traces: codepipeline.poll-for-jobs-unknown-action-type-rejected, codepipeline.third-party-job-details-reflect-polled-job, codepipeline.third-party-job-nonce-single-use-visibility.

=== Cause 7 of 10: a dispatched job's JobData carries no artifactCredentials ===
AWS: Smithy JobData has member `artifactCredentials: AWSSessionCredentials` (sensitive) — the temporary STS credentials a custom-action job worker uses to read input and write output artifacts in the artifact store. AWS populates it on jobs returned by PollForJobs/GetJobDetails for actions with artifacts.
Emulator: job['data'] has no artifactCredentials key, so a worker cannot build an S3 client to upload the output artifact (KeyError in the checks).
Why: custom-action workers cannot complete any artifact-producing action, which is the documented contract for a Custom action type.
Fix: the job dispatch path in the codepipeline crate (populate JobData.artifactCredentials, and the matching field on GetJobDetails/GetThirdPartyJobDetails).
Traces: codepipeline.override-stage-condition-conflict-detection.

=== Cause 8 of 10: artifactRevisions entries carry only `name`, never revisionId ===
AWS: Smithy ArtifactRevision has name, revisionId, revisionChangeIdentifier, revisionSummary, created, revisionUrl; `revisionId` is documented as "The revision ID of the artifact" and for an S3 source is the consumed object version. GetPipelineExecution reports one ArtifactRevision per artifact the execution consumed.
Emulator: GetPipelineExecution returns artifactRevisions as [{'name': 'SourceOutput'}] — no revisionId — for ordinary executions, for executions started with a sourceRevisions override, and for rollback executions.
Why: a client cannot tell WHICH source version an execution consumed; pinning a revision (sourceRevisions) and rollback-to-a-previous-execution become unverifiable, so an auditor cannot prove what was deployed.
Fix: the execution record/state in the codepipeline crate (persist the consumed source revision) and the GetPipelineExecution serializer.
Traces: codepipeline.deploy-artifact-bytes-are-identical-to-the-source-object, codepipeline.rollback-stage-restores-the-previous-artifact-bytes, codepipeline.source-revision-override-delivers-the-pinned-object-version.

=== Cause 9 of 10: ListActionExecutions returns action executions oldest-first ===
AWS: recorded behavior only (the model documents the members, not the order) — ListActionExecutions returns the action-execution history most-recent-first, so for a Source-then-Build execution the stage order observed is ['Build', 'Source'].
Emulator: returns ['Source', 'Build'] — ascending by start time.
Why: a client reading page 1 to find the LATEST action execution (the common use with maxResults) gets the oldest instead, so paginated history reads are inverted.
Fix: the ordering applied by the ListActionExecutions handler before pagination.
Traces: codepipeline.list-action-executions-chronological-order.

=== Cause 10 of 10: ListRuleExecutions reports every declared condition rule as 'Abandoned' instead of 'Succeeded' ===
AWS: Smithy RuleExecutionDetail$status is a RuleExecutionStatus whose documented categories are InProgress, Succeeded and Failed; a condition's rules are evaluated when the stage is entered and a passing rule is reported Succeeded with its name.
Emulator: for a pipeline whose stage condition declares rules Alpha, Bravo and Charlie, ListRuleExecutions reports [('Alpha','Abandoned'), ('Bravo','Abandoned'), ('Charlie','Abandoned')] — the rules are recorded but never evaluated to a verdict.
Why: stage conditions silently never run, so a client cannot see whether a gate passed, and a pipeline that relies on a condition to block a stage is not gated at all.
Fix: the condition/rule evaluation in the execution plane and the status it publishes through ListRuleExecutions.
Traces: codepipeline.list-rule-executions-order-matches-invocation-sequence.

## Evidence
All quotes verbatim from this replay's failing-check details; generated names, ids and object versions are per-run values.

Cause 1 — codepipeline.create-pipeline-dangling-input-artifact-rejected [mismatch]: "CreatePipeline accepted (or misclassified) a dangling inputArtifacts reference: error code was 'InvalidStructureException', expected InvalidActionDeclarationException".
Cause 2 — codepipeline.update-pipeline-invalid-stage-declaration-rejected [mismatch]: "expected InvalidStageDeclarationException, got InvalidStructureException".
Cause 3 — codepipeline.create-pipeline-v1-rejects-v2-only-fields [mismatch]: "the refusal must name `variables` as the V2-only field ('Pipeline variable can only be used with V2 pipelines'); a blanket V1 rejection is not the same contract: 'pipeline.variables is only supported on a V2 pipeline, but this declaration states pipelineType V1'".
Cause 4 — codepipeline.action-configuration-empty-vs-absent-preserved [mismatch]: code ValidationException matched; message was "1 validation error detected: Value at 'pipeline.stages.2.member.actions.1.member.configuration' failed to satisfy constraint: Map value must satisfy constraint: [Member must have length less than or equal to 1000, Member must have length greater than or equal to 1]" where AWS states 50000 as the upper bound.
Cause 5 — codepipeline.condition-not-overridable-rejected [mismatch]: assert code == "ConditionNotOverridableException" -> AssertionError: NotImplemented (OverrideStageCondition with conditionType='BEFORE_ENTRY' on a Skip-result condition).
Cause 6 — codepipeline.poll-for-jobs-unknown-action-type-rejected [error], codepipeline.third-party-job-details-reflect-polled-job [error], codepipeline.third-party-job-nonce-single-use-visibility [error]: all three KeyError: 'jobs' on the PollForJobs/PollForThirdPartyJobs response for a freshly registered action type with no job pending.
Cause 7 — codepipeline.override-stage-condition-conflict-detection [error]: KeyError: 'artifactCredentials' at creds = job["data"]["artifactCredentials"] after PollForJobs handed back a job.
Cause 8 — codepipeline.deploy-artifact-bytes-are-identical-to-the-source-object [mismatch]: "the execution must record the S3 object version it consumed as its artifact revision: artifactRevisions [{'name': 'SourceOutput'}], uploaded version '265175309b...'". codepipeline.source-revision-override-delivers-the-pinned-object-version: "the recorded source revision is not the pinned one: artifactRevisions [{'name': 'SourceOutput'}], pinned '405c1919...', current '653df9ee...'". codepipeline.rollback-stage-restores-the-previous-artifact-bytes: "the rollback execution did not re-use the targeted execution's artifact revision: [{'name': 'SourceOutput'}], expected the old object version 'abfb522c...'".
Cause 9 — codepipeline.list-action-executions-chronological-order [mismatch]: assert stage_order == ["Build", "Source"] -> AssertionError: ['Source', 'Build'].
Cause 10 — codepipeline.list-rule-executions-order-matches-invocation-sequence [mismatch]: expected sorted (name,'Succeeded') pairs, got [('Alpha', 'Abandoned'), ('Bravo', 'Abandoned'), ('Charlie', 'Abandoned')].

## Steps to Reproduce
Boot the emulator and point botocore at it: cp = boto3.client('codepipeline').

Cause 1: cp.create_pipeline with a Build action whose inputArtifacts=[{'name':'NoSuchArtifact'}] -> expect InvalidActionDeclarationException; observe InvalidStructureException.
Cause 2: cp.update_pipeline with a stage-level fault (e.g. a stage with no actions) -> expect InvalidStageDeclarationException; observe InvalidStructureException.
Cause 3: cp.create_pipeline with pipelineType='V1' and variables=[{'name':'v','defaultValue':'x'}] -> expect the refusal message 'Pipeline variable can only be used with V2 pipelines'.
Cause 4: with a client built with Config(parameter_validation=False), cp.update_pipeline where one action's configuration value is "" -> expect ValidationException whose message ends '[Member must have length less than or equal to 50000, Member must have length greater than or equal to 1]'.
Cause 5: create a pipeline with a beforeEntry condition whose rule result is Skip, start an execution, wait for the stage to report Skipped, then cp.override_stage_condition(pipelineName=P, stageName=S, pipelineExecutionId=E, conditionType='BEFORE_ENTRY') -> expect ConditionNotOverridableException; observe error code NotImplemented.
Cause 6: cp.create_custom_action_type(...) then cp.poll_for_jobs(actionTypeId=<that type>, maxBatchSize=10) with nothing pending -> expect the response to contain 'jobs': []; observe the key absent (KeyError). Same for cp.poll_for_third_party_jobs.
Cause 7: start an execution of a pipeline with a Custom action that has input/output artifacts, cp.poll_for_jobs -> expect job['data']['artifactCredentials'] with accessKeyId/secretAccessKey/sessionToken; observe the key absent.
Cause 8: create a versioned source bucket, run an execution, then cp.get_pipeline_execution(pipelineName=P, pipelineExecutionId=E) -> expect artifactRevisions[0]['revisionId'] == the consumed S3 object version; observe only {'name': ...}. Repeat with cp.start_pipeline_execution(sourceRevisions=[...]) and with cp.rollback_stage.
Cause 9: run a two-stage (Source, Build) execution and call cp.list_action_executions(pipelineName=P) -> expect the Build action first (most recent first); observe Source first.
Cause 10: create a V2 pipeline whose stage condition declares three rules, run it, then cp.list_rule_executions(pipelineName=P) -> expect each rule Succeeded; observe Abandoned.
Then re-replay the check ids listed in trace_ids.
