Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.put-job-failure-invalid-state-rejected codepipeline.stop-completed-execution-rejected codepipeline.stop-pipeline-execution-not-stoppable-rejected codepipeline.superseded-execution-mode-drops-the-middle-queued-execution
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.put-job-failure-invalid-state-rejected|run#14|AssertionError|assert error.get("Code") == "InvalidJobStateException", error
Issue-Key: codepipeline.stop-completed-execution-rejected|run#13|AssertionError|assert message == (
Issue-Key: codepipeline.stop-pipeline-execution-not-stoppable-rejected|run#14|AssertionError|assert message == (
Issue-Key: codepipeline.superseded-execution-mode-drops-the-middle-queued-execution|run#33|AssertionError|assert keys == [f"deployed/{survivor}.zip"], (
Issue-Commit: e0eecc5f72a782e2fd5c24b79b4afd214154b2e2

# CodePipeline: 3 net-new root causes — a terminated job is forgotten (JobNotFoundException instead of InvalidJobStateException), StopPipelineExecution's not-stoppable message is emulator-invented, and the built-in #{codepipeline.PipelineExecutionId} variable is never resolved

> _Note: this issue was identified against the code base at commit `e0eecc5f72a782e2fd5c24b79b4afd214154b2e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutJobFailureResult

## Description
THREE net-new root causes in the CodePipeline job-result, stop-refusal and action-variable paths. A fixer must close ALL THREE before submitting; each cause names its own failing check ids below.

None of the four failing checks grounding this issue is marked ALREADY FILED or OWNED in this replay. Related but NOT covering (none of them carries these four check ids / replay keys): the open issue 20261029T183239Z-codepipeline-refusal-classification-and-wording-10-net-new.md describes the same FAMILY as Causes 1 and 2 but only for OTHER check ids (put-job-failure-invalid-job-state-rejected, put-job-failure-on-completed-job-rejected, put-job-failure-result-wrong-job-state-rejected, stop-pipeline-execution-duplicate-request-rejected, stop-pipeline-execution-terminal-not-stoppable); the open issue 20261028T112857Z-codepipeline-execution-plane-5-net-new-root-causes.md describes the same FAMILY as Cause 3 but only for the #{SourceVariables.VersionId} namespace on check codepipeline.source-output-variable-resolves-into-the-downstream-object-key, not the service's own #{codepipeline.*} namespace; the open issue 20261028T112857Z-codepipeline-job-worker-lifecycle-and-refusal-wording-8.md carries other job-worker ids. A fixer closing those issues must still close these three causes and re-replay the ids in trace_ids.

=== Cause 1 of 3: a job that terminated via PutJobSuccessResult is forgotten, so a later PutJobFailureResult on it is refused JobNotFoundException instead of InvalidJobStateException ===
AWS: Smithy (com.amazonaws.codepipeline#CodePipeline_20150709) models BOTH InvalidJobStateException (client, HTTP 400) and JobNotFoundException (client, HTTP 400) on PutJobFailureResult (input jobId @pattern ^[0-9a-f]{8}-...-[0-9a-f]{12}$, failureDetails, both required). A jobId that WAS issued and whose job has already reached a terminal state is refused InvalidJobStateException with the message `Job with id <jobId> has already terminated` — fixed text `Job with id ` + the jobId FROM THE REQUEST (unquoted, no trailing period) + ` has already terminated`. JobNotFoundException is reserved for a jobId that was never issued to a worker. Recorded behavior for this check confirms that classification: the check's own positive control shows a genuinely InProgress job still accepts a failure result, and the pipeline keeps advancing from the earlier success.
Emulator: after PutJobSuccessResult(jobId=J) succeeds, PutJobFailureResult(jobId=J, failureDetails={type:JobFailed,...}) is refused with code JobNotFoundException and message `Job with id '<jobId>' does not exist.` (the id single-quoted, a period appended). Hypothesis (neither the model nor the recording states the mechanism): terminal jobs are evicted from the job table instead of being retained with their terminal status, so the state check degenerates to a lookup miss.
Why: a job worker cannot distinguish the benign, retry-safe case "I already reported this job" from the programming error "this job id is bogus", so an at-least-once worker treats its own duplicate report as a fatal bug. The documented single-terminal-state guarantee also becomes unobservable, and the message diverges on two counts (quoting and trailing period) even for the code the emulator does send.
Fix: the job state store in the codepipeline crate (retain terminated jobs together with their terminal status) and the state classification in the PutJobFailureResult handler, which must choose InvalidJobStateException for a known-but-terminated job and build the `Job with id <jobId> has already terminated` message.
Traces: codepipeline.put-job-failure-invalid-state-rejected.

=== Cause 2 of 3: StopPipelineExecution's PipelineExecutionNotStoppableException carries an emulator-authored message that names the terminal status instead of AWS's fixed wording ===
AWS: Smithy models PipelineExecutionNotStoppableException (client, HTTP 400) on StopPipelineExecution, alongside DuplicatedStopRequestException, PipelineNotFoundException, ConflictException (409) and ValidationException. An execution that has already reached a terminal status has no action executions in progress, so the stop is refused PipelineExecutionNotStoppableException with the message `The pipeline execution with ID '<pipelineExecutionId>' in '<pipelineName>' can't be stopped now. Either it does not exist in the pipeline, or the pipeline execution has no action executions in progress.` Fixed text: everything except the two single-quoted values; the variable parts are the pipelineExecutionId and pipelineName taken FROM THE REQUEST. The same wording is used whichever terminal status the execution holds, and AWS does not disclose that status in the message — "either it does not exist, or it has no action executions in progress" is deliberately the one message for both cases.
Emulator: the CODE is correct (PipelineExecutionNotStoppableException) but the message is the emulator's own sentence `Pipeline execution <pipelineExecutionId> has already reached the terminal status <status> and cannot be stopped` — different fixed text, the execution id unquoted, the pipeline name absent, and an extra disclosed terminal status (observed as Succeeded on one check and Failed on the other, i.e. the status is interpolated from emulator state, not fixed text).
Why: the message is part of the refusal contract. A client, log pattern or alarm that matches the documented AWS text does not match, and a client that parses the message for the pipeline name (AWS puts it there) finds nothing. The divergence is independent of which terminal status the execution reached, so it affects every stop of a finished execution.
Fix: the message construction for the not-stoppable refusal in the StopPipelineExecution handler in the codepipeline crate — one fixed template filled with the request's pipelineExecutionId and pipelineName, with no terminal status interpolated.
Traces: codepipeline.stop-completed-execution-rejected, codepipeline.stop-pipeline-execution-not-stoppable-rejected.

