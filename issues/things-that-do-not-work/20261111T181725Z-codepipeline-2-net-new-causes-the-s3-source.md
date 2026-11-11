Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.missing-s3-source-bucket-fails-the-source-action-with-its-error-details codepipeline.retry-stage-not-latest-execution-rejected
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.missing-s3-source-bucket-fails-the-source-action-with-its-error-details|run#18|AssertionError|assert latest.get("errorDetails") == expected, (
Issue-Key: codepipeline.retry-stage-not-latest-execution-rejected|_poll_until#26|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Commit: 109a38b6a58a7a1a061f30f160f2510a3720b32a

# codepipeline: 2 net-new causes — the S3 source action's failure leaks raw Rust SDK debug text instead of AWS's ConfigurationError sentence, and an accepted RetryStageExecution drives no new action attempt

> _Note: this issue was identified against the code base at commit `109a38b6a58a7a1a061f30f160f2510a3720b32a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPipelineState

## Description
Two independent defects in the CodePipeline action-execution plane, found by replaying the behavioral suite against this checkout. Both must be closed before this issue is submitted.

Cause 1 of 2: a failed S3 source action records the emulator's internal Rust SDK error, Debug-formatted, as the action's error message instead of AWS's ConfigurationError sentence for a missing source bucket.
  AWS: Smithy (com-amazonaws-codepipeline-codepipeline-20150709): GetPipelineState -> StageStateList; ActionState.latestExecution -> ActionExecution.errorDetails -> ErrorDetails{code: "The system ID or number code of the error", message: "The text of the error message", @length 1..5000}. ListActionExecutions -> ActionExecutionDetail.output -> ActionExecutionOutput.executionResult -> ActionExecutionResult.externalExecutionSummary ("The action provider's summary for the action execution") and .errorDetails. For an AWS/Source/S3 action whose configured S3Bucket does not exist, the provider classifies the failure as a configuration error: errorDetails.code is the fixed string "ConfigurationError" and errorDetails.message is the fixed template "[No bucket with the name " + <S3Bucket from the action's configuration> + " was found]" — the brackets and the words are fixed text, only the bucket name is request-derived. The same sentence is the action execution's externalExecutionSummary, and there ListActionExecutions carries executionResult.errorDetails with the code ONLY (no message member).
  Emulator: the execution does fail and the code is right ("ConfigurationError"), but the message is the emulator's own S3 client error rendered with Rust's Debug formatter, including a nested dump of the whole HTTP response: 'the source object s3://<bucket>/source.zip could not be read: service error: unhandled error (NoSuchBucket): Error { code: "NoSuchBucket", message: "The specified bucket does not exist", aws_request_id: "...", s3_extended_request_id: "..." } (ServiceError(ServiceError { source: Unhandled(Unhandled { source: ErrorMetadata { ... } }), raw: Response { status: StatusCode(404), headers: {...}, body: SdkBody { inner: Once(Some(b"<Error><Code>NoSuchBucket</Code>...")) } } }))'. Nothing of that text is AWS's; it leaks internal Rust types, the emulator's S3 request ids and a raw XML body.
  Why: errorDetails.message and externalExecutionSummary are the user-facing failure text that consoles, notifications and CI gating logic display and match on. A client that shows it to an operator prints an emulator stack-dump instead of an actionable sentence; a client that matches AWS's wording (or merely parses the bucket name out of it) never matches. The text also exceeds what ErrorDetails.message is modelled to hold (@length max 5000) for a long bucket/key and discloses emulator-internal identifiers.
  Fix: the S3 source provider's action-execution failure recording in the codepipeline service crate — the internal SDK error must be MAPPED to a ConfigurationError provider summary (template above) before it is stored on the action execution, not stringified; the GetPipelineState and ListActionExecutions serializers then emit that stored text. Note: open issue 20261102T084502Z-codepipeline-execution-read-path-5-net-new-root.md describes the same leak for the CodeCommit source provider (branch-does-not-exist) and does NOT cover the S3 provider's missing-bucket classification, which is a separate mapping.
  Traces: codepipeline.missing-s3-source-bucket-fails-the-source-action-with-its-error-details.

Cause 2 of 2: RetryStageExecution on the stage's actual latest failed execution is accepted and echoes the execution id, but never re-runs the stage — no new action execution is appended, so ListActionExecutions still reports one attempt.
  AWS: Smithy: RetryStageExecution(pipelineName, stageName, pipelineExecutionId, retryMode: StageRetryMode) -> RetryStageExecutionOutput{pipelineExecutionId}; errors include StageNotRetryableException, NotLatestPipelineExecutionException and ConflictException. Recorded behavior only for the attempt history (the model does not state attempt accumulation): a retry that AWS ACCEPTS re-runs the failed action(s) of that stage and appends a NEW ActionExecutionDetail for the new attempt (its own actionExecutionId, startTime/lastUpdateTime, input and output.executionResult), so ListActionExecutions filtered by that pipelineExecutionId reports one detail per attempt — strictly more after an accepted retry than before it.
  Emulator: retry_stage_execution(retryMode="ALL_ACTIONS") on the latest execution returns 200 and echoes the correct pipelineExecutionId, and the preceding rejection path is right (the older, superseded execution is refused StageNotRetryableException with AWS's three-reason sentence), but the count of Deploy/Deploy actionExecutionDetails for that execution stays at 1 for the whole 45 s poll window; the check fails with "condition not met within 45.0s; last=1". The acceptance is therefore a no-op: no new attempt is started and none is recorded.
  Why: a client cannot tell an accepted retry from a silently dropped one. Retry-aware CI logic polls ListActionExecutions (or GetPipelineState's latestExecution) to confirm the stage re-ran and to read the new attempt's result; here it concludes the stage was never re-run, and an automated "retry until it passes" loop either hangs or retries forever. A 200 that changes no state is worse than a refusal.
  Fix: the RetryStageExecution handler and the action-execution history/state in the codepipeline service crate — an accepted retry must re-dispatch the stage's failed actions under the same pipelineExecutionId and APPEND a new action-execution record per attempt rather than mutate or discard the previous one. Related open issue 20261102T084502Z-codepipeline-execution-read-path-5-net-new-root.md (Cause 5) reports the missing appended record for a CodeCommit Source retry from the wire corpus; it does not carry this check's replay key and does not cover the Deploy-action retry path exercised here, so close both or confirm one fix covers both.
  Traces: codepipeline.retry-stage-not-latest-execution-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against this checkout: 173/215 passed; these are the two failing checks not already owned by an open issue.

Cause 1 — [mismatch] codepipeline.missing-s3-source-bucket-fails-the-source-action-with-its-error-details
  The check creates a V2/QUEUED pipeline whose Source stage is an AWS/Source/S3 action pointing at a bucket that was never created (the pipeline role is granted s3:* on that bucket name, so the failure is absence, not denial), waits for the execution to reach Failed, and reads GetPipelineState.
  Assertion: GetPipelineState stageStates[Source].actionStates[Source].latestExecution.errorDetails
    expected: {'code': 'ConfigurationError', 'message': '[No bucket with the name <missing-bucket> was found]'}
    actual:   {'code': 'ConfigurationError', 'message': 'the source object s3://<missing-bucket>/source.zip could not be read: service error: unhandled error (NoSuchBucket): Error { code: "NoSuchBucket", message: "The specified bucket does not exist", aws_request_id: "...", s3_extended_request_id: "..." } (ServiceError(ServiceError { source: Unhandled(Unhandled { source: ErrorMetadata { code: Some("NoSuchBucket"), ... }, meta: ErrorMetadata { ... } }), raw: Response { status: StatusCode(404), headers: {"content-type": "application/xml", ...}, body: SdkBody { inner: Once(Some(b"<Error><Code>NoSuchBucket</Code><Message>The specified bucket does not exist</Message><BucketName>...</BucketName>...")) } } }))'}
  The code matches; only the message diverges. Fixed text of the requirement: "[No bucket with the name " ... " was found]"; the single variable part is the bucket name taken from the action's S3Bucket configuration. The bucket name, request ids and host in the actual value above are per-run values, not part of the contract.
  The same check then requires ListActionExecutions(filter={pipelineExecutionId}) to report exactly one Source detail with output.executionResult == {'errorDetails': {'code': 'ConfigurationError'}, 'externalExecutionSummary': '<that same sentence>'} — i.e. the code alone in errorDetails and the sentence as the provider summary.

Cause 2 — [mismatch] codepipeline.retry-stage-not-latest-execution-rejected
  The check builds a pipeline whose Source stage succeeds (versioned S3 bucket with a real zip) and whose Deploy stage targets a bucket that is never created, so every execution fails in Deploy. It runs two executions to Failed, confirms GetPipelineState's Deploy latestExecution is execution 2, and confirms the retry of execution 1 is refused StageNotRetryableException with AWS's wording and leaves latestExecution unchanged — all of that passes.
  It then counts Deploy/Deploy entries in ListActionExecutions(filter={pipelineExecutionId: <execution 2>}), calls RetryStageExecution(pipelineName, stageName='Deploy', pipelineExecutionId=<execution 2>, retryMode='ALL_ACTIONS') — which returns 200 and echoes <execution 2> — and polls the count for a strict increase.
    expected: count > 1 (a new Deploy action execution for the retried attempt)
    actual:   AssertionError: condition not met within 45.0s; last=1
  Smithy citations (com-amazonaws-codepipeline-codepipeline-20150709): ErrorDetails.code/.message (message @length 1..5000); ActionExecutionResult.externalExecutionSummary ("The action provider's summary for the action execution") and .errorDetails; ActionExecution.errorDetails on ActionState.latestExecution; RetryStageExecution input {pipelineName, stageName, pipelineExecutionId, retryMode} -> output {pipelineExecutionId}, errors StageNotRetryableException / NotLatestPipelineExecutionException / ConflictException; ListActionExecutions paginated over actionExecutionDetails.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay suite simparity-behavioral-aws-v1.

2. Cause 1 — run check codepipeline.missing-s3-source-bucket-fails-the-source-action-with-its-error-details, or by hand:
   a. CreateBucket an artifact bucket (versioning enabled); choose a second bucket name and do NOT create it.
   b. CreateRole trusting codepipeline.amazonaws.com and grant it s3:GetObject/GetObjectVersion/PutObject/GetBucketVersioning/ListBucket on BOTH bucket names, so the failure cannot be a permission denial.
   c. CreatePipeline (pipelineType V2, executionMode QUEUED) with a Source stage action AWS/Source/S3 version 1, configuration {S3Bucket: <the bucket that does not exist>, S3ObjectKey: 'source.zip', PollForSourceChanges: 'false'}, output artifact SourceOutput, and a Deploy stage AWS/Deploy/S3 writing SourceOutput into the artifact bucket.
   d. Poll ListPipelineExecutions/GetPipelineExecution until the execution started by CreatePipeline is Failed.
   e. GetPipelineState(name) and read stageStates[Source].actionStates[Source].latestExecution.errorDetails — the message is the Rust Debug dump instead of '[No bucket with the name <bucket> was found]'.
   f. ListActionExecutions(pipelineName, filter={pipelineExecutionId}) and read the Source detail's output.executionResult — externalExecutionSummary carries the same leaked text.

3. Cause 2 — run check codepipeline.retry-stage-not-latest-execution-rejected, or by hand:
   a. Create an artifact bucket and a versioned source bucket; PutObject a zip at src/app.zip.
   b. CreatePipeline with Source = AWS/Source/S3 (the real bucket/key, PollForSourceChanges false, output SourceOutput) and Deploy = AWS/Deploy/S3 with configuration {BucketName: <a bucket that is never created>, Extract: 'true'}, input SourceOutput.
   c. StartPipelineExecution twice, each time polling GetPipelineExecution to Failed; call them E1 and E2.
   d. Confirm GetPipelineState's Deploy latestExecution.pipelineExecutionId == E2, and that RetryStageExecution for E1 is refused StageNotRetryableException (this part already behaves).
   e. Count Deploy/Deploy entries in ListActionExecutions(pipelineName, filter={pipelineExecutionId: E2}).
   f. RetryStageExecution(pipelineName, stageName='Deploy', pipelineExecutionId=E2, retryMode='ALL_ACTIONS') — it returns 200 echoing E2.
   g. Re-count for up to 45 s: the count never rises above its pre-retry value, i.e. the accepted retry started and recorded no new Deploy attempt.
