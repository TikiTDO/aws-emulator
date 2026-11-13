Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__s3__stepfunctions.glue-start-job-run-sync-task-output-carries-the-sdk-envelope glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-without-get-job-run-is-denied-and-times-out
Issue-Key: glue__s3__stepfunctions.glue-start-job-run-sync-task-output-carries-the-sdk-envelope|_assert_envelope#12|AssertionError|assert isinstance(req_id, str) and req_id, (
Issue-Key: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-without-get-job-run-is-denied-and-times-out|run#22|AssertionError|assert bad["status"] == "FAILED", (
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# glue__s3__stepfunctions: 2 net-new causes — a glue:startJobRun.sync TaskSubmitted event carries no SdkResponseMetadata.RequestId, and a .sync task whose role lacks glue:GetJobRun still SUCCEEDS

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Two causes.

Cause 1 of 2: the TaskSubmitted event of an .sync service integration carries no SdkResponseMetadata.RequestId.
AWS: for an optimized/SDK service integration, the TaskSubmitted event's output is the API response envelope — for glue:startJobRun it carries JobRunId plus SdkHttpMetadata and SdkResponseMetadata, and SdkResponseMetadata.RequestId is the non-empty request id string of the underlying StartJobRun call (Smithy: glue StartJobRun returns JobRunId; the AWS request id is carried on every response's metadata). Callers read it to correlate the state machine with the service call.
Emulator: SdkResponseMetadata.RequestId is None in the TaskSubmitted event.
Why: the envelope a state machine (or a downstream Choice/Pass state, or an operator correlating logs) reads is incomplete; any JSONPath into $.SdkResponseMetadata.RequestId yields null and the state machine or the consumer breaks. The *value* of a request id is per-run, but its presence and string type are the requirement.
Fix: the Step Functions service-integration response envelope builder must populate SdkResponseMetadata.RequestId (and SdkHttpMetadata) from the underlying Glue call's response metadata.
Traces: glue__s3__stepfunctions.glue-start-job-run-sync-task-output-carries-the-sdk-envelope.

Cause 2 of 2: a .sync task whose execution role cannot poll the job run reports SUCCEEDED instead of failing.
AWS: an .sync integration completes only after Step Functions polls the job to a terminal state. If the state machine role is denied glue:GetJobRun, the polling call is refused and the task fails — the execution ends FAILED (a States.TaskFailed/AccessDeniedException-class error naming the denied action) and eventually times out rather than succeeding. Smithy: glue GetJobRun is the poll; its AccessDeniedException is a client error.
Emulator: the execution with the stripped role ends 'SUCCEEDED' with no error/cause at all, i.e. the .sync wait is not performed through an authorized GetJobRun at all.
Why: a least-privilege regression becomes invisible: a workflow that in AWS fails loudly (and is caught/retried) instead reports success, so downstream states run on a job whose outcome was never observed.
Fix: the Step Functions .sync integration must perform its completion poll as the execution role and propagate the poll's authorization refusal as a task failure.
Traces: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-without-get-job-run-is-denied-and-times-out. The open issue 20261107T185637Z-glue-s3-stepfunctions-a-glue-job-run-reached-through-a-step.md covers a neighbouring check and does not carry this one's replay key.

## Evidence
Cause 1: AssertionError: the glue:startJobRun.sync TaskSubmitted SdkResponseMetadata.RequestId was None (expected a non-empty string).
Cause 2: AssertionError: a role without glue:GetJobRun ended 'SUCCEEDED' (None None), expected FAILED.

## Steps to Reproduce
Cause 1: create a Glue job; run a state machine with a Task state Resource 'arn:aws:states:::glue:startJobRun.sync'; read the TaskSubmitted event's output and inspect SdkResponseMetadata.RequestId.
Cause 2: give the state machine role glue:StartJobRun but deny/omit glue:GetJobRun; start the same .sync execution -> it ends SUCCEEDED instead of FAILED.
