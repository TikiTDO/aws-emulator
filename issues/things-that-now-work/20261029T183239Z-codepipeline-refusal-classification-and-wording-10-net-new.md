Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.acknowledge-job-not-found codepipeline.get-job-details-not-found codepipeline.put-job-success-result-wrong-state-rejected codepipeline.put-job-failure-invalid-job-state-rejected codepipeline.put-job-failure-on-completed-job-rejected codepipeline.put-job-failure-result-wrong-job-state-rejected codepipeline.put-job-success-on-already-completed-job-not-permissive codepipeline.put-job-success-result-completed-job-rejected codepipeline.output-variables-size-exceeded-rejected codepipeline.put-job-success-output-variables-size-exceeded-rejected codepipeline.put-approval-result-invalid-token-rejected codepipeline.put-approval-result-stale-token-rejected codepipeline.put-approval-result-token-semantics codepipeline.put-approval-result-nonexistent-action-rejected codepipeline.get-action-type-not-found codepipeline.pipeline-lifecycle-delete-then-404 codepipeline.e2e-pipeline-create-run-approve-teardown codepipeline.e2e-pipeline-create-start-approve-complete-teardown codepipeline.get-pipeline-execution-unknown-execution-id-rejected codepipeline.start-pipeline-execution-concurrency-limit-exceeded codepipeline.start-pipeline-execution-concurrency-limit-rejected codepipeline.stop-pipeline-execution-duplicate-request-rejected codepipeline.stop-pipeline-execution-terminal-not-stoppable codepipeline.retry-stage-execution-not-failed-rejected codepipeline.retry-stage-not-latest-execution-rejected
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.acknowledge-job-not-found|run#19|AssertionError|assert (code, message) == ("JobNotFoundException", f"Job with id {unknown_job_id} does not exist"), (code, message)
Issue-Key: codepipeline.get-job-details-not-found|run#11|AssertionError|assert refusal == ("JobNotFoundException", f"Job with id {bogus_id} does not exist"), refusal
Issue-Key: codepipeline.put-job-success-result-wrong-state-rejected|run#23|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: codepipeline.put-job-failure-invalid-job-state-rejected|run#18|AssertionError|assert refusal == (
Issue-Key: codepipeline.put-job-failure-on-completed-job-rejected|run#12|AssertionError|assert refusal == (
Issue-Key: codepipeline.put-job-failure-result-wrong-job-state-rejected|run#12|AssertionError|assert error.get("Code") == "InvalidJobStateException", (
Issue-Key: codepipeline.put-job-success-on-already-completed-job-not-permissive|run#21|AssertionError|assert error.get("Code") == "InvalidJobStateException", f"unexpected error: {error!r}"
Issue-Key: codepipeline|PutJobSuccessResult|JobNotFoundException
Issue-Key: codepipeline.output-variables-size-exceeded-rejected|run#11|AssertionError|assert (code, message) == (
Issue-Key: codepipeline.put-job-success-output-variables-size-exceeded-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codepipeline.put-approval-result-invalid-token-rejected|run#19|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: codepipeline.put-approval-result-stale-token-rejected|run#12|AssertionError|assert (code, message) == (
Issue-Key: codepipeline.put-approval-result-token-semantics|run#23|AssertionError|assert (code, msg) == (_ERR_TOKEN, expected_msg), (
Issue-Key: codepipeline.put-approval-result-nonexistent-action-rejected|run#13|AssertionError|assert (code, msg) == ("ActionNotFoundException", _MSG_BAD_ACTION), (
Issue-Key: codepipeline.get-action-type-not-found|run#9|AssertionError|assert refusal == (
Issue-Key: codepipeline.pipeline-lifecycle-delete-then-404|run#18|AssertionError|assert start_refusal == start_expected, f"expected StartPipelineExecution on deleted pipeline to raise {start_expected!r}, got {start_refusal!r}"
Issue-Key: codepipeline.e2e-pipeline-create-run-approve-teardown|run#26|AssertionError|assert mutate_refusal == (
Issue-Key: codepipeline.e2e-pipeline-create-start-approve-complete-teardown|run#25|AssertionError|assert start_refusal == (
Issue-Key: codepipeline.get-pipeline-execution-unknown-execution-id-rejected|run#16|AssertionError|assert refusal2 == (
Issue-Key: codepipeline.start-pipeline-execution-concurrency-limit-exceeded|run#13|AssertionError|assert limit_message == _LIMIT_MESSAGE, limit_message
Issue-Key: codepipeline.start-pipeline-execution-concurrency-limit-rejected|run#17|AssertionError|assert limit_messages == {expected}, f"expected message {expected!r}, got {limit_messages!r}"
Issue-Key: codepipeline.stop-pipeline-execution-duplicate-request-rejected|run#13|AssertionError|assert code == "PipelineExecutionNotStoppableException", f"expected PipelineExecutionNotStoppableException, got code={code!r}"
Issue-Key: codepipeline.stop-pipeline-execution-terminal-not-stoppable|run#20|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codepipeline.retry-stage-execution-not-failed-rejected|run#30|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codepipeline.retry-stage-not-latest-execution-rejected|run#19|AssertionError|assert code == "StageNotRetryableException", f"got code={code!r}"
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# CodePipeline refusal classification and wording: 10 net-new root causes (job lifecycle states, approval tokens, not-found texts, limits, stop, retry)

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutJobFailureResult

