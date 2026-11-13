Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-gpn-traj-retry-stop-and-restart-the-failed-source
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-gpn-traj-retry-stop-and-restart-the-failed-source
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|RetryStageExecution|4xx->4xx|$.message
Issue-Commit: 14a884b09fa9584b0ed8d02922f2a6aac4ab163d

# CodePipeline RetryStageExecution StageNotRetryableException message is emulator-worded instead of AWS's three-reason text

> _Note: this issue was identified against the code base at commit `14a884b09fa9584b0ed8d02922f2a6aac4ab163d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RetryStageExecution

## Description
ONE net-new root cause in the codepipeline RetryStageExecution refusal path, found by wire replay. This divergence is NOT marked ALREADY FILED or OWNED in this replay. Related but NOT covering: the open issue 20261029T183239Z-codepipeline-refusal-classification-and-wording-10-net-new.md describes the same wording defect as its Cause 10, but it is a BEHAVIORAL issue (corpus simparity-behavioral-aws-v1) and carries only behavioral check ids (codepipeline.retry-stage-execution-not-failed-rejected, codepipeline.retry-stage-not-latest-execution-rejected) — it carries no wire replay key for this fixture step, so re-replaying this corpus would not exercise it.

=== Cause 1 of 1: StageNotRetryableException's message is the emulator's own wording and omits AWS's three documented reasons ===
AWS: Smithy (com.amazonaws.codepipeline#CodePipeline_20150709) models StageNotRetryableException on RetryStageExecution as a client error, HTTP 400, documented as 'Unable to retry. The pipeline structure or stage state might have changed while actions awaited retry, or the stage contains no failed actions.' The recorded AWS response for a stage that holds no failed action is HTTP 400, error code StageNotRetryableException, with the message: "The stage named <stageName> in the <pipelineName> pipeline can't be retried now. Either it contains no failed actions, the execution '<pipelineExecutionId>' is already superseded, or the pipeline definition has changed since the action(s) failed." Fixed text: "The stage named ", " in the ", " pipeline can't be retried now. Either it contains no failed actions, the execution '", "' is already superseded, or the pipeline definition has changed since the action(s) failed." Variable parts, all taken from the request: the stageName, the pipelineName and the pipelineExecutionId. The recorded values (stage Deploy, pipeline sim2real-probe-gpn-rt-pipe-c096a281, execution 4dab8511-b83b-4ca1-b408-c7ac66d992f5) merely fill that template and are not themselves requirements.
Emulator: answers the correct HTTP 400 and the correct error code StageNotRetryableException, but the message body is its own wording: "Stage '<stageName>' of pipeline execution <pipelineExecutionId> is not in a failed state, so it cannot be retried" — single-quoted stage name, the pipeline NAME never appears, and the three AWS-documented reasons (no failed actions / execution already superseded / pipeline definition changed) are collapsed into the single 'not in a failed state' claim.
Why: a client relying on AWS's behavior cannot act on the refusal. A retry driver or operator runbook that matches the documented AWS text sees an unknown message; because the emulator names only one of the three reasons, a caller concludes the stage simply has not failed when the real cause could be a superseded execution or a changed pipeline definition, and it also loses the pipeline name the AWS message carries (it is not otherwise in the response).
Fix: the StageNotRetryableException message template in the RetryStageExecution handler's retryability classification in the codepipeline service crate — build the message from the request's pipelineName, stageName and pipelineExecutionId rather than from the emulator's internal 'not in a failed state' phrasing.
Traces: codepipeline-gpn-traj-retry-stop-and-restart-the-failed-source#task#11 (RetryStageExecution, verdict mismatch, expected 400 / actual 400, $.message value_mismatch).

## Evidence
Wire replay of fixture codepipeline-gpn-traj-retry-stop-and-restart-the-failed-source, step task#11, operation RetryStageExecution, service codepipeline, verdict mismatch. HTTP status agrees (expected 400, actual 400) and the error code agrees (StageNotRetryableException); the body's $.message diverges:

  $.message: value_mismatch
    expected: "The stage named Deploy in the sim2real-probe-gpn-rt-pipe-c096a281 pipeline can't be retried now. Either it contains no failed actions, the execution '4dab8511-b83b-4ca1-b408-c7ac66d992f5' is already superseded, or the pipeline definition has changed since the action(s) failed."
    actual:   "Stage 'Deploy' of pipeline execution 4dab8511-b83b-4ca1-b408-c7ac66d992f5 is not in a failed state, so it cannot be retried"

In the expected string the stage name (Deploy), the pipeline name (sim2real-probe-gpn-rt-pipe-c096a281) and the execution id (4dab8511-b83b-4ca1-b408-c7ac66d992f5) are per-run values filled in from the request; everything around them is AWS's fixed text and is the requirement. Smithy evidence: RetryStageExecution input requires pipelineName, stageName, pipelineExecutionId and retryMode, and models StageNotRetryableException (client error, HTTP 400, defaulted) alongside NotLatestPipelineExecutionException, StageNotFoundException and PipelineNotFoundException; StageNotRetryableException's documentation is 'Unable to retry. The pipeline structure or stage state might have changed while actions awaited retry, or the stage contains no failed actions.' — the three reasons the AWS message enumerates.

## Steps to Reproduce
1. Boot the emulator and point botocore at it: cp = boto3.client('codepipeline').
2. Create an artifact bucket, a pipeline service role and a pipeline P with a Source stage and a Deploy stage, then StartPipelineExecution(name=P) and let the execution reach a state in which the Deploy stage holds no failed action (in the fixture the Source stage fails, so Deploy never runs). Capture the pipelineExecutionId E.
3. Call cp.retry_stage_execution(pipelineName=P, stageName='Deploy', pipelineExecutionId=E, retryMode='FAILED_ACTIONS').
4. Expect HTTP 400 with code StageNotRetryableException and message exactly "The stage named Deploy in the P pipeline can't be retried now. Either it contains no failed actions, the execution 'E' is already superseded, or the pipeline definition has changed since the action(s) failed."
5. Observe the same status and code but the message "Stage 'Deploy' of pipeline execution E is not in a failed state, so it cannot be retried" — no pipeline name, and only one of AWS's three reasons.
6. Re-replay fixture codepipeline-gpn-traj-retry-stop-and-restart-the-failed-source step task#11 to confirm the fix.
