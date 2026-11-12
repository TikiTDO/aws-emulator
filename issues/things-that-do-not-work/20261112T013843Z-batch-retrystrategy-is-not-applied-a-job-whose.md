Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.retry-strategy-unmatched-evaluate-on-exit-condition-still-retries
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.retry-strategy-unmatched-evaluate-on-exit-condition-still-retries|run#11|AssertionError|assert len(u_attempts) == _ATTEMPTS, (
Issue-Commit: 914d2c73e73c203555b0bde73b4090e0da9a0d25

# batch: retryStrategy is not applied — a job whose evaluateOnExit list matches no attempt is failed after a single attempt instead of being retried to `attempts`, and that attempt carries the no-container-runtime sentinel reason with no taskArn/exitCode

> _Note: this issue was identified against the code base at commit `914d2c73e73c203555b0bde73b4090e0da9a0d25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
One net-new root cause in the AWS Batch job attempt/retry lifecycle, grounded in the failing behavioral check the suite lists under `### batch` as `batch.retry-strategy-unmatched-evaluate-on-exit-condition-still-retries`. The suite marked this check neither ALREADY FILED nor OWNED: the open issue 20261107T210820Z-batch-the-job-data-plane-never-executes-a.md (Cause 5) carries this check's OLD replay key, which was a `KeyError: 'attempts'` on a DescribeJobs response that omitted the member entirely; the fixed issue 20261107T153156Z-batch-ecr-lambda-logs-describejobs-reports-no-attempts.md made `attempts` present. The member now exists but its CONTENT diverges, which is a different failure signature and the reason the check is unowned. This is not a refusal — the operations all answer 2xx — so it is not a reopening of a refusal-shaped fix.

=== Cause 1 of 1: an unmatched evaluateOnExit condition is treated as terminal, so the job records exactly one attempt instead of `attempts`, and that attempt is a synthetic non-execution record ===

