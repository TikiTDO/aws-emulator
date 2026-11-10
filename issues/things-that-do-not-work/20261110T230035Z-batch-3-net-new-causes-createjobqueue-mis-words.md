Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.job-queue-state-time-limit-action-max-time-seconds-range-enforced batch.job-queue-snapshot-reports-accurate-running-and-runnable-counts batch.job-queue-snapshot-frontofqueue-ordered-and-deduplicated-array-jobs batch.get-job-queue-snapshot-unknown-queue-rejected batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.job-queue-state-time-limit-action-max-time-seconds-range-enforced|run#4|AssertionError|assert message == expected, (
Issue-Key: batch.job-queue-snapshot-reports-accurate-running-and-runnable-counts|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Key: batch.job-queue-snapshot-frontofqueue-ordered-and-deduplicated-array-jobs|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: batch.get-job-queue-snapshot-unknown-queue-rejected|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job|_poll_until#29|Timeout|time.sleep(interval_s)
Issue-Commit: 324b832f69f4f69786da0229af3ae0e1d89202ec

# batch: 3 net-new causes — CreateJobQueue mis-words the jobStateTimeLimitActions maxTimeSeconds refusal, a submitted Fargate job never becomes RUNNABLE or enters GetJobQueueSnapshot.frontOfQueue, and a CFN-deployed job queue never runs the job

