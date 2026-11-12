Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__ecr__lambda__logs.refarch-batch-job-fails-before-any-container-on-empty-repository
Issue-Key: batch__ecr__lambda__logs.refarch-batch-job-fails-before-any-container-on-empty-repository|run#25|KeyError|assert len(job['attempts']) == 1, (
Issue-Commit: afd9c28cd10d0281c7cc3c2a5c51ef0c1f3c612b

# batch__ecr__lambda__logs: DescribeJobs reports no `attempts` member for a job that reached FAILED after placement, so the task ARN, container reason, exit code and log stream name are never reported (1 cause)

> _Note: this issue was identified against the code base at commit `afd9c28cd10d0281c7cc3c2a5c51ef0c1f3c612b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJobs

## Description
One net-new behavioral root cause grounded in a failing check of the simparity-behavioral-aws-v1 replay (batch__ecr__lambda__logs 7/15; every other failure in that heading is OWNED by an open issue and is not part of this one). Related open issue, named for the fixer: 20261104T041055Z-batch-ecr-lambda-logs-a-cfn-created-ecr.md mentions "DescribeComputeEnvironments/DescribeJobs omit empty collections"; this check is not marked OWNED by it, and the member missing here is not an empty collection — it is the attempt record itself, which must have exactly one element.

Cause 1 of 1 — a terminal job's JobDetail carries no attempts list.
AWS: Smithy com.amazonaws.batch#JobDetail.attempts targets AttemptDetails and is documented as "A list of job attempts that are associated with this job". Each AttemptDetail carries startedAt/stoppedAt/statusReason and container:AttemptContainerDetail, whose taskArn is documented "Each container attempt receives a task ARN when they reach the STARTING status", with reason, exitCode ("The exit code for the job attempt. A non-zero exit code is considered failed") and logStreamName. For a FARGATE job submitted against an ECR repository that holds no image, AWS places the task (so an attempt exists and carries a taskArn), the pull fails, and the job is FAILED with exactly ONE AttemptDetail whose container has a taskArn and a reason, and no exitCode — the job never reaches RUNNING, so no container ran.
Emulator: the job does reach FAILED and does carry a job-level statusReason, but the DescribeJobs projection has no `attempts` key at all, so reading job['attempts'] raises KeyError. Because the emulator never places a task and never records an attempt, nothing reports the task ARN, the per-attempt reason, the exit code or the attempt's log stream name.
Why: the attempt record is the only place Batch exposes the ECS task identity, the container's exit code, the per-attempt failure reason and the log stream to read — it is how an operator joins a Batch job to its ECS task and its CloudWatch Logs stream, and how a retried job is told apart from a first attempt. A client that reads job['attempts'] (the documented, always-present list) breaks outright instead of seeing the one attempt AWS records.
Fix: the batch crate's DescribeJobs projection together with the job placement/lifecycle state machine that must record an AttemptDetail (with its container taskArn, reason and, once a container runs, exitCode and logStreamName) the moment a job is placed.
Traces: batch__ecr__lambda__logs.refarch-batch-job-fails-before-any-container-on-empty-repository.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout.

[error] batch__ecr__lambda__logs.refarch-batch-job-fails-before-any-container-on-empty-repository
File "check_batch__ecr__lambda__logs_refarch_batch_job_fails_before_any_container_on_empty_repository.py", line 89, in run
  assert len(job['attempts']) == 1, (
KeyError: 'attempts'

The preceding assertions passed: the job reached FAILED, never passed through RUNNING, began at SUBMITTED and carried a non-empty statusReason. The divergence is purely that DescribeJobs' JobDetail has no `attempts` member.

Expected (AWS): attempts is present with exactly one AttemptDetail; attempts[0].container.taskArn names the placed ECS task, attempts[0].container.reason reports the pull failure, and no exitCode is present because no container ran.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create an ECR repository and push nothing to it.
3. Create a MANAGED FARGATE compute environment and an ENABLED job queue, and register a FARGATE job definition whose containerProperties.image is that empty repository.
4. aws batch submit-job --job-name j --job-queue <queue> --job-definition <jdArn>, poll aws batch describe-jobs --jobs <jobId> until status is FAILED, and read the response.
   Observed: the job object has no `attempts` member (KeyError for any client that reads it). AWS: attempts is a one-element list whose container carries taskArn and the pull reason.
5. Re-replay the check id in trace_ids.
