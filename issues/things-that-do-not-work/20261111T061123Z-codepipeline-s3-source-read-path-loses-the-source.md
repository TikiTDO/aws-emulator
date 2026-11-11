Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-s3-source-manual-approval-run-succeeds
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-s3-source-manual-approval-run-succeeds
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|GetPipelineExecution|2xx->2xx|$.pipelineExecution.artifactRevisions[*].revisionSummary
Issue-Key: codepipeline|GetPipelineState|2xx->2xx|$.created
Issue-Key: codepipeline|GetPipelineState|2xx->2xx|$.stageStates[*].actionStates[*].currentRevision
Issue-Key: codepipeline|GetPipelineState|2xx->2xx|$.stageStates[*].actionStates[*].latestExecution.actionExecutionId
Issue-Key: codepipeline|GetPipelineState|2xx->2xx|$.stageStates[*].actionStates[*].latestExecution.externalExecutionId
Issue-Key: codepipeline|GetPipelineState|2xx->2xx|$.stageStates[*].actionStates[*].latestExecution.lastStatusChange
Issue-Key: codepipeline|GetPipelineState|2xx->2xx|$.stageStates[*].actionStates[*].latestExecution.summary
Issue-Key: codepipeline|GetPipelineState|2xx->2xx|$.updated
Issue-Related: 20261102T084502Z-codepipeline-execution-read-path-5-net-new-root.md
Issue-Commit: d0a04fabcd83cf2b721ac740047fd92b0e777ff8

# CodePipeline S3-source read path loses the source revision, the trigger detail and the action entity URL