> _Note: this issue was identified against the code base at commit `324b832f69f4f69786da0229af3ae0e1d89202ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJobQueue

## Description
Three net-new root causes in the batch emulator, all grounded in failing behavioral checks that the suite did NOT mark as already filed or owned. (Related open issues that do NOT carry these check ids and therefore do not cover them: 20261104T041055Z-batch-array-jobs-unimplemented-managed-ces-have-no.md, 20261107T210820Z-batch-the-job-data-plane-never-executes-a.md, 20261105T005839Z-batch-a-job-submitted-to-a-cloudformation-deployed.md, 20261106T061928Z-batch-a-job-submitted-to-a-cloudformation-deployed-d662c3.md, 20261109T161717Z-batch-3-net-new-causes-tag-untag-204.md.)

Cause 1 of 3: CreateJobQueue's refusal message for an out-of-range jobStateTimeLimitActions[].maxTimeSeconds is emulator-authored.
AWS: Smithy (com.amazonaws.batch) — JobStateTimeLimitAction.maxTimeSeconds is a required Integer whose doc states "The minimum value is 600 (10 minutes) and the maximum value is 86,400". AWS refuses a value outside that range with HTTP 400 ClientException and the fixed sentence 'Invalid job action wait time. Value must be in range: [600..86400]'. The whole sentence is fixed text — 600 and 86400 are the model's documented bounds, not values taken from the request; the offending value is NOT echoed. Reference: API_JobStateTimeLimitAction.
Emulator: the verdict and the error code are right (the call is refused with ClientException), but the message is re-authored as 'jobStateTimeLimitActions maxTimeSeconds must be between 600 and 86400, got 599' — different fixed text, and it appends the request's value, which AWS does not.
Why: a client that classifies or surfaces the refusal by message text (log scrapers, user-facing error mapping, tests pinned to AWS's wording) does not recognise the emulator's sentence, and anything parsing AWS's '[min..max]' form finds no match.
Fix: the CreateJobQueue / UpdateJobQueue validation of jobStateTimeLimitActions in the batch handler — emit AWS's fixed sentence for both the below-minimum (599) and above-maximum (86401) cases instead of a locally worded one.
Traces: batch.job-queue-state-time-limit-action-max-time-seconds-range-enforced.

Cause 2 of 3: a Fargate job submitted to an ENABLED job queue never reaches RUNNABLE and never appears in GetJobQueueSnapshot.frontOfQueue, so every snapshot observation is unreachable.
AWS: recorded behavior only (GetJobQueueSnapshot has no modeled constraint covering this). On AWS, SubmitJob to an ENABLED queue whose compute environment is VALID moves the job SUBMITTED -> PENDING -> RUNNABLE within the checks' bounded waits, DescribeJobs reports status RUNNABLE, and GetJobQueueSnapshot returns that job's jobArn in frontOfQueue.jobs, ordered and de-duplicated (one entry per array parent), with running/runnable counts that match the queue's real contents.
Emulator: all three snapshot checks exhaust their 60 s budget — the suite reports 'Check timed out after 60.0s' for each. Each of them first polls DescribeJobs for the job to reach RUNNABLE (90-180 s budgets) and then polls GetJobQueueSnapshot until the submitted job's jobArn is present in frontOfQueue.jobs; neither predicate is ever satisfied, so the check never gets to its assertions. Hypothesis: this is the same job-dispatch gap other batch checks show directly (submitted jobs go SUBMITTED -> FAILED or never settle), so nothing is ever placed on the queue for the snapshot to report.
Why: a client that waits for RUNNABLE/RUNNING before reading GetJobQueueSnapshot (schedulers, dashboards, autoscaling glue) hangs forever and sees an always-empty frontOfQueue with zero running/runnable counts; capacity and queue-depth logic built on the snapshot reads the queue as idle while jobs are outstanding.
Fix: the SubmitJob -> queue placement state machine and the GetJobQueueSnapshot handler that projects frontOfQueue/running+runnable counts out of it.
Traces: batch.job-queue-snapshot-reports-accurate-running-and-runnable-counts, batch.job-queue-snapshot-frontofqueue-ordered-and-deduplicated-array-jobs, batch.get-job-queue-snapshot-unknown-queue-rejected (the last one also never reaches its ClientException assertion on an unknown queue name, because its positive control hangs first).

Cause 3 of 3: a job queue deployed by a CloudFormation stack accepts a job that is then never run.
AWS: recorded behavior only. A stack that reaches CREATE_COMPLETE with AWS::Batch::ComputeEnvironment + AWS::Batch::JobQueue yields a queue that accepts SubmitJob and carries the job through to a terminal state inside the check's bounded waits.
Emulator: the check exhausts its 60 s budget — 'Check timed out after 60.0s' — never reaching the assertions that follow the submitted job's progression on the CFN-created queue.
Why: a client that deploys its Batch stack by template and then submits work gets a queue that looks healthy but never executes anything, with no error to key off.
Fix: the CloudFormation-to-Batch resource provisioning path for AWS::Batch::JobQueue/ComputeEnvironment plus the same job placement state machine as Cause 2.
Traces: batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job.

## Evidence
Cause 1 — batch.job-queue-state-time-limit-action-max-time-seconds-range-enforced:
  AssertionError: maxTimeSeconds=599: expected message 'Invalid job action wait time. Value must be in range: [600..86400]', got 'jobStateTimeLimitActions maxTimeSeconds must be between 600 and 86400, got 599'
  Expected: HTTP 400, Code=ClientException, Message='Invalid job action wait time. Value must be in range: [600..86400]' (entire sentence fixed; emitted for both 599 and 86401).
  Actual:   HTTP 400, Code=ClientException, Message='jobStateTimeLimitActions maxTimeSeconds must be between 600 and 86400, got <value>' (re-worded, echoes the request value).

Cause 2 — three checks, each reported by the suite as:
  Error: Check timed out after 60.0s
  batch.job-queue-snapshot-reports-accurate-running-and-runnable-counts
  batch.job-queue-snapshot-frontofqueue-ordered-and-deduplicated-array-jobs
  batch.get-job-queue-snapshot-unknown-queue-rejected
  Each check creates a MANAGED/FARGATE compute environment, waits it VALID, creates an ENABLED queue, registers a FARGATE job definition, calls SubmitJob and then polls:
    _poll_until(lambda: batch.describe_jobs(jobs=[job_id])['jobs'][0]['status'], lambda s: s == 'RUNNABLE', timeout_s=90.0)
    _poll_until(lambda: batch.get_job_queue_snapshot(jobQueue=jq_name), lambda s: job_arn in {j['jobArn'] for j in s['frontOfQueue']['jobs']}, timeout_s=90.0)
  Expected: both predicates satisfied (status RUNNABLE; jobArn present in frontOfQueue.jobs).
  Actual:   neither is ever satisfied; the check is killed at its 60 s budget before any assertion runs.

Cause 3 — batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job:
  Error: Check timed out after 60.0s
  Expected: stack CREATE_COMPLETE, SubmitJob to the CFN-created queue, job progresses and settles.
  Actual:   check killed at its 60 s budget with no job progression observed.

## Steps to Reproduce
Cause 1:
1. Create a MANAGED/FARGATE compute environment and wait for status VALID.
2. Call CreateJobQueue with computeEnvironmentOrder pointing at it and jobStateTimeLimitActions=[{reason:'MISCONFIGURATION:COMPUTE_ENVIRONMENT_MAX_RESOURCE', state:'RUNNABLE', action:'CANCEL', maxTimeSeconds:599}].
3. Observe the refusal: code is ClientException, but Message is 'jobStateTimeLimitActions maxTimeSeconds must be between 600 and 86400, got 599' instead of AWS's 'Invalid job action wait time. Value must be in range: [600..86400]'. Repeat with maxTimeSeconds=86401 for the same divergence.

Cause 2:
1. Create a VPC/subnet/SG and an ECS task execution role; create a MANAGED/FARGATE compute environment and wait for VALID.
2. CreateJobQueue (state ENABLED, priority 1) over that CE and wait for VALID.
3. RegisterJobDefinition (type container, platformCapabilities FARGATE, VCPU/MEMORY resourceRequirements) and call SubmitJob.
4. Poll DescribeJobs for status == 'RUNNABLE' — it never arrives.
5. Poll GetJobQueueSnapshot(jobQueue=<queue>) for the submitted jobArn in frontOfQueue.jobs — it never appears; running/runnable counts stay empty.

Cause 3:
1. CreateStack with a template containing AWS::IAM::Role (batch.amazonaws.com), AWS::Batch::ComputeEnvironment (MANAGED/FARGATE) and AWS::Batch::JobQueue; wait for CREATE_COMPLETE.
2. SubmitJob to the stack's job queue and poll DescribeJobs — the job never progresses to a terminal state within the check's bounded waits.