## Description
TEN net-new root causes in the codepipeline refusal paths. A fixer must close ALL TEN before submitting; each cause names its own failing check ids below. None of these checks is marked ALREADY FILED or OWNED in this replay. Related but NOT covering: the open issue 20261028T112857Z-codepipeline-job-worker-lifecycle-and-refusal-wording-8.md carries OTHER job-worker check ids (put-job-success-invalid-job-state-rejected, put-job-success-result-wrong-job-state-rejected, retry-stage-execution-stale-token-rejected), and the open issue 20261023T080113Z-codepipeline-control-plane-declaration-validation-stage-transition-state.md carries other control-plane ids; neither carries the ids listed here.

=== Cause 1 of 10: JobNotFoundException's message adds a trailing period and single-quotes the job id ===
AWS: Smithy (com.amazonaws.codepipeline#CodePipeline_20150709) models JobNotFoundException on AcknowledgeJob/GetJobDetails/PutJobSuccessResult/PutJobFailureResult (client error, HTTP 400). Recorded AWS message for an unknown job id is exactly `Job with id <jobId> does not exist` — fixed text `Job with id ` + the jobId FROM THE REQUEST, unquoted, with NO terminating period.
Emulator: returns code JobNotFoundException (correct) with `Job with id '<jobId>' does not exist.` — the id is wrapped in single quotes and a period is appended.
Why: a client (and any log/alarm pattern) matching the documented AWS text does not match; message equality is part of the refusal contract.
Fix: the message template used by the job-lookup refusal in the codepipeline crate's job handlers (AcknowledgeJob/GetJobDetails/PutJob*Result shared not-found path).
Traces: codepipeline.acknowledge-job-not-found, codepipeline.get-job-details-not-found, codepipeline.put-job-success-result-wrong-state-rejected.

=== Cause 2 of 10: a job that reached a terminal state is forgotten, so a late result is refused JobNotFoundException instead of InvalidJobStateException ===
AWS: Smithy models BOTH InvalidJobStateException and JobNotFoundException on PutJobSuccessResult/PutJobFailureResult. A job that EXISTS but has already terminated is refused InvalidJobStateException with the message `Job with id <jobId> has already terminated`; reporting the SAME terminal status twice is refused InvalidJobStateException with `The status of job with id <jobId> is already set to the desired value. It cannot be updated with the same status, but different result or output variables.` (fixed text; the jobId comes from the request). JobNotFoundException is reserved for an id that was never issued.
Emulator: after a job completes (via PutJobSuccessResult or PutJobFailureResult) a further Put*Result on that same jobId answers JobNotFoundException `Job with id '<jobId>' does not exist.` Hypothesis (not stated by the model or the recording): completed jobs are dropped from the job table instead of being retained in a terminal state.
Why: a job worker cannot distinguish "I already reported this job" (benign, retry-safe) from "this job id is bogus" (a programming error), and the documented single-terminal-state guarantee becomes unobservable.
Fix: the job state store in the codepipeline crate (retain terminal jobs and their status) and the state check in the PutJobSuccessResult/PutJobFailureResult handlers.
Traces: codepipeline.put-job-failure-invalid-job-state-rejected, codepipeline.put-job-failure-on-completed-job-rejected, codepipeline.put-job-failure-result-wrong-job-state-rejected, codepipeline.put-job-success-on-already-completed-job-not-permissive, codepipeline.put-job-success-result-completed-job-rejected.

=== Cause 3 of 10: OutputVariablesSizeExceededException's message is the emulator's own wording and omits the per-variable byte breakdown ===
AWS: Smithy models OutputVariablesSizeExceededException (client, 400) on PutJobSuccessResult. Recorded AWS message: `The total size of all output variables for this action exceeds the limit of 122880 bytes. Output variables (in bytes): [<name>: <bytes>]` — the fixed text and the 122880 limit are constant; the bracketed list names each output variable from the request with its measured byte size.
Emulator: code is right, message is `Exceeded the total size limit for all variables in the pipeline: <n> bytes reported, at most 122880 allowed` — different fixed text and no per-variable breakdown.
Why: a worker cannot tell WHICH variable blew the budget, and the message does not match the documented contract.
Fix: the message construction in the PutJobSuccessResult handler's output-variable size validation.
Traces: codepipeline.output-variables-size-exceeded-rejected, codepipeline.put-job-success-output-variables-size-exceeded-rejected.

=== Cause 4 of 10: InvalidApprovalTokenException's message does not name the action and uses the wrong wording ===
AWS: Smithy models InvalidApprovalTokenException on PutApprovalResult. Recorded AWS message for a token that is not the current one: `Approval token \u2018<token>\u2019 either does not exist or is not the correct token for action <actionName> and stage <stageName>.` — fixed text with the token wrapped in U+2018/U+2019 curly quotes, and the ACTION and STAGE names taken from the request. The same code with a DIFFERENT message is used when the addressed action is not an approval action: `<pipelineName> does not include an approval action named <actionName>.`
Emulator: `The approval token '<token>' does not match the token of an approval awaiting a response in stage '<stage>' of pipeline '<pipeline>'` — ASCII quotes, different fixed text, names the pipeline instead of the action, so the two same-code cases are indistinguishable.
Why: the message is the only discriminator between the two InvalidApprovalTokenException cases AWS distinguishes, so a client cannot tell "wrong token" from "not an approval action".
Fix: the PutApprovalResult handler's token/approval-action validation messages.
Traces: codepipeline.put-approval-result-invalid-token-rejected, codepipeline.put-approval-result-stale-token-rejected, codepipeline.put-approval-result-token-semantics.

=== Cause 5 of 10: ActionNotFoundException's message for PutApprovalResult uses the wrong wording ===
AWS: Smithy models ActionNotFoundException on PutApprovalResult. Recorded AWS message: `<stageName> does not include an action named <actionName>. Make sure you are using the correct name for the action, and then try again.` — fixed text around the stage and action names from the request.
Emulator: code is right, message is `Stage '<stage>' of pipeline '<pipeline>' does not have an action with name '<action>'`.
Why: wording divergence on a modeled refusal; clients and operators matching the AWS text see an unknown message.
Fix: the action-resolution refusal message in the PutApprovalResult handler.
Traces: codepipeline.put-approval-result-nonexistent-action-rejected.

=== Cause 6 of 10: ActionTypeNotFoundException's message for GetActionType is bracketed and carries an extra sentence ===
AWS: Smithy models ActionTypeNotFoundException on GetActionType (client, 400). Recorded AWS message: `Action type with owner=<owner>, category=<category>, provider=<provider> and version=<version> does not exist.` — fixed text with the four identity values from the request, unbracketed, one sentence.
Emulator: `An action type with owner=[ThirdParty], category=[Build], provider=[<provider>] and version=[1] doesn't exist. To create an action type, contact the CodePipeline service team.` — values wrapped in square brackets, `doesn't exist` instead of `does not exist`, plus a trailing sentence AWS does not send.
Why: wording divergence on the only refusal GetActionType has a reachable path to.
Fix: the ActionTypeNotFoundException message template in the GetActionType handler.
Traces: codepipeline.get-action-type-not-found.

=== Cause 7 of 10: PipelineNotFoundException carries ONE canned message for every operation; AWS's text is operation-specific ===
AWS: Smithy models PipelineNotFoundException on GetPipeline, StartPipelineExecution, GetPipelineExecution and others. The recorded AWS messages differ per operation: GetPipeline -> `Account '<accountId>' does not have a pipeline with name '<name>'`; StartPipelineExecution -> `Pipeline with name <name> does not exist in account <accountId>.`; GetPipelineExecution (unknown pipeline NAME) -> `The account with id '<accountId>' does not include a pipeline with the name '<name>'`. In each the fixed text is per-operation and the account id and pipeline name come from the request/caller identity.
Emulator: every operation answers the GetPipeline wording `Account '<accountId>' does not have a pipeline with name '<name>'`, so GetPipeline passes while StartPipelineExecution and GetPipelineExecution diverge.
Why: a client that matches the documented per-operation text (common in deployment tooling that distinguishes a deleted pipeline on a read from one on a mutation) does not match; the refusals are also indistinguishable from each other.
Fix: per-operation PipelineNotFoundException message construction (StartPipelineExecution and GetPipelineExecution handlers) rather than one shared not-found string in the codepipeline crate's pipeline lookup.
Traces: codepipeline.pipeline-lifecycle-delete-then-404, codepipeline.e2e-pipeline-create-run-approve-teardown, codepipeline.e2e-pipeline-create-start-approve-complete-teardown, codepipeline.get-pipeline-execution-unknown-execution-id-rejected.

=== Cause 8 of 10: ConcurrentPipelineExecutionsLimitExceededException's message is the emulator's own wording ===
AWS: Smithy models ConcurrentPipelineExecutionsLimitExceededException (client, 400) on StartPipelineExecution. Recorded AWS message, a constant with no variable parts: `The limit for concurrent pipeline executions has been exceeded. Please try again later.`
Emulator: code is right, message is `The pipeline has reached the limit for concurrent pipeline executions.`
Why: wording divergence on a throttling-class refusal clients commonly match before backing off.
Fix: the concurrency-limit refusal message in the StartPipelineExecution handler.
Traces: codepipeline.start-pipeline-execution-concurrency-limit-exceeded, codepipeline.start-pipeline-execution-concurrency-limit-rejected.

=== Cause 9 of 10: stopping an execution that is already terminal is refused with the wrong code AND message ===
AWS: Smithy models both DuplicatedStopRequestException and PipelineExecutionNotStoppableException on StopPipelineExecution. An execution that has already reached a terminal status has no action executions in progress, so AWS refuses PipelineExecutionNotStoppableException with `The pipeline execution with ID '<pipelineExecutionId>' in '<pipelineName>' can't be stopped now. Either it does not exist in the pipeline, or the pipeline execution has no action executions in progress.` — fixed text with the execution id and pipeline name from the request. DuplicatedStopRequestException is for a stop issued while a stop is already in flight on a still-running execution.
Emulator: answers DuplicatedStopRequestException for a second stop of an already-Stopped execution, with the message `Pipeline execution <id> has already reached the terminal status Stopped and cannot be stopped`.
Why: a client retrying a stop sees a code that means "your first stop is still being processed" instead of "this execution is not stoppable", and the message text diverges too.
Fix: the state classification and message in the StopPipelineExecution handler (terminal status -> PipelineExecutionNotStoppableException; in-flight stop -> DuplicatedStopRequestException).
Traces: codepipeline.stop-pipeline-execution-duplicate-request-rejected, codepipeline.stop-pipeline-execution-terminal-not-stoppable.

=== Cause 10 of 10: RetryStageExecution refusals use the wrong code and the wrong message ===
AWS: Smithy models StageNotRetryableException, NotLatestPipelineExecutionException, StageNotFoundException and others on RetryStageExecution. A stage that is not in a retryable (failed) state is refused StageNotRetryableException with `The stage named <stageName> in the <pipelineName> pipeline can't be retried now. Either it contains no failed actions, the execution '<pipelineExecutionId>' is already superseded, or the pipeline definition has changed since the action(s) failed.` — fixed text with the stage name, pipeline name and execution id from the request. The recorded AWS verdict for a retry addressed at an execution that is not the latest is also StageNotRetryableException (the stage holds no failed action for that superseded execution).
Emulator: for a non-failed stage it answers the right code with its own message `Stage '<stage>' of pipeline execution <id> is not in a failed state, so it cannot be retried`; for a non-latest execution it answers NotLatestPipelineExecutionException where AWS answers StageNotRetryableException.
Why: a retry driver branching on the code retries or gives up incorrectly, and the message does not carry the three AWS-documented reasons.
Fix: the retryability classification and its message in the RetryStageExecution handler.
Traces: codepipeline.retry-stage-execution-not-failed-rejected, codepipeline.retry-stage-not-latest-execution-rejected.

## Evidence
All quotes are verbatim from this replay's failing-check details; job ids, pipeline names, tokens, execution ids and account ids are per-run values that fill the message template and are not themselves requirements.

Cause 1 — codepipeline.acknowledge-job-not-found [mismatch]:
  expected ('JobNotFoundException', "Job with id 8208d258-... does not exist"), got ('JobNotFoundException', "Job with id '8208d258-a241-4061-b5c8-84737d488a14' does not exist.")
  codepipeline.get-job-details-not-found: same pair. codepipeline.put-job-success-result-wrong-state-rejected: "expected message \"Job with id 'def0fa90-...' does not exist\", got \"Job with id 'def0fa90-...' does not exist.\""

Cause 2 — codepipeline.put-job-failure-result-wrong-job-state-rejected [mismatch]:
  AssertionError: expected InvalidJobStateException failing a Succeeded job, got {'Message': "Job with id 'ea9f907c-...' does not exist.", 'Code': 'JobNotFoundException'}
  codepipeline.put-job-success-on-already-completed-job-not-permissive: unexpected error: {'Code': 'JobNotFoundException', 'Message': "Job with id '076fbcd4-...' does not exist."}
  codepipeline.put-job-success-result-completed-job-rejected [error]: botocore.errorfactory.JobNotFoundException ... when calling the PutJobSuccessResult operation: Job with id '29ead18a-...' does not exist.
  codepipeline.put-job-failure-on-completed-job-rejected expected ('InvalidJobStateException', 'Job with id <id> has already terminated'); codepipeline.put-job-failure-invalid-job-state-rejected expected ('InvalidJobStateException', 'The status of job with id <id> is already set to the desired value. It cannot be updated with the same status, but different result or output variables.').

Cause 3 — codepipeline.put-job-success-output-variables-size-exceeded-rejected [mismatch]:
  expected message 'The total size of all output variables for this action exceeds the limit of 122880 bytes. Output variables (in bytes): [BIG: 123003]', got 'Exceeded the total size limit for all variables in the pipeline: 123003 bytes reported, at most 122880 allowed'
  codepipeline.output-variables-size-exceeded-rejected: got ('OutputVariablesSizeExceededException', 'Exceeded the total size limit for all variables in the pipeline: 130006 bytes reported, at most 122880 allowed') where AWS sends '... Output variables (in bytes): [BigVar: 130006]'.

Cause 4 — codepipeline.put-approval-result-invalid-token-rejected [mismatch]:
  expected message 'Approval token \u2018458558ed-...\u2019 either does not exist or is not the correct token for action ApproveAction and stage Approve.', got "The approval token '458558ed-...' does not match the token of an approval awaiting a response in stage 'Approve' of pipeline 'sim2real-cp-pipe-...'"
  Same shape in codepipeline.put-approval-result-stale-token-rejected and codepipeline.put-approval-result-token-semantics (the latter also asserts the non-approval-action variant '<pipeline> does not include an approval action named <action>.').

Cause 5 — codepipeline.put-approval-result-nonexistent-action-rejected [mismatch]:
  expected ('ActionNotFoundException', 'Approval does not include an action named NoSuchApprovalAction. Make sure you are using the correct name for the action, and then try again.'), got 'ActionNotFoundException' ("Stage 'Approval' of pipeline 'sim2real-cp-pipe-...' does not have an action with name 'NoSuchApprovalAction'")

Cause 6 — codepipeline.get-action-type-not-found [mismatch]:
  expected ('ActionTypeNotFoundException', 'Action type with owner=ThirdParty, category=Build, provider=<p> and version=1 does not exist.'), got ("An action type with owner=[ThirdParty], category=[Build], provider=[<p>] and version=[1] doesn't exist. To create an action type, contact the CodePipeline service team.")

Cause 7 — codepipeline.e2e-pipeline-create-run-approve-teardown [mismatch]: GetPipeline on the deleted pipeline matched AWS exactly ("Account '000000000000' does not have a pipeline with name '<name>'"), then StartPipelineExecution returned that SAME string where AWS sends 'Pipeline with name <name> does not exist in account 000000000000.'. codepipeline.pipeline-lifecycle-delete-then-404 and codepipeline.e2e-pipeline-create-start-approve-complete-teardown fail identically. codepipeline.get-pipeline-execution-unknown-execution-id-rejected passed the PipelineExecutionNotFoundException assertion and failed only on the unknown-pipeline-name wording: expected "The account with id '000000000000' does not include a pipeline with the name '<name>'".

Cause 8 — codepipeline.start-pipeline-execution-concurrency-limit-rejected [mismatch]:
  expected message 'The limit for concurrent pipeline executions has been exceeded. Please try again later.', got {'The pipeline has reached the limit for concurrent pipeline executions.'}

Cause 9 — codepipeline.stop-pipeline-execution-duplicate-request-rejected [mismatch]: expected PipelineExecutionNotStoppableException, got code='DuplicatedStopRequestException'. codepipeline.stop-pipeline-execution-terminal-not-stoppable [mismatch]: expected "The pipeline execution with ID '<id>' in '<pipeline>' can't be stopped now. Either it does not exist in the pipeline, or the pipeline execution has no action executions in progress.", got 'Pipeline execution <id> has already reached the terminal status Stopped and cannot be stopped'.

Cause 10 — codepipeline.retry-stage-not-latest-execution-rejected [mismatch]: expected StageNotRetryableException, got code='NotLatestPipelineExecutionException'. codepipeline.retry-stage-execution-not-failed-rejected [mismatch]: expected "The stage named Build in the <pipeline> pipeline can't be retried now. Either it contains no failed actions, the execution '<id>' is already superseded, or the pipeline definition has changed since the action(s) failed.", got "Stage 'Build' of pipeline execution <id> is not in a failed state, so it cannot be retried".

## Steps to Reproduce
Boot the emulator and point botocore at it: cp = boto3.client('codepipeline'), s3 = boto3.client('s3'), iam = boto3.client('iam').

Cause 1: cp.acknowledge_job(jobId=str(uuid.uuid4()), nonce='1') and cp.get_job_details(jobId=str(uuid.uuid4())) -> expect JobNotFoundException with message exactly 'Job with id <id> does not exist' (no quotes around the id, no trailing period).
Cause 2: create an artifact bucket + pipeline role, CreateCustomActionType, create a pipeline with that Custom action, StartPipelineExecution, PollForJobs, AcknowledgeJob, then PutJobSuccessResult(jobId=J). Now call PutJobFailureResult(jobId=J, failureDetails={'type':'JobFailed','message':'late'}) -> expect InvalidJobStateException 'Job with id J has already terminated'; observe JobNotFoundException. Also call PutJobFailureResult twice on a failed job -> expect InvalidJobStateException 'The status of job with id J is already set to the desired value. It cannot be updated with the same status, but different result or output variables.'
Cause 3: on an acknowledged job call cp.put_job_success_result(jobId=J, outputVariables={'BigVar': 'X'*130000}) -> expect OutputVariablesSizeExceededException 'The total size of all output variables for this action exceeds the limit of 122880 bytes. Output variables (in bytes): [BigVar: 130006]'.
Cause 4/5: create a pipeline with a Manual approval action, wait for its token, then (a) cp.put_approval_result(..., token=str(uuid.uuid4())) -> expect InvalidApprovalTokenException 'Approval token \u2018<token>\u2019 either does not exist or is not the correct token for action <action> and stage <stage>.'; (b) address a non-approval action in another stage -> expect the same code with '<pipeline> does not include an approval action named <action>.'; (c) actionName='NoSuchApprovalAction' -> expect ActionNotFoundException '<stage> does not include an action named NoSuchApprovalAction. Make sure you are using the correct name for the action, and then try again.'
Cause 6: cp.get_action_type(category='Build', owner='ThirdParty', provider='<unused>', version='1') -> expect ActionTypeNotFoundException 'Action type with owner=ThirdParty, category=Build, provider=<unused> and version=1 does not exist.'
Cause 7: create a pipeline P, cp.delete_pipeline(name=P), then cp.get_pipeline(name=P) (message A), cp.start_pipeline_execution(name=P) (expect 'Pipeline with name P does not exist in account <acct>.') and cp.get_pipeline_execution(pipelineName=P, pipelineExecutionId=<any uuid>) (expect "The account with id '<acct>' does not include a pipeline with the name 'P'"). All three currently return message A.
Cause 8: drive StartPipelineExecution past the concurrent-execution limit -> expect ConcurrentPipelineExecutionsLimitExceededException 'The limit for concurrent pipeline executions has been exceeded. Please try again later.'
Cause 9: stop a running execution, let it reach Stopped, then cp.stop_pipeline_execution(pipelineName=P, pipelineExecutionId=E) again -> expect PipelineExecutionNotStoppableException with the "can't be stopped now. Either it does not exist in the pipeline, or the pipeline execution has no action executions in progress." message.
Cause 10: cp.retry_stage_execution on a stage with no failed action -> expect StageNotRetryableException with the three-reason message; same on a superseded (non-latest) execution -> expect StageNotRetryableException, not NotLatestPipelineExecutionException.
Then re-replay the check ids listed in trace_ids.