=== Cause 3 of 3: the service's own #{codepipeline.PipelineExecutionId} variable is passed through an action configuration literally, so the artifact lands at a key containing the raw template ===
AWS: recorded behavior only (the Smithy model types an action configuration as a string->string map and says nothing about templating). CodePipeline resolves variable references of the form #{<namespace>.<key>} in an action's configuration BEFORE the action runs. Besides namespaces declared by upstream actions, the service publishes its own reserved `codepipeline` namespace, whose PipelineExecutionId key is the id of the execution the action is running in. A Deploy action configured to write `deployed/#{codepipeline.PipelineExecutionId}.zip` therefore creates exactly one object named `deployed/<that execution's id>.zip`, and under SUPERSEDED execution mode only the surviving (non-superseded) execution delivers it — the recording shows the statuses settling correctly as Superseded / Succeeded / Superseded, with the delivered object keyed by the Succeeded execution's id.
Emulator: the placeholder is shipped through verbatim. The object created is literally `deployed/#{codepipeline.PipelineExecutionId}.zip`, so every execution of such a pipeline writes to the SAME key regardless of which execution survives supersession. (The execution statuses themselves were correct in this replay, so this is the resolution step alone, not the supersession engine.)
Why: any pipeline that threads the execution id into a downstream action's configuration — the standard way to make a deploy target, S3 key or build tag unique per execution — writes to a path containing a raw template. Artifacts collide on one key, per-execution deliveries overwrite each other, the surviving-execution guarantee becomes unobservable, and nothing reports an error.
Fix: the variable-resolution step of the codepipeline crate's execution plane — the point where an action's configuration is materialised for a run must expand #{namespace.key} for the service-reserved `codepipeline` namespace (PipelineExecutionId and its siblings) in addition to upstream action namespaces. Note this is the same component as Cause 4 of the open issue 20261028T112857Z-codepipeline-execution-plane-5-net-new-root-causes.md, but a different namespace source: that issue's expansion of upstream output variables does not supply #{codepipeline.*}.
Traces: codepipeline.superseded-execution-mode-drops-the-middle-queued-execution.

## Evidence
All strings are verbatim from this replay's failing-check details. Job ids, execution ids, pipeline names and bucket names are generated per run: they FILL the message/key templates and are not themselves requirements.

Cause 1 — codepipeline.put-job-failure-invalid-state-rejected [mismatch], at check_codepipeline_put_job_failure_invalid_state_rejected.py line 71:
  assert error.get("Code") == "InvalidJobStateException", error
  AssertionError: {'Message': "Job with id 'b37cb423-f3ad-4223-a4ea-413f25677710' does not exist.", 'Code': 'JobNotFoundException'}
  The check then asserts the message: expected_message = f"Job with id {job['id']} has already terminated". So expected ('InvalidJobStateException', 'Job with id <jobId> has already terminated'); actual ('JobNotFoundException', "Job with id '<jobId>' does not exist."). Fixed text of the required message: 'Job with id ' + <jobId from the request> + ' has already terminated'.

