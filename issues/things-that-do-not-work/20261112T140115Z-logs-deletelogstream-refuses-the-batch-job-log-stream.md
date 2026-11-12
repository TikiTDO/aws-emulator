Issue-Source: trace-sweep-divergence
Issue-Trace: batch-fargate-job-runs-to-succeeded
Issue-Step: teardown#18
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: batch-fargate-job-runs-to-succeeded
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: logs|DeleteLogStream|2xx->4xx|status
Issue-Commit: a6732131b4577aead0e874dcd27f439fc83f19d6

# logs DeleteLogStream refuses the Batch job log stream with 400 instead of deleting it

> _Note: this issue was identified against the code base at commit `a6732131b4577aead0e874dcd27f439fc83f19d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteLogStream

## Description
One net-new root cause in the logs (CloudWatch Logs) emulator, found by replaying the wire fixture corpus (trace set simparity-wire-aws-v1).

Cause 1 of 1: DeleteLogStream answers HTTP 400 where AWS answers 200 with an empty body, for the /aws/batch/job log stream of a job that has run.

AWS: DeleteLogStream (Smithy: logs DeleteLogStream, input DeleteLogStreamRequest.{logGroupName required, @length 1..512, @pattern ^[\.\-_/#A-Za-z0-9]+$; logStreamName required, @length 1..512, @pattern ^[^:*]*$}, output Unit — an empty body) deletes the named stream and its stored events and answers HTTP 200 with no body. It refuses with 400 only on a modeled condition: ResourceNotFoundException (400) when the log group or log stream does not exist, InvalidParameterException / ValidationException (400) for a malformed parameter, OperationAbortedException (400) for a concurrent conflicting operation. For a stream that exists, 200 is the contract — the recorded AWS response for this step is 200.

Emulator: the same request answers HTTP 400 (`status: expected 200, got 400`), so the stream is never deleted and the caller sees a refusal for a delete AWS performs. Hypothesis: the stream is absent from logs state because nothing ever created it — this corpus step deletes the log stream that the Batch job's container writes, and this emulator never runs the job's container (see the sibling batch divergence on the same fixture, step task#17), so the delete resolves to the modeled not-found refusal. Either way the step is served by the logs emulator and diverges: a log stream AWS has at this point in the trajectory does not exist here, and the delete is refused.

Why: a client tearing down resources (a CI teardown, a CDK/CloudFormation destroy, or any cleanup loop) treats a 400 from DeleteLogStream as a real failure and either aborts the teardown or reports leaked resources; a client that retries gets the same refusal forever. More broadly, any consumer that expects a job's /aws/batch/job stream to exist after the job terminates — reading it, filtering it, or deleting it — cannot observe it at all.

Fix: the DeleteLogStream handler and the log-stream state it consults in the logs crate (a delete of an existing stream must answer 200 with an empty body and remove the stream and its events), together with the log-stream creation path for job/container log delivery that should have materialized the stream before this step.

Traces: batch-fargate-job-runs-to-succeeded#teardown#18.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up and serving; score 84.8%, 24 mismatched of 165 steps).

DeleteLogStream, service logs, fixture batch-fargate-job-runs-to-succeeded step teardown#18 (cluster affected_count 1, owned by no open issue, no stub):
  status: expected 200, got 400

The recorded AWS response is 200 with an empty body (output shape Unit). The emulator's 400 is the only reported difference — no body assertion is carried by this step, so the requirement at issue is the status: 200 for a stream that exists, and a 400 only under the modeled refusals (ResourceNotFoundException, InvalidParameterException, ValidationException, OperationAbortedException).

Model citations: Smithy logs DeleteLogStream — input DeleteLogStreamRequest.logGroupName (required, length 1..512, pattern ^[\.\-_/#A-Za-z0-9]+$) and .logStreamName (required, length 1..512, pattern ^[^:*]*$); output Unit; errors InvalidParameterException 400, OperationAbortedException 400, ResourceNotFoundException 400, ServiceUnavailableException 500, ValidationException 400.

The log group name, stream name and job id in the recording are per-run values and are not requirements.

## Steps to Reproduce
Replay fixture batch-fargate-job-runs-to-succeeded through its teardown: submit a FARGATE Batch job, let it terminate, then call logs DeleteLogStream for the job's log group and stream (step teardown#18). AWS answers HTTP 200 with an empty body and the stream is gone; the emulator answers HTTP 400 and the delete never happens.