AWS: Smithy: `RetryStrategy.attempts` — "The number of times to move a job to the RUNNABLE status. You can specify between 1 and 10 attempts. If the value of attempts is greater than one, the job is retried on failure the same number of attempts as the value." `RetryStrategy.evaluateOnExit` — "Array of up to 5 objects that specify the conditions where jobs are retried or failed. If this parameter is specified, then the attempts parameter must also be specified. If none of the listed conditions match, then the job is retried." The `EvaluateOnExit` shape repeats the rule: "If none of the EvaluateOnExit conditions in a RetryStrategy match, then the job is retried." So for a job submitted with `retryStrategy={attempts: N, evaluateOnExit: [{onExitCode: "7", action: "EXIT"}]}` whose container exits with a code that matches NO listed condition (here 3, which the `onExitCode` glob "7" does not match), AWS retries to the full `attempts` budget: `JobDetail.attempts` ends with exactly N `AttemptDetail` entries, the job ends FAILED, and each entry is a distinct execution — `AttemptDetail.container.exitCode` is "The exit code for the job attempt" (the container's real status, 3 for every attempt here), and `AttemptDetail.container.taskArn` is "The Amazon Resource Name (ARN) of the Amazon ECS task that's associated with the job attempt. Each container attempt receives a task ARN when they reach the STARTING status" — so N attempts carry N distinct, non-null taskArns. The symmetric leg of the same rule list (a container exiting 7, which DOES match `action: EXIT`) stops after exactly 1 attempt even though `attempts` is N.

Emulator: the unmatched-condition job ends FAILED with `attempts` of length 1 regardless of `retryStrategy.attempts` — the single attempt is reported with `statusReason`/`container.reason` set to the emulator-authored sentence "This emulator does not run containers, so the job's container was never started", no `container.exitCode` and no `container.taskArn`. The retry budget is never consumed: an unmatched condition is handled as if it were `action: EXIT`, and no further attempt is ever moved to RUNNABLE. Because the attempt record is synthetic rather than an execution, the per-attempt exit code (3) and the distinct per-attempt ECS task ARNs are also absent; the matched-condition leg is indistinguishable from the unmatched one, since both report one non-executing attempt.

Why: a client relying on AWS's documented retry default gets the opposite semantics — a transient failure that AWS would retry up to `attempts` times is reported terminal after one pass, so retry-based resilience silently disappears and the `evaluateOnExit` rule list has no observable effect at all (an EXIT rule and no rule produce the same answer). Attempt-level diagnosis is also impossible: a client reading `attempts[i].container.exitCode` to classify the failure, or `attempts[i].container.taskArn` to correlate an attempt with its ECS task and its CloudWatch log stream, finds those members absent and finds a reason string that exists nowhere in the AWS API, so any code branching on the reason text matches nothing.

Fix: the job attempt/retry lifecycle behind SubmitJob — the retry decision must default to RETRY when no `evaluateOnExit` condition matches the completed attempt, re-queue the job to RUNNABLE, and stop only when the attempt count reaches `retryStrategy.attempts` or a matching condition's action is EXIT; and the attempt recorder that DescribeJobs serializes must append one `AttemptDetail` per real execution carrying that execution's `container.exitCode`, its own `container.taskArn`, and `startedAt`/`stoppedAt`, instead of a single placeholder attempt bearing the no-container-runtime sentinel reason. (The sentinel reason is the same non-AWS string tracked by the open data-plane issue; this item is specifically the retry/attempt accounting layered on it.)

Traces: batch.retry-strategy-unmatched-evaluate-on-exit-condition-still-retries.

## Evidence
Suite: 161/275 passed; service `batch` 152/233 (65%), 81 failed. One of those failures was neither ALREADY FILED nor OWNED — the one below.

Failing check (verdict: mismatch): batch.retry-strategy-unmatched-evaluate-on-exit-condition-still-retries

Submitted (leg 1): job definition on a MANAGED/FARGATE CE with `retryStrategy={"attempts": 3, "evaluateOnExit": [{"onExitCode": "7", "action": "EXIT"}]}`, container command `["/bin/sh", "-c", "exit 3"]` — exit code 3 matches no listed condition.

Expected (AWS, per RetryStrategy.evaluateOnExit: "If none of the listed conditions match, then the job is retried"):
  - job status FAILED
  - len(JobDetail.attempts) == 3 (the full `attempts` budget)
  - every attempts[i].container.exitCode == 3
  - 3 distinct, non-null attempts[i].container.taskArn values (one fresh ECS task per retry)
  - leg 2 (same rule list, container exits 7 -> matching EXIT) stops at exactly 1 attempt with exitCode 7

Actual (HTTP 200 throughout):
  AssertionError: evaluateOnExit matched no attempt, so the documented default (retry) must run exactly 3 attempts; DescribeJobs reports 1 (statusReason="This emulator does not run containers, so the job's container was never started"). 1 means retryStrategy was ignored or an unmatched condition was treated as EXIT
  (raised at check_batch_retry_strategy_unmatched_evaluate_on_exit_condition_still_retries.py line 523, `assert len(u_attempts) == _ATTEMPTS`)

The single reported attempt also carries no container.exitCode and no container.taskArn, so the subsequent per-attempt exit-code and distinct-taskArn assertions are unreachable. Fixed text of the diverging reason: "This emulator does not run containers, so the job's container was never started" — it has no variable parts and no AWS counterpart; AWS's AttemptDetail.container.reason/statusReason for a container that ran and exited describes the container's own stop reason, and the attempt's exitCode carries the integer the request's command produced (3 here; 3 and 7 are values from the request, not part of any requirement).

## Steps to Reproduce
Preconditions: an IAM Fargate task execution role (ecs-tasks.amazonaws.com trust, AmazonECSTaskExecutionRolePolicy attached), a MANAGED/FARGATE compute environment polled to VALID, an ENABLED job queue polled to VALID, and a Fargate job definition (image public.ecr.aws/docker/library/busybox:1.36, resourceRequirements VCPU/MEMORY, assignPublicIp ENABLED) registered with:
    retryStrategy = {"attempts": 3, "evaluateOnExit": [{"onExitCode": "7", "action": "EXIT"}]}

1. SubmitJob against that definition/queue with containerOverrides command ["/bin/sh", "-c", "exit 3"] (leg 1: no condition matches exit 3).
2. SubmitJob again with command ["/bin/sh", "-c", "exit 7"] (leg 2: the EXIT condition matches).
3. Poll DescribeJobs on both job ids until each reaches a terminal status.
4. Read JobDetail.attempts for leg 1: observe a single AttemptDetail whose statusReason/container.reason is "This emulator does not run containers, so the job's container was never started", with no container.exitCode and no container.taskArn — instead of 3 AttemptDetail entries each reporting exitCode 3 and its own distinct taskArn.
5. Read JobDetail.attempts for leg 2: it is also a single non-executing attempt, so the EXIT rule's effect is indistinguishable from the unmatched case.

Reference: https://docs.aws.amazon.com/batch/latest/userguide/job_retries.html