Cause 2 — codepipeline.stop-completed-execution-rejected [mismatch], at check_codepipeline_stop_completed_execution_rejected.py line 73:
  assert message == (
      f"The pipeline execution with ID '{done_exec_id}' in '{pipeline_name}' can't be stopped now. "
      "Either it does not exist in the pipeline, or the pipeline execution has no action "
      "executions in progress."
  ), message
  AssertionError: Pipeline execution 9d2e49ad-a588-4d7f-941e-d476b9ba1968 has already reached the terminal status Succeeded and cannot be stopped
  The preceding assertion `assert code == "PipelineExecutionNotStoppableException"` PASSED, so only the message diverges. The execution was confirmed 'Succeeded' before the stop, and the rejected stop correctly left it 'Succeeded'.
  codepipeline.stop-pipeline-execution-not-stoppable-rejected [mismatch], at check_codepipeline_stop_pipeline_execution_not_stoppable_rejected.py line 72: same assertion, actual message 'Pipeline execution 7ed3204a-ad13-4b7c-98e7-191e2976bff9 has already reached the terminal status Failed and cannot be stopped' — demonstrating that the emulator interpolates the terminal status (Succeeded vs Failed) where AWS's text is fixed and names the pipeline instead.

Cause 3 — codepipeline.superseded-execution-mode-drops-the-middle-queued-execution [mismatch], at check_codepipeline_superseded_execution_mode_drops_the_middle_queued_execution.py line 367:
  assert keys == [f"deployed/{survivor}.zip"], (...)
  AssertionError: supersession must deliver the artifact exactly once, keyed by the surviving execution '5baa0309-5abb-4d33-b6ea-9b1a83a7e3f9': objects under deployed/ are ['deployed/#{codepipeline.PipelineExecutionId}.zip'] and the statuses were {'e7b4c7b4-1657-4513-9de1-51c1a8b338e1': 'Superseded', '5baa0309-5abb-4d33-b6ea-9b1a83a7e3f9': 'Succeeded', 'e5b5a29b-197c-4250-ac61-90d2ea9b35e6': 'Superseded'}
  The three execution statuses are exactly what AWS produces, so the only divergence is the unresolved #{codepipeline.PipelineExecutionId} reference in the Deploy action's configured object key.

Run context: codepipeline per-service pass rate in this replay 88/162 (54%), 74 failed; all other codepipeline failures in this run are marked OWNED by existing open issues.

## Steps to Reproduce
Boot the emulator and point botocore at it: cp = boto3.client('codepipeline'), s3 = boto3.client('s3'), iam = boto3.client('iam').

Cause 1 (codepipeline.put-job-failure-invalid-state-rejected):
1. cp.create_custom_action_type for a Source category (0 input / 1 output artifact) and a Build category (1 input / 0 output).
2. Create an artifact bucket and a pipeline role; cp.create_pipeline with a Source stage using the custom Source action (outputArtifacts=[{'name':'SourceOutput'}]) and a Build stage consuming it.
3. cp.start_pipeline_execution, then cp.poll_for_jobs for the Source action type and cp.acknowledge_job to claim job J; upload a dummy zip to each output artifact's s3Location.
4. cp.put_job_success_result(jobId=J['id'], currentRevision={'revision':'1','changeIdentifier':'<any>'}) -> succeeds.
5. cp.put_job_failure_result(jobId=J['id'], failureDetails={'type':'JobFailed','message':'too-late'}).
   EXPECT: InvalidJobStateException with message exactly 'Job with id <J.id> has already terminated' (id unquoted, no trailing period).
   OBSERVE: JobNotFoundException with "Job with id '<J.id>' does not exist."
6. Positive control that must keep working: a job still InProgress accepts PutJobFailureResult, and the Build stage's job is still dispatched for the same execution id.

Cause 2 (codepipeline.stop-completed-execution-rejected, codepipeline.stop-pipeline-execution-not-stoppable-rejected):
1. Create an artifact bucket, a pipeline role, a custom Build action type, and a pipeline with an S3 Source stage plus that Build stage.
2. Start an execution E and drive it to a terminal status (let it reach 'Succeeded' for the first check; fail its job with PutJobFailureResult so it reaches 'Failed' for the second). Confirm with cp.get_pipeline_execution.
3. cp.stop_pipeline_execution(pipelineName=P, pipelineExecutionId=E, abandon=True, reason='rejected').
   EXPECT: PipelineExecutionNotStoppableException (already correct) with message exactly "The pipeline execution with ID '<E>' in '<P>' can't be stopped now. Either it does not exist in the pipeline, or the pipeline execution has no action executions in progress." — identical text for every terminal status.
   OBSERVE: 'Pipeline execution <E> has already reached the terminal status <Succeeded|Failed> and cannot be stopped'.
4. Re-read cp.get_pipeline_execution: the terminal status must be unchanged (it already is).

Cause 3 (codepipeline.superseded-execution-mode-drops-the-middle-queued-execution):
1. Create an artifact bucket with a source object and a pipeline whose executionMode is SUPERSEDED, with an S3 Source stage and a downstream S3 Deploy action whose configured object key is 'deployed/#{codepipeline.PipelineExecutionId}.zip'.
2. Fire three cp.start_pipeline_execution calls in quick succession so the middle one is superseded; poll cp.get_pipeline_execution for each until terminal (expect Superseded / Succeeded / Superseded).
3. s3.list_objects_v2 under the 'deployed/' prefix.
   EXPECT exactly one key 'deployed/<the Succeeded execution id>.zip'.
   OBSERVE the literal key 'deployed/#{codepipeline.PipelineExecutionId}.zip'.

Then re-replay the four check ids listed in trace_ids.