> _Note: this issue was identified against the code base at commit `d0a04fabcd83cf2b721ac740047fd92b0e777ff8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPipelineExecution

## Description
After a pipeline with an S3 source action and a manual-approval stage runs to Succeeded, neither execution-level nor state-level read reports the revision the source action consumed, who triggered the execution, or the action's entity URL: the emulator never records that metadata, so every read that projects it omits it. Both reads answer 200, so the loss is silent. 2 causes, one per read surface.

Cause 1 of 2 - GetPipelineExecution omits artifactRevisions[].revisionSummary and trigger.triggerDetail (fixture codepipeline-s3-source-manual-approval-run-succeeds, step task#10).
AWS: Smithy GetPipelineExecution output.pipelineExecution is a PipelineExecution carrying artifactRevisions (ArtifactRevisionList) and trigger (ExecutionTrigger). ArtifactRevision.revisionSummary (RevisionSummary, @length 1..2048) is documented as 'Summary information about the most recent revision of the artifact. For GitHub and CodeCommit repositories, the commit message. For Amazon S3 buckets or actions, the user-provided content of a codepipeline-artifact-revision-summary key specified in the object metadata.' ExecutionTrigger.triggerDetail (TriggerDetail, @length 0..1024) is documented as 'Detail related to the event that started a pipeline execution, such as the webhook ARN of the webhook that triggered the pipeline execution or the user ARN for a user-initiated start-pipeline-execution CLI command.' For an S3 source the recording's revisionSummary is the fixed text 'Amazon S3 version id: ' followed by the version id of the source object taken from state, and triggerDetail is the ARN of the caller identity that invoked StartPipelineExecution.
Emulator: both members are absent from the response (expected 'Amazon S3 version id: <source object version id>', got None; expected the starting principal's ARN, got None). The artifactRevisions entry itself is present, so the list is built but its summary member is never populated.
Why: a client that reads an execution to find out WHICH source revision it shipped - the standard way to correlate a release with the artifact that produced it, to build a changelog, or to decide whether a redeploy is needed - gets an entry with no summary and must guess. A client that reads trigger.triggerDetail to attribute the run to a user or webhook gets nothing, so an audit trail of who released what cannot be reconstructed.
Fix: the pipeline-execution state - the source action's execution must record the revision it resolved (id and summary) and the execution must record its trigger detail - plus the GetPipelineExecution response projection that reads them.
Traces: codepipeline-s3-source-manual-approval-run-succeeds#task#10.

Cause 2 of 2 - GetPipelineState omits currentRevision, entityUrl, latestExecution.externalExecutionId and inboundExecutions, and words the source action's summary itself (fixture codepipeline-s3-source-manual-approval-run-succeeds, step task#12).
AWS: Smithy GetPipelineState output.stageStates is a StageStateList of StageState {stageName, inboundExecutions (StageExecutionList), actionStates (ActionStateList), latestExecution, ...}; ActionState carries currentRevision (ActionRevision: revisionId/revisionChangeId/created, all @required), entityUrl (Url, @length 1..2048, 'A URL link for more information about the state of the action, such as a deployment group details page') and latestExecution (ActionExecution) whose externalExecutionId (ExecutionId, @length 1..1500) is 'The external ID of the run of the action' and whose summary (ExecutionSummary, @length 1..2048) is 'A summary of the run of the action'. The recording shows, for a succeeded S3 source action: currentRevision.revisionId = the S3 object version id, latestExecution.externalExecutionId = the same version id, latestExecution.summary = the fixed text 'Amazon S3 version id: ' plus that version id, entityUrl = the S3 action type's entityUrlTemplate resolved for the request's region ('https://console.aws.amazon.com/s3/home?region=<region>#'), and inboundExecutions serialized as an empty list on every stage.
Emulator: currentRevision, entityUrl, latestExecution.externalExecutionId and stageStates[*].inboundExecutions are all absent, and latestExecution.summary is the emulator's own sentence 'Source object s3://<bucket>/<key> delivered as the output artifact' instead of AWS's 'Amazon S3 version id: <version id>'. The fixed text that must match is 'Amazon S3 version id: '; the only variable part is the source object's version id, read from the emulator's own S3 state.
Why: GetPipelineState is the console/polling surface. Without currentRevision and externalExecutionId a client cannot tell which revision the stage is sitting on or correlate the action run with the external system that performed it; without entityUrl there is no link out to the source resource; the re-worded summary breaks any consumer that parses the revision out of it, and an omitted inboundExecutions forces a client that iterates the list to special-case a null.
Fix: the same source-action execution state as Cause 1, plus the GetPipelineState projection - resolve entityUrl from the action type's entityUrlTemplate, emit currentRevision/externalExecutionId from the recorded revision, word the summary as AWS does, and serialize inboundExecutions as an empty list rather than omitting it.

Not claimed here: stageStates[1].actionStates[0].latestExecution.actionExecutionId differs from the recording's uuid, but it matches the id the emulator's own ListActionExecutions reports in the same run, so that is a per-run identifier and not a defect. The created/updated/lastStatusChange timestamp divergences on this step are the sub-millisecond serialization fault filed separately in the codepipeline pipelineType/timestamp-precision issue from this same sweep.

Related open issue: 20261102T084502Z-codepipeline-execution-read-path-5-net-new-root.md covers triggerDetail/artifactRevisions for the CodeCommit trajectory fixtures and owns those replay keys; the replay reports these two S3-source clusters as unowned, so they are filed here with their own bindings.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, /__admin/health 200). Both clusters: verdict mismatch, expected status 200, actual status 200.

Cause 1 - codepipeline-s3-source-manual-approval-run-succeeds#task#10, GetPipelineExecution.
  $.pipelineExecution.artifactRevisions[0].revisionSummary: missing - expected 'Amazon S3 version id: e5a8df785023becaf3bf770ed4949139', got None.
  $.pipelineExecution.trigger.triggerDetail: missing - expected a trigger detail string (recorded as '<shaped-not-asserted>'; the sibling CodeCommit fixtures record it as the starting user's ARN), got None.

Cause 2 - codepipeline-s3-source-manual-approval-run-succeeds#task#12, GetPipelineState.
  $.stageStates[0].actionStates[0].currentRevision: missing - expected {'revisionId': 'e5a8df785023becaf3bf770ed4949139'}, got None.
  $.stageStates[0].actionStates[0].entityUrl: missing - expected 'https://console.aws.amazon.com/s3/home?region=us-west-2#', got None.
  $.stageStates[0].actionStates[0].latestExecution.externalExecutionId: missing - expected 'e5a8df785023becaf3bf770ed4949139', got None.
  $.stageStates[0].actionStates[0].latestExecution.summary: value_mismatch - expected 'Amazon S3 version id: e5a8df785023becaf3bf770ed4949139', got 'Source object s3://sim2real-probe-codepipeline-25907c28/source/app.zip delivered as the output artifact'.
  $.stageStates[0].inboundExecutions: missing - expected [], got None. Same for $.stageStates[1].inboundExecutions.
  (Also reported on this step and NOT claimed as causes here: $.created / $.updated / latestExecution.lastStatusChange timestamp precision, and stageStates[1].actionStates[0].latestExecution.actionExecutionId, a per-run uuid consistent with the emulator's own ListActionExecutions output.)

Message parts: in 'Amazon S3 version id: <id>' the fixed text is 'Amazon S3 version id: ' and the variable part is the S3 object version id of the source object the action consumed, read from the emulator's own bucket state. The bucket name, object key and version id above are this recording's values, not requirements.

Model consulted: services/service-com-amazonaws-codepipeline-codepipeline-20150709/smithy-model/com-amazonaws-codepipeline-codepipeline-20150709.json - GetPipelineExecution (output.pipelineExecution: PipelineExecution; errors PipelineExecutionNotFoundException, PipelineNotFoundException, ValidationException), GetPipelineState (output stageStates/created/updated; errors PipelineNotFoundException, ValidationException), shapes PipelineExecution, ArtifactRevision, ExecutionTrigger, StageState, ActionState, ActionRevision, ActionExecution.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay fixture codepipeline-s3-source-manual-approval-run-succeeds: it creates an artifact bucket and a versioned source object, a pipeline with an S3 source action and a Manual approval action, starts an execution and approves it.
3. Step task#10: call GetPipelineExecution for that execution id. artifactRevisions[0] is present but has no revisionSummary, and trigger has no triggerDetail.
4. Step task#12: call GetPipelineState for the pipeline. The source action's state has no currentRevision, no entityUrl and no latestExecution.externalExecutionId; its latestExecution.summary reads 'Source object s3://.../... delivered as the output artifact' instead of 'Amazon S3 version id: <version id>'; and neither stage serializes inboundExecutions.
5. Both calls answer 200, so a client sees a complete-looking response with the revision and trigger metadata silently absent.
